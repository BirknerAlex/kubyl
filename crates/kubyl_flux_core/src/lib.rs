//! Flux CD without the UI (phase 23).
//!
//! - [`kinds`]: the Flux kinds, their groups and controllers.
//! - [`model`]: status parsing shared by every kind (conditions, suspend, revisions) and the
//!   state it adds up to; [`details`]: per-kind spec and status for the details views.
//! - [`inventory`], [`ownership`], [`deps`]: what an object applied, which Flux object manages
//!   an object, `dependsOn` as a graph.
//! - [`overview`], [`rows`]: the overview's counts and "Needs attention"; list rows, filters.
//! - [`ops`]: reconcile, suspend, resume and delete as the patches the `flux` CLI sends.
//! - [`detect`], [`service`]: the controllers and their versions, per cluster on a `Host`.
//! - [`links`], [`agent`]: URLs without credentials, commit links; what "Ask agent" sends.
//!
//! `kubyl_flux` re-exports these modules and adds the views.

pub mod agent;
pub mod deps;
pub mod details;
pub mod detect;
pub mod inventory;
pub mod kinds;
pub mod links;
pub mod model;
pub mod ops;
pub mod overview;
pub mod ownership;
pub mod rows;
pub mod service;

#[cfg(any(test, feature = "test-support"))]
pub mod fixtures;
