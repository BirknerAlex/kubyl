//! File operations in a container, run through `exec` off the UI thread.
//!
//! [`probe`] finds out what the image offers (a shell, tar, sha256sum, which listing method…).
//! Distroless images have no shell: the browser then adds an ephemeral busybox container that
//! shares the target's process namespace and reaches its files at `/proc/1/root`
//! ([`RemoteTarget::root`]). Every command passes paths as positional arguments to `sh -c`, so
//! file names never need quoting.

use std::collections::HashMap;

use anyhow::Context as _;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Status;
use kube::Api;
use kube::api::AttachParams;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::entry::{Entry, EntryKind, sort_entries};
use crate::listing::{self, ListingMethod};

/// What a container's image offers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub shell: bool,
    pub tar: bool,
    pub sha256sum: bool,
    pub dd: bool,
    pub cat: bool,
    pub du: bool,
    pub head: bool,
    pub listing: Option<ListingMethod>,
}

const TOOLS: &str = "tar ls find stat sha256sum dd cat du head mkdir mv rm chmod readlink";

/// Prints the tools found, one per line, plus the listing features.
const PROBE: &str = "for t in tar ls find stat sha256sum dd cat du head mkdir mv rm chmod readlink; do command -v $t >/dev/null 2>&1 && echo $t; done
find / -maxdepth 0 -printf '' >/dev/null 2>&1 && echo find-printf
stat -c %n / >/dev/null 2>&1 && echo stat-c
ls --time-style=full-iso -d / >/dev/null 2>&1 && echo ls-full-iso
true";

impl Capabilities {
    /// Parses the output of the probe script.
    pub fn parse(output: &str) -> Self {
        let found: Vec<&str> = output.lines().map(str::trim).collect();
        let has = |t: &str| found.contains(&t);
        let listing = if has("find-printf") {
            Some(ListingMethod::FindPrintf)
        } else if has("stat-c") && has("readlink") {
            Some(ListingMethod::StatLoop)
        } else if has("ls-full-iso") {
            Some(ListingMethod::LsFullIso)
        } else if has("ls") {
            Some(ListingMethod::Ls)
        } else {
            None
        };
        Self {
            shell: true,
            tar: has("tar"),
            sha256sum: has("sha256sum"),
            dd: has("dd"),
            cat: has("cat"),
            du: has("du"),
            head: has("head"),
            listing,
        }
    }

    /// The toolbar chip: `tar available · /bin/sh`.
    pub fn summary(&self) -> String {
        if !self.shell {
            return "no shell".into();
        }
        let mut parts = Vec::new();
        parts.push(if self.tar {
            "tar available"
        } else {
            "no tar: single files only"
        });
        if !self.sha256sum {
            parts.push("unverified");
        }
        parts.push("/bin/sh");
        parts.join(" · ")
    }

    /// Everything the browser needs works.
    pub fn complete(&self) -> bool {
        self.shell && self.tar && self.listing.is_some() && self.sha256sum
    }
}

/// The tools the probe looks for (for messages).
pub fn tool_names() -> &'static str {
    TOOLS
}

/// Output of a finished command.
#[derive(Clone, Debug, Default)]
pub struct ExecOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub message: Option<String>,
}

impl ExecOutput {
    /// The error to show for a failed command.
    pub fn error(&self) -> String {
        let stderr = self.stderr.trim();
        if !stderr.is_empty() {
            return stderr.lines().last().unwrap_or(stderr).to_string();
        }
        match (&self.message, self.exit_code) {
            (Some(message), _) => message.clone(),
            (None, Some(code)) => format!("exited with code {code}"),
            (None, None) => "command failed".into(),
        }
    }
}

/// The exit code in an exec status (`details.causes[reason=ExitCode]`).
pub fn exit_code(status: &Status) -> Option<i32> {
    status
        .details
        .as_ref()?
        .causes
        .as_ref()?
        .iter()
        .find(|c| c.reason.as_deref() == Some("ExitCode"))?
        .message
        .as_ref()?
        .parse()
        .ok()
}

