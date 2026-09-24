//! API discovery: every resource type the cluster serves.
//!
//! Uses aggregated discovery (`/api` and `/apis` with `apidiscovery.k8s.io/v2`, Kubernetes
//! 1.26+) and falls back to legacy per-group discovery on older servers.

use std::collections::BTreeMap;

use futures::{StreamExt as _, TryStreamExt as _};
use kube::Client;
use kube::core::discovery::v2::APIGroupDiscoveryList;
use kubyl_core::{Gvk, Gvr};

/// One resource type in one version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiResourceInfo {
    pub gvk: Gvk,
    /// `gvr.resource` is the plural name used in API paths.
    pub gvr: Gvr,
    /// Lower-case singular name, e.g. `deployment`.
    pub singular: String,
    pub namespaced: bool,
    pub verbs: Vec<String>,
    pub short_names: Vec<String>,
    /// e.g. `all`.
    pub categories: Vec<String>,
    /// `status`, `scale`, `log`, `exec`…
    pub subresources: Vec<String>,
    /// This is the group's preferred version.
    pub preferred: bool,
}

impl ApiResourceInfo {
    pub fn supports(&self, verb: &str) -> bool {
        self.verbs.iter().any(|v| v == verb)
    }

    /// Resources you can list and watch (the sidebar shows these).
    pub fn is_listable(&self) -> bool {
        self.supports("list") && self.supports("watch")
    }
}

/// A group and its versions, preferred first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiGroupInfo {
    /// Empty for the core group.
    pub name: String,
    pub versions: Vec<String>,
}

impl ApiGroupInfo {
    pub fn preferred_version(&self) -> Option<&str> {
        self.versions.first().map(String::as_str)
    }
}

/// The result of discovery.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Discovery {
    pub groups: Vec<ApiGroupInfo>,
    /// Every resource in every served version, in server order.
    pub resources: Vec<ApiResourceInfo>,
    /// Served through aggregated discovery (false: legacy fallback).
    pub aggregated: bool,
}

impl Discovery {
    pub fn has_group(&self, group: &str) -> bool {
        self.groups.iter().any(|g| g.name == group)
    }

    /// Resources in their group's preferred version.
    pub fn preferred(&self) -> impl Iterator<Item = &ApiResourceInfo> {
        self.resources.iter().filter(|r| r.preferred)
    }

    /// Number of distinct kinds (preferred versions).
    pub fn kind_count(&self) -> usize {
        self.preferred().count()
    }

    /// Resolves what a user typed (`po`, `pods`, `pod`, `deployments.apps`, `Pod`) to the
    /// preferred resource, like `kubectl get`.
    pub fn resolve(&self, name: &str) -> Option<&ApiResourceInfo> {
        let lower = name.to_lowercase();
        let (name, group) = match lower.split_once('.') {
            Some((name, group)) => (name.to_string(), Some(group.to_string())),
            None => (lower.clone(), None),
        };
        let matches = |r: &&ApiResourceInfo| {
            group.as_ref().is_none_or(|g| &r.gvr.group == g)
                && (r.gvr.resource == name
                    || r.singular == name
                    || r.short_names.contains(&name)
                    || r.gvk.kind.to_lowercase() == name)
        };
        // Core and well-known groups win over CRDs with the same short name, like kubectl.
        self.preferred()
            .filter(matches)
            .min_by_key(|r| (!r.gvr.group.is_empty(), r.gvr.group.contains('.')))
    }

    pub fn by_gvk(&self, gvk: &Gvk) -> Option<&ApiResourceInfo> {
        self.resources.iter().find(|r| &r.gvk == gvk)
    }

    pub fn by_gvr(&self, gvr: &Gvr) -> Option<&ApiResourceInfo> {
        self.resources.iter().find(|r| &r.gvr == gvr)
    }
}

/// Discovers every resource type. Aggregated discovery first, legacy as a fallback.
pub async fn discover(client: &Client) -> Result<Discovery, kube::Error> {
    match discover_aggregated(client).await {
        Ok(discovery) if !discovery.resources.is_empty() => Ok(discovery),
        Ok(_) => discover_legacy(client).await,
        Err(err) => {
            tracing::debug!("aggregated discovery failed, using legacy discovery: {err}");
            discover_legacy(client).await
        }
    }
}

