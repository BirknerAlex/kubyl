//! Bookkeeping of RBAC checks a view already asked the API server for.

use std::collections::HashSet;
use std::time::Duration;

use kubyl_core::ClusterId;
use kubyl_kube::access::AccessQuery;

/// How long after a failed check the query may be asked again.
pub(crate) const RBAC_RETRY: Duration = Duration::from_secs(30);

/// The `(cluster, query)` pairs a view asked for, so a render doesn't repeat a request in
/// flight. Forget them when the answer may change: a reconnect (the cache starts empty) or a
/// failed check.
#[derive(Default)]
pub struct RbacRequests(HashSet<(ClusterId, AccessQuery)>);

impl RbacRequests {
    /// True the first time `query` is asked on `cluster` (since it was last forgotten).
    pub fn first(&mut self, cluster: &ClusterId, query: &AccessQuery) -> bool {
        self.0.insert((cluster.clone(), query.clone()))
    }

    /// Allows `query` on `cluster` to be asked again (its check failed).
    pub fn forget(&mut self, cluster: &ClusterId, query: &AccessQuery) {
        self.0.remove(&(cluster.clone(), query.clone()));
    }

    /// Allows every query on `cluster` to be asked again (it connected or disconnected).
    pub fn reset(&mut self, cluster: &ClusterId) {
        self.0.retain(|(c, _)| c != cluster);
    }
}

#[cfg(test)]
mod tests {
    use kubyl_core::Gvr;

    use super::*;

    #[test]
    fn requests_are_per_cluster_and_can_be_repeated() {
        let (a, b) = (ClusterId::new("a"), ClusterId::new("b"));
        let query = AccessQuery::new("list", &Gvr::new("", "v1", "pods"), None);
        let mut requests = RbacRequests::default();
        assert!(requests.first(&a, &query));
        assert!(!requests.first(&a, &query));
        // The same query on another cluster is a separate request.
        assert!(requests.first(&b, &query));
        // A failed check is asked again, a reconnect asks everything of that cluster again.
        requests.forget(&a, &query);
        assert!(requests.first(&a, &query));
        requests.reset(&a);
        assert!(requests.first(&a, &query));
        assert!(!requests.first(&b, &query));
    }
}
