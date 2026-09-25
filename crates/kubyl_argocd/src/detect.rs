//! Where Argo CD is installed on a cluster: the namespace of its `argocd-cm` ConfigMap, the
//! `argocd-server` Service (API mode talks to it), the application controller (its logs), the
//! version (from the server's image tag), and settings Kubyl needs: the external URL, "apps in
//! any namespace" (`application.namespaces`), `server.insecure`, `server.rootpath`, SSO.
//!
//! Only ConfigMaps, Services and workloads are read, never Secrets. Detection works with partial
//! access: without cluster-wide ConfigMap listing it checks the namespaces the Applications'
//! `status.controllerNamespace` names, then the usual ones (`argocd`, `openshift-gitops`).
//!
//! Finding an install doesn't make it trusted: API mode signs in only after the user confirmed
//! this namespace and Service (see `session`).

use std::collections::BTreeSet;

use k8s_openapi::api::apps::v1::{Deployment, StatefulSet};
use k8s_openapi::api::core::v1::{ConfigMap, Service};
use kube::Client;
use kube::api::{Api, ListParams};

/// Namespaces checked when ConfigMaps can't be listed cluster-wide.
const USUAL_NAMESPACES: &[&str] = &["argocd", "openshift-gitops", "argo-cd", "gitops"];

/// The `argocd-server` Service of an install.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerService {
    pub name: String,
    /// The Service's UID: a re-created Service must be confirmed again.
    pub uid: String,
    pub https_port: Option<u16>,
    pub http_port: Option<u16>,
}

/// A workload of an install (the application controller).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workload {
    /// `statefulsets` or `deployments`.
    pub resource: &'static str,
    pub name: String,
}

/// How users sign in besides local accounts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sso {
    /// Argo CD's bundled Dex has connectors (`dex.config`).
    pub dex: bool,
    /// An external OIDC provider (`oidc.config`), by issuer.
    pub oidc_issuer: Option<String>,
}

/// One Argo CD installation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Install {
    pub namespace: String,
    /// `v3.5.3`, from the server's (or controller's) image tag.
    pub version: Option<String>,
    pub server: Option<ServerService>,
    pub controller: Option<Workload>,
    /// The external URL users open (`argocd-cm` `url`). Shown, never connected to.
    pub url: Option<String>,
    /// Namespaces Applications may live in besides this one (`application.namespaces`, may
    /// hold globs).
    pub app_namespaces: Vec<String>,
    /// `server.insecure`: the server speaks plain HTTP.
    pub insecure: bool,
    /// `server.rootpath`, e.g. `/argocd`; empty when served at the root.
    pub root_path: String,
    /// `application.resourceTrackingMethod` (`annotation` since 3.0, `label` before).
    pub tracking: Option<String>,
    pub admin_enabled: bool,
    pub sso: Sso,
    /// Some objects couldn't be read (RBAC): the details above may be incomplete.
    pub partial: bool,
}

impl Install {
    /// `argocd · v3.5.3`.
    pub fn label(&self) -> String {
        match &self.version {
            Some(version) => format!("{} · {version}", self.namespace),
            None => self.namespace.clone(),
        }
    }

    /// Whether Applications in `namespace` belong to this install.
    pub fn manages_namespace(&self, namespace: &str) -> bool {
        namespace == self.namespace
            || self
                .app_namespaces
                .iter()
                .any(|p| crate::windows::glob_match(p, namespace))
    }

    /// Resource tracking by label (`app.kubernetes.io/instance`) is in use.
    pub fn tracks_by_label(&self) -> bool {
        matches!(
            self.tracking.as_deref(),
            Some("label") | Some("annotation+label")
        )
    }
}

fn forbidden(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(status) if status.code == 403 || status.code == 401)
}

fn not_found(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(status) if status.code == 404)
}

