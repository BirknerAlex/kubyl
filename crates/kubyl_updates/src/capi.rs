//! Cluster API management clusters: the workload clusters they manage, each with its control
//! plane (a `KubeadmControlPlane`, or `Cluster.spec.topology.version` for ClusterClass
//! clusters) and `MachineDeployment`s. Updating bumps those versions; Cluster API replaces the
//! machines. There's no update graph: the user types the version (images must exist for it).

use kubyl_core::Gvr;
use serde_json::{Value, json};

use crate::kube_api::{self, items, str_at};
use crate::model::{
    Current, Note, ObjectLink, Plan, Pool, PoolKind, PoolState, ProviderKind, Scope, Status, Writes,
};
use crate::provider::{ProviderError, ProviderFuture, UpdateProvider};
use crate::version::Version;

/// The API versions the management cluster serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapiVersions {
    /// `cluster.x-k8s.io` (Clusters, MachineDeployments).
    pub cluster: String,
    /// `controlplane.cluster.x-k8s.io` (KubeadmControlPlanes), if served.
    pub control_plane: Option<String>,
}

impl Default for CapiVersions {
    fn default() -> Self {
        Self {
            cluster: "v1beta1".into(),
            control_plane: Some("v1beta1".into()),
        }
    }
}

/// The Cluster API provider of a management cluster.
pub struct ClusterApi {
    client: kube::Client,
    versions: CapiVersions,
}

impl ClusterApi {
    pub fn new(client: kube::Client, versions: CapiVersions) -> Self {
        Self { client, versions }
    }
}

impl UpdateProvider for ClusterApi {
    fn kind(&self) -> ProviderKind {
        ProviderKind::ClusterApi
    }

    fn read(&self) -> ProviderFuture<Result<Status, ProviderError>> {
        let client = self.client.clone();
        let versions = self.versions.clone();
        Box::pin(async move {
            let version = client
                .apiserver_version()
                .await
                .map_err(|e| ProviderError::from_kube(&e, "get", "/version"))?;
            let clusters = kube_api::get(
                &client,
                &format!("/apis/cluster.x-k8s.io/{}/clusters", versions.cluster),
            )
            .await
            .map_err(|e| ProviderError::from_kube(&e, "list", "clusters.cluster.x-k8s.io"))?;
            let deployments = kube_api::get(
                &client,
                &format!(
                    "/apis/cluster.x-k8s.io/{}/machinedeployments",
                    versions.cluster
                ),
            )
            .await
            .map_err(|e| {
                ProviderError::from_kube(&e, "list", "machinedeployments.cluster.x-k8s.io")
            })?;
            let control_planes = match &versions.control_plane {
                Some(v) => kube_api::get(
                    &client,
                    &format!("/apis/controlplane.cluster.x-k8s.io/{v}/kubeadmcontrolplanes"),
                )
                .await
                .map(|l| items(&l).to_vec())
                .unwrap_or_default(),
                None => Vec::new(),
            };
            Ok(status(
                &version.git_version,
                items(&clusters),
                &control_planes,
                items(&deployments),
                &versions,
            ))
        })
    }

    fn plan(
        &self,
        status: &Status,
        scope: &Scope,
        target: &str,
        cluster: &str,
    ) -> Result<Plan, String> {
        plan(status, scope, target, cluster)
    }

    fn start(&self, plan: &Plan) -> ProviderFuture<Result<String, ProviderError>> {
        let client = self.client.clone();
        let request = plan.request.clone();
        Box::pin(async move {
            let path = request["path"].as_str().unwrap_or_default().to_string();
            let resource = request["resource"].as_str().unwrap_or_default().to_string();
            kube_api::merge_patch(&client, &path, &request["patch"])
                .await
                .map_err(|e| ProviderError::from_kube(&e, "patch", &resource))?;
            Ok(request["done"].as_str().unwrap_or("Done.").to_string())
        })
    }
}

fn count(object: &Value, pointers: &[&str]) -> Option<usize> {
    pointers
        .iter()
        .find_map(|p| object.pointer(p).and_then(Value::as_i64))
        .and_then(|n| usize::try_from(n).ok())
}

fn path(link: &ObjectLink) -> String {
    format!(
        "/apis/{}/{}/namespaces/{}/{}/{}",
        link.gvr.group,
        link.gvr.version,
        link.namespace.clone().unwrap_or_default(),
        link.gvr.resource,
        link.name
    )
}