/// Runs `command` and collects its output. `stdin` is written and closed first.
pub async fn exec_capture(
    api: &Api<Pod>,
    pod: &str,
    container: &str,
    command: Vec<String>,
    stdin: Option<Vec<u8>>,
) -> anyhow::Result<ExecOutput> {
    let params = AttachParams::default()
        .container(container)
        .stdin(stdin.is_some())
        .stdout(true)
        .stderr(true);
    let mut process = api.exec(pod, command, &params).await?;
    if let Some(bytes) = stdin
        && let Some(mut writer) = process.stdin()
    {
        writer.write_all(&bytes).await.ok();
        writer.shutdown().await.ok();
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let out_reader = process.stdout();
    let err_reader = process.stderr();
    let read_out = async {
        if let Some(mut r) = out_reader {
            r.read_to_end(&mut stdout).await.ok();
        }
    };
    let read_err = async {
        if let Some(mut r) = err_reader {
            r.read_to_end(&mut stderr).await.ok();
        }
    };
    tokio::join!(read_out, read_err);
    let status = match process.take_status() {
        Some(status) => status.await,
        None => None,
    };
    let success = status
        .as_ref()
        .is_none_or(|s| s.status.as_deref() == Some("Success"));
    Ok(ExecOutput {
        stdout,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        success,
        exit_code: status.as_ref().and_then(exit_code),
        message: status.and_then(|s| s.message),
    })
}

/// Whether an exec error means the command (here: `sh`) doesn't exist in the image.
pub fn missing_executable(error: &str) -> bool {
    let error = error.to_lowercase();
    error.contains("executable file not found")
        || error.contains("no such file or directory")
        || error.contains("not found in $path")
}

/// Finds out what `container` offers. `Ok(caps)` with `shell: false` for images without `sh`.
pub async fn probe(api: &Api<Pod>, pod: &str, container: &str) -> anyhow::Result<Capabilities> {
    let command = vec!["sh".into(), "-c".into(), PROBE.into()];
    match exec_capture(api, pod, container, command, None).await {
        Ok(output) if output.success => Ok(Capabilities::parse(&String::from_utf8_lossy(
            &output.stdout,
        ))),
        Ok(output) if missing_executable(&output.error()) => Ok(Capabilities::default()),
        Ok(output) => Err(anyhow::anyhow!(output.error())),
        Err(err) if missing_executable(&err.to_string()) => Ok(Capabilities::default()),
        Err(err) => Err(err),
    }
}

/// Where remote operations run, and how to reach the browsed container's files.
#[derive(Clone)]
pub struct RemoteTarget {
    pub cluster: kubyl_core::ClusterId,
    pub client: kube::Client,
    pub namespace: String,
    pub pod: String,
    /// The browsed container.
    pub container: String,
    /// The container commands run in: the browsed one, or a debug container sharing its
    /// processes.
    pub exec_container: String,
    /// Prefix of every path in commands: `""`, or `/proc/1/root` through a debug container.
    pub root: String,
    pub caps: Capabilities,
}

/// `sh -c SCRIPT sh ARGS…`.
pub fn sh(script: &str, args: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut command = vec!["sh".into(), "-c".into(), script.into(), "sh".into()];
    command.extend(args);
    command
}

impl RemoteTarget {
    pub fn api(&self) -> Api<Pod> {
        Api::namespaced(self.client.clone(), &self.namespace)
    }

    /// The path as the exec container sees it.
    pub fn real(&self, path: &str) -> String {
        if self.root.is_empty() {
            path.to_string()
        } else {
            format!("{}{}", self.root, path)
        }
    }

    pub fn is_debug(&self) -> bool {
        !self.root.is_empty()
    }

    pub async fn exec(
        &self,
        command: Vec<String>,
        stdin: Option<Vec<u8>>,
    ) -> anyhow::Result<ExecOutput> {
        exec_capture(&self.api(), &self.pod, &self.exec_container, command, stdin).await
    }

    /// Runs a script with paths as arguments; errors when it fails.
    pub async fn run(&self, script: &str, paths: &[&str]) -> anyhow::Result<Vec<u8>> {
        let args = paths.iter().map(|p| self.real(p));
        let output = self.exec(sh(script, args), None).await?;
        if output.success {
            Ok(output.stdout)
        } else {
            Err(anyhow::anyhow!(output.error()))
        }
    }

    /// Lists `dir`, sorted (directories first).
    pub async fn list(&self, dir: &str) -> anyhow::Result<Vec<Entry>> {
        let method = self
            .caps
            .listing
            .ok_or_else(|| anyhow::anyhow!("the image has neither ls nor stat"))?;
        let output = self
            .exec(listing::command(method, &self.real(dir)), None)
            .await?;
        if !output.success {
            anyhow::bail!(output.error());
        }
        let mut entries = listing::parse(method, &output.stdout);
        sort_entries(&mut entries);
        Ok(entries)
    }

    pub async fn mkdir(&self, path: &str) -> anyhow::Result<()> {
        self.run("mkdir -p -- \"$1\"", &[path]).await.map(drop)
    }

    pub async fn rename(&self, from: &str, to: &str) -> anyhow::Result<()> {
        self.run(
            "[ -e \"$2\" ] && { echo \"$2 exists\" >&2; exit 1; }; mv -- \"$1\" \"$2\"",
            &[from, to],
        )
        .await
        .map(drop)
    }

    pub async fn delete(&self, paths: &[&str]) -> anyhow::Result<()> {
        self.run("rm -rf -- \"$@\"", paths).await.map(drop)
    }

    pub async fn chmod(&self, mode: &str, paths: &[&str]) -> anyhow::Result<()> {
        let mut args = vec![mode.to_string()];
        args.extend(paths.iter().map(|p| self.real(p)));
        let output = self
            .exec(sh("m=$1; shift; chmod -- \"$m\" \"$@\"", args), None)
            .await?;
        if output.success {
            Ok(())
        } else {
            Err(anyhow::anyhow!(output.error()))
        }
    }

    /// The first `bytes` bytes of a file (previews).
    pub async fn head(&self, path: &str, bytes: u64) -> anyhow::Result<Vec<u8>> {
        let script = if self.caps.head {
            format!("head -c {bytes} -- \"$1\"")
        } else {
            format!("dd if=\"$1\" bs={bytes} count=1 2>/dev/null")
        };
        self.run(&script, &[path]).await
    }

    /// The whole file (edit in place).
    pub async fn read(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        self.run("cat -- \"$1\"", &[path]).await
    }

    /// Overwrites a file with `bytes`, keeping its mode.
    pub async fn write(&self, path: &str, bytes: Vec<u8>) -> anyhow::Result<()> {
        let output = self
            .exec(sh("cat > \"$1\"", [self.real(path)]), Some(bytes))
            .await?;
        if output.success {
            Ok(())
        } else {
            Err(anyhow::anyhow!(output.error()))
        }
    }

    /// Modification time (seconds) and size of a file, for edit-in-place conflict checks.
    pub async fn stat(&self, path: &str) -> anyhow::Result<(i64, u64)> {
        let output = self
            .run(
                "stat -c '%Y %s' -- \"$1\" 2>/dev/null || ls -ln --time-style=+%s -- \"$1\" | awk '{print $6, $5}'",
                &[path],
            )
            .await?;
        let text = String::from_utf8_lossy(&output);
        let mut words = text.split_whitespace();
        let mtime = words.next().and_then(|w| w.parse().ok()).unwrap_or(0);
        let size = words.next().and_then(|w| w.parse().ok()).unwrap_or(0);
        Ok((mtime, size))
    }

    /// SHA-256 of every regular file under `paths`, keyed by path relative to `base`.
    pub async fn sha256_tree(
        &self,
        base: &str,
        names: &[&str],
    ) -> anyhow::Result<HashMap<String, String>> {
        if !self.caps.sha256sum {
            anyhow::bail!("sha256sum isn't available");
        }
        let mut args = vec![self.real(base)];
        args.extend(names.iter().map(|n| n.to_string()));
        let output = self
            .exec(
                sh(
                    "cd -- \"$1\" || exit 2; shift; find \"$@\" -type f -exec sha256sum {} +",
                    args,
                ),
                None,
            )
            .await?;
        if !output.success {
            anyhow::bail!(output.error());
        }
        Ok(parse_sha256sum(&String::from_utf8_lossy(&output.stdout)))
    }

    /// Total size in bytes of `paths` (`du -sk`, so rounded to KiB).
    pub async fn du(&self, paths: &[&str]) -> anyhow::Result<u64> {
        let output = self.run("du -sk -- \"$@\"", paths).await?;
        Ok(String::from_utf8_lossy(&output)
            .lines()
            .filter_map(|l| l.split_whitespace().next()?.parse::<u64>().ok())
            .sum::<u64>()
            * 1024)
    }

    /// Whether `path` exists (conflict checks).
    pub async fn exists(&self, path: &str) -> anyhow::Result<bool> {
        let output = self
            .exec(
                sh("[ -e \"$1\" ] || [ -L \"$1\" ]", [self.real(path)]),
                None,
            )
            .await?;
        Ok(output.success)
    }
}

/// `sha256sum` output (`<hex>  <path>`) → path (without a leading `./`) → hash.
pub fn parse_sha256sum(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (hash, path) = line.split_once("  ").or_else(|| line.split_once(" *"))?;
            let path = path.strip_prefix("./").unwrap_or(path);
            Some((path.to_string(), hash.to_string()))
        })
        .collect()
}

