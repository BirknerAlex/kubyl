//! RBAC checks: "can I do this?" via `SelfSubjectAccessReview`, and the full rule list via
//! `SelfSubjectRulesReview`. [`crate::ConnectionManager::can_i`] caches the answers so UI
//! actions can hide or disable themselves without a request per render.

use std::time::{Duration, Instant};

use k8s_openapi::api::authorization::v1::{
    ResourceAttributes, SelfSubjectAccessReview, SelfSubjectAccessReviewSpec,
    SelfSubjectRulesReview, SelfSubjectRulesReviewSpec, SubjectRulesReviewStatus,
};
use kube::{Api, Client};
use kubyl_core::Gvr;

/// How long an answer is reused.
pub const TTL: Duration = Duration::from_secs(300);

/// What an access check asks.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AccessQuery {
    /// `get`, `list`, `watch`, `create`, `update`, `patch`, `delete`…
    pub verb: String,
    pub group: String,
    pub resource: String,
    pub subresource: Option<String>,
    /// `None`: all namespaces (or a cluster-scoped resource).
    pub namespace: Option<String>,
    pub name: Option<String>,
}

impl AccessQuery {
    pub fn new(verb: &str, gvr: &Gvr, namespace: Option<&str>) -> Self {
        Self {
            verb: verb.to_string(),
            group: gvr.group.clone(),
            resource: gvr.resource.clone(),
            subresource: None,
            namespace: namespace.map(String::from),
            name: None,
        }
    }

    pub fn subresource(mut self, subresource: &str) -> Self {
        self.subresource = Some(subresource.to_string());
        self
    }

    pub fn name(mut self, name: &str) -> Self {
        self.name = Some(name.to_string());
        self
    }
}

/// Cached answers of one cluster.
#[derive(Default)]
pub struct AccessCache {
    answers: std::collections::HashMap<AccessQuery, (bool, Instant)>,
}

impl AccessCache {
    pub fn get(&self, query: &AccessQuery) -> Option<bool> {
        self.answers
            .get(query)
            .filter(|(_, at)| at.elapsed() < TTL)
            .map(|(allowed, _)| *allowed)
    }

    pub fn insert(&mut self, query: AccessQuery, allowed: bool) {
        self.answers.insert(query, (allowed, Instant::now()));
    }

    pub fn clear(&mut self) {
        self.answers.clear();
    }
}

/// Asks the API server whether the current user may do `query`.
pub async fn check(client: Client, query: AccessQuery) -> Result<bool, kube::Error> {
    let api: Api<SelfSubjectAccessReview> = Api::all(client);
    let review = SelfSubjectAccessReview {
        spec: SelfSubjectAccessReviewSpec {
            resource_attributes: Some(ResourceAttributes {
                verb: Some(query.verb),
                group: Some(query.group),
                resource: Some(query.resource),
                subresource: query.subresource,
                namespace: query.namespace,
                name: query.name,
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let result = api.create(&Default::default(), &review).await?;
    Ok(result
        .status
        .is_some_and(|s| s.allowed && !s.denied.unwrap_or(false)))
}

/// Every rule the current user has in `namespace`.
pub async fn rules(
    client: Client,
    namespace: &str,
) -> Result<SubjectRulesReviewStatus, kube::Error> {
    let api: Api<SelfSubjectRulesReview> = Api::all(client);
    let review = SelfSubjectRulesReview {
        spec: SelfSubjectRulesReviewSpec {
            namespace: Some(namespace.to_string()),
        },
        ..Default::default()
    };
    let result = api.create(&Default::default(), &review).await?;
    Ok(result.status.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_expires_answers() {
        let mut cache = AccessCache::default();
        let query = AccessQuery::new("delete", &Gvr::new("", "v1", "pods"), Some("payments"));
        assert_eq!(cache.get(&query), None);
        cache.insert(query.clone(), true);
        assert_eq!(cache.get(&query), Some(true));
        cache.answers.get_mut(&query).unwrap().1 = Instant::now() - TTL;
        assert_eq!(cache.get(&query), None);
    }
}
