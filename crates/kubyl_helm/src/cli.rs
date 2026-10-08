//! [`HelmCli`]: hosts [`HelmCliCore`] (the user's `helm`, probed once on Tokio, again when
//! `helm.path` changes or the user asks) in GPUI, and how to reach a cluster with it.

use std::convert::Infallible;
use std::ops::Deref;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global};
use kubyl_core::ClusterId;
use kubyl_core::host::{Hosts, hosted};
pub use kubyl_helm_core::cli::{CliState, HelmCliCore};
use kubyl_helm_core::cli::{HelmInfo, Probe};
use kubyl_helm_core::settings::HelmSettings;
use kubyl_kube::ConnectionManager;
use kubyl_kube::cli::CliTarget;
use kubyl_settings::Settings;

pub struct HelmCli {
    core: HelmCliCore,
    _settings: Option<gpui::Subscription>,
}

impl Deref for HelmCli {
    type Target = HelmCliCore;

    fn deref(&self) -> &HelmCliCore {
        &self.core
    }
}

impl EventEmitter<Infallible> for HelmCli {}

impl Hosts<HelmCliCore> for HelmCli {
    fn service(&mut self) -> &mut HelmCliCore {
        &mut self.core
    }

    fn apply(&mut self, effect: Infallible, _cx: &mut Context<Self>) {
        match effect {}
    }
}

struct GlobalHelmCli(Entity<HelmCli>);

impl Global for GlobalHelmCli {}

impl HelmCli {
    /// Installs the global. `probe`: look for `helm` now (off in GPUI tests, which set a probe).
    pub fn install(probe: bool, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx: &mut Context<Self>| {
            let settings_sub = cx.has_global::<Settings>().then(|| {
                cx.observe_global::<Settings>(|this: &mut Self, cx| {
                    let configured = self::settings(cx).path.clone();
                    hosted(this, cx, |core, host| core.set_configured(configured, host));
                })
            });
            let mut this = Self {
                core: HelmCliCore::new(settings(cx).path.clone()),
                _settings: settings_sub,
            };
            if probe {
                this.reprobe(cx);
            }
            this
        });
        cx.set_global(GlobalHelmCli(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalHelmCli>().map(|g| g.0.clone())
    }

    /// The usable `helm`, if any.
    pub fn info(cx: &App) -> Option<HelmInfo> {
        Self::global(cx)?.read(cx).core.info().cloned()
    }

    /// Looks for `helm` again (after installing it, or changing `helm.path`).
    pub fn reprobe(&mut self, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.reprobe(host));
    }

    /// Sets the probe's answer (tests, screenshots of the missing state).
    pub fn set_probe(&mut self, probe: Probe, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.set_probe(probe, host));
    }
}

/// The `"helm"` settings.
pub fn settings(cx: &App) -> HelmSettings {
    if cx.has_global::<Settings>() {
        Settings::get::<HelmSettings>(cx).clone()
    } else {
        HelmSettings::default()
    }
}

/// How `helm` reaches `cluster` (kubeconfig, context, Kubyl's sign-in).
pub fn target(cluster: &ClusterId, cx: &App) -> Option<CliTarget> {
    ConnectionManager::try_global(cx)?
        .read(cx)
        .cli_target(cluster)
}

/// Whether Kubyl treats `cluster` as read-only (Helm write actions are hidden).
pub fn read_only(cluster: &ClusterId, cx: &App) -> bool {
    ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(cluster).read_only)
}
