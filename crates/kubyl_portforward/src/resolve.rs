//! Resolving a forward target (Pod, Service or Deployment/StatefulSet/DaemonSet) to a pod name
//! and a remote container port. Re-run before every new local connection, so a Service forward
//! automatically picks a fresh pod after the old one dies (see [`crate::listener`]).

use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::Api;
use kube::api::ListParams;

/// What to forward to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardKind {
    Pod { pod: String },
    Service { service: String },
    Workload { label_selector: String },
}

/// The remote port, either given directly (Pod/Workload forwards) or a Service port to resolve
/// against `spec.ports[].targetPort`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemotePort {
    Container(u16),
    Service(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub pod: String,
    pub port: u16,
}

/// One `spec.ports[]` entry of a Service, in a form independent of `k8s-openapi` so the
/// matching logic is unit-testable without building full API objects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServicePortInfo {
    pub port: i32,
    pub target_port: TargetPort,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetPort {
    Number(u16),
    Name(String),
}

/// Picks the Service port matching `requested` (or the first one when `None`).
pub fn match_service_port(
    ports: &[ServicePortInfo],
    requested: Option<i32>,
) -> Option<ServicePortInfo> {
    match requested {
        Some(port) => ports.iter().find(|p| p.port == port).cloned(),
        None => ports.first().cloned(),
    }
}

/// Resolves a (possibly named) target port against a pod's container ports.
pub fn resolve_target_port(target: &TargetPort, container_ports: &[(String, u16)]) -> Option<u16> {
    match target {
        TargetPort::Number(port) => Some(*port),
        TargetPort::Name(name) => container_ports
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, port)| *port),
    }
}

/// Picks the first pod whose containers all report Ready (or just the first pod if none do —
/// better to try a forward and fail than to report "no pods").
pub fn pick_pod(pods: &[PodInfo]) -> Option<&PodInfo> {
    pods.iter().find(|p| p.ready).or_else(|| pods.first())
}

/// A pod's forward-relevant state, independent of `k8s-openapi`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodInfo {
    pub name: String,
    pub ready: bool,
    pub container_ports: Vec<(String, u16)>,
}

