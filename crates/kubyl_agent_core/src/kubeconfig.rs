//! The kubeconfig the agent's commands get (`agent.kubectl`).
//!
//! - `none` (default): [`EMPTY`], so `kubectl` reaches nothing and the agent uses Kubyl's tools.
//! - `context`: [`context_kubeconfig`], only the thread's context, and only when its user holds
//!   no inline credentials (no token, password, client key, token file or legacy auth provider):
//!   exec plugins (`aws eks get-token`, `kubelogin`) and the cluster's CA are fine, since they
//!   hold no secret. Kubyl never hands a credential to an agent.

use std::path::Path;

use kube::config::{Kubeconfig, NamedAuthInfo, NamedCluster, NamedContext};
use serde_json::json;

/// A kubeconfig with nothing in it.
pub const EMPTY: &str =
    "apiVersion: v1\nkind: Config\nclusters: []\ncontexts: []\nusers: []\npreferences: {}\n";

/// A kubeconfig (JSON, which kubectl reads) with only `context` from `file`, or why it can't be
/// made.
pub fn context_kubeconfig(file: &Path, context: &str) -> Result<String, String> {
    let config = Kubeconfig::read_from(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let NamedContext {
        name,
        context: Some(ctx),
        ..
    } = config
        .contexts
        .iter()
        .find(|c| c.name == context)
        .cloned()
        .ok_or_else(|| format!("{context} isn't in {}", file.display()))?
    else {
        return Err(format!("{context} is empty"));
    };
    let NamedCluster {
        name: cluster_name,
        cluster: Some(cluster),
        ..
    } = config
        .clusters
        .iter()
        .find(|c| c.name == ctx.cluster)
        .cloned()
        .ok_or_else(|| format!("{context}'s cluster isn't in the file"))?
    else {
        return Err(format!("{context}'s cluster is empty"));
    };
    if cluster
        .proxy_url
        .as_deref()
        .is_some_and(|u| u.contains('@'))
    {
        return Err("the cluster's proxy URL holds a password".into());
    }
    let user = match &ctx.user {
        Some(user) => Some(
            config
                .auth_infos
                .iter()
                .find(|u| &u.name == user)
                .cloned()
                .ok_or_else(|| format!("{context}'s user isn't in the file"))?,
        ),
        None => None,
    };
    let user_json = match user {
        Some(NamedAuthInfo {
            name: user_name,
            auth_info: Some(auth),
            ..
        }) => {
            if auth.token.is_some()
                || auth.password.is_some()
                || auth.token_file.is_some()
                || auth.client_key.is_some()
                || auth.client_key_data.is_some()
                || auth.auth_provider.is_some()
            {
                return Err(format!(
                    "{context}'s user holds credentials in the kubeconfig; only exec plugins can be shared"
                ));
            }
            let mut value = json!({});
            if let Some(exec) = &auth.exec {
                value["exec"] = serde_json::to_value(exec).map_err(|e| e.to_string())?;
            }
            if let Some(cert) = &auth.client_certificate_data {
                value["client-certificate-data"] = json!(cert);
            }
            if let Some(user) = &auth.impersonate {
                value["as"] = json!(user);
            }
            if let Some(groups) = &auth.impersonate_groups {
                value["as-groups"] = json!(groups);
            }
            Some(json!({ "name": user_name, "user": value }))
        }
        _ => None,
    };
    let cluster_json = serde_json::to_value(&cluster).map_err(|e| e.to_string())?;
    let mut context_json = json!({ "cluster": cluster_name });
    if let Some(user) = &ctx.user {
        context_json["user"] = json!(user);
    }
    if let Some(ns) = &ctx.namespace {
        context_json["namespace"] = json!(ns);
    }
    let out = json!({
        "apiVersion": "v1",
        "kind": "Config",
        "current-context": name,
        "clusters": [{ "name": cluster_name, "cluster": cluster_json }],
        "contexts": [{ "name": name, "context": context_json }],
        "users": user_json.into_iter().collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
apiVersion: v1
kind: Config
current-context: eks
clusters:
- name: eks-cluster
  cluster:
    server: https://example.eks.amazonaws.com
    certificate-authority-data: Q0E=
- name: kind
  cluster:
    server: https://127.0.0.1:6443
contexts:
- name: eks
  context: {cluster: eks-cluster, user: eks-user, namespace: shop}
- name: kind
  context: {cluster: kind, user: kind-admin}
users:
- name: eks-user
  user:
    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      command: aws
      args: [eks, get-token, --cluster-name, prod]
- name: kind-admin
  user:
    client-certificate-data: Q0VSVA==
    client-key-data: S0VZ
"#;

    #[test]
    fn exec_users_are_shared_and_inline_credentials_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config");
        std::fs::write(&file, CONFIG).unwrap();

        let eks = context_kubeconfig(&file, "eks").unwrap();
        let value: serde_json::Value = serde_json::from_str(&eks).unwrap();
        assert_eq!(value["current-context"], "eks");
        assert_eq!(value["contexts"][0]["context"]["namespace"], "shop");
        assert_eq!(value["users"][0]["user"]["exec"]["command"], "aws");
        assert_eq!(value["clusters"].as_array().unwrap().len(), 1);
        assert!(!eks.contains("kind-admin"));

        let err = context_kubeconfig(&file, "kind").unwrap_err();
        assert!(err.contains("holds credentials"), "{err}");
        assert!(context_kubeconfig(&file, "missing").is_err());
    }
}
