//! The read-only view for clusters Kubyl can't update: kubeadm, kind, MicroShift, unknown
//! distributions, and managed clouds whose provider this build doesn't include or that have no
//! credentials. It shows the version, the nodes' kubelets and docs; pre-flight checks still run
//! against a target the user picks.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::kube_api::{self, items, str_at};
use crate::model::{Current, Note, Pool, PoolKind, ProviderKind, Status, Writes};
use crate::provider::{ProviderError, ProviderFuture, UpdateProvider};

/// Read-only provider.
pub struct ReadOnly {
    client: kube::Client,
    kind: ProviderKind,
    /// `kind`, `kubeadm`, `Amazon EKS`…
    label: String,
    /// Why Kubyl can't update it.
    reason: String,
    notes: Vec<Note>,
}

impl ReadOnly {
    pub fn new(
        client: kube::Client,
        kind: ProviderKind,
        label: impl Into<String>,
        reason: impl Into<String>,
        notes: Vec<Note>,
    ) -> Self {
        Self {
            client,
            kind,
            label: label.into(),
            reason: reason.into(),
            notes,
        }
    }
}

impl UpdateProvider for ReadOnly {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn read(&self) -> ProviderFuture<Result<Status, ProviderError>> {
        let client = self.client.clone();
        let label = self.label.clone();
        let reason = self.reason.clone();
        let notes = self.notes.clone();
        let kind = self.kind;
        Box::pin(async move {
            let version = client
                .apiserver_version()
                .await
                .map_err(|e| ProviderError::from_kube(&e, "get", "/version"))?;
            let nodes = match kube_api::get(&client, "/api/v1/nodes").await {
                Ok(list) => node_pools(items(&list)),
                Err(err) if kube_api::is_forbidden(&err) => Vec::new(),
                Err(err) => return Err(ProviderError::from_kube(&err, "list", "nodes")),
            };
            Ok(Status {
                provider: format!("{label} (read-only)"),
                current: Current {
                    version: version.git_version.clone(),
                    platform: (!version.platform.is_empty()).then(|| version.platform.clone()),
                    ..Current::default()
                },
                pools: nodes,
                notes,
                writes: Writes::none(reason),
                docs: docs(kind, &label),
                ..Status::default()
            })
        })
    }
}

/// Where to read how to update a cluster Kubyl can't update.
fn docs(kind: ProviderKind, label: &str) -> Vec<(String, String)> {
    let mut docs = Vec::new();
    match kind {
        ProviderKind::Eks => docs.push((
            "Update an EKS cluster".into(),
            "https://docs.aws.amazon.com/eks/latest/userguide/update-cluster.html".into(),
        )),
        ProviderKind::Gke => docs.push((
            "Upgrade a GKE cluster".into(),
            "https://cloud.google.com/kubernetes-engine/docs/how-to/upgrading-a-cluster".into(),
        )),
        ProviderKind::Aks => docs.push((
            "Upgrade an AKS cluster".into(),
            "https://learn.microsoft.com/azure/aks/upgrade-aks-cluster".into(),
        )),
        ProviderKind::K3s => docs.push((
            "k3s automated upgrades".into(),
            "https://docs.k3s.io/upgrades/automated".into(),
        )),
        ProviderKind::Rke2 => docs.push((
            "RKE2 automated upgrades".into(),
            "https://docs.rke2.io/upgrades/automated_upgrade".into(),
        )),
        _ if label == "kind" => docs.push((
            "kind: create a cluster with a newer node image".into(),
            "https://kind.sigs.k8s.io/docs/user/quick-start/#creating-a-cluster".into(),
        )),
        _ => docs.push((
            "kubeadm upgrade".into(),
            "https://kubernetes.io/docs/tasks/administer-cluster/kubeadm/kubeadm-upgrade/".into(),
        )),
    }
    docs.push((
        "Version skew policy".into(),
        "https://kubernetes.io/releases/version-skew-policy/".into(),
    ));
    docs
}

/// Nodes grouped by role, with their kubelet versions.
pub fn node_pools(nodes: &[Value]) -> Vec<Pool> {
    let mut groups: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for node in nodes {
        groups.entry(role(node)).or_default().push(node);
    }
    let mut pools: Vec<Pool> = groups
        .into_iter()
        .map(|(role, nodes)| {
            let mut versions: Vec<&str> = nodes
                .iter()
                .filter_map(|n| str_at(n, "/status/nodeInfo/kubeletVersion"))
                .collect();
            versions.sort();
            versions.dedup();
            let ready = nodes
                .iter()
                .filter(|n| {
                    kube_api::array_at(n, "/status/conditions").iter().any(|c| {
                        str_at(c, "/type") == Some("Ready") && str_at(c, "/status") == Some("True")
                    })
                })
                .count();
            let kind = if role == "control-plane" {
                PoolKind::ControlPlane
            } else {
                PoolKind::Nodes
            };
            let mut pool = Pool::new(format!("nodes:{role}"), role, kind);
            pool.nodes = Some(nodes.len());
            pool.ready = Some(ready);
            pool.version = Some(versions.join(", "));
            pool
        })
        .collect();
    pools.sort_by_key(|p| p.kind != PoolKind::ControlPlane);
    pools
}

/// `control-plane`, the first other `node-role.kubernetes.io/<role>`, else `workers`.
fn role(node: &Value) -> String {
    let labels = node.pointer("/metadata/labels").and_then(Value::as_object);
    let roles: Vec<&str> = labels
        .map(|l| {
            l.keys()
                .filter_map(|k| k.strip_prefix("node-role.kubernetes.io/"))
                .collect()
        })
        .unwrap_or_default();
    if roles
        .iter()
        .any(|r| *r == "control-plane" || *r == "master")
    {
        "control-plane".into()
    } else {
        roles
            .first()
            .map(|r| r.to_string())
            .unwrap_or_else(|| "workers".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(name: &str, role: Option<&str>, kubelet: &str) -> Value {
        let mut labels = serde_json::Map::new();
        if let Some(role) = role {
            labels.insert(format!("node-role.kubernetes.io/{role}"), json!(""));
        }
        json!({"metadata": {"name": name, "labels": labels},
               "status": {"nodeInfo": {"kubeletVersion": kubelet},
                          "conditions": [{"type": "Ready", "status": "True"}]}})
    }

    #[test]
    fn groups_nodes_by_role() {
        let pools = node_pools(&[
            node("w1", None, "v1.37.0"),
            node("cp", Some("control-plane"), "v1.37.0"),
            node("w2", None, "v1.36.2"),
        ]);
        assert_eq!(pools[0].name, "control-plane");
        assert_eq!(pools[0].kind, PoolKind::ControlPlane);
        assert_eq!(pools[1].name, "workers");
        assert_eq!(pools[1].nodes, Some(2));
        assert_eq!(pools[1].version.as_deref(), Some("v1.36.2, v1.37.0"));
    }
}
