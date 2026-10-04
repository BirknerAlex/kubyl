//! Operators without the UI.
//!
//! - [`olm`]: OLM v0 (`operators.coreos.com`) objects, how they join into installed operators,
//!   the upgrade review, OperatorHub, installs and uninstalls; OLM v1 in [`olm::v1`].
//! - [`helm`]: Helm v3 releases from their storage objects.
//! - [`api`]: the installed-operator types other crates read.
//! - [`errors`]: user-facing API error messages.
//!
//! `kubyl_operators` re-exports these modules and adds the OLM and Helm caches, views and
//! dialogs.

pub mod api;
pub mod errors;
pub mod helm;
pub mod olm;
