//! Cluster updates without the UI.
//!
//! - [`detect`], [`provider`], [`providers`]: which distribution a cluster runs and who
//!   updates it: [`openshift`], [`capi`], [`suc`], the cloud APIs (EKS, GKE, AKS behind
//!   features), or [`fallback`].
//! - [`check`], [`version`], [`model`]: available versions and channels.
//! - [`removed`], [`preflight`], [`kube_api`]: removed APIs and pre-flight checks.
//! - [`settings`]: the `"updates"` settings.json section.
//!
//! `kubyl_updates` re-exports these modules and adds the updates service and view.

pub mod capi;
pub mod check;
pub mod detect;
pub mod fallback;
pub mod kube_api;
pub mod model;
pub mod openshift;
pub mod preflight;
pub mod provider;
pub mod providers;
pub mod removed;
pub mod service;
pub mod settings;
pub mod suc;
pub mod version;
