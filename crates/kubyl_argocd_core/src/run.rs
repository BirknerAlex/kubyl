//! What can be done to an Application, and its labels, confirmations and messages.

use kubyl_base::ResourceRef;

use crate::ops::{AppTarget, Cascade, PolicyChange, SyncRequest};

/// What to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Refresh {
        hard: bool,
    },
    Sync(SyncRequest),
    Rollback {
        id: i64,
        prune: bool,
        dry_run: bool,
        /// Turn auto-sync off first (the rollback dialog's checkbox).
        disable_auto_sync: bool,
    },
    Terminate,
    Policy(PolicyChange),
    Delete(Cascade),
}

impl Op {
    /// `Sync`, `Rollback`… for messages.
    pub fn label(&self) -> &'static str {
        match self {
            Op::Refresh { hard: false } => "Refresh",
            Op::Refresh { hard: true } => "Hard refresh",
            Op::Sync(request) if request.dry_run => "Dry run",
            Op::Sync(_) => "Sync",
            Op::Rollback { .. } => "Rollback",
            Op::Terminate => "Terminate",
            Op::Policy(PolicyChange::AutoSync(true)) => "Enable auto-sync",
            Op::Policy(PolicyChange::AutoSync(false)) => "Disable auto-sync",
            Op::Policy(PolicyChange::Prune(_)) => "Prune setting",
            Op::Policy(PolicyChange::SelfHeal(_)) => "Self-heal setting",
            Op::Delete(_) => "Delete",
        }
    }

    /// Whether the UI asks first: Terminate and switching a sync policy on (auto-sync, prune and
    /// self-heal can delete or overwrite live resources). Switching one off is harmless.
    pub fn needs_confirm(&self) -> bool {
        matches!(
            self,
            Op::Terminate
                | Op::Policy(
                    PolicyChange::AutoSync(true)
                        | PolicyChange::Prune(true)
                        | PolicyChange::SelfHeal(true)
                )
        )
    }

    /// The Kubernetes verbs Kubernetes mode needs (a delete first patches the finalizers).
    pub fn verbs(&self) -> &'static [&'static str] {
        match self {
            Op::Delete(_) => &["patch", "delete"],
            _ => &["patch"],
        }
    }

    /// Done message.
    pub fn done(&self, app: &str) -> String {
        match self {
            Op::Refresh { .. } => format!("Refreshing {app}"),
            Op::Sync(request) if request.dry_run => format!("Dry run of {app} started"),
            Op::Sync(_) => format!("Sync of {app} started"),
            Op::Rollback { dry_run: true, .. } => format!("Rollback dry run of {app} started"),
            Op::Rollback { .. } => format!("Rollback of {app} started"),
            Op::Terminate => format!("Terminating the operation of {app}"),
            Op::Policy(PolicyChange::AutoSync(on)) => {
                format!("Auto-sync {} for {app}", if *on { "on" } else { "off" })
            }
            Op::Policy(PolicyChange::Prune(on)) => {
                format!("Prune {} for {app}", if *on { "on" } else { "off" })
            }
            Op::Policy(PolicyChange::SelfHeal(on)) => {
                format!("Self-heal {} for {app}", if *on { "on" } else { "off" })
            }
            Op::Delete(Cascade::None) => format!("{app} deleted; its resources stay"),
            Op::Delete(_) => format!("Deleting {app} and its resources"),
        }
    }
}

/// The app an object ref points at.
pub fn app_target(target: &ResourceRef) -> Option<AppTarget> {
    Some(AppTarget::new(
        target.namespace.clone()?,
        target.name.clone()?,
    ))
}