/// Finds Argo CD installs. `hints`: namespaces Applications report as their controller's.
pub async fn detect(client: Client, hints: Vec<String>) -> Result<Vec<Install>, String> {
    let mut namespaces: BTreeSet<String> = BTreeSet::new();
    let configmaps: Api<ConfigMap> = Api::all(client.clone());
    let listed = match configmaps
        .list_metadata(&ListParams::default().fields("metadata.name=argocd-cm"))
        .await
    {
        Ok(list) => {
            namespaces.extend(
                list.items
                    .into_iter()
                    .filter_map(|cm| cm.metadata.namespace),
            );
            true
        }
        Err(err) if forbidden(&err) => false,
        Err(err) => return Err(err.to_string()),
    };
    if !listed {
        namespaces.extend(hints.iter().cloned());
        namespaces.extend(USUAL_NAMESPACES.iter().map(|s| s.to_string()));
    } else {
        // A controller namespace without a readable argocd-cm is still an install.
        namespaces.extend(hints.iter().cloned());
    }
    let mut installs = Vec::new();
    for namespace in namespaces {
        let hinted = hints.contains(&namespace);
        if let Some(install) = inspect(client.clone(), &namespace, hinted).await {
            installs.push(install);
        }
    }
    Ok(installs)
}

/// Reads one namespace. `None` when it isn't an Argo CD install.
async fn inspect(client: Client, namespace: &str, hinted: bool) -> Option<Install> {
    let configmaps: Api<ConfigMap> = Api::namespaced(client.clone(), namespace);
    let mut install = Install {
        namespace: namespace.to_string(),
        admin_enabled: true,
        ..Default::default()
    };
    match configmaps.get("argocd-cm").await {
        Ok(cm) => apply_cm(&mut install, &cm),
        Err(err) if not_found(&err) && !hinted => return None,
        Err(err) if forbidden(&err) || not_found(&err) => {
            if !hinted {
                return None;
            }
            install.partial = true;
        }
        Err(err) => {
            tracing::debug!(namespace, "argocd-cm: {err}");
            if !hinted {
                return None;
            }
            install.partial = true;
        }
    }
    match configmaps.get("argocd-cmd-params-cm").await {
        Ok(cm) => apply_params(&mut install, &cm),
        Err(err) if not_found(&err) => {}
        Err(_) => install.partial = true,
    }
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    match services.list(&ListParams::default()).await {
        Ok(list) => install.server = pick_server(&list.items),
        Err(_) => install.partial = true,
    }
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), namespace);
    let stateful_sets: Api<StatefulSet> = Api::namespaced(client, namespace);
    let params = ListParams::default();
    let (deployments, stateful_sets) =
        futures::join!(deployments.list(&params), stateful_sets.list(&params));
    let deployments = deployments.map(|l| l.items).unwrap_or_else(|_| {
        install.partial = true;
        Vec::new()
    });
    let stateful_sets = stateful_sets.map(|l| l.items).unwrap_or_else(|_| {
        install.partial = true;
        Vec::new()
    });
    install.controller = pick_controller(&deployments, &stateful_sets);
    install.version = version_of(&deployments, &stateful_sets);
    Some(install)
}

fn apply_cm(install: &mut Install, cm: &ConfigMap) {
    let data = cm.data.clone().unwrap_or_default();
    install.url = data.get("url").filter(|u| !u.is_empty()).cloned();
    install.tracking = data
        .get("application.resourceTrackingMethod")
        .filter(|t| !t.is_empty())
        .cloned();
    install.admin_enabled = data
        .get("admin.enabled")
        .map(|v| v != "false")
        .unwrap_or(true);
    install.sso.dex = data.get("dex.config").is_some_and(|c| !c.trim().is_empty());
    // Only the issuer: the rest may name client secrets (as `$references`).
    install.sso.oidc_issuer = data.get("oidc.config").and_then(|c| {
        c.lines()
            .find_map(|l| l.trim().strip_prefix("issuer:"))
            .map(|i| i.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|i| !i.is_empty())
    });
}

fn apply_params(install: &mut Install, cm: &ConfigMap) {
    let data = cm.data.clone().unwrap_or_default();
    install.app_namespaces = data
        .get("application.namespaces")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    install.insecure = data.get("server.insecure").is_some_and(|v| v == "true");
    install.root_path = data
        .get("server.rootpath")
        .or_else(|| data.get("server.basehref"))
        .map(|p| p.trim().trim_end_matches('/').to_string())
        .filter(|p| !p.is_empty() && p != "/")
        .map(|p| {
            if p.starts_with('/') {
                p
            } else {
                format!("/{p}")
            }
        })
        .unwrap_or_default();
}

