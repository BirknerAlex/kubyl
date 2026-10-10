//! Finding OpenCost: its Service, from a cluster's Services (name, namespace, labels, ports).

use crate::settings::Endpoint;

/// What detection needs to know of a Service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceInfo {
    pub namespace: String,
    pub name: String,
    /// `app.kubernetes.io/name` (the chart sets it) and `app` labels' values.
    pub app_labels: Vec<String>,
    /// `(name, port)` of each port.
    pub ports: Vec<(String, u16)>,
}

/// OpenCost's API port (`9003`); the chart names it `http`, the UI `http-ui` (`9090`).
pub const API_PORT: u16 = 9003;

fn is_opencost(service: &ServiceInfo) -> bool {
    service.app_labels.iter().any(|l| l == "opencost") || service.name == "opencost"
}

/// The port that serves the API: 9003, else a port named `http` or `api` that isn't the UI's.
fn api_port(service: &ServiceInfo) -> Option<u16> {
    if let Some((_, port)) = service.ports.iter().find(|(_, p)| *p == API_PORT) {
        return Some(*port);
    }
    service
        .ports
        .iter()
        .find(|(name, _)| matches!(name.as_str(), "http" | "api" | "opencost"))
        .map(|(_, port)| *port)
}

/// OpenCost's API endpoint among `services`: the Service the chart makes (label or name
/// `opencost`; the labelled one first, then the one named `opencost`, then by namespace). `None` when there is none, or
/// none with a port for the API.
pub fn find(services: &[ServiceInfo]) -> Option<Endpoint> {
    let mut candidates: Vec<&ServiceInfo> = services.iter().filter(|s| is_opencost(s)).collect();
    candidates.sort_by(|a, b| {
        let key = |s: &ServiceInfo| {
            (
                !s.app_labels.iter().any(|l| l == "opencost"),
                s.name != "opencost",
                s.namespace.clone(),
                s.name.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
    candidates.into_iter().find_map(|s| {
        Some(Endpoint {
            namespace: s.namespace.clone(),
            service: s.name.clone(),
            port: api_port(s)?.to_string(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(ns: &str, name: &str, labels: &[&str], ports: &[(&str, u16)]) -> ServiceInfo {
        ServiceInfo {
            namespace: ns.into(),
            name: name.into(),
            app_labels: labels.iter().map(|l| l.to_string()).collect(),
            ports: ports.iter().map(|(n, p)| (n.to_string(), *p)).collect(),
        }
    }

    #[test]
    fn finds_the_chart_service_and_its_api_port() {
        // What the chart makes (recorded from kind): http 9003, mcp-server 8081, http-ui 9090.
        let found = find(&[
            service(
                "monitoring",
                "kube-prometheus-stack-prometheus",
                &["prometheus"],
                &[("http-web", 9090)],
            ),
            service(
                "opencost",
                "opencost",
                &["opencost"],
                &[("http", 9003), ("mcp-server", 8081), ("http-ui", 9090)],
            ),
        ])
        .unwrap();
        assert_eq!(
            (
                found.namespace.as_str(),
                found.service.as_str(),
                found.port.as_str()
            ),
            ("opencost", "opencost", "9003")
        );
        assert_eq!(found.to_string(), "opencost/opencost:9003");
    }

    #[test]
    fn other_installs_are_found_by_label_and_port_name() {
        let found = find(&[service(
            "finops",
            "cost-model",
            &["opencost"],
            &[("ui", 9090), ("http", 8080)],
        )])
        .unwrap();
        assert_eq!(found.to_string(), "finops/cost-model:8080");
        // The UI port alone isn't the API.
        assert_eq!(
            find(&[service(
                "a",
                "opencost",
                &["opencost"],
                &[("http-ui", 9090)]
            )]),
            None
        );
        assert_eq!(
            find(&[service("a", "web", &["web"], &[("http", 9003)])]),
            None
        );
        assert_eq!(find(&[]), None);
    }

    #[test]
    fn a_labelled_service_beats_one_that_only_has_the_name() {
        // Anyone who can create a Service could name it `opencost` in an early namespace.
        let found = find(&[
            service("aaa", "opencost", &[], &[("http", 9003)]),
            service("cost", "my-cost", &["opencost"], &[("http", 9003)]),
        ])
        .unwrap();
        assert_eq!(
            (found.namespace.as_str(), found.service.as_str()),
            ("cost", "my-cost")
        );
    }

    #[test]
    fn the_service_named_opencost_wins() {
        let found = find(&[
            service("a", "opencost-extra", &["opencost"], &[("http", 9003)]),
            service("z", "opencost", &["opencost"], &[("http", 9003)]),
        ])
        .unwrap();
        assert_eq!(found.namespace, "z");
    }
}
