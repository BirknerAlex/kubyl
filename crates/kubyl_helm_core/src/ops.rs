//! [`HelmOpsCore`]: the Helm writes that are running or just finished. They live here, not in
//! the dialog that started them: closing a dialog doesn't cancel an install (killing `helm`
//! midway would leave the release pending). Cancelling interrupts `helm` like Ctrl-C, so Helm
//! records the release's state.
//!
//! A plain service on a [`Host`]; `kubyl_helm::ops::HelmOps` hosts it in GPUI.

use std::convert::Infallible;
use std::time::Instant;

use kubyl_base::host::{Flow, Host, HostExt as _, Pace, Service, TaskHandle};
use kubyl_base::{ClusterId, Notice};
use kubyl_kube_core::cli::CliTarget;
use tokio::sync::{mpsc, oneshot};

use crate::cli::{self, HelmError, HelmInfo, Invocation};

/// Helm's last lines an operation keeps.
const MAX_LINES: usize = 400;

/// What an operation does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Install,
    Upgrade,
    Rollback,
    Uninstall,
}

impl OpKind {
    pub fn verb(self) -> &'static str {
        match self {
            OpKind::Install => "Installing",
            OpKind::Upgrade => "Upgrading",
            OpKind::Rollback => "Rolling back",
            OpKind::Uninstall => "Uninstalling",
        }
    }

    pub fn done(self) -> &'static str {
        match self {
            OpKind::Install => "Installed",
            OpKind::Upgrade => "Upgraded",
            OpKind::Rollback => "Rolled back",
            OpKind::Uninstall => "Uninstalled",
        }
    }
}

/// Where an operation is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpState {
    Running,
    /// Done; the new revision for installs and upgrades (from Helm's JSON output).
    Done {
        revision: Option<u32>,
    },
    Failed(HelmError),
}

/// One Helm write.
pub struct Operation {
    pub id: u64,
    pub cluster: ClusterId,
    pub kind: OpKind,
    pub namespace: String,
    pub release: String,
    pub state: OpState,
    /// Helm's stderr while it runs (scrubbed), for the dialog.
    pub lines: Vec<String>,
    pub started: Instant,
    pub finished: Option<Instant>,
    cancel: Option<oneshot::Sender<()>>,
    _work: TaskHandle,
    _lines: TaskHandle,
}

impl Operation {
    pub fn is_running(&self) -> bool {
        self.state == OpState::Running
    }

    /// `Installing web in shop`.
    pub fn title(&self) -> String {
        format!(
            "{} {} in {}",
            self.kind.verb(),
            self.release,
            self.namespace
        )
    }
}

/// What to start.
pub struct Start {
    pub cluster: ClusterId,
    pub kind: OpKind,
    pub namespace: String,
    pub release: String,
    pub helm: HelmInfo,
    pub target: Option<CliTarget>,
    pub invocation: Invocation,
}

/// The running and recent Helm writes.
#[derive(Default)]
pub struct HelmOpsCore {
    ops: Vec<Operation>,
    next: u64,
}

impl Service for HelmOpsCore {
    type Event = Infallible;
    type Effect = Infallible;
}

impl HelmOpsCore {
    pub fn get(&self, id: u64) -> Option<&Operation> {
        self.ops.iter().find(|op| op.id == id)
    }

    /// The running operation on a release, if any (the release's tab shows it).
    pub fn running_on(
        &self,
        cluster: &ClusterId,
        namespace: &str,
        release: &str,
    ) -> Option<&Operation> {
        self.ops.iter().find(|op| {
            op.is_running()
                && &op.cluster == cluster
                && op.namespace == namespace
                && op.release == release
        })
    }

    /// Starts a write on Tokio. Returns its id.
    pub fn start(&mut self, start: Start, host: &mut dyn Host<Self>) -> u64 {
        // Keep the last few finished ones (pruned here, never from a callback of their own).
        while self.ops.iter().filter(|op| !op.is_running()).count() >= 20 {
            let Some(ix) = self.ops.iter().position(|op| !op.is_running()) else {
                break;
            };
            self.ops.remove(ix);
        }
        self.next += 1;
        let id = self.next;
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (lines_tx, lines_rx) = mpsc::unbounded_channel::<String>();
        let Start {
            cluster,
            kind,
            namespace,
            release,
            helm,
            target,
            invocation,
        } = start;
        // Helm's lines as they come; the stream ends when `cli::run` lets go of the sender
        // (it does even when a grandchild keeps the pipe open).
        let lines = futures::stream::unfold(lines_rx, |mut rx| async move {
            rx.recv().await.map(|line| (line, rx))
        });
        let lines_task = host.batches(lines, Pace::IMMEDIATE, move |this, batch, host| {
            if let Some(op) = this.ops.iter_mut().find(|op| op.id == id) {
                op.lines.extend(batch);
                if op.lines.len() > MAX_LINES {
                    let extra = op.lines.len() - MAX_LINES + 100;
                    op.lines.drain(..extra);
                }
                host.notify();
            }
            Flow::Continue
        });
        let work = host.spawn(
            async move {
                cli::run(
                    &helm,
                    target.as_ref(),
                    invocation,
                    Some(lines_tx),
                    Some(cancel_rx),
                )
                .await
            },
            move |this, result, host| this.finish(id, result, host),
        );
        self.ops.push(Operation {
            id,
            cluster,
            kind,
            namespace,
            release,
            state: OpState::Running,
            lines: Vec::new(),
            started: Instant::now(),
            finished: None,
            cancel: Some(cancel_tx),
            _work: work,
            _lines: lines_task,
        });
        host.notify();
        id
    }