/// Where the target's files are, seen from a debug container sharing its process namespace
/// (the target's first process is PID 1 there).
pub const DEBUG_ROOT: &str = "/proc/1/root";

/// Prefix of the debug containers the browser adds.
pub const DEBUG_PREFIX: &str = "kubyl-files-";

/// Connects to `container` of `pod` and probes it. Images without a shell come back with
/// `caps.shell == false`; [`through_debug_container`] reaches them.
pub async fn open(
    cluster: kubyl_core::ClusterId,
    client: kube::Client,
    namespace: String,
    pod: String,
    container: String,
) -> anyhow::Result<RemoteTarget> {
    let api: Api<Pod> = Api::namespaced(client.clone(), &namespace);
    let caps = probe(&api, &pod, &container).await?;
    Ok(RemoteTarget {
        cluster,
        client,
        namespace,
        pod,
        exec_container: container.clone(),
        container,
        root: String::new(),
        caps,
    })
}

/// A debug container of this browser that targets `container` and still runs, if any.
fn running_debug_container(pod: &Pod, container: &str) -> Option<String> {
    let spec = pod.spec.as_ref()?;
    let status = pod.status.as_ref()?;
    spec.ephemeral_containers
        .iter()
        .flatten()
        .filter(|c| {
            c.name.starts_with(DEBUG_PREFIX)
                && c.target_container_name.as_deref() == Some(container)
                && c.security_context
                    .as_ref()
                    .and_then(|s| s.capabilities.as_ref())
                    .and_then(|c| c.add.as_ref())
                    .is_some_and(|add| add.iter().any(|cap| cap == "SYS_PTRACE"))
        })
        .find(|c| {
            status
                .ephemeral_container_statuses
                .iter()
                .flatten()
                .any(|s| {
                    s.name == c.name && s.state.as_ref().is_some_and(|st| st.running.is_some())
                })
        })
        .map(|c| c.name.clone())
}

