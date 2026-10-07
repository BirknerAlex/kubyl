//! Helm without the UI.
//!
//! - [`decode`]: Helm v3 releases from their storage objects (Secrets or ConfigMaps).
//! - [`present`]: values and manifests for display (masked), the objects of a manifest.
//! - [`release`]: grouping revisions into releases, loading a revision on Tokio.
//! - [`service`]: [`service::HelmCore`], the releases of each cluster a view shows.
//! - [`cli`]: the user's `helm` (found, probed, run on Tokio with the context's kubeconfig and,
//!   for Kubyl sign-ins, the token in its environment; [`cli::HelmCliCore`] keeps the probe);
//!   [`cmd`]: the command lines; [`ops`]: [`ops::HelmOpsCore`], the writes that run.
//! - [`repo`]: repositories, registries, chart search and details, Artifact Hub (opt-in).
//! - [`preview`]: what an install, upgrade, rollback or uninstall changes.
//! - [`values`]: the values editors (parse, overrides, schema check, masked diffs).
//! - [`agent`]: a release as text for the user's agent. [`settings`]: the `"helm"` section.
//!
//! `kubyl_helm` re-exports these modules and adds the views.

pub mod agent;
pub mod cli;
pub mod cmd;
pub mod decode;
pub mod ops;
pub mod present;
pub mod preview;
pub mod release;
pub mod repo;
pub mod service;
pub mod settings;
pub mod values;
