//! A release as text for the user's agent (phase 21's "Ask agent"): chart, versions, status,
//! the last revisions and the resources that aren't ready. Never values, manifests or notes.

use std::fmt::Write as _;

use crate::decode::Summary;
use crate::release::Revision;

/// A resource of the release that isn't ready (`Deployment web: 0/3 ready`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failing {
    pub kind: String,
    pub name: String,
    pub reason: String,
}

/// The text attached to the agent's prompt.
pub fn release_text(
    namespace: &str,
    name: &str,
    summary: Option<&Summary>,
    revisions: &[Revision],
    failing: &[Failing],
) -> String {
    let mut text = format!("Helm release {namespace}/{name}");
    if let Some(latest) = revisions.first() {
        let _ = write!(
            text,
            ": revision {}, status {}",
            latest.revision, latest.status
        );
    }
    text.push('\n');
    if let Some(summary) = summary {
        let _ = writeln!(
            text,
            "Chart: {} {}{}",
            summary.chart.name,
            summary.chart.version,
            summary
                .chart
                .app_version
                .as_deref()
                .map(|v| format!(" (app {v})"))
                .unwrap_or_default()
        );
        if let Some(description) = &summary.description {
            let _ = writeln!(text, "Last operation: {description}");
        }
    }
    if !revisions.is_empty() {
        text.push_str("Revisions (newest first):\n");
        for revision in revisions.iter().take(5) {
            let _ = writeln!(
                text,
                "  {} {}{}",
                revision.revision,
                revision.status,
                revision
                    .modified
                    .map(|t| format!(" at {t}"))
                    .unwrap_or_default()
            );
        }
    }
    if failing.is_empty() {
        text.push_str("All of the release's workloads are ready.\n");
    } else {
        text.push_str("Resources that aren't ready:\n");
        for f in failing.iter().take(40) {
            let _ = writeln!(text, "  {} {}: {}", f.kind, f.name, f.reason);
        }
    }
    text.push_str("(Values and the manifest aren't included: they can hold credentials.)\n");
    text
}

/// The workloads of a release that aren't ready, read on Tokio (Deployments, StatefulSets,
/// DaemonSets and Jobs of the manifest; a failed read is skipped).
pub async fn failing(
    client: kube::Client,
    namespace: &str,
    objects: &[crate::present::ManifestObject],
) -> Vec<Failing> {
    use kube::api::{Api, DynamicObject};
    use kube::core::{ApiResource, GroupVersionKind};
    let mut out = Vec::new();
    for object in objects.iter().filter(|o| !o.hook).take(60) {
        let (group, version, plural) = match object.kind.as_str() {
            "Deployment" => ("apps", "v1", "deployments"),
            "StatefulSet" => ("apps", "v1", "statefulsets"),
            "DaemonSet" => ("apps", "v1", "daemonsets"),
            "Job" => ("batch", "v1", "jobs"),
            _ => continue,
        };
        let resource = ApiResource::from_gvk_with_plural(
            &GroupVersionKind::gvk(group, version, &object.kind),
            plural,
        );
        let ns = object.namespace.as_deref().unwrap_or(namespace);
        let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), ns, &resource);
        let Ok(live) = api.get(&object.name).await else {
            continue;
        };
        if let Some(reason) = not_ready(&object.kind, &live.data) {
            out.push(Failing {
                kind: object.kind.clone(),
                name: object.name.clone(),
                reason,
            });
        }
    }
    out
}

/// Why a workload isn't ready (from its status), `None` when it is.
pub fn not_ready(kind: &str, data: &serde_json::Value) -> Option<String> {
    let int = |path: &str| {
        data.pointer(path)
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
    };
    match kind {
        "Deployment" | "StatefulSet" => {
            let want = data
                .pointer("/spec/replicas")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(1);
            let ready = int("/status/readyReplicas");
            (ready < want).then(|| format!("{ready}/{want} ready"))
        }
        "DaemonSet" => {
            let want = int("/status/desiredNumberScheduled");
            let ready = int("/status/numberReady");
            (ready < want).then(|| format!("{ready}/{want} ready"))
        }
        "Job" => {
            let failed = int("/status/failed");
            (failed > 0).then(|| format!("{failed} failed pods"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::ChartMetadata;
    use serde_json::json;

    #[test]
    fn agent_text_has_no_values() {
        let summary = Summary {
            name: "web".into(),
            namespace: "shop".into(),
            revision: 3,
            chart: ChartMetadata {
                name: "demo".into(),
                version: "0.2.0".into(),
                app_version: Some("1.1.0".into()),
                ..Default::default()
            },
            status: "failed".into(),
            description: Some("Upgrade \"web\" failed: timed out".into()),
            ..Default::default()
        };
        let revisions = vec![
            Revision {
                revision: 3,
                status: "failed".into(),
                modified: None,
                object: "sh.helm.release.v1.web.v3".into(),
            },
            Revision {
                revision: 2,
                status: "deployed".into(),
                modified: None,
                object: "sh.helm.release.v1.web.v2".into(),
            },
        ];
        let text = release_text(
            "shop",
            "web",
            Some(&summary),
            &revisions,
            &[Failing {
                kind: "Deployment".into(),
                name: "web".into(),
                reason: "0/3 ready".into(),
            }],
        );
        assert!(text.starts_with("Helm release shop/web: revision 3, status failed\n"));
        assert!(text.contains("Chart: demo 0.2.0 (app 1.1.0)"));
        assert!(text.contains("  2 deployed"));
        assert!(text.contains("Deployment web: 0/3 ready"));
    }

    #[test]
    fn workloads_report_what_isnt_ready() {
        assert_eq!(
            not_ready(
                "Deployment",
                &json!({"spec": {"replicas": 3}, "status": {"readyReplicas": 1}})
            ),
            Some("1/3 ready".into())
        );
        assert_eq!(
            not_ready(
                "Deployment",
                &json!({"spec": {"replicas": 1}, "status": {"readyReplicas": 1}})
            ),
            None
        );
        assert_eq!(
            not_ready("Job", &json!({"status": {"failed": 2}})),
            Some("2 failed pods".into())
        );
    }
}
