//! Where Flux's controllers run and which versions they are: Deployments in `flux-system`, or
//! anywhere labelled `app.kubernetes.io/part-of: flux` (installs into another namespace),
//! versions from their image tags. Reads Deployments only.

use k8s_openapi::api::apps::v1::Deployment;
use kube::Client;
use kube::api::{Api, ListParams};

/// Flux's default namespace.
pub const DEFAULT_NAMESPACE: &str = "flux-system";
/// The label Flux's install manifests (and the Flux Operator) put on the controllers.
pub const PART_OF: &str = "app.kubernetes.io/part-of=flux";

/// A controller Deployment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Controller {
    /// `kustomize-controller`.
    pub name: String,
    pub namespace: String,
    pub image: String,
    /// `v1.9.6` from the image tag.
    pub version: Option<String>,
    pub ready: i32,
    pub replicas: i32,
}

impl Controller {
    /// All desired replicas are ready (a controller scaled to 0 isn't).
    pub fn is_ready(&self) -> bool {
        self.replicas > 0 && self.ready >= self.replicas
    }

    /// The controller as its Deployment (`deployment`, as JSON from a live watch) says now;
    /// `None`: the Deployment is gone, nothing is ready.
    pub fn with_live(&self, deployment: Option<&serde_json::Value>) -> Controller {
        let mut live = self.clone();
        match deployment {
            Some(d) => {
                let int = |p: &str| d.pointer(p).and_then(serde_json::Value::as_i64);
                live.ready = int("/status/readyReplicas").unwrap_or(0) as i32;
                live.replicas = int("/spec/replicas").unwrap_or(1) as i32;
                if let Some(image) = d
                    .pointer("/spec/template/spec/containers/0/image")
                    .and_then(serde_json::Value::as_str)
                {
                    live.version = image_version(image);
                    live.image = image.to_string();
                }
            }
            None => live.ready = 0,
        }
        live
    }
}

/// What detection found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Install {
    /// Where the controllers run (the first one's namespace).
    pub namespace: Option<String>,
    pub controllers: Vec<Controller>,
    /// The Flux release (`v2.9.6`), from an `app.kubernetes.io/version` label when an install
    /// sets one.
    pub distribution: Option<String>,
    /// The user may not list Deployments where Flux runs.
    pub forbidden: bool,
}

impl Install {
    pub fn controller(&self, name: &str) -> Option<&Controller> {
        self.controllers.iter().find(|c| c.name == name)
    }

    /// `v2.9.6`, else the kustomize-controller's version: the badge of the sidebar group.
    pub fn version(&self) -> Option<String> {
        self.distribution.clone().or_else(|| {
            self.controller("kustomize-controller")
                .or_else(|| self.controllers.first())
                .and_then(|c| c.version.clone())
        })
    }
}

/// `v1.9.6` from `ghcr.io/fluxcd/kustomize-controller:v1.9.6@sha256:…`.
pub fn image_version(image: &str) -> Option<String> {
    let image = image.split('@').next()?;
    let last = image.rsplit('/').next()?;
    let (_, tag) = last.rsplit_once(':')?;
    (!tag.is_empty()).then(|| tag.to_string())
}

/// The controllers among `deployments`.
pub fn controllers(deployments: &[Deployment]) -> Vec<Controller> {
    let mut list: Vec<Controller> = deployments
        .iter()
        .filter_map(|d| {
            let name = d.metadata.name.clone()?;
            let labels = d.metadata.labels.clone().unwrap_or_default();
            let part_of = labels.get("app.kubernetes.io/part-of").map(String::as_str);
            let is_flux = part_of == Some("flux")
                || (name.ends_with("-controller")
                    && d.spec
                        .as_ref()
                        .and_then(|s| s.template.spec.as_ref())
                        .is_some_and(|s| {
                            s.containers
                                .iter()
                                .any(|c| c.image.as_deref().is_some_and(|i| i.contains("/fluxcd/")))
                        }));
            if !is_flux {
                return None;
            }
            let image = d
                .spec
                .as_ref()
                .and_then(|s| s.template.spec.as_ref())
                .and_then(|s| s.containers.first())
                .and_then(|c| c.image.clone())
                .unwrap_or_default();
            Some(Controller {
                version: image_version(&image),
                image,
                namespace: d.metadata.namespace.clone().unwrap_or_default(),
                ready: d
                    .status
                    .as_ref()
                    .and_then(|s| s.ready_replicas)
                    .unwrap_or(0),
                replicas: d.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1),
                name,
            })
        })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