/// Reaches a container without a shell through an ephemeral `image` container that shares its
/// process namespace (its files are at `/proc/1/root`). Reuses a running one from earlier:
/// ephemeral containers can't be removed until the pod is deleted.
pub async fn through_debug_container(
    target: &RemoteTarget,
    image: &str,
) -> anyhow::Result<RemoteTarget> {
    let api = target.api();
    let pod = api.get(&target.pod).await?;
    let name = match running_debug_container(&pod, &target.container) {
        Some(name) => name,
        None => {
            let name = format!("{DEBUG_PREFIX}{}", short_id());
            let patch = serde_json::json!({"spec": {"ephemeralContainers": [{
                "name": name,
                "image": image,
                "imagePullPolicy": "IfNotPresent",
                "command": ["sh", "-c", "trap 'exit 0' TERM; while true; do sleep 3600; done"],
                "targetContainerName": target.container,
                "terminationMessagePolicy": "File",
                // Entering another user's /proc/<pid>/root needs ptrace access (like
                // `kubectl debug --profile=general`).
                "securityContext": {"capabilities": {"add": ["SYS_PTRACE"]}},
            }]}});
            api.patch_ephemeral_containers(
                &target.pod,
                &kube::api::PatchParams::default(),
                &kube::api::Patch::Strategic(patch),
            )
            .await
            .context("adding the debug container")?;
            kubyl_terminal::exec::wait_running(
                &api,
                &target.pod,
                &name,
                std::time::Duration::from_secs(180),
            )
            .await?;
            name
        }
    };
    let caps = probe(&api, &target.pod, &name).await?;
    if !caps.shell {
        anyhow::bail!("the debug image {image} has no shell");
    }
    Ok(RemoteTarget {
        exec_container: name,
        root: DEBUG_ROOT.into(),
        caps,
        ..target.clone()
    })
}

