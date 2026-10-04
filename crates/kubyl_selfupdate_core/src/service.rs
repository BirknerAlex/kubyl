//! [`SelfUpdateCore`]: the self-update state machine. Checks the signed manifest, downloads and
//! verifies the artifact once a newer version shows up, and waits for the user to restart
//! (never restarts on its own — a Kubernetes desktop client shouldn't vanish out from under
//! someone mid-task). `kubyl_selfupdate::SelfUpdate` holds it and runs the poll loop.

use std::convert::Infallible;

use kubyl_base::host::{Host, HostExt as _, Service};
use semver::Version;

use crate::download::{self, FetchError};
use crate::installed::InstallKind;
use crate::manifest::{Channel, UpdateManifest};

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

pub struct SelfUpdateCore {
    install_kind: InstallKind,
    state: UpdateState,
}

impl Service for SelfUpdateCore {
    type Event = Infallible;
    type Effect = Infallible;
}

impl SelfUpdateCore {
    pub fn new(install_kind: InstallKind) -> Self {
        Self {
            install_kind,
            state: UpdateState::Idle,
        }
    }

    pub fn install_kind(&self) -> InstallKind {
        self.install_kind
    }

    pub fn state(&self) -> &UpdateState {
        &self.state
    }

    /// Whether a scheduled check should run: nothing in flight, downloading or done.
    pub fn should_check(&self) -> bool {
        !matches!(
            self.state,
            UpdateState::Checking | UpdateState::Downloading(_) | UpdateState::ReadyToRestart(_)
        )
    }

    /// Checks the manifest of `channel` now. A check already in flight, or a download already
    /// running or done, is left alone.
    pub fn check(&mut self, channel: Channel, host: &mut dyn Host<Self>) {
        if !self.install_kind.can_self_update() || !self.should_check() {
            return;
        }
        self.state = UpdateState::Checking;
        host.notify();
        host.spawn(
            async move {
                let manifest = download::fetch_manifest(channel).await?;
                Ok::<UpdateManifest, FetchError>(manifest)
            },
            |this, result, host| this.on_checked(result, host),
        )
        .detach();
    }

    pub fn on_checked(
        &mut self,
        result: Result<UpdateManifest, FetchError>,
        host: &mut dyn Host<Self>,
    ) {
        match result {
            Ok(manifest) => {
                let current: Version = env!("CARGO_PKG_VERSION")
                    .parse()
                    .expect("CARGO_PKG_VERSION is always a valid semver");
                if manifest.version > current {
                    self.state = UpdateState::Available(manifest.clone());
                    self.download(manifest, host);
                } else {
                    self.state = UpdateState::UpToDate;
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "self-update check failed");
                self.state = UpdateState::Failed(err.to_string());
            }
        }
        host.notify();
    }

    fn download(&mut self, manifest: UpdateManifest, host: &mut dyn Host<Self>) {
        self.state = UpdateState::Downloading(manifest.clone());
        host.notify();
        let for_task = manifest.clone();
        host.spawn(
            async move {
                let bytes = download::fetch_artifact(&for_task).await?;
                // Archive extraction and (on macOS) a `codesign --verify` subprocess are
                // synchronous; run them on the blocking pool so they don't stall this runtime's
                // worker threads, which also carry Kubernetes API calls.
                run_blocking(move || crate::apply::apply(&bytes))
                    .await
                    .inspect_err(|err| tracing::error!(error = %err, "self-update apply failed"))?;
                Ok::<(), ApplyOrFetch>(())
            },
            move |this, result, host| {
                this.state = match result {
                    Ok(()) => UpdateState::ReadyToRestart(manifest),
                    Err(err) => UpdateState::Failed(err.to_string()),
                };
                host.notify();
            },
        )
        .detach();
    }
}

/// Runs `work` on the blocking pool. A panic in it becomes an error (shown as a failed update)
/// instead of being re-raised into the caller.
pub async fn run_blocking<T, E>(
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
pub enum ApplyOrFetch {
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
    use kubyl_base::host::TestHost;
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

    #[test]
    fn versions_decide_between_up_to_date_and_downloading() {
        let mut host = TestHost::new();
        let mut update = SelfUpdateCore::new(InstallKind::Manual);
        update.on_checked(Ok(manifest("0.0.1")), &mut host);
        assert!(matches!(update.state(), UpdateState::UpToDate));
        update.on_checked(Ok(manifest("999.0.0")), &mut host);
        assert!(matches!(update.state(), UpdateState::Downloading(_)));
    }
}