async fn discover_aggregated(client: &Client) -> Result<Discovery, kube::Error> {
    let (core, groups) = futures::try_join!(
        client.list_core_api_versions_aggregated(),
        client.list_api_groups_aggregated()
    )?;
    let mut discovery = Discovery {
        aggregated: true,
        ..Default::default()
    };
    for list in [core, groups] {
        add_aggregated(&mut discovery, list);
    }
    Ok(discovery)
}

fn add_aggregated(discovery: &mut Discovery, list: APIGroupDiscoveryList) {
    for group in list.items {
        let name = group
            .metadata
            .as_ref()
            .and_then(|m| m.name.clone())
            .unwrap_or_default();
        let versions: Vec<String> = group
            .versions
            .iter()
            .filter_map(|v| v.version.clone())
            .collect();
        for (index, version) in group.versions.into_iter().enumerate() {
            let Some(version_name) = version.version else {
                continue;
            };
            for resource in version.resources {
                let Some(plural) = resource.resource else {
                    continue;
                };
                let kind = resource
                    .response_kind
                    .as_ref()
                    .and_then(|k| k.kind.clone())
                    .unwrap_or_default();
                discovery.resources.push(ApiResourceInfo {
                    gvk: Gvk::new(&name, &version_name, kind.clone()),
                    gvr: Gvr::new(&name, &version_name, &plural),
                    singular: resource
                        .singular_resource
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| kind.to_lowercase()),
                    namespaced: resource.scope.as_deref() == Some("Namespaced"),
                    verbs: resource.verbs,
                    short_names: resource.short_names,
                    categories: resource.categories,
                    subresources: resource
                        .subresources
                        .into_iter()
                        .filter_map(|s| s.subresource)
                        .collect(),
                    preferred: index == 0,
                });
            }
        }
        discovery.groups.push(ApiGroupInfo { name, versions });
    }
}