fn short_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let alphabet = b"bcdfghjklmnpqrstvwxz2456789";
    let mut n = nanos;
    (0..5)
        .map(|_| {
            let c = alphabet[(n % alphabet.len() as u128) as usize] as char;
            n /= alphabet.len() as u128;
            c
        })
        .collect()
}

/// Entries of a directory in a debug container list their symlinks relative to the target's
/// root; nothing to rewrite, but absolute link targets point into the debug container.
pub fn rebase_link(entry: &mut Entry, root: &str) {
    if entry.kind == EntryKind::Symlink
        && let Some(target) = &entry.link_target
        && target.starts_with(root)
    {
        entry.link_target = Some(target[root.len()..].to_string());
    }
}

/// Makes sure the tools a transfer needs exist, with a clear message otherwise.
pub fn require(caps: &Capabilities, what: &str, tool: bool, name: &str) -> anyhow::Result<()> {
    if !caps.shell {
        anyhow::bail!("{what} needs a shell in the container");
    }
    if !tool {
        anyhow::bail!("{what} needs {name} in the container");
    }
    Ok(())
}

/// Runs `command` for streaming (tar, cat, dd): the caller reads stdout / writes stdin.
pub async fn exec_stream(
    target: &RemoteTarget,
    command: Vec<String>,
    stdin: bool,
) -> anyhow::Result<kube::api::AttachedProcess> {
    let params = AttachParams::default()
        .container(target.exec_container.clone())
        .stdin(stdin)
        .stdout(true)
        .stderr(true);
    target
        .api()
        .exec(&target.pod, command, &params)
        .await
        .context("starting the transfer")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_capabilities() {
        let caps = Capabilities::parse("tar\nls\nstat\nsha256sum\ndd\ncat\nreadlink\nstat-c\n");
        assert!(caps.shell && caps.tar && caps.sha256sum && caps.dd);
        assert_eq!(caps.listing, Some(ListingMethod::StatLoop));
        assert_eq!(caps.summary(), "tar available · /bin/sh");
        assert!(caps.complete());

        let gnu = Capabilities::parse("tar\nls\nfind\nfind-printf\nstat-c\nreadlink\n");
        assert_eq!(gnu.listing, Some(ListingMethod::FindPrintf));
        assert!(gnu.summary().contains("unverified"));

        let bare = Capabilities::parse("ls\n");
        assert_eq!(bare.listing, Some(ListingMethod::Ls));
        assert!(!bare.tar);
        assert_eq!(Capabilities::default().summary(), "no shell");
    }

    #[test]
    fn parses_sha256sum_output() {
        let hashes = parse_sha256sum(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  ./a.txt\n\
             2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae  certs/ca.pem\n",
        );
        assert_eq!(hashes.len(), 2);
        assert!(hashes["a.txt"].starts_with("e3b0"));
        assert!(hashes.contains_key("certs/ca.pem"));
    }

    #[test]
    fn recognizes_missing_shells() {
        assert!(missing_executable(
            "exec: \"sh\": executable file not found in $PATH"
        ));
        assert!(!missing_executable("permission denied"));
    }

    #[test]
    fn sh_passes_arguments_after_the_script() {
        assert_eq!(
            sh(
                "mv -- \"$1\" \"$2\"",
                ["/a b".to_string(), "/c".to_string()]
            ),
            ["sh", "-c", "mv -- \"$1\" \"$2\"", "sh", "/a b", "/c"]
        );
    }
}
