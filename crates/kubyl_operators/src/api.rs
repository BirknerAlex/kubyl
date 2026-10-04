//! For other crates (phase 13's pre-flight check): the installed operators of a cluster with
//! the Kubernetes and OpenShift versions they declare, without reading OLM again.
//!
//! ```ignore
//! match kubyl_operators::api::installed(&cluster, cx) {
//!     Installed::Ready(operators) => {
//!         for op in operators {
//!             // op.min_kube_version, op.max_kube_version, op.max_openshift_version
//!         }
//!     }
//!     Installed::Loading => { /* observe `Olm::global(cx)` and ask again */ }
//!     Installed::NoOlm | Installed::NotConnected | Installed::Problem(_) => {}
//! }
//! ```
//!
//! Asking starts the OLM watches of the cluster (shared with the views) and keeps them for
//! two minutes after the last ask.

use gpui::App;
use kubyl_core::ClusterId;

use crate::service::{Availability, Olm};
pub use kubyl_operators_core::api::*;

/// The installed operators of `cluster` (OLM v0), with their version constraints.
pub fn installed(cluster: &ClusterId, cx: &mut App) -> Installed {
    let Some(olm) = Olm::global(cx) else {
        return Installed::NotConnected;
    };
    olm.update(cx, |olm, cx| olm.ensure(cluster, None, cx));
    let olm = olm.read(cx);
    match olm.availability(cluster, cx) {
        Availability::NotConnected => return Installed::NotConnected,
        Availability::NoOlm => return Installed::NoOlm,
        Availability::Loading | Availability::Ready => {}
    }
    let Some(snapshot) = olm.snapshot(cluster, cx) else {
        return Installed::Loading;
    };
    // Only the Subscription and CSV watches feed the list: a CatalogSource or OLM v1 watch
    // that can't list doesn't hide the operators.
    if let Some(problem) = &snapshot.operators_problem {
        return Installed::Problem(problem.clone());
    }
    if snapshot.loading {
        return Installed::Loading;
    }
    Installed::Ready(
        snapshot
            .operators
            .iter()
            .filter_map(|op| {
                let csv = op.csv.as_ref()?;
                Some(InstalledOperator {
                    name: op.display_name(),
                    package: op.package().map(str::to_string),
                    namespace: csv.namespace.clone(),
                    csv: csv.name.clone(),
                    version: csv.version.clone(),
                    channel: op.channel().map(str::to_string),
                    min_kube_version: csv.min_kube_version.clone(),
                    max_kube_version: csv.max_kube_version(),
                    max_openshift_version: csv.max_openshift_version(),
                    status: op.status,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    /// Before discovery the cluster's operators are loading, not an empty list: phase 13 must
    /// not read "no operators" while discovery runs.
    #[gpui::test]
    fn unknown_discovery_is_loading_not_empty(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let cluster = ClusterId::new("c");
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_resources::init(cx);
            Olm::install(false, cx);
            assert!(matches!(installed(&cluster, cx), Installed::Loading));
            let olm = Olm::global(cx).unwrap();
            let olm = olm.read(cx);
            assert!(olm.snapshot(&cluster, cx).unwrap().loading);
            assert_eq!(olm.availability(&cluster, cx), Availability::Loading);
        });
    }
}
