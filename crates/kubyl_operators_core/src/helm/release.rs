//! Helm releases from their storage objects: grouping revisions into releases, and loading a
//! revision's summary or whole release on Tokio.

use std::collections::BTreeMap;
use std::sync::Arc;

use jiff::Timestamp;
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use kube::Api;
use kubyl_base::{ClusterId, Gvr, ResourceRef};
use kubyl_resources_core::store::StoreStatus;
use serde_json::Value;

use super::decode::{self, Driver, Release, Summary};

/// The label selector of Helm's storage objects.
pub const SELECTOR: &str = "owner=helm";

/// One revision of a release, from the storage object's labels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    pub revision: u32,
    pub status: String,
    pub modified: Option<Timestamp>,
    /// The storage object (`sh.helm.release.v1.<name>.v<revision>`).
    pub object: String,
}

/// The summary of a release's latest revision.
#[derive(Clone, Debug)]
pub enum SummaryState {
    Loading,
    Ready(Arc<Summary>),
    Failed(String),
}

/// One release (its latest revision) and its history.
#[derive(Clone, Debug)]
pub struct ReleaseRow {
    pub namespace: String,
    pub name: String,
    pub driver: Driver,
    /// Newest first.
    pub revisions: Vec<Revision>,
    pub summary: SummaryState,
}

impl ReleaseRow {
    pub fn latest(&self) -> &Revision {
        &self.revisions[0]
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }

    pub fn summary(&self) -> Option<&Arc<Summary>> {
        match &self.summary {
            SummaryState::Ready(s) => Some(s),
            _ => None,
        }
    }

    /// The storage object of the latest revision.
    pub fn object_ref(&self, cluster: &ClusterId) -> ResourceRef {
        object_ref(cluster, self.driver, &self.namespace, &self.latest().object)
    }
}

/// The ref of a release's storage object (what a release tab is opened for).
pub fn object_ref(
    cluster: &ClusterId,
    driver: Driver,
    namespace: &str,
    object: &str,
) -> ResourceRef {
    let resource = match driver {
        Driver::Secret => "secrets",
        Driver::ConfigMap => "configmaps",
    };
    ResourceRef::object(
        cluster.clone(),
        Gvr::new("", "v1", resource),
        Some(namespace.to_string()),
        object.to_string(),
    )
}

/// What the Helm tab shows for a cluster.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub releases: Vec<ReleaseRow>,
    pub loading: bool,
    /// Why releases are missing or limited (403 on Secrets…).
    pub problem: Option<String>,
    /// The namespace the releases are limited to, when listing cluster-wide isn't allowed.
    pub scope: Option<String>,
}

pub fn label<'a>(object: &'a Value, key: &str) -> Option<&'a str> {
    object
        .pointer("/metadata/labels")
        .and_then(|l| l.get(key))
        .and_then(Value::as_str)
}

/// Groups storage objects (metadata) into releases, newest revision first.
pub fn group(
    objects: &[(Driver, Arc<Value>)],
) -> Vec<(String, String, Driver, Vec<Revision>, String)> {
    let mut releases: BTreeMap<(String, String, Driver), Vec<(Revision, String)>> = BTreeMap::new();
    for (driver, object) in objects {
        let Some(name) = label(object, "name") else {
            continue;
        };
        let Some(revision) = label(object, "version").and_then(|v| v.parse::<u32>().ok()) else {
            continue;
        };
        let namespace = object
            .pointer("/metadata/namespace")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let object_name = object
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let resource_version = object
            .pointer("/metadata/resourceVersion")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let modified = label(object, "modifiedAt")
            .and_then(|t| t.parse::<i64>().ok())
            .and_then(|t| Timestamp::from_second(t).ok())
            .or_else(|| {
                object
                    .pointer("/metadata/creationTimestamp")
                    .and_then(Value::as_str)
                    .and_then(|t| t.parse().ok())
            });
        releases
            .entry((namespace, name.to_string(), *driver))
            .or_default()
            .push((
                Revision {
                    revision,
                    status: label(object, "status").unwrap_or_default().to_string(),
                    modified,
                    object: object_name,
                },
                resource_version,
            ));
    }
    releases
        .into_iter()
        .map(|((namespace, name, driver), mut revisions)| {
            revisions.sort_by_key(|r| std::cmp::Reverse(r.0.revision));
            let resource_version = revisions[0].1.clone();
            (
                namespace,
                name,
                driver,
                revisions.into_iter().map(|(r, _)| r).collect(),
                resource_version,
            )
        })
        .collect()
}

