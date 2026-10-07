//! [`HelmOps`]: hosts [`HelmOpsCore`] (the Helm writes that run or just finished) in GPUI. They
//! live here, not in the dialog that started them: closing a dialog doesn't cancel an install.

use std::convert::Infallible;
use std::ops::Deref;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global};
use kubyl_core::host::{Hosts, hosted};
pub use kubyl_helm_core::ops::{HelmOpsCore, OpKind, OpState, Operation, Start};

/// The running and recent Helm writes.
#[derive(Default)]
pub struct HelmOps {
    core: HelmOpsCore,
}

impl Deref for HelmOps {
    type Target = HelmOpsCore;

    fn deref(&self) -> &HelmOpsCore {
        &self.core
    }
}

impl EventEmitter<Infallible> for HelmOps {}

impl Hosts<HelmOpsCore> for HelmOps {
    fn service(&mut self) -> &mut HelmOpsCore {
        &mut self.core
    }

    fn apply(&mut self, effect: Infallible, _cx: &mut Context<Self>) {
        match effect {}
    }
}

struct GlobalHelmOps(Entity<HelmOps>);

impl Global for GlobalHelmOps {}

impl HelmOps {
    pub fn install(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self::default());
        cx.set_global(GlobalHelmOps(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalHelmOps>().map(|g| g.0.clone())
    }

    /// Starts a write on Tokio. Returns its id.
    pub fn start(&mut self, start: Start, cx: &mut Context<Self>) -> u64 {
        hosted(self, cx, |core, host| core.start(start, host))
    }

    /// Interrupts a running operation (Helm records the release's state).
    pub fn cancel(&mut self, id: u64, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.cancel(id, host));
    }
}

/// Starts an operation from a dialog (`None` when Helm isn't set up).
pub fn start(start: Start, cx: &mut App) -> Option<u64> {
    let ops = HelmOps::global(cx)?;
    Some(ops.update(cx, |ops, cx| ops.start(start, cx)))
}