fn distribution(deployments: &[Deployment]) -> Option<String> {
    deployments.iter().find_map(|d| {
        d.metadata
            .labels
            .as_ref()?
            .get("app.kubernetes.io/version")
            .filter(|v| v.starts_with("v2"))
            .cloned()
    })
}

fn forbidden(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(status) if status.code == 403)
}

/// Finds the controllers: `flux-system` first, then by label in every namespace.
pub async fn detect(client: Client) -> Result<Install, String> {
    let params = ListParams::default();
    let mut forbidden_here = false;
    let local: Api<Deployment> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
    match local.list(&params).await {
        Ok(list) => {
            let found = controllers(&list.items);
            if !found.is_empty() {
                return Ok(Install {
                    namespace: Some(DEFAULT_NAMESPACE.into()),
                    distribution: distribution(&list.items),
                    controllers: found,
                    forbidden: false,
                });
            }
        }
        Err(err) if forbidden(&err) => forbidden_here = true,
        Err(err) => return Err(err.to_string()),
    }
    let all: Api<Deployment> = Api::all(client);
    match all.list(&ListParams::default().labels(PART_OF)).await {
        Ok(list) => {
            let found = controllers(&list.items);
            Ok(Install {
                namespace: found.first().map(|c| c.namespace.clone()),
                distribution: distribution(&list.items),
                controllers: found,
                forbidden: false,
            })
        }
        Err(err) if forbidden(&err) => Ok(Install {
            // Only the cluster-wide list denied: flux-system was readable and just empty.
            forbidden: forbidden_here,
            namespace: Some(DEFAULT_NAMESPACE.to_string()),
            ..Install::default()
        }),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deployment(name: &str, image: &str, labels: &[(&str, &str)], ready: i32) -> Deployment {
        serde_json::from_value(serde_json::json!({
            "metadata": {"name": name, "namespace": "flux-system",
                "labels": labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<std::collections::BTreeMap<_, _>>()},
            "spec": {"replicas": 1, "selector": {}, "template": {"spec": {"containers": [{"name": "manager", "image": image}]}}},
            "status": {"readyReplicas": ready}
        }))
        .unwrap()
    }

    #[test]
    fn controllers_and_versions() {
        assert_eq!(
            image_version("ghcr.io/fluxcd/kustomize-controller:v1.9.6").as_deref(),
            Some("v1.9.6")
        );
        assert_eq!(
            image_version("registry:5000/fluxcd/helm-controller:v1.6.5@sha256:abc").as_deref(),
            Some("v1.6.5")
        );
        assert_eq!(image_version("registry:5000/fluxcd/helm-controller"), None);
        let list = vec![
            deployment(
                "kustomize-controller",
                "ghcr.io/fluxcd/kustomize-controller:v1.9.6",
                &[("app.kubernetes.io/part-of", "flux")],
                1,
            ),
            deployment(
                "helm-controller",
                "mirror.example.com/fluxcd/helm-controller:v1.6.5",
                &[],
                0,
            ),
            deployment("web", "nginx:1.27", &[], 1),
        ];
        let found = controllers(&list);
        let names: Vec<_> = found.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["helm-controller", "kustomize-controller"]);
        assert!(!found[0].is_ready() && found[1].is_ready());
        // Two replicas wanted, one ready: not ready.
        let mut half = found[1].clone();
        half.replicas = 2;
        assert!(!half.is_ready());
        half.replicas = 0;
        assert!(!half.is_ready());
        // Live from a watch.
        let live = found[0].with_live(Some(&serde_json::json!({
            "spec": {"replicas": 1, "template": {"spec": {"containers": [{"image": "ghcr.io/fluxcd/helm-controller:v1.7.0"}]}}},
            "status": {"readyReplicas": 1}})));
        assert!(live.is_ready());
        assert_eq!(live.version.as_deref(), Some("v1.7.0"));
        assert!(!found[1].with_live(None).is_ready());
        let install = Install {
            namespace: Some("flux-system".into()),
            controllers: found,
            distribution: None,
            forbidden: false,
        };
        assert_eq!(install.version().as_deref(), Some("v1.9.6"));
        let mut labelled = list.clone();
        labelled[0]
            .metadata
            .labels
            .as_mut()
            .unwrap()
            .insert("app.kubernetes.io/version".into(), "v2.9.6".into());
        assert_eq!(distribution(&labelled).as_deref(), Some("v2.9.6"));
    }
}