/// The banner over the release list: what the Secrets watch can't read, or a failing watch
/// (its error, also in a single namespace), or the namespace the list is limited to.
pub fn problem(status: &StoreStatus, scope: Option<&str>) -> Option<String> {
    match (status, scope) {
        (StoreStatus::Forbidden, Some(ns)) => Some(format!(
            "Helm stores releases in Secrets, and you can't list secrets in {ns} either. Ask for a role that can list and watch secrets."
        )),
        (StoreStatus::Forbidden, None) => Some(
            "Helm stores releases in Secrets, and you can't list secrets cluster-wide. Ask for a role that can list and watch secrets, or pick a namespace you can read."
                .to_string(),
        ),
        (StoreStatus::Error(err), _) => Some(err.clone()),
        (_, Some(ns)) => Some(format!(
            "You can't list secrets cluster-wide: showing the releases in {ns} only."
        )),
        _ => None,
    }
}

/// The raw `data.release` of a storage object.
pub async fn fetch_encoded(
    client: kube::Client,
    driver: Driver,
    namespace: &str,
    object: &str,
) -> Result<Vec<u8>, String> {
    match driver {
        Driver::Secret => {
            let api: Api<Secret> = Api::namespaced(client, namespace);
            let secret = api
                .get(object)
                .await
                .map_err(|e| crate::errors::describe(&e, "get", "secrets", Some(namespace)))?;
            secret
                .data
                .and_then(|mut d| d.remove("release"))
                .map(|b| b.0)
                .ok_or_else(|| decode::DecodeError::Missing("Secret").to_string())
        }
        Driver::ConfigMap => {
            let api: Api<ConfigMap> = Api::namespaced(client, namespace);
            let config_map = api
                .get(object)
                .await
                .map_err(|e| crate::errors::describe(&e, "get", "configmaps", Some(namespace)))?;
            config_map
                .data
                .and_then(|mut d| d.remove("release"))
                .map(String::into_bytes)
                .ok_or_else(|| decode::DecodeError::Missing("ConfigMap").to_string())
        }
    }
}

/// Fetches and decodes the summary of one revision, on Tokio.
pub async fn fetch_summary(
    client: kube::Client,
    driver: Driver,
    namespace: String,
    object: String,
) -> Result<Summary, String> {
    let encoded = fetch_encoded(client, driver, &namespace, &object).await?;
    decode::decode_summary(&encoded).map_err(|e| e.to_string())
}

/// Loads a whole release revision (values, manifest, notes) for a release tab, on Tokio.
pub async fn load(
    client: kube::Client,
    driver: Driver,
    namespace: String,
    object: String,
) -> Result<Release, String> {
    let encoded = fetch_encoded(client, driver, &namespace, &object).await?;
    decode::decode(&encoded).map_err(|e| e.to_string())
}

/// Loads the summaries of several revisions (the History tab), on Tokio.
pub async fn load_summaries(
    client: kube::Client,
    driver: Driver,
    namespace: String,
    objects: Vec<String>,
) -> Vec<(String, Result<Summary, String>)> {
    let mut out = Vec::new();
    for object in objects {
        let summary =
            fetch_summary(client.clone(), driver, namespace.clone(), object.clone()).await;
        out.push((object, summary));
    }
    out
}
