//! [`SelfUpdate`]: the app-wide self-update state. Polls the signed manifest on a timer,
//! downloads and verifies the artifact once a newer version shows up, and waits for the user
//! to restart (never restarts on its own — a Kubernetes desktop client shouldn't vanish out
//! from under someone mid-task).

use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use semver::Version;

use crate::download::{self, FetchError};
use crate::installed::{self, InstallKind};
use crate::manifest::UpdateManifest;
use crate::settings::SelfUpdateSettings;

/// How often the poll loop checks the manifest while idle.
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// What a view (the status bar item) shows.
#[derive(Clone, Debug)]
pub enum UpdateState {
    /// Either this install can't self-update ([`InstallKind::PackageManager`]) or nothing has
    /// run yet.
    Idle,
    Checking,
    UpToDate,
    Available(UpdateManifest),
    Downloading(UpdateManifest),
    ReadyToRestart(UpdateManifest),
    Failed(String),
}

pub struct SelfUpdate {
    install_kind: InstallKind,
    state: UpdateState,
    /// The poll loop; kept so it isn't dropped from inside itself.
    _poll: Option<Task<()>>,
}

struct GlobalSelfUpdate(Entity<SelfUpdate>);

impl Global for GlobalSelfUpdate {}

impl SelfUpdate {
    /// Installs the global and, when this install can self-update, starts the poll loop.
    /// `poll`: off in GPUI tests, same convention as `kubyl_updates::Updates::install`.
    pub fn install(poll: bool, cx: &mut App) -> Entity<Self> {
        let install_kind = installed::detect();
        let entity = cx.new(|cx: &mut Context<Self>| {
            let mut this = Self {
                install_kind,
                state: UpdateState::Idle,
                _poll: None,
            };
            if poll && install_kind.can_self_update() {
                this.start_poll(cx);
            }
            this
        });
        cx.set_global(GlobalSelfUpdate(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalSelfUpdate>().map(|g| g.0.clone())
    }

    pub fn install_kind(&self) -> InstallKind {
        self.install_kind
    }

    pub fn state(&self) -> &UpdateState {
        &self.state
    }

    fn start_poll(&mut self, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            loop {
                let should_check = this
                    .update(cx, |this, _| {
                        !matches!(
                            this.state,
                            UpdateState::Checking
                                | UpdateState::Downloading(_)
                                | UpdateState::ReadyToRestart(_)
                        )
                    })
                    .unwrap_or(false);
                if should_check {
                    let auto_check = this
                        .update(cx, |_, cx| {
                            cx.has_global::<kubyl_settings::Settings>()
                                && kubyl_settings::Settings::get::<SelfUpdateSettings>(cx)
                                    .auto_check
                        })
                        .unwrap_or(false);
                    if auto_check {
                        let _ = this.update(cx, |this, cx| this.check(cx));
                    }
                }
                cx.background_executor().timer(CHECK_INTERVAL).await;
            }
        });
        self._poll = Some(task);
    }

