//! Bridges a `kube::Api::exec`/`attach` websocket to byte channels the GPUI view drives, off
//! the UI thread, plus the Kubernetes objects some sessions need first: ephemeral debug
//! containers (`kubectl debug`) and privileged node-shell pods.

use std::time::Duration;

use anyhow::Context as _;
use futures::channel::mpsc;
use k8s_openapi::api::core::v1::{ContainerStatus, Pod};
use kube::Api;
use kube::api::{AttachParams, DeleteParams, Patch, PatchParams, PostParams};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// What to run: a command (shell) exec, or an attach to the container's main process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Exec {
        command: Vec<String>,
    },
    /// `kubectl attach`. `tty`/`stdin` follow the container spec: attaching with a TTY to a
    /// container without one is rejected by the API server.
    Attach {
        tty: bool,
        stdin: bool,
    },
}

#[derive(Clone, Debug)]
pub struct ExecTarget {
    pub namespace: String,
    pub pod: String,
    pub container: Option<String>,
    pub mode: Mode,
}

/// Annotation kubectl reads to pick a default container on multi-container pods.
const DEFAULT_CONTAINER_ANNOTATION: &str = "kubectl.kubernetes.io/default-container";

/// A container of a pod, for pickers and attach parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerInfo {
    pub name: String,
    pub ephemeral: bool,
    pub tty: bool,
    pub stdin: bool,
    pub running: bool,
}

/// The containers of a pod and the one to use by default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PodInfo {
    pub containers: Vec<ContainerInfo>,
    pub default: Option<String>,
}

impl PodInfo {
    pub fn of(pod: &Pod) -> Self {
        let status = pod.status.as_ref();
        let running = |name: &str, statuses: Option<&Vec<ContainerStatus>>| {
            statuses
                .into_iter()
                .flatten()
                .find(|s| s.name == name)
                .and_then(|s| s.state.as_ref())
                .is_some_and(|s| s.running.is_some())
        };
        let mut containers = Vec::new();
        if let Some(spec) = &pod.spec {
            for c in &spec.containers {
                containers.push(ContainerInfo {
                    name: c.name.clone(),
                    ephemeral: false,
                    tty: c.tty.unwrap_or(false),
                    stdin: c.stdin.unwrap_or(false),
                    running: running(&c.name, status.and_then(|s| s.container_statuses.as_ref())),
                });
            }
            for c in spec.ephemeral_containers.iter().flatten() {
                containers.push(ContainerInfo {
                    name: c.name.clone(),
                    ephemeral: true,
                    tty: c.tty.unwrap_or(false),
                    stdin: c.stdin.unwrap_or(false),
                    running: running(
                        &c.name,
                        status.and_then(|s| s.ephemeral_container_statuses.as_ref()),
                    ),
                });
            }
        }
        let default = pod
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(DEFAULT_CONTAINER_ANNOTATION))
            .cloned()
            .or_else(|| {
                containers
                    .iter()
                    .find(|c| !c.ephemeral)
                    .map(|c| c.name.clone())
            });
        Self {
            containers,
            default,
        }
    }

    pub fn get(&self, name: &str) -> Option<&ContainerInfo> {
        self.containers.iter().find(|c| c.name == name)
    }
}

/// Fetches the pod's containers. Without a pinned container the API server rejects exec on
/// multi-container pods ("a container name must be specified").
pub async fn pod_info(
    client: &kube::Client,
    namespace: &str,
    pod: &str,
) -> anyhow::Result<PodInfo> {
    let api: Api<Pod> = Api::namespaced(client.clone(), namespace);
    Ok(PodInfo::of(&api.get(pod).await?))
}