async fn discover_legacy(client: &Client) -> Result<Discovery, kube::Error> {
    let mut discovery = Discovery::default();
    let mut group_versions: Vec<(String, String, bool)> = Vec::new();

    let core = client.list_core_api_versions().await?;
    let core_versions = core.versions.clone();
    discovery.groups.push(ApiGroupInfo {
        name: String::new(),
        versions: core_versions.clone(),
    });
    for (index, version) in core_versions.iter().enumerate() {
        group_versions.push((String::new(), version.clone(), index == 0));
    }

    for group in client.list_api_groups().await?.groups {
        let preferred = group
            .preferred_version
            .as_ref()
            .map(|p| p.version.clone())
            .or_else(|| group.versions.first().map(|v| v.version.clone()));
        let mut versions: Vec<String> = group.versions.iter().map(|v| v.version.clone()).collect();
        if let Some(preferred) = &preferred {
            versions.retain(|v| v != preferred);
            versions.insert(0, preferred.clone());
        }
        for version in &versions {
            group_versions.push((
                group.name.clone(),
                version.clone(),
                Some(version) == preferred.as_ref(),
            ));
        }
        discovery.groups.push(ApiGroupInfo {
            name: group.name,
            versions,
        });
    }

    let lists: Vec<_> = futures::stream::iter(group_versions)
        .map(|(group, version, preferred)| async move {
            let list = if group.is_empty() {
                client.list_core_api_resources(&version).await
            } else {
                client
                    .list_api_group_resources(&format!("{group}/{version}"))
                    .await
            };
            match list {
                Ok(list) => Ok(Some((group, version, preferred, list))),
                // An unavailable aggregated API (e.g. a broken metrics-server) must not fail
                // the whole discovery.
                Err(err) => {
                    tracing::debug!("discovery of {group}/{version} failed: {err}");
                    Ok::<_, kube::Error>(None)
                }
            }
        })
        .buffered(8)
        .try_collect()
        .await?;

    // (group, version, resource) -> subresources.
    let mut by_name: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
    for (group, version, preferred, list) in lists.into_iter().flatten() {
        for resource in &list.resources {
            if let Some((parent, sub)) = resource.name.split_once('/') {
                by_name
                    .entry((group.clone(), version.clone(), parent.to_string()))
                    .or_default()
                    .push(sub.to_string());
            }
        }
        for resource in list.resources {
            if resource.name.contains('/') {
                continue;
            }
            discovery.resources.push(ApiResourceInfo {
                gvk: Gvk::new(&group, &version, &resource.kind),
                gvr: Gvr::new(&group, &version, &resource.name),
                singular: if resource.singular_name.is_empty() {
                    resource.kind.to_lowercase()
                } else {
                    resource.singular_name
                },
                namespaced: resource.namespaced,
                verbs: resource.verbs,
                short_names: resource.short_names.unwrap_or_default(),
                categories: resource.categories.unwrap_or_default(),
                subresources: Vec::new(),
                preferred,
            });
        }
    }
    for resource in &mut discovery.resources {
        let key = (
            resource.gvr.group.clone(),
            resource.gvr.version.clone(),
            resource.gvr.resource.clone(),
        );
        if let Some(subs) = by_name.get(&key) {
            resource.subresources = subs.clone();
        }
    }
    Ok(discovery)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aggregated() -> APIGroupDiscoveryList {
        serde_json::from_value(serde_json::json!({
            "kind": "APIGroupDiscoveryList",
            "apiVersion": "apidiscovery.k8s.io/v2",
            "items": [
                {
                    "metadata": { "name": "" },
                    "versions": [{ "version": "v1", "resources": [
                        { "resource": "pods", "responseKind": { "group": "", "version": "v1", "kind": "Pod" },
                          "scope": "Namespaced", "singularResource": "pod", "shortNames": ["po"],
                          "categories": ["all"], "verbs": ["get", "list", "watch", "delete"],
                          "subresources": [{ "subresource": "log", "verbs": ["get"] }] },
                        { "resource": "namespaces", "responseKind": { "group": "", "version": "v1", "kind": "Namespace" },
                          "scope": "Cluster", "singularResource": "namespace", "shortNames": ["ns"], "verbs": ["get", "list", "watch"] }
                    ]}]
                },
                {
                    "metadata": { "name": "cert-manager.io" },
                    "versions": [
                        { "version": "v1", "resources": [
                            { "resource": "certificates", "responseKind": { "group": "cert-manager.io", "version": "v1", "kind": "Certificate" },
                              "scope": "Namespaced", "singularResource": "certificate", "shortNames": ["cert", "certs"], "verbs": ["get", "list", "watch"] }
                        ]},
                        { "version": "v1alpha2", "resources": [
                            { "resource": "certificates", "responseKind": { "group": "cert-manager.io", "version": "v1alpha2", "kind": "Certificate" },
                              "scope": "Namespaced", "singularResource": "certificate", "verbs": ["get", "list"] }
                        ]}
                    ]
                }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn parses_aggregated_discovery() {
        let mut discovery = Discovery::default();
        add_aggregated(&mut discovery, aggregated());
        assert_eq!(discovery.groups.len(), 2);
        assert_eq!(discovery.groups[1].preferred_version(), Some("v1"));
        assert_eq!(discovery.resources.len(), 4);
        assert_eq!(discovery.kind_count(), 3);

        let pods = discovery.resolve("po").unwrap();
        assert_eq!(pods.gvk, Gvk::new("", "v1", "Pod"));
        assert!(pods.namespaced && pods.is_listable());
        assert_eq!(pods.subresources, ["log"]);
        assert!(!discovery.resolve("ns").unwrap().namespaced);

        let cert = discovery.resolve("certs").unwrap();
        assert_eq!(cert.gvr.version, "v1");
        assert_eq!(
            discovery
                .resolve("certificates.cert-manager.io")
                .unwrap()
                .gvk
                .kind,
            "Certificate"
        );
        assert_eq!(
            discovery.resolve("Certificate").unwrap().gvr.resource,
            "certificates"
        );
        assert!(discovery.resolve("nope").is_none());
        assert!(discovery.has_group("cert-manager.io"));
        assert!(
            !discovery
                .by_gvk(&Gvk::new("cert-manager.io", "v1alpha2", "Certificate"))
                .unwrap()
                .is_listable()
        );
    }
}
