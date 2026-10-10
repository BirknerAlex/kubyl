//! The Security Center without the UI (phase 25): what Trivy Operator found, read from its
//! report CRDs (`aquasecurity.github.io`).
//!
//! Lists are cheap by design: the views watch the reports' **metadata** and take the severity
//! counts from the API server's printer columns (a `Table`), so a cluster with thousands of
//! reports never ships their findings. A report's full content (a vulnerability list can be
//! megabytes) is fetched when one is opened and parsed by [`details`].
//!
//! - [`kinds`]: the five report kinds.
//! - [`model`]: a report as a row ([`model::Report`]), severities and counts.
//! - [`aggregate`]: the Images, Resources and Roles views' rows.
//! - [`details`]: a full report's findings. **Exposed secrets are masked by construction**: the
//!   parser never reads the `match` field that holds the secret text.
//! - [`table`]: the CSV records of each view.
//! - [`install`]: the Helm chart that installs the operator, and the same as a command.
//!
//! `kubyl_security` adds the tab, the sidebar row and the actions.

pub mod aggregate;
pub mod details;
pub mod install;
pub mod kinds;
pub mod model;
pub mod table;

pub use kinds::ReportKind;
pub use model::{Counts, Report, Severity};