/// Runs until `input` closes or the connection drops. Output bytes (stdout and stderr) go to
/// `output`; `resize` carries `(columns, rows)`. `connected` is signaled once the websocket is
/// established (`Ok(())`) or failed (`Err(message)`).
pub async fn run(
    client: kube::Client,
    target: ExecTarget,
    mut input: mpsc::UnboundedReceiver<Vec<u8>>,
    output: mpsc::UnboundedSender<Vec<u8>>,
    mut resize: mpsc::UnboundedReceiver<(u16, u16)>,
    connected: futures::channel::oneshot::Sender<Result<(), String>>,
) -> anyhow::Result<()> {
    use futures::StreamExt as _;

    let api: Api<Pod> = Api::namespaced(client, &target.namespace);
    let mut ap = match &target.mode {
        Mode::Exec { .. } => AttachParams::interactive_tty(),
        Mode::Attach { tty, stdin } => AttachParams::default()
            .stdin(*stdin)
            .stdout(true)
            .stderr(!tty)
            .tty(*tty),
    };
    if matches!(target.mode, Mode::Exec { .. }) {
        // With a TTY, stderr is merged into stdout by the runtime.
        ap = ap.stderr(false);
    }
    if let Some(container) = &target.container {
        ap = ap.container(container.clone());
    }

    let attached = match &target.mode {
        Mode::Exec { command } => api.exec(&target.pod, command, &ap).await,
        Mode::Attach { .. } => api.attach(&target.pod, &ap).await,
    };
    let mut attached = match attached {
        Ok(attached) => {
            connected.send(Ok(())).ok();
            attached
        }
        Err(err) => {
            connected.send(Err(err.to_string())).ok();
            return Err(err.into());
        }
    };

    let mut stdin = attached.stdin();
    let mut stdout = attached.stdout();
    let mut stderr = attached.stderr();
    let mut resize_tx = attached.terminal_size();

    let mut out_buf = [0u8; 8192];
    let mut err_buf = [0u8; 4096];
    loop {
        tokio::select! {
            bytes = input.next() => {
                let Some(bytes) = bytes else { break };
                if let Some(stdin) = stdin.as_mut()
                    && stdin.write_all(&bytes).await.is_err()
                {
                    break;
                }
            }
            size = resize.next() => {
                let Some((columns, rows)) = size else { continue };
                if let Some(tx) = resize_tx.as_mut() {
                    use futures::SinkExt as _;
                    tx.send(kube::api::TerminalSize { width: columns, height: rows })
                        .await
                        .ok();
                }
            }
            read = async {
                match stdout.as_mut() {
                    Some(stdout) => stdout.read(&mut out_buf).await,
                    None => std::future::pending().await,
                }
            } => {
                match read {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if output.unbounded_send(out_buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
            read = async {
                match stderr.as_mut() {
                    Some(stderr) => stderr.read(&mut err_buf).await,
                    None => std::future::pending().await,
                }
            } => {
                match read {
                    Ok(0) | Err(_) => stderr = None,
                    Ok(n) => {
                        if output.unbounded_send(err_buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }
    drop(stdin);
    attached.join().await.ok();
    Ok(())
}

/// A short random-looking suffix for generated names (`debugger-k3x9q`).
fn suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let alphabet = b"bcdfghjklmnpqrstvwxz2456789";
    let mut n = nanos ^ (std::process::id() as u128) << 64;
    (0..5)
        .map(|_| {
            let c = alphabet[(n % alphabet.len() as u128) as usize] as char;
            n /= alphabet.len() as u128;
            c
        })
        .collect()
}

/// Waiting reasons that won't resolve on their own.
const FATAL_WAITING: &[&str] = &[
    "ErrImagePull",
    "ImagePullBackOff",
    "InvalidImageName",
    "CreateContainerError",
    "CreateContainerConfigError",
    "RunContainerError",
];

/// Waits until `container` of `pod` is running (or fails with the reason it won't).
pub async fn wait_running(
    api: &Api<Pod>,
    pod: &str,
    container: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let object = api.get(pod).await?;
        let status = object.status.as_ref();
        let found = status.and_then(|s| {
            s.container_statuses
                .iter()
                .flatten()
                .chain(s.ephemeral_container_statuses.iter().flatten())
                .find(|c| c.name == container)
        });
        if let Some(state) = found.and_then(|c| c.state.as_ref()) {
            if state.running.is_some() {
                return Ok(());
            }
            if let Some(waiting) = &state.waiting
                && let Some(reason) = &waiting.reason
                && FATAL_WAITING.contains(&reason.as_str())
            {
                anyhow::bail!(
                    "{container}: {reason}{}",
                    waiting
                        .message
                        .as_ref()
                        .map(|m| format!(" ({m})"))
                        .unwrap_or_default()
                );
            }
            if let Some(terminated) = &state.terminated {
                anyhow::bail!(
                    "{container} exited with code {}{}",
                    terminated.exit_code,
                    terminated
                        .reason
                        .as_ref()
                        .map(|r| format!(" ({r})"))
                        .unwrap_or_default()
                );
            }
        }
        if status.and_then(|s| s.phase.as_deref()) == Some("Failed") {
            anyhow::bail!("pod {pod} failed");
        }
        if tokio::time::Instant::now() >= deadline {
            let reason = status
                .and_then(|s| s.conditions.as_ref())
                .and_then(|c| c.iter().find(|c| c.status == "False"))
                .and_then(|c| c.message.clone())
                .unwrap_or_else(|| "still not running".into());
            anyhow::bail!("timed out waiting for {container}: {reason}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// What `kubectl debug` adds to a pod.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugSpec {
    pub image: String,
    /// Shares this container's process namespace (`--target`), so its files are reachable at
    /// `/proc/1/root` even when the image is distroless.
    pub target: Option<String>,
    pub command: Vec<String>,
}

/// The ephemeral-container patch for `spec`.
pub fn debug_patch(name: &str, spec: &DebugSpec) -> serde_json::Value {
    let mut container = serde_json::json!({
        "name": name,
        "image": spec.image,
        "imagePullPolicy": "IfNotPresent",
        "stdin": true,
        "tty": true,
        "terminationMessagePolicy": "File",
    });
    if !spec.command.is_empty() {
        container["command"] = serde_json::json!(spec.command);
    }
    if let Some(target) = &spec.target {
        container["targetContainerName"] = serde_json::json!(target);
    }
    serde_json::json!({"spec": {"ephemeralContainers": [container]}})
}

/// Adds an ephemeral debug container to `pod` and waits until it runs. Returns its name.
pub async fn create_debug_container(
    client: &kube::Client,
    namespace: &str,
    pod: &str,
    spec: &DebugSpec,
) -> anyhow::Result<String> {
    let api: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let name = format!("debugger-{}", suffix());
    api.patch_ephemeral_containers(
        pod,
        &PatchParams::default(),
        &Patch::Strategic(debug_patch(&name, spec)),
    )
    .await
    .context("adding the ephemeral container")?;
    wait_running(&api, pod, &name, Duration::from_secs(180)).await?;
    Ok(name)
}

/// Label on node-shell pods, so leftovers are easy to find.
pub const NODE_SHELL_LABEL: &str = "kubyl.dev/node-shell";

/// The privileged pod that runs a node shell: host PID/network/IPC namespaces, pinned to the
/// node, tolerating every taint, deleted after at most 12 hours even if Kubyl never cleans up.
pub fn node_shell_pod(node: &str, image: &str) -> serde_json::Value {
    serde_json::json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {
            "generateName": "kubyl-node-shell-",
            "labels": {
                NODE_SHELL_LABEL: node,
                "app.kubernetes.io/managed-by": "kubyl",
            },
        },
        "spec": {
            "nodeName": node,
            "hostPID": true,
            "hostNetwork": true,
            "hostIPC": true,
            "restartPolicy": "Never",
            "terminationGracePeriodSeconds": 0,
            "activeDeadlineSeconds": 43200,
            "tolerations": [{"operator": "Exists"}],
            "containers": [{
                "name": "shell",
                "image": image,
                "imagePullPolicy": "IfNotPresent",
                "command": ["sh", "-c", "sleep 43200"],
                "securityContext": {"privileged": true},
                "resources": {"requests": {"cpu": "10m", "memory": "16Mi"}},
            }],
        },
    })
}

/// The command a node shell runs in its pod: enters PID 1's (the host's) namespaces and starts
/// a login shell there.
pub fn node_shell_command() -> Vec<String> {
    [
        "nsenter",
        "--target",
        "1",
        "--mount",
        "--uts",
        "--ipc",
        "--net",
        "--pid",
        "--",
        "sh",
        "-c",
        "if [ -x /bin/bash ]; then exec /bin/bash -l; else exec /bin/sh -l; fi",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Creates a node-shell pod on `node` and waits until it runs. Returns its name.
pub async fn create_node_shell(
    client: &kube::Client,
    namespace: &str,
    node: &str,
    image: &str,
) -> anyhow::Result<NodeShellPod> {
    let api: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let pod: Pod = serde_json::from_value(node_shell_pod(node, image))?;
    let created = api
        .create(&PostParams::default(), &pod)
        .await
        .context("creating the node-shell pod")?;
    let name = created
        .metadata
        .name
        .ok_or_else(|| anyhow::anyhow!("the node-shell pod has no name"))?;
    // From here on the pod is deleted if anything goes wrong, including the task being
    // cancelled while it waits.
    let pod = NodeShellPod {
        client: client.clone(),
        namespace: namespace.to_string(),
        name: Some(name),
    };
    wait_running(&api, pod.name(), "shell", Duration::from_secs(180)).await?;
    Ok(pod)
}

/// A node-shell pod that is deleted when dropped, until [`NodeShellPod::keep`] hands it over.
/// Privileged pods must not outlive their session, even when a task is cancelled or its
/// result never arrives.
pub struct NodeShellPod {
    client: kube::Client,
    namespace: String,
    name: Option<String>,
}

impl NodeShellPod {
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or_default()
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Hands the pod over: `(client, namespace, name)`. The caller deletes it now.
    pub fn keep(mut self) -> (kube::Client, String, String) {
        let name = self.name.take().unwrap_or_default();
        (self.client.clone(), self.namespace.clone(), name)
    }
}

impl Drop for NodeShellPod {
    fn drop(&mut self) {
        if let Some(name) = self.name.take() {
            let (client, namespace) = (self.client.clone(), self.namespace.clone());
            kubyl_core::runtime::handle().spawn(async move {
                delete_pod(&client, &namespace, &name).await;
            });
        }
    }
}

/// Deletes a pod right away (node-shell cleanup). Errors are logged, not returned.
pub async fn delete_pod(client: &kube::Client, namespace: &str, name: &str) {
    let api: Api<Pod> = Api::namespaced(client.clone(), namespace);
    if let Err(err) = api
        .delete(name, &DeleteParams::default().grace_period(0))
        .await
    {
        tracing::warn!(pod = name, "couldn't delete the node-shell pod: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pod_info_picks_the_annotated_or_first_container() {
        let pod: Pod = serde_json::from_value(serde_json::json!({
            "metadata": {"annotations": {DEFAULT_CONTAINER_ANNOTATION: "sidecar"}},
            "spec": {
                "containers": [{"name": "app", "tty": true, "stdin": true}, {"name": "sidecar"}],
                "ephemeralContainers": [{"name": "debugger-x"}]
            },
            "status": {"containerStatuses": [
                {"name": "app", "ready": true, "restartCount": 0, "image": "", "imageID": "",
                 "state": {"running": {}}}
            ]}
        }))
        .unwrap();
        let info = PodInfo::of(&pod);
        assert_eq!(info.default.as_deref(), Some("sidecar"));
        let app = info.get("app").unwrap();
        assert!(app.tty && app.stdin && app.running);
        assert!(info.get("debugger-x").unwrap().ephemeral);

        let plain: Pod = serde_json::from_value(serde_json::json!({
            "spec": {"containers": [{"name": "api"}]}
        }))
        .unwrap();
        assert_eq!(PodInfo::of(&plain).default.as_deref(), Some("api"));
    }

    #[test]
    fn debug_patch_targets_the_container() {
        let patch = debug_patch(
            "debugger-abcde",
            &DebugSpec {
                image: "busybox:1.37".into(),
                target: Some("api".into()),
                command: vec!["sh".into()],
            },
        );
        let container = &patch["spec"]["ephemeralContainers"][0];
        assert_eq!(container["name"], "debugger-abcde");
        assert_eq!(container["targetContainerName"], "api");
        assert_eq!(container["tty"], true);
        assert_eq!(container["command"][0], "sh");
    }

    #[test]
    fn node_shell_pod_is_privileged_and_pinned() {
        let pod: Pod = serde_json::from_value(node_shell_pod("worker-1", "busybox:1.37")).unwrap();
        let spec = pod.spec.unwrap();
        assert_eq!(spec.node_name.as_deref(), Some("worker-1"));
        assert_eq!(spec.host_pid, Some(true));
        assert_eq!(
            spec.containers[0]
                .security_context
                .as_ref()
                .and_then(|s| s.privileged),
            Some(true)
        );
        assert_eq!(spec.active_deadline_seconds, Some(43200));
        assert_eq!(node_shell_command()[0], "nsenter");
    }

    #[test]
    fn suffixes_are_short_and_dns_safe() {
        let s = suffix();
        assert_eq!(s.len(), 5);
        assert!(
            s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
    }
}