fn label<'a>(
    meta: &'a k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
    key: &str,
) -> Option<&'a str> {
    meta.labels.as_ref()?.get(key).map(String::as_str)
}

/// The API server's Service: `argocd-server`, else the `server` component of the install
/// (Helm releases prefix it, the OpenShift GitOps operator names it `<name>-server`).
pub fn pick_server(services: &[Service]) -> Option<ServerService> {
    let is_metrics = |s: &Service| {
        s.metadata
            .name
            .as_deref()
            .is_some_and(|n| n.ends_with("-metrics"))
    };
    let candidate = services
        .iter()
        .find(|s| s.metadata.name.as_deref() == Some("argocd-server"))
        .or_else(|| {
            services.iter().find(|s| {
                !is_metrics(s)
                    && label(&s.metadata, "app.kubernetes.io/component") == Some("server")
                    && label(&s.metadata, "app.kubernetes.io/part-of") == Some("argocd")
            })
        })
        .or_else(|| {
            services.iter().find(|s| {
                !is_metrics(s)
                    && s.metadata
                        .name
                        .as_deref()
                        .is_some_and(|n| n.ends_with("-server"))
                    && label(&s.metadata, "app.kubernetes.io/part-of") == Some("argocd")
            })
        })?;
    let ports = candidate
        .spec
        .as_ref()
        .and_then(|s| s.ports.clone())
        .unwrap_or_default();
    let port = |names: &[&str], number: i32| {
        ports
            .iter()
            .find(|p| p.name.as_deref().is_some_and(|n| names.contains(&n)))
            .or_else(|| ports.iter().find(|p| p.port == number))
            .and_then(|p| u16::try_from(p.port).ok())
    };
    Some(ServerService {
        name: candidate.metadata.name.clone().unwrap_or_default(),
        uid: candidate.metadata.uid.clone().unwrap_or_default(),
        https_port: port(&["https"], 443),
        http_port: port(&["http"], 80),
    })
}

fn is_controller(meta: &k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta) -> bool {
    label(meta, "app.kubernetes.io/component") == Some("application-controller")
        || meta
            .name
            .as_deref()
            .is_some_and(|n| n.ends_with("application-controller"))
}

fn pick_controller(deployments: &[Deployment], stateful_sets: &[StatefulSet]) -> Option<Workload> {
    if let Some(sts) = stateful_sets.iter().find(|s| is_controller(&s.metadata)) {
        return Some(Workload {
            resource: "statefulsets",
            name: sts.metadata.name.clone().unwrap_or_default(),
        });
    }
    deployments
        .iter()
        .find(|d| is_controller(&d.metadata))
        .map(|d| Workload {
            resource: "deployments",
            name: d.metadata.name.clone().unwrap_or_default(),
        })
}

/// `v3.5.3` from `quay.io/argoproj/argocd:v3.5.3` (digests dropped).
pub fn image_version(image: &str) -> Option<String> {
    let image = image.split('@').next()?;
    let (_, tag) = image.rsplit_once(':')?;
    // A registry port (`host:5000/argocd`) isn't a tag.
    if tag.contains('/') || tag.is_empty() {
        return None;
    }
    Some(tag.to_string())
}

