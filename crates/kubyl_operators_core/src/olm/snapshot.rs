//! What `kubyl_operators::Olm` shows for a cluster, without the UI: OLM's availability, the
//! joined snapshot of its objects and what its watches can't read.

use std::sync::Arc;
use std::time::Duration;

use kubyl_resources_core::store::StoreStatus;

use super::join::Operator;
use super::model::{CatalogSource, Csv, InstallPlan, OperatorGroup, Subscription};
use super::v1::{ClusterCatalog, ClusterExtension};

/// How long a cluster's watches stay after the last lease went and the API stopped asking.
pub const KEEP: Duration = Duration::from_secs(120);
pub const SWEEP: Duration = Duration::from_secs(30);
/// OperatorHub's packages are listed again after this while a view shows them.
pub const HUB_REFRESH: Duration = Duration::from_secs(600);
/// Reviews older than this are built again.
pub const REVIEW_TTL: Duration = Duration::from_secs(60);

/// What a cluster has of OLM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Not connected, or discovery hasn't run yet.
    NotConnected,
    Loading,
    /// Neither `operators.coreos.com` nor `olm.operatorframework.io`.
    NoOlm,
    Ready,
}

/// Everything the views show about one cluster's OLM, joined. Cheap to clone (Arcs).
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// OLM v0 (`operators.coreos.com`) is served.
    pub v0: bool,
    /// OLM v1 (`olm.operatorframework.io`) is served.
    pub v1: bool,
    pub subscriptions: Vec<Arc<Subscription>>,
    pub csvs: Vec<Arc<Csv>>,
    pub plans: Vec<Arc<InstallPlan>>,
    pub catalogs: Vec<Arc<CatalogSource>>,
    pub groups: Vec<OperatorGroup>,
    pub operators: Vec<Operator>,
    pub extensions: Vec<ClusterExtension>,
    pub cluster_catalogs: Vec<ClusterCatalog>,
    /// Some watch hasn't listed yet.
    pub loading: bool,
    /// Watches that can't list (403…): what's missing.
    pub problems: Vec<String>,
    /// The first of `problems` from the Subscription or CSV watch: the installed operators are
    /// incomplete (other watches failing leave them intact).
    pub operators_problem: Option<String>,
}

impl Snapshot {
    /// The operator of a key (`sub:ns/name`, `csv:ns/name`).
    pub fn operator(&self, key: &str) -> Option<&Operator> {
        self.operators.iter().find(|o| o.key == key)
    }

    pub fn plan(&self, namespace: &str, name: &str) -> Option<&Arc<InstallPlan>> {
        self.plans
            .iter()
            .find(|p| p.namespace == namespace && p.name == name)
    }

    /// The installed operator of a package (for OperatorHub's "Installed").
    pub fn installed(&self, package: &str, catalog: &str) -> Option<&Operator> {
        self.operators.iter().find(|o| {
            o.subscription.as_ref().is_some_and(|s| {
                s.package == package && (catalog.is_empty() || s.source == catalog)
            })
        })
    }

    /// Install plans that wait for approval.
    pub fn pending_plans(&self) -> usize {
        self.plans.iter().filter(|p| p.needs_approval()).count()
    }
}

/// A number per store status, for change detection.
pub fn status_code(status: &StoreStatus) -> u64 {
    match status {
        StoreStatus::Waiting => 0,
        StoreStatus::Loading => 1,
        StoreStatus::Ready => 2,
        StoreStatus::Forbidden => 3,
        StoreStatus::Unsupported => 4,
        StoreStatus::Error(_) => 5,
        StoreStatus::Paused => 6,
    }
}

/// What the watches say: whether one still lists, what can't be read, and the first problem
/// of the Subscription or CSV watch (the installed operators come from those two).
#[derive(Debug, Default, PartialEq)]
pub struct WatchProblems {
    pub loading: bool,
    pub problems: Vec<String>,
    pub operators: Option<String>,
}

/// `watches`: (resource, status, empty?) of each running watch.
pub fn watch_problems(watches: &[(&str, StoreStatus, bool)]) -> WatchProblems {
    let mut out = WatchProblems::default();
    for (what, status, empty) in watches {
        let problem = match status {
            StoreStatus::Waiting | StoreStatus::Loading => {
                out.loading = true;
                None
            }
            StoreStatus::Forbidden => Some(format!(
                "Forbidden: you can't list {what} cluster-wide. Ask for a role that can list and watch {what}."
            )),
            StoreStatus::Error(err) if !status.is_settled() || *empty => {
                Some(format!("{what}: {err}"))
            }
            _ => None,
        };
        if let Some(problem) = problem {
            if out.operators.is_none()
                && matches!(*what, "subscriptions" | "clusterserviceversions")
            {
                out.operators = Some(problem.clone());
            }
            out.problems.push(problem);
        }
    }
    out
}
