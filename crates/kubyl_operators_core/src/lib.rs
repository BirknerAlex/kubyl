//! Operators without the UI.
//!
//! - [`olm`]: OLM v0 (`operators.coreos.com`) objects, how they join into installed operators,
//!   the upgrade review, OperatorHub, installs and uninstalls; OLM v1 in [`olm::v1`].
//! - [`helm`]: Helm v3 releases, re-exported from `kubyl_helm_core` (phase 22 moved them).
//! - [`api`]: the installed-operator types other crates read.
//! - [`errors`]: user-facing API error messages (from `kubyl_resources_core`).
//!
//! `kubyl_operators` re-exports these modules and adds the OLM cache, views and dialogs.

pub mod api;
pub mod olm;

pub use kubyl_resources_core::errors;

/// Helm v3 releases: phase 22 moved them to `kubyl_helm_core`; the old paths stay.
pub mod helm {
    pub use kubyl_helm_core::{decode, present, release, service};
}