fn version_of(deployments: &[Deployment], stateful_sets: &[StatefulSet]) -> Option<String> {
    let image_of = |spec: Option<&k8s_openapi::api::core::v1::PodTemplateSpec>| {
        spec.and_then(|t| t.spec.as_ref())
            .and_then(|s| s.containers.first())
            .and_then(|c| c.image.clone())
    };
    let server = deployments.iter().find(|d| {
        label(&d.metadata, "app.kubernetes.io/component") == Some("server")
            || d.metadata.name.as_deref() == Some("argocd-server")
    });
    server
        .and_then(|d| image_of(d.spec.as_ref().map(|s| &s.template)))
        .or_else(|| {
            stateful_sets
                .iter()
                .find(|s| is_controller(&s.metadata))
                .and_then(|s| image_of(s.spec.as_ref().map(|s| &s.template)))
        })
        .and_then(|image| image_version(&image))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn service(value: serde_json::Value) -> Service {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn picks_the_server_service() {
        let services = vec![
            service(
                json!({"metadata": {"name": "argocd-server-metrics", "uid": "m",
                "labels": {"app.kubernetes.io/component": "server", "app.kubernetes.io/part-of": "argocd"}},
                "spec": {"ports": [{"name": "metrics", "port": 8083}]}}),
            ),
            service(json!({"metadata": {"name": "argocd-server", "uid": "s1"},
                "spec": {"ports": [{"name": "http", "port": 80}, {"name": "https", "port": 443}]}})),
        ];
        let server = pick_server(&services).unwrap();
        assert_eq!(server.name, "argocd-server");
        assert_eq!(server.uid, "s1");
        assert_eq!((server.https_port, server.http_port), (Some(443), Some(80)));

        // A Helm release prefixes the name.
        let helm = vec![service(
            json!({"metadata": {"name": "prod-argocd-server", "uid": "h",
            "labels": {"app.kubernetes.io/component": "server", "app.kubernetes.io/part-of": "argocd"}},
            "spec": {"ports": [{"name": "https", "port": 8443}]}}),
        )];
        let server = pick_server(&helm).unwrap();
        assert_eq!(server.name, "prod-argocd-server");
        assert_eq!(server.https_port, Some(8443));
        // OpenShift GitOps.
        let gitops = vec![service(
            json!({"metadata": {"name": "openshift-gitops-server", "uid": "o",
            "labels": {"app.kubernetes.io/part-of": "argocd"}}, "spec": {"ports": [{"port": 443}]}}),
        )];
        assert_eq!(
            pick_server(&gitops).unwrap().name,
            "openshift-gitops-server"
        );
        // Some other app's server isn't Argo CD's.
        let other = vec![service(
            json!({"metadata": {"name": "web-server", "uid": "w"}}),
        )];
        assert!(pick_server(&other).is_none());
    }

    #[test]
    fn versions_from_images() {
        assert_eq!(
            image_version("quay.io/argoproj/argocd:v3.5.3").as_deref(),
            Some("v3.5.3")
        );
        assert_eq!(
            image_version("registry:5000/argoproj/argocd:v3.4.9@sha256:abc").as_deref(),
            Some("v3.4.9")
        );
        assert_eq!(image_version("registry:5000/argoproj/argocd"), None);
    }

    #[test]
    fn settings_from_config_maps() {
        let mut install = Install::default();
        let cm: ConfigMap = serde_json::from_value(json!({"metadata": {"name": "argocd-cm"},
            "data": {"url": "https://argocd.example.com", "admin.enabled": "false",
                     "application.resourceTrackingMethod": "label",
                     "oidc.config": "name: Okta\nissuer: https://acme.okta.com\nclientID: abc\nclientSecret: $oidc.okta.clientSecret\n"}}))
        .unwrap();
        apply_cm(&mut install, &cm);
        assert_eq!(install.url.as_deref(), Some("https://argocd.example.com"));
        assert!(!install.admin_enabled);
        assert!(install.tracks_by_label());
        assert_eq!(
            install.sso.oidc_issuer.as_deref(),
            Some("https://acme.okta.com")
        );
        assert!(!install.sso.dex);
        let params: ConfigMap =
            serde_json::from_value(json!({"metadata": {"name": "argocd-cmd-params-cm"},
            "data": {"application.namespaces": "argocd-apps, team-*", "server.insecure": "true",
                     "server.rootpath": "argocd/"}}))
            .unwrap();
        install.namespace = "argocd".into();
        apply_params(&mut install, &params);
        assert_eq!(install.app_namespaces, ["argocd-apps", "team-*"]);
        assert!(install.insecure);
        assert_eq!(install.root_path, "/argocd");
        assert!(install.manages_namespace("argocd"));
        assert!(install.manages_namespace("team-payments"));
        assert!(!install.manages_namespace("default"));
    }
}