/// The status: one control plane pool per workload cluster, then its MachineDeployments.
pub fn status(
    git_version: &str,
    clusters: &[Value],
    control_planes: &[Value],
    deployments: &[Value],
    versions: &CapiVersions,
) -> Status {
    let mut pools = Vec::new();
    for cluster in clusters {
        let ns = str_at(cluster, "/metadata/namespace").unwrap_or_default();
        let name = str_at(cluster, "/metadata/name").unwrap_or_default();
        let topology = str_at(cluster, "/spec/topology/version");
        let kcp = cluster
            .pointer("/spec/controlPlaneRef")
            .filter(|r| str_at(r, "/kind") == Some("KubeadmControlPlane"))
            .and_then(|r| {
                let kcp_name = str_at(r, "/name")?;
                control_planes.iter().find(|k| {
                    str_at(k, "/metadata/name") == Some(kcp_name)
                        && str_at(k, "/metadata/namespace") == Some(ns)
                })
            });
        let mut pool = Pool::new(
            format!("cp:{ns}/{name}"),
            format!("{ns}/{name} control plane"),
            PoolKind::ControlPlane,
        );
        match (topology, kcp) {
            (Some(version), kcp) => {
                pool.version = Some(version.to_string());
                if let Some(kcp) = kcp {
                    pool.nodes = count(kcp, &["/spec/replicas"]);
                    pool.updated = count(
                        kcp,
                        &["/status/upToDateReplicas", "/status/updatedReplicas"],
                    );
                    pool.ready = count(kcp, &["/status/readyReplicas"]);
                }
                pool.message = Some("ClusterClass topology: its MachineDeployments follow".into());
                pool.object = Some(ObjectLink::new(
                    Gvr::new("cluster.x-k8s.io", versions.cluster.clone(), "clusters"),
                    Some(ns),
                    name,
                ));
                pool.updatable = true;
            }
            (None, Some(kcp)) => {
                pool.version = str_at(kcp, "/spec/version").map(str::to_string);
                pool.nodes = count(kcp, &["/spec/replicas"]);
                pool.updated = count(
                    kcp,
                    &["/status/upToDateReplicas", "/status/updatedReplicas"],
                );
                pool.ready = count(kcp, &["/status/readyReplicas"]);
                if let Some(v) = &versions.control_plane {
                    pool.object = Some(ObjectLink::new(
                        Gvr::new(
                            "controlplane.cluster.x-k8s.io",
                            v.clone(),
                            "kubeadmcontrolplanes",
                        ),
                        Some(ns),
                        str_at(kcp, "/metadata/name").unwrap_or_default(),
                    ));
                    pool.updatable = true;
                }
            }
            (None, None) => {
                pool.message = Some(
                    "its control plane isn't a KubeadmControlPlane: Kubyl can't update it".into(),
                );
            }
        }
        pool.state = match (pool.updated, pool.nodes) {
            (Some(updated), Some(nodes)) if updated < nodes => PoolState::Updating,
            _ => PoolState::Idle,
        };
        pools.push(pool);
        for md in deployments.iter().filter(|md| {
            str_at(md, "/metadata/namespace") == Some(ns)
                && (str_at(md, "/spec/clusterName") == Some(name)
                    || md
                        .pointer("/metadata/labels")
                        .and_then(|l| l.get("cluster.x-k8s.io/cluster-name"))
                        .and_then(Value::as_str)
                        == Some(name))
        }) {
            let md_name = str_at(md, "/metadata/name").unwrap_or_default();
            let mut pool = Pool::new(
                format!("md:{ns}/{md_name}"),
                format!("{ns}/{name} · {md_name}"),
                PoolKind::MachineDeployment,
            );
            pool.version = str_at(md, "/spec/template/spec/version").map(str::to_string);
            pool.nodes = count(md, &["/spec/replicas"]);
            pool.updated = count(md, &["/status/upToDateReplicas", "/status/updatedReplicas"]);
            pool.ready = count(md, &["/status/readyReplicas"]);
            pool.surge = md.pointer("/spec/strategy/rollingUpdate").map(|r| {
                let get = |k: &str| {
                    r.get(k)
                        .map(|v| v.to_string().trim_matches('"').to_string())
                };
                format!(
                    "maxSurge {} · maxUnavailable {}",
                    get("maxSurge").unwrap_or_else(|| "1".into()),
                    get("maxUnavailable").unwrap_or_else(|| "0".into())
                )
            });
            pool.state = match (pool.updated, pool.nodes) {
                (Some(updated), Some(nodes)) if updated < nodes => PoolState::Updating,
                _ => PoolState::Idle,
            };
            pool.updatable = topology.is_none();
            if topology.is_some() {
                pool.message = Some("follows the cluster's topology version".into());
            }
            pool.object = Some(ObjectLink::new(
                Gvr::new(
                    "cluster.x-k8s.io",
                    versions.cluster.clone(),
                    "machinedeployments",
                ),
                Some(ns),
                md_name,
            ));
            pools.push(pool);
        }
    }
    Status {
        provider: "Cluster API (management cluster)".into(),
        current: Current {
            version: git_version.to_string(),
            ..Current::default()
        },
        pools,
        writes: Writes {
            pools: true,
            ..Writes::default()
        },
        notes: vec![Note {
            warning: false,
            title: "Workload clusters managed from here".into(),
            text: "Pick a control plane or MachineDeployment and the Kubernetes version to roll it to. Cluster API replaces the machines; their templates need an image for that version.".into(),
            command: None,
            url: Some("https://cluster-api.sigs.k8s.io/tasks/upgrading-clusters".into()),
        }],
        ..Status::default()
    }
}