    fn finish(
        &mut self,
        id: u64,
        result: Result<cli::Output, HelmError>,
        host: &mut dyn Host<Self>,
    ) {
        let Some(op) = self.ops.iter_mut().find(|op| op.id == id) else {
            return;
        };
        op.cancel = None;
        op.finished = Some(Instant::now());
        op.state = match result {
            Ok(output) => OpState::Done {
                revision: serde_json::from_str::<serde_json::Value>(&output.stdout)
                    .ok()
                    .and_then(|v| v.get("version")?.as_u64())
                    .and_then(|v| u32::try_from(v).ok()),
            },
            Err(err) => OpState::Failed(err),
        };
        let notice = match &op.state {
            OpState::Done { revision } => Notice::info(match revision {
                Some(revision) => format!(
                    "{} {} in {} (revision {revision}).",
                    op.kind.done(),
                    op.release,
                    op.namespace
                ),
                None => format!("{} {} in {}.", op.kind.done(), op.release, op.namespace),
            }),
            OpState::Failed(err) => Notice::error(format!(
                "{} {} failed: {}",
                op.kind.verb(),
                op.release,
                err.message
            )),
            OpState::Running => return,
        };
        host.toast(notice);
        host.notify();
    }

    /// Interrupts a running operation (Helm records the release's state).
    pub fn cancel(&mut self, id: u64, host: &mut dyn Host<Self>) {
        if let Some(op) = self.ops.iter_mut().find(|op| op.id == id)
            && let Some(cancel) = op.cancel.take()
        {
            cancel.send(()).ok();
            op.lines.push("Cancelling…".into());
            host.notify();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Duration;

    use kubyl_base::host::TestHost;

    use super::*;
    use crate::cli::ErrorKind;

    /// A fake `helm`: upgrades print a line and the new release, uninstalls hang (with an exec
    /// plugin-like grandchild holding the pipes) until interrupted.
    fn fake() -> (tempfile::TempDir, HelmInfo) {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("helm");
        std::fs::write(
            &program,
            "#!/bin/sh\ncase \"$1\" in\n  upgrade) echo 'upgrading web' >&2; echo '{\"version\":4}' ;;\n  uninstall) (sleep 8) & exec sleep 30 ;;\nesac\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let info = HelmInfo {
            path: program,
            version: crate::cli::Version {
                major: 4,
                minor: 3,
                patch: 0,
            },
            version_text: "v4.3.0".into(),
            env: Default::default(),
            search_path: Some(std::env::join_paths(["/usr/bin", "/bin"]).unwrap()),
        };
        (dir, info)
    }

    fn start(kind: OpKind, args: &[&str], helm: &HelmInfo) -> Start {
        Start {
            cluster: ClusterId::new("kind-dev"),
            kind,
            namespace: "shop".into(),
            release: "web".into(),
            helm: helm.clone(),
            target: Some(CliTarget::new("/dev/null", "fake")),
            invocation: Invocation::cluster(args.iter().copied()),
        }
    }

    #[test]
    fn writes_report_their_lines_and_the_new_revision() {
        let (_dir, helm) = fake();
        let mut ops = HelmOpsCore::default();
        let mut host = TestHost::new();
        let id = ops.start(
            start(OpKind::Upgrade, &["upgrade", "web"], &helm),
            &mut host,
        );
        assert!(ops.get(id).unwrap().is_running());
        let cluster = ClusterId::new("kind-dev");
        assert!(ops.running_on(&cluster, "shop", "web").is_some());
        host.run_until_idle(&mut ops);
        let op = ops.get(id).unwrap();
        assert_eq!(op.state, OpState::Done { revision: Some(4) });
        assert_eq!(op.lines, ["upgrading web"]);
        assert!(op.finished.is_some());
        assert!(ops.running_on(&cluster, "shop", "web").is_none());
        assert_eq!(
            host.notices[0].message,
            "Upgraded web in shop (revision 4)."
        );
    }

    #[test]
    fn cancelling_ends_the_operation_even_when_a_grandchild_holds_the_pipes() {
        let (_dir, helm) = fake();
        let mut ops = HelmOpsCore::default();
        let mut host = TestHost::new();
        let id = ops.start(
            start(OpKind::Uninstall, &["uninstall", "web"], &helm),
            &mut host,
        );
        std::thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        ops.cancel(id, &mut host);
        host.run_until_idle(&mut ops);
        let op = ops.get(id).unwrap();
        assert!(
            matches!(&op.state, OpState::Failed(err) if err.kind == ErrorKind::Cancelled),
            "{:?}",
            op.state
        );
        assert!(started.elapsed() < Duration::from_secs(7));
        assert!(op.lines.iter().any(|l| l == "Cancelling…"));
    }

    #[test]
    fn helm_is_probed_again_when_its_path_changes() {
        use crate::cli::{CliState, HelmCliCore, Probe};
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("helm");
        std::fs::write(
            &program,
            "#!/bin/sh\n[ \"$1\" = version ] && echo v3.19.0+g3d8990f\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([dir.path(), std::path::Path::new("/bin")]).unwrap();
        let mut cli = HelmCliCore::new(None);
        let mut host = TestHost::new();
        assert_eq!(cli.state(), &CliState::Probing);
        cli.probe_in(Some(path), &mut host);
        host.run_until_idle(&mut cli);
        assert_eq!(cli.info().unwrap().version.to_string(), "3.19.0");
        // Unchanged: no new probe.
        cli.set_configured(None, &mut host);
        assert!(cli.info().is_some());
        cli.set_configured(Some("/nowhere/helm".into()), &mut host);
        assert_eq!(cli.state(), &CliState::Probing);
        host.run_until_idle(&mut cli);
        assert!(matches!(
            cli.probe(),
            Some(Probe::Missing {
                configured: Some(_)
            })
        ));
    }
}