    /// Checks the manifest now (also called by the poll loop). A check already in flight, or a
    /// download already running or done, is left alone.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        if !self.install_kind.can_self_update() {
            return;
        }
        if matches!(
            self.state,
            UpdateState::Checking | UpdateState::Downloading(_) | UpdateState::ReadyToRestart(_)
        ) {
            return;
        }
        self.state = UpdateState::Checking;
        cx.notify();
        let channel = kubyl_settings::Settings::get::<SelfUpdateSettings>(cx).channel;
        let task = kubyl_core::spawn_kube(cx, async move {
            let manifest = download::fetch_manifest(channel).await?;
            Ok::<UpdateManifest, FetchError>(manifest)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| this.on_checked(result, cx)).ok();
        })
        .detach();
    }

    fn on_checked(&mut self, result: Result<UpdateManifest, FetchError>, cx: &mut Context<Self>) {
        match result {
            Ok(manifest) => {
                let current: Version = env!("CARGO_PKG_VERSION")
                    .parse()
                    .expect("CARGO_PKG_VERSION is always a valid semver");
                if manifest.version > current {
                    self.state = UpdateState::Available(manifest.clone());
                    self.download(manifest, cx);
                } else {
                    self.state = UpdateState::UpToDate;
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "self-update check failed");
                self.state = UpdateState::Failed(err.to_string());
            }
        }
        cx.notify();
    }

    fn download(&mut self, manifest: UpdateManifest, cx: &mut Context<Self>) {
        self.state = UpdateState::Downloading(manifest.clone());
        cx.notify();
        let for_task = manifest.clone();
        let task = kubyl_core::spawn_kube(cx, async move {
            let bytes = download::fetch_artifact(&for_task).await?;
            // Archive extraction and (on macOS) a `codesign --verify` subprocess are
            // synchronous; run them on the blocking pool so they don't stall this runtime's
            // worker threads, which also carry Kubernetes API calls.
            run_blocking(move || crate::apply::apply(&bytes))
                .await
                .inspect_err(|err| tracing::error!(error = %err, "self-update apply failed"))?;
            Ok::<(), ApplyOrFetch>(())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.state = match result {
                    Ok(()) => UpdateState::ReadyToRestart(manifest),
                    Err(err) => UpdateState::Failed(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Relaunches the (now-replaced) executable and quits this process. The caller is the
    /// "Restart to update" click — never automatic.
    pub fn restart(cx: &mut App) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        if std::process::Command::new(exe).spawn().is_ok() {
            cx.quit();
        }
    }
}

/// Runs `work` on the blocking pool. A panic in it becomes an error (shown as a failed update)
/// instead of being re-raised into the caller.
async fn run_blocking<T, E>(
    work: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> Result<T, ApplyOrFetch>
where
    T: Send + 'static,
    E: Send + 'static,
    ApplyOrFetch: From<E>,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result.map_err(ApplyOrFetch::from),
        Err(err) => Err(ApplyOrFetch::Panicked(err.to_string())),
    }
}

#[derive(Debug, thiserror::Error)]
enum ApplyOrFetch {
    #[error("the update task failed: {0}")]
    Panicked(String),
    #[error(transparent)]
    Fetch(#[from] FetchError),
    #[error(transparent)]
    Apply(#[from] crate::apply::ApplyError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Channel;
    use gpui::TestAppContext;
    use std::collections::BTreeMap;

    fn manifest(version: &str) -> UpdateManifest {
        UpdateManifest {
            version: version.parse().unwrap(),
            channel: Channel::Stable,
            notes_url: "https://example.com".into(),
            platforms: BTreeMap::new(),
        }
    }

    #[test]
    fn a_panic_while_applying_is_an_error_not_a_crash() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let result = runtime.block_on(run_blocking(|| -> Result<(), crate::apply::ApplyError> {
            panic!("boom")
        }));
        assert!(matches!(result, Err(ApplyOrFetch::Panicked(_))));
        let ok = runtime.block_on(run_blocking(|| Ok::<_, crate::apply::ApplyError>(3)));
        assert!(matches!(ok, Ok(3)));
    }

    #[gpui::test]
    fn a_lower_manifest_version_reports_up_to_date(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let update = cx.update(|cx| {
            kubyl_settings::init_with_dir(cx, dir.path());
            SelfUpdate::install(false, cx)
        });
        update.update(cx, |this, cx| {
            this.on_checked(Ok(manifest("0.0.1")), cx);
            assert!(matches!(this.state(), UpdateState::UpToDate));
        });
    }

    #[gpui::test]
    fn a_higher_manifest_version_moves_toward_downloading(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let update = cx.update(|cx| {
            kubyl_settings::init_with_dir(cx, dir.path());
            SelfUpdate::install(false, cx)
        });
        update.update(cx, |this, cx| {
            this.on_checked(Ok(manifest("999.0.0")), cx);
            // on_checked kicks off `download`, which needs the platform artifact to exist to
            // finish; without one it stays in `Downloading` for this synchronous assertion.
            assert!(matches!(this.state(), UpdateState::Downloading(_)));
        });
    }
}
