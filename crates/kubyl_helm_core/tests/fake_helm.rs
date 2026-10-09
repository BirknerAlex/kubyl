//! `kubyl_helm_core::cli` against a fake `helm` (a shell script that records its arguments,
//! environment and stdin, and prints recorded output), so these tests never need the real CLI.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use kubyl_helm_core::cli::{self, ErrorKind, Invocation, Probe};
use kubyl_helm_core::cmd::{self, InstallSpec, Mode, WriteOptions};
use kubyl_helm_core::repo::{self, ChartRef, NewRepository};
use kubyl_kube_core::cli::CliTarget;

struct Fake {
    dir: tempfile::TempDir,
    program: PathBuf,
}

const DRY_RUN: &str = r#"{"name":"web","namespace":"shop","version":1,"info":{"status":"pending-install"},"chart":{"metadata":{"name":"demo","version":"0.2.0"}},"config":{"replicaCount":2},"manifest":"---\napiVersion: v1\nkind: Secret\nmetadata:\n  name: web\ndata:\n  password: aHVudGVyMg==\n","hooks":[]}"#;

impl Fake {
    fn new(version: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("helm");
        let out = dir.path().display().to_string();
        std::fs::write(dir.path().join("dry-run.json"), DRY_RUN).unwrap();
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > "{out}/args"
env > "{out}/env"
cat > "{out}/stdin"
case "$1" in
  version) echo "{version}" ;;
  env) printf 'HELM_REPOSITORY_CONFIG="{out}/repositories.yaml"\nHELM_KUBETOKEN=""\n' ;;
  install) cat "{out}/dry-run.json" ;;
  repo) if [ "$2" = list ]; then echo "Error: no repositories to show" >&2; exit 1; fi ;;
  upgrade) echo "Error: UPGRADE FAILED: another operation (install/upgrade/rollback) is in progress" >&2; exit 1 ;;
  uninstall) exec sleep 30 ;;
esac
"#
        );
        std::fs::write(&program, script).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir, program }
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(name)).unwrap_or_default()
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    async fn info(&self) -> cli::HelmInfo {
        let search =
            std::env::join_paths([self.path(), Path::new("/usr/bin"), Path::new("/bin")]).unwrap();
        match cli::probe(None, Some(search)).await {
            Probe::Ready(info) => info,
            other => panic!("{other:?}"),
        }
    }
}

fn target() -> CliTarget {
    CliTarget::new("/home/me/kube/dev.yaml", "kind-kubyl-dev")
        .with_server(Some("https://127.0.0.1:6443".into()))
}

#[tokio::test]
async fn probe_finds_helm_and_reads_its_env() {
    let fake = Fake::new("v3.16.2+g13654a5");
    let info = fake.info().await;
    assert_eq!(info.path, fake.program);
    assert_eq!(info.version.to_string(), "3.16.2");
    assert_eq!(
        info.env.repository_config,
        Some(fake.path().join("repositories.yaml"))
    );
    let old = Fake::new("v3.12.3+g3a31588");
    let search = std::env::join_paths([old.path(), Path::new("/bin")]).unwrap();
    assert!(matches!(
        cli::probe(None, Some(search.clone())).await,
        Probe::TooOld { .. }
    ));
    let missing = cli::probe(Some("/nowhere/helm"), Some(search)).await;
    assert!(matches!(
        missing,
        Probe::Missing {
            configured: Some(_)
        }
    ));
    assert!(missing.problem().unwrap().contains("/nowhere/helm"));
}