fn pod_info(pod: &Pod) -> Option<PodInfo> {
    let name = pod.metadata.name.clone()?;
    let ready = pod
        .status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .is_some_and(|conds| {
            conds
                .iter()
                .any(|c| c.type_ == "Ready" && c.status == "True")
        });
    let container_ports = pod
        .spec
        .as_ref()
        .map(|spec| {
            spec.containers
                .iter()
                .flat_map(|c| c.ports.iter().flatten())
                .filter_map(|p| {
                    Some((
                        p.name.clone().unwrap_or_default(),
                        u16::try_from(p.container_port).ok()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(PodInfo {
        name,
        ready,
        container_ports,
    })
}

/// Resolves `kind`/`port` to a live pod and port. Called fresh for every new local connection.
pub async fn resolve(
    client: kube::Client,
    namespace: &str,
    kind: &ForwardKind,
    port: RemotePort,
) -> anyhow::Result<Resolved> {
    match kind {
        ForwardKind::Pod { pod } => {
            let RemotePort::Container(port) = port else {
                anyhow::bail!("a Pod forward needs a container port");
            };
            Ok(Resolved {
                pod: pod.clone(),
                port,
            })
        }
        ForwardKind::Workload { label_selector } => {
            let RemotePort::Container(port) = port else {
                anyhow::bail!("a workload forward needs a container port");
            };
            let api: Api<Pod> = Api::namespaced(client, namespace);
            let list = api
                .list(&ListParams::default().labels(label_selector))
                .await?;
            let pods: Vec<PodInfo> = list.items.iter().filter_map(pod_info).collect();
            let chosen = pick_pod(&pods)
                .ok_or_else(|| anyhow::anyhow!("no pods match selector {label_selector}"))?;
            Ok(Resolved {
                pod: chosen.name.clone(),
                port,
            })
        }
        ForwardKind::Service { service } => {
            let RemotePort::Service(requested) = port else {
                anyhow::bail!("a Service forward needs a service port");
            };
            let services: Api<Service> = Api::namespaced(client.clone(), namespace);
            let svc = services.get(service).await?;
            let spec = svc
                .spec
                .ok_or_else(|| anyhow::anyhow!("service {service} has no spec"))?;
            let ports: Vec<ServicePortInfo> = spec
                .ports
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| {
                    let target_port = match p.target_port {
                        Some(k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(n)) => {
                            TargetPort::Number(u16::try_from(n).ok()?)
                        }
                        Some(
                            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::String(name),
                        ) => TargetPort::Name(name),
                        None => TargetPort::Number(u16::try_from(p.port).ok()?),
                    };
                    Some(ServicePortInfo {
                        port: p.port,
                        target_port,
                    })
                })
                .collect();
            let matched = match_service_port(&ports, Some(requested as i32))
                .ok_or_else(|| anyhow::anyhow!("service {service} has no port {requested}"))?;
            let selector = spec.selector.unwrap_or_default();
            let label_selector = selector
                .into_iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",");
            let pods_api: Api<Pod> = Api::namespaced(client, namespace);
            let list = pods_api
                .list(&ListParams::default().labels(&label_selector))
                .await?;
            let pods: Vec<PodInfo> = list.items.iter().filter_map(pod_info).collect();
            let chosen = pick_pod(&pods)
                .ok_or_else(|| anyhow::anyhow!("service {service} has no backing pods"))?;
            let port = resolve_target_port(&matched.target_port, &chosen.container_ports)
                .unwrap_or(requested);
            Ok(Resolved {
                pod: chosen.name.clone(),
                port,
            })
        }
    }
}

/// Builds the workload's label selector for Deployment/StatefulSet/DaemonSet forwards, mirroring
/// `kubyl_logs::view::resolve_source`'s approach for the log source.
pub async fn workload_selector(
    client: kube::Client,
    namespace: &str,
    resource: &str,
    name: &str,
) -> anyhow::Result<String> {
    let labels = match resource {
        "deployments" => {
            let api: Api<Deployment> = Api::namespaced(client, namespace);
            api.get(name)
                .await?
                .spec
                .and_then(|s| s.selector.match_labels)
        }
        "statefulsets" => {
            let api: Api<StatefulSet> = Api::namespaced(client, namespace);
            api.get(name)
                .await?
                .spec
                .and_then(|s| s.selector.match_labels)
        }
        "daemonsets" => {
            let api: Api<DaemonSet> = Api::namespaced(client, namespace);
            api.get(name)
                .await?
                .spec
                .and_then(|s| s.selector.match_labels)
        }
        other => anyhow::bail!("port-forwarding isn't supported for {other}"),
    }
    .unwrap_or_default();
    Ok(labels
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_requested_service_port_or_falls_back_to_first() {
        let ports = vec![
            ServicePortInfo {
                port: 80,
                target_port: TargetPort::Name("http".into()),
            },
            ServicePortInfo {
                port: 443,
                target_port: TargetPort::Number(8443),
            },
        ];
        assert_eq!(
            match_service_port(&ports, Some(443)).unwrap().target_port,
            TargetPort::Number(8443)
        );
        assert_eq!(
            match_service_port(&ports, None).unwrap().target_port,
            TargetPort::Name("http".into())
        );
        assert!(match_service_port(&ports, Some(9999)).is_none());
    }

    #[test]
    fn resolves_named_target_ports_against_container_ports() {
        let container_ports = vec![("http".to_string(), 8080u16), ("metrics".to_string(), 9090)];
        assert_eq!(
            resolve_target_port(&TargetPort::Name("http".into()), &container_ports),
            Some(8080)
        );
        assert_eq!(
            resolve_target_port(&TargetPort::Number(1234), &container_ports),
            Some(1234)
        );
        assert_eq!(
            resolve_target_port(&TargetPort::Name("missing".into()), &container_ports),
            None
        );
    }

    #[test]
    fn pick_pod_prefers_ready_pods() {
        let pods = vec![
            PodInfo {
                name: "a".into(),
                ready: false,
                container_ports: vec![],
            },
            PodInfo {
                name: "b".into(),
                ready: true,
                container_ports: vec![],
            },
        ];
        assert_eq!(pick_pod(&pods).unwrap().name, "b");
    }

    #[test]
    fn pick_pod_falls_back_when_none_ready() {
        let pods = vec![PodInfo {
            name: "a".into(),
            ready: false,
            container_ports: vec![],
        }];
        assert_eq!(pick_pod(&pods).unwrap().name, "a");
    }
}
