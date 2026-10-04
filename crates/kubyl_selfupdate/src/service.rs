//! [`SelfUpdate`]: the app-wide self-update state ([`SelfUpdateCore`] from
//! `kubyl_selfupdate_core`). Polls the signed manifest on a timer and restarts on request.

use std::convert::Infallible;
use std::ops::Deref;
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, Task};
use kubyl_core::host::{Hosts, hosted};
pub use kubyl_selfupdate_core::service::{SelfUpdateCore, UpdateState};

use crate::installed;
use crate::settings::SelfUpdateSettings;

/// How often the poll loop checks the manifest while idle.
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

pub struct SelfUpdate {
    core: SelfUpdateCore,
    /// The poll loop; kept so it isn't dropped from inside itself.
    _poll: Option<Task<()>>,
}

impl Deref for SelfUpdate {
    type Target = SelfUpdateCore;

    fn deref(&self) -> &SelfUpdateCore {
        &self.core
    }
}

impl EventEmitter<Infallible> for SelfUpdate {}

impl Hosts<SelfUpdateCore> for SelfUpdate {
    fn service(&mut self) -> &mut SelfUpdateCore {
        &mut self.core
    }

    fn apply(&mut self, effect: Infallible, _cx: &mut Context<Self>) {
        match effect {}
    }
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
                core: SelfUpdateCore::new(install_kind),
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

    fn start_poll(&mut self, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            loop {
                let should_check = this
                    .update(cx, |this, _| this.core.should_check())
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
        let channel = kubyl_settings::Settings::get::<SelfUpdateSettings>(cx).channel;
        hosted(self, cx, |core, host| core.check(channel, host));
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