#[tokio::test]
async fn installs_send_values_on_stdin_and_the_token_in_the_environment() {
    let fake = Fake::new("v4.3.0+gbec5b06");
    let info = fake.info().await;
    let spec = InstallSpec {
        name: "web".into(),
        namespace: "shop".into(),
        create_namespace: true,
        chart: ChartRef::Repo {
            repo: "kubyl-dev".into(),
            name: "demo".into(),
        },
        version: None,
        options: WriteOptions::default(),
    };
    let values = "replicaCount: 2\nauth:\n  password: hunter2\n";
    let output = cli::run_with_token(
        &info,
        Some(&target()),
        Some("sha256~TOKEN-VALUE".to_string().into()),
        cmd::install(&spec, values, Mode::DryRunServer, info.version),
        None,
        None,
    )
    .await
    .unwrap();
    let args = fake.read("args");
    let env = fake.read("env");
    // Values only on stdin, the token only in the environment.
    assert_eq!(fake.read("stdin"), values);
    assert!(!args.contains("hunter2"));
    assert!(!args.contains("TOKEN-VALUE"));
    assert!(env.contains("HELM_KUBETOKEN=sha256~TOKEN-VALUE"));
    assert!(env.contains("HELM_KUBEAPISERVER=https://127.0.0.1:6443"));
    assert!(env.contains("HELM_DEBUG=false"));
    let lines: Vec<&str> = args.lines().collect();
    assert!(
        lines
            .windows(2)
            .any(|w| w == ["--kubeconfig", "/home/me/kube/dev.yaml"])
    );
    assert!(
        lines
            .windows(2)
            .any(|w| w == ["--kube-context", "kind-kubyl-dev"])
    );
    assert!(lines.windows(2).any(|w| w == ["--values", "-"]));
    assert!(lines.contains(&"--dry-run=server"));
    // The output parses into a preview with the Secret masked.
    let preview = kubyl_helm_core::preview::install_preview(&output.stdout, false).unwrap();
    assert_eq!(preview.changes.len(), 1);
    assert!(!preview.changes[0].text.contains("aHVudGVyMg=="));
    assert!(!format!("{output:?}").contains("aHVudGVyMg=="));

    // Without a Kubyl sign-in no token is set (the kubeconfig's own credentials apply), and
    // commands that don't reach the cluster get no kubeconfig.
    cli::run(
        &info,
        Some(&target()),
        repo::search("demo", false),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(!fake.read("args").contains("--kubeconfig"));
    assert!(!fake.read("env").contains("HELM_KUBETOKEN=sha256"));
}

#[tokio::test]
async fn errors_are_mapped_and_passwords_go_on_stdin() {
    let fake = Fake::new("v3.16.2+g13654a5");
    let info = fake.info().await;
    let err = cli::run(&info, None, repo::list_repositories(), None, None)
        .await
        .unwrap_err();
    assert!(repo::is_no_repositories(&err.message), "{err:?}");
    let err = cli::run(
        &info,
        Some(&target()),
        Invocation::cluster(["upgrade", "web", "kubyl-dev/demo"]),
        None,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::InProgress);

    let add = repo::add_repository(&NewRepository {
        name: "private".into(),
        url: "https://charts.example.com".into(),
        username: Some("alice".into()),
        password: Some("s3cret-pw".to_string().into()),
        ..NewRepository::default()
    });
    cli::run(&info, None, add, None, None).await.unwrap();
    assert_eq!(fake.read("stdin"), "s3cret-pw");
    assert!(!fake.read("args").contains("s3cret-pw"));
}

#[tokio::test]
async fn long_commands_time_out_or_cancel() {
    let fake = Fake::new("v3.16.2+g13654a5");
    let info = fake.info().await;
    let slow = || Invocation::cluster(["uninstall", "web"]);
    let err = cli::run(
        &info,
        Some(&target()),
        slow().timeout(Duration::from_millis(300)),
        None,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Timeout);
    let (cancel, rx) = tokio::sync::oneshot::channel();
    let started = std::time::Instant::now();
    let run = tokio::spawn({
        let info = info.clone();
        async move { cli::run(&info, Some(&target()), slow(), None, Some(rx)).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.send(()).unwrap();
    let err = run.await.unwrap().unwrap_err();
    assert_eq!(err.kind, ErrorKind::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(20));
}

/// A fake `helm` that runs `body` (a POSIX shell script).
fn scripted(body: &str) -> (tempfile::TempDir, cli::HelmInfo) {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("helm");
    std::fs::write(&program, format!("#!/bin/sh\n{body}")).unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let info = cli::HelmInfo {
        path: program,
        version: cli::Version {
            major: 4,
            minor: 3,
            patch: 0,
        },
        version_text: "v4.3.0".into(),
        env: Default::default(),
        search_path: Some(std::env::join_paths(["/usr/bin", "/bin"]).unwrap()),
        cli_env: Default::default(),
    };
    (dir, info)
}

fn wait_for(path: &Path, limit: Duration) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < limit {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    path.exists()
}

/// Kubyl quitting mid-write: the runtime, the future and the readers of `helm`'s pipes go away.
/// A write keeps running and may still print to stderr (Helm's warnings come while it works; a
/// closed pipe would kill it with SIGPIPE), a read is killed. Like Helm, the fake prints its
/// result to stdout only once the work is done.
#[test]
fn writes_outlive_kubyl_and_reads_do_not() {
    for (write, args) in [(true, ["install", "web"]), (false, ["status", "web"])] {
        let (dir, info) = scripted(
            "sleep 1\necho 'W1007 warnings.go: something deprecated' >&2\necho 'more' >&2\ntouch \"$(dirname \"$0\")/done\"\necho '{}'\n",
        );
        let done = dir.path().join("done");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let (progress, _lines) = tokio::sync::mpsc::unbounded_channel();
        runtime.spawn(async move {
            cli::run(
                &info,
                Some(&target()),
                Invocation::cluster(args),
                Some(progress),
                None,
            )
            .await
        });
        std::thread::sleep(Duration::from_millis(300));
        runtime.shutdown_background();
        assert_eq!(
            wait_for(&done, Duration::from_secs(5)),
            write,
            "write: {write}"
        );
    }
}

/// An exec plugin (a grandchild) that keeps `helm`'s pipes open doesn't keep the operation (and
/// its progress stream) running: after a cancel, a timeout, or a normal exit.
#[tokio::test]
async fn a_grandchild_holding_the_pipes_does_not_hang_the_run() {
    let (_dir, info) = scripted(
        "(sleep 9) &\ncase \"$1\" in\n  uninstall) exec sleep 30 ;;\n  *) echo '{\"version\":3}' ;;\nesac\n",
    );
    let started = std::time::Instant::now();
    let (progress, mut lines) = tokio::sync::mpsc::unbounded_channel();
    let output = cli::run(
        &info,
        Some(&target()),
        Invocation::cluster(["upgrade", "web", "kubyl-dev/demo"]),
        Some(progress),
        None,
    )
    .await
    .unwrap();
    assert_eq!(output.stdout.trim(), r#"{"version":3}"#);
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(lines.recv().await, None, "the progress stream ended");

    let started = std::time::Instant::now();
    let (progress, mut lines) = tokio::sync::mpsc::unbounded_channel();
    let (cancel, rx) = tokio::sync::oneshot::channel();
    let run = tokio::spawn({
        let info = info.clone();
        async move {
            cli::run(
                &info,
                Some(&target()),
                Invocation::cluster(["uninstall", "web"]),
                Some(progress),
                Some(rx),
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.send(()).unwrap();
    let err = run.await.unwrap().unwrap_err();
    assert_eq!(err.kind, ErrorKind::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(lines.recv().await, None, "the progress stream ended");
}

/// A server-side dry run that isn't allowed (lookups need `get`) falls back to a client-side
/// one; other errors don't.
#[tokio::test]
async fn previews_fall_back_to_a_client_side_dry_run() {
    let (dir, info) = scripted(
        "case \"$*\" in\n  *--dry-run=server*web*|*web*--dry-run=server*) echo 'Error: INSTALLATION FAILED: secrets \"db\" is forbidden: User \"jane\" cannot get resource \"secrets\" in API group \"\" in the namespace \"shop\"' >&2; exit 1 ;;\n  *--dry-run=server*) echo 'Error: INSTALLATION FAILED: chart requires kubeVersion: >=1.40' >&2; exit 1 ;;\n  *--dry-run=client*) cat \"$(dirname \"$0\")/dry-run.json\" ;;\nesac\n",
    );
    std::fs::write(dir.path().join("dry-run.json"), DRY_RUN).unwrap();
    let mut spec = InstallSpec {
        name: "web".into(),
        namespace: "shop".into(),
        create_namespace: false,
        chart: ChartRef::Repo {
            repo: "kubyl-dev".into(),
            name: "demo".into(),
        },
        version: None,
        options: WriteOptions::default(),
    };
    let preview = kubyl_helm_core::preview::install_dry_run(&info, Some(&target()), &spec, "")
        .await
        .unwrap();
    assert!(preview.client_side);
    assert_eq!(
        kubyl_helm_core::preview::rendered_version(&preview).as_deref(),
        Some("0.2.0")
    );
    spec.name = "api".into();
    let err = kubyl_helm_core::preview::install_dry_run(&info, Some(&target()), &spec, "")
        .await
        .unwrap_err();
    assert!(err.contains("kubeVersion"), "{err}");
}

/// No credential store: the OpenShift sign-in below falls back to the kubeconfig's token.
struct EmptyStore;

impl kubyl_kube_core::auth::store::SecretStore for EmptyStore {
    fn name(&self) -> &'static str {
        "empty"
    }
    fn get(&self, _key: &str) -> Result<Option<secrecy::SecretString>, String> {
        Ok(None)
    }
    fn set(&self, _key: &str, _secret: &secrecy::SecretString) -> Result<(), String> {
        Ok(())
    }
    fn delete(&self, _key: &str) -> Result<(), String> {
        Ok(())
    }
}

/// A context that signs in through Kubyl (OpenShift OAuth here; OIDC takes the same path): its
/// token reaches `helm` only in the environment, and only for commands against the cluster.
#[tokio::test]
async fn kubyl_sign_ins_hand_helm_the_token_in_its_environment() {
    use kubyl_kube_core::auth::CredentialSource;
    use kubyl_kube_core::auth::openshift::{OpenShiftAuth, OpenShiftParams};
    kubyl_kube_core::auth::store::install(Box::new(EmptyStore)).ok();
    let fake = Fake::new("v4.3.0+gbec5b06");
    let info = fake.info().await;
    let auth = OpenShiftAuth::shared(
        OpenShiftParams {
            server: "https://api.ocp.example.com:6443".into(),
            user: "fake-helm-test".into(),
            roots: Vec::new(),
            insecure: false,
            proxy: None,
            // The default handle: the desktop's one user, whose kubeconfig token may stand in.
            credentials: kubyl_kube_core::auth::Credentials::default(),
        },
        Some("sha256~SIGNED-IN-TOKEN".to_string().into()),
    );
    let target = CliTarget::new("/home/me/kube/ocp.yaml", "ocp")
        .with_server(Some("https://api.ocp.example.com:6443".into()))
        .with_sign_in(Some(CredentialSource::OpenShift(auth)));
    cli::run(
        &info,
        Some(&target),
        cmd::status("web", "shop", kubyl_helm_core::decode::Driver::Secret),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(
        fake.read("env")
            .contains("HELM_KUBETOKEN=sha256~SIGNED-IN-TOKEN")
    );
    assert!(
        fake.read("env")
            .contains("HELM_KUBEAPISERVER=https://api.ocp.example.com:6443")
    );
    assert!(!fake.read("args").contains("SIGNED-IN-TOKEN"));
    cli::run(&info, Some(&target), repo::list_repositories(), None, None)
        .await
        .ok();
    assert!(!fake.read("env").contains("SIGNED-IN-TOKEN"));
}

/// A scoped credentials handle (someone other than the kubeconfig's author) without a sign-in:
/// `helm` doesn't run at all, so it can't fall back to the tokens written into the kubeconfig.
#[tokio::test]
async fn scoped_handles_without_a_sign_in_never_run_helm_on_the_kubeconfigs_tokens() {
    use kubyl_kube_core::auth::openshift::{OpenShiftAuth, OpenShiftParams};
    use kubyl_kube_core::auth::{CredentialSource, Credentials};
    kubyl_kube_core::auth::store::install(Box::new(EmptyStore)).ok();
    let fake = Fake::new("v4.3.0+gbec5b06");
    let info = fake.info().await;
    let scoped = Credentials::scoped("fake-helm-alice");
    let auth = OpenShiftAuth::shared(
        OpenShiftParams {
            server: "https://api.ocp.example.com:6443".into(),
            user: "fake-helm-scoped".into(),
            roots: Vec::new(),
            insecure: false,
            proxy: None,
            credentials: scoped.clone(),
        },
        // What `client::build` hands a scoped handle: never the kubeconfig's token.
        None,
    );
    let target = CliTarget::new("/home/me/kube/ocp.yaml", "ocp")
        .with_sign_in(Some(CredentialSource::OpenShift(auth)))
        .with_credentials(&scoped, true);
    let err = cli::run(
        &info,
        Some(&target),
        cmd::status("web", "shop", kubyl_helm_core::decode::Driver::Secret),
        None,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::SignInRequired, "{err:?}");
    // The probe ran `helm env` last: nothing ran against the cluster.
    assert!(!fake.read("args").contains("--kubeconfig"));
    // Commands that don't reach the cluster still run.
    cli::run(
        &info,
        Some(&target),
        repo::search("demo", false),
        None,
        None,
    )
    .await
    .unwrap();
}
