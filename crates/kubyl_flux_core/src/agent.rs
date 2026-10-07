//! What "Ask agent" sends about a Flux object: its state, conditions, revision, source,
//! dependencies, the inventory objects that aren't healthy and recent warning Events. Built from
//! status fields only: no Secret values, no Helm values, no post-build substitution values, no
//! `last-applied-configuration`.

use std::fmt::Write as _;

use crate::deps::Dependency;
use crate::details::{HelmReleaseSpec, KustomizationSpec, failures};
use crate::kinds::FluxKind;
use crate::links::strip_credentials;
use crate::model::FluxObject;

/// An inventory object that isn't healthy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unhealthy {
    /// `Deployment flux-podinfo/podinfo`.
    pub label: String,
    /// `0/2 ready`, `CrashLoopBackOff`, `not found`.
    pub status: String,
}

/// A recent warning Event about the object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    pub reason: String,
    pub message: String,
    /// `3m` ago.
    pub age: String,
    pub count: i64,
}

/// The prompt attachment.
pub fn text(
    object: &FluxObject,
    dependencies: &[Dependency],
    unhealthy: &[Unhealthy],
    warnings: &[Warning],
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Flux {} {}/{} ({})",
        object.kind, object.namespace, object.name, object.api_version
    );
    let _ = writeln!(
        out,
        "State: {} — {}",
        object.state().label(),
        object.message()
    );
    if object.suspended {
        let _ = writeln!(out, "Suspended: yes (spec.suspend)");
    }
    if let Some(interval) = &object.interval {
        let _ = writeln!(out, "Interval: {interval}");
    }
    if let Some(revision) = object.revision() {
        let _ = writeln!(out, "Revision: {revision}");
    }
    if let Some(attempted) = &object.last_attempted_revision
        && object.last_applied_revision.as_ref() != Some(attempted)
    {
        let _ = writeln!(out, "Last attempted revision: {attempted}");
    }
    if let Some(source) = &object.source {
        let _ = writeln!(out, "Source: {}", source.label(&object.namespace));
    }
    if let Some(url) = object.raw.pointer("/spec/url").and_then(|u| u.as_str()) {
        let _ = writeln!(out, "URL: {}", strip_credentials(url));
    }
    match object.kind {
        FluxKind::Kustomization => {
            let spec = KustomizationSpec::parse(object);
            let _ = writeln!(
                out,
                "Path: {} · prune: {} · target namespace: {}",
                spec.path,
                spec.prune,
                spec.target_namespace.as_deref().unwrap_or("(none)")
            );
            if !spec.substitutions.is_empty() || !spec.substitute_from.is_empty() {
                let names: Vec<String> = spec
                    .substitutions
                    .iter()
                    .map(|(k, _)| k.clone())
                    .chain(
                        spec.substitute_from
                            .iter()
                            .map(|r| format!("{} {}", r.kind, r.name)),
                    )
                    .collect();
                let _ = writeln!(
                    out,
                    "Post-build substitutions (names only): {}",
                    names.join(", ")
                );
            }
        }
        FluxKind::HelmRelease => {
            let spec = HelmReleaseSpec::parse(object);
            let _ = writeln!(
                out,
                "Chart: {} {} · release {}",
                spec.chart,
                spec.version.as_deref().unwrap_or(""),
                spec.release_name
            );
            if !spec.values_from.is_empty() {
                let names: Vec<String> = spec
                    .values_from
                    .iter()
                    .map(|r| format!("{} {}", r.kind, r.name))
                    .collect();
                let _ = writeln!(out, "Values from (names only): {}", names.join(", "));
            }
            let failures = failures(object);
            if failures.install + failures.upgrade + failures.total > 0 {
                let _ = writeln!(
                    out,
                    "Failures: install {}, upgrade {}, total {}",
                    failures.install, failures.upgrade, failures.total
                );
            }
        }
        _ => {}
    }
    if !object.conditions.is_empty() {
        let _ = writeln!(out, "\nConditions:");
        for c in &object.conditions {
            let _ = writeln!(
                out,
                "- {}={} ({}): {}",
                c.kind, c.status, c.reason, c.message
            );
        }
    }
    if !dependencies.is_empty() {
        let _ = writeln!(out, "\nDepends on:");
        for dep in dependencies {
            let state = dep.state.map_or("not found", |s| s.label());
            let _ = writeln!(out, "- {} ({state})", dep.key);
        }
    }
    if !unhealthy.is_empty() {
        let _ = writeln!(out, "\nInventory objects that aren't healthy:");
        for object in unhealthy.iter().take(30) {
            let _ = writeln!(out, "- {}: {}", object.label, object.status);
        }
        if unhealthy.len() > 30 {
            let _ = writeln!(out, "- … and {} more", unhealthy.len() - 30);
        }
    }
    if !warnings.is_empty() {
        let _ = writeln!(out, "\nRecent warning events:");
        for warning in warnings.iter().take(15) {
            let _ = writeln!(
                out,
                "- {} ago {} ×{}: {}",
                warning.age, warning.reason, warning.count, warning.message
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::model::State;
    use std::sync::Arc;

    #[test]
    fn agent_text_without_secret_values() {
        let mut value = fixtures::kustomization_ready();
        value["metadata"]["annotations"] = serde_json::json!({
            "kubectl.kubernetes.io/last-applied-configuration": "{\"secret\":\"leak\"}"});
        let ks = FluxObject::parse(&Arc::new(value)).unwrap();
        let text = text(
            &ks,
            &[Dependency {
                key: "flux-demo/infra".into(),
                state: Some(State::Ready),
                ready: true,
                custom_check: false,
            }],
            &[Unhealthy {
                label: "Deployment flux-podinfo/podinfo".into(),
                status: "0/2 ready".into(),
            }],
            &[Warning {
                reason: "HealthCheckFailed".into(),
                message: "timeout".into(),
                age: "3m".into(),
                count: 2,
            }],
        );
        assert!(text.contains("State: Ready"));
        assert!(text.contains("Source: GitRepository/podinfo"));
        assert!(text.contains("- flux-demo/infra (Ready)"));
        assert!(text.contains("Deployment flux-podinfo/podinfo: 0/2 ready"));
        assert!(text.contains("HealthCheckFailed ×2"));
        assert!(text.contains("names only): cluster_env, Secret podinfo-substitutions"));
        assert!(!text.contains("leak"));
        // The inline substitution's value stays out.
        assert!(!text.contains("dev\n") && !text.contains("= dev"));
        let hr = FluxObject::parse(&Arc::new(fixtures::helm_release_v2())).unwrap();
        let text = super::text(&hr, &[], &[], &[]);
        assert!(text.contains(
            "Values from (names only): ConfigMap podinfo-values, Secret podinfo-secret-values"
        ));
        assert!(!text.contains("32Mi"));
        let git = FluxObject::parse(&Arc::new(fixtures::git_repository_v1beta2())).unwrap();
        assert!(!super::text(&git, &[], &[], &[]).contains("hunter2"));
    }
}