/// Bumps one control plane or MachineDeployment to `target`.
pub fn plan(status: &Status, scope: &Scope, target: &str, _cluster: &str) -> Result<Plan, String> {
    let Scope::Pool(id) = scope else {
        return Err("Pick a control plane or MachineDeployment.".into());
    };
    let pool = status
        .pools
        .iter()
        .find(|p| &p.id == id)
        .ok_or_else(|| "That pool is gone.".to_string())?;
    if !pool.updatable {
        return Err(pool
            .message
            .clone()
            .unwrap_or_else(|| "Kubyl can't update this pool.".into()));
    }
    let link = pool.object.clone().ok_or("No object to update.")?;
    let wanted = Version::parse(target).ok_or_else(|| format!("{target} isn't a version."))?;
    if wanted.patch.is_none() {
        return Err("Give a full version, like v1.31.4.".into());
    }
    let from = pool.version.clone().unwrap_or_default();
    if let Some(current) = Version::parse(&from) {
        if wanted <= current {
            return Err(format!("{} already runs {from}.", pool.name));
        }
        if current.minors_to(&wanted) > 1 {
            return Err(format!(
                "Kubernetes updates one minor at a time: {}.{} first.",
                current.major,
                current.minor.saturating_add(1)
            ));
        }
    }
    let target = if target.starts_with('v') {
        target.to_string()
    } else {
        format!("v{target}")
    };
    let (patch, field, notes) = match pool.kind {
        PoolKind::MachineDeployment => {
            // Workers may not run a newer version than their control plane.
            let cluster_prefix = pool.name.split(" · ").next().unwrap_or_default();
            if let Some(cp) = status
                .pools
                .iter()
                .find(|p| p.kind == PoolKind::ControlPlane && p.name == format!("{cluster_prefix} control plane"))
                .and_then(|p| p.version.as_deref())
                .and_then(Version::parse)
                && wanted > cp
            {
                return Err(format!("Update the control plane to {target} first."));
            }
            (
                json!({"spec": {"template": {"spec": {"version": target}}}}),
                "spec.template.spec.version",
                vec!["Machines are replaced one by one with the rollout strategy.".to_string()],
            )
        }
        _ if link.gvr.resource == "clusters" => (
            json!({"spec": {"topology": {"version": target}}}),
            "spec.topology.version",
            vec!["The topology controller updates the control plane first, then the MachineDeployments.".to_string()],
        ),
        _ => (
            json!({"spec": {"version": target}}),
            "spec.version",
            vec!["MachineDeployments keep their version: update each after the control plane.".to_string()],
        ),
    };
    let mut notes = notes;
    notes.push(format!(
        "The machine templates need an image for {target} (CAPD: kindest/node:{target}; clouds: an image built for it)."
    ));
    let kind = match link.gvr.resource.as_str() {
        "clusters" => "Cluster",
        "kubeadmcontrolplanes" => "KubeadmControlPlane",
        _ => "MachineDeployment",
    };
    Ok(Plan {
        scope: scope.clone(),
        title: format!("Update {} to {target}?", pool.name),
        from,
        to: target.clone(),
        kind_label: "Cluster API rollout".into(),
        changes: vec![format!(
            "{kind} {}/{}: {field} = {target}",
            link.namespace.clone().unwrap_or_default(),
            link.name
        )],
        risks: Vec::new(),
        notes,
        irreversible: true,
        request: json!({
            "path": path(&link),
            "resource": format!("{}.{}", link.gvr.resource, link.gvr.group),
            "patch": patch,
            "done": format!("{} rolls to {target}.", pool.name),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> (Vec<Value>, Vec<Value>, Vec<Value>) {
        let clusters = vec![
            json!({"metadata": {"namespace": "fleet", "name": "edge-1"},
                   "spec": {"controlPlaneRef": {"kind": "KubeadmControlPlane", "name": "edge-1-cp"}}}),
            json!({"metadata": {"namespace": "fleet", "name": "edge-2"}, "spec": {"topology": {"version": "v1.30.4", "class": "quick-start"}}}),
        ];
        let kcps = vec![
            json!({"metadata": {"namespace": "fleet", "name": "edge-1-cp"},
            "spec": {"version": "v1.30.4", "replicas": 3}, "status": {"updatedReplicas": 3, "readyReplicas": 3}}),
        ];
        let mds = vec![
            json!({"metadata": {"namespace": "fleet", "name": "edge-1-md-0", "labels": {"cluster.x-k8s.io/cluster-name": "edge-1"}},
                   "spec": {"clusterName": "edge-1", "replicas": 4, "template": {"spec": {"version": "v1.30.4"}},
                            "strategy": {"rollingUpdate": {"maxSurge": 1, "maxUnavailable": 0}}},
                   "status": {"updatedReplicas": 2, "readyReplicas": 4}}),
            json!({"metadata": {"namespace": "fleet", "name": "edge-2-md-0"},
                   "spec": {"clusterName": "edge-2", "replicas": 2, "template": {"spec": {"version": "v1.30.4"}}}}),
        ];
        (clusters, kcps, mds)
    }

    #[test]
    fn workload_clusters_and_pools() {
        let (clusters, kcps, mds) = fixtures();
        let status = status("v1.37.0", &clusters, &kcps, &mds, &CapiVersions::default());
        let ids: Vec<&str> = status.pools.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "cp:fleet/edge-1",
                "md:fleet/edge-1-md-0",
                "cp:fleet/edge-2",
                "md:fleet/edge-2-md-0"
            ]
        );
        assert_eq!(status.pools[1].state, PoolState::Updating);
        assert_eq!(
            status.pools[1].surge.as_deref(),
            Some("maxSurge 1 · maxUnavailable 0")
        );
        assert!(!status.pools[3].updatable);
    }

    #[test]
    fn version_bumps() {
        let (clusters, kcps, mds) = fixtures();
        let status = status("v1.37.0", &clusters, &kcps, &mds, &CapiVersions::default());
        let kcp = plan(
            &status,
            &Scope::Pool("cp:fleet/edge-1".into()),
            "v1.31.2",
            "mgmt",
        )
        .unwrap();
        assert_eq!(
            kcp.request["patch"],
            json!({"spec": {"version": "v1.31.2"}})
        );
        assert_eq!(
            kcp.request["path"],
            "/apis/controlplane.cluster.x-k8s.io/v1beta1/namespaces/fleet/kubeadmcontrolplanes/edge-1-cp"
        );
        let topology = plan(
            &status,
            &Scope::Pool("cp:fleet/edge-2".into()),
            "1.31.2",
            "mgmt",
        )
        .unwrap();
        assert_eq!(
            topology.request["patch"],
            json!({"spec": {"topology": {"version": "v1.31.2"}}})
        );
        // Workers can't pass their control plane; minors one at a time; full versions only.
        assert!(
            plan(
                &status,
                &Scope::Pool("md:fleet/edge-1-md-0".into()),
                "v1.31.2",
                "mgmt"
            )
            .is_err()
        );
        assert!(
            plan(
                &status,
                &Scope::Pool("md:fleet/edge-1-md-0".into()),
                "v1.30.5",
                "mgmt"
            )
            .is_err()
        );
        assert!(
            plan(
                &status,
                &Scope::Pool("cp:fleet/edge-1".into()),
                "v1.32.0",
                "mgmt"
            )
            .is_err()
        );
        assert!(
            plan(
                &status,
                &Scope::Pool("cp:fleet/edge-1".into()),
                "v1.31",
                "mgmt"
            )
            .is_err()
        );
        assert!(
            plan(
                &status,
                &Scope::Pool("md:fleet/edge-2-md-0".into()),
                "v1.30.5",
                "mgmt"
            )
            .is_err()
        );
    }
}
