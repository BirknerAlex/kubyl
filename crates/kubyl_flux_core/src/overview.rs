//! The Flux overview: counts and ready totals per kind, the health banner, and what needs
//! attention (failed, stalled, waiting for a dependency, sources that weren't fetched,
//! suspended).
//!
//! Suspended objects are listed without a duration: suspending only sets `spec.suspend` and
//! Flux records no time for it (conditions keep their last transition, and Kubyl's stores drop
//! `managedFields`), so "suspended for more than a day" can't be known from the object.

use std::collections::BTreeMap;

use crate::deps::Graph;
use crate::kinds::FluxKind;
use crate::model::{FluxObject, State};

/// Counts of one kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KindCount {
    pub total: usize,
    pub ready: usize,
    pub failed: usize,
    pub suspended: usize,
    pub reconciling: usize,
    /// Static objects that count as ready (no status): not part of the ready totals.
    pub statics: usize,
}

/// Why an object is listed under "Needs attention".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attention {
    Failed,
    Stalled,
    /// Suspended (`spec.suspend`): Flux skips it until it's resumed.
    Suspended,
    /// A source without an artifact (never fetched) or whose last fetch failed.
    NotFetched,
    /// Waits for a dependency that isn't ready (`namespace/name`).
    Waiting(String),
}

impl Attention {
    pub fn label(&self) -> String {
        match self {
            Attention::Failed => "Failed".into(),
            Attention::Stalled => "Stalled".into(),
            Attention::Suspended => "Suspended".into(),
            Attention::NotFetched => "Not fetched".into(),
            Attention::Waiting(key) => format!("Waiting for {key}"),
        }
    }

    /// Failures sort before waits and suspends.
    fn rank(&self) -> u8 {
        match self {
            Attention::Stalled => 0,
            Attention::Failed => 1,
            Attention::NotFetched => 2,
            Attention::Waiting(_) => 3,
            Attention::Suspended => 4,
        }
    }
}

/// An object under "Needs attention".
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub object: FluxObject,
    pub why: Attention,
    pub message: String,
}

/// The overall health.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// Nothing loaded yet.
    Unknown,
    Healthy,
    /// Only waits and suspends.
    Warning,
    Failing,
}

/// Everything the overview shows, computed from the loaded objects.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub counts: BTreeMap<FluxKind, KindCount>,
    pub problems: Vec<Problem>,
}

impl Summary {
    pub fn new(objects: &[FluxObject], now: jiff::Timestamp) -> Summary {
        let mut counts: BTreeMap<FluxKind, KindCount> = BTreeMap::new();
        let mut problems = Vec::new();
        let graphs: BTreeMap<FluxKind, Graph> = [FluxKind::Kustomization, FluxKind::HelmRelease]
            .into_iter()
            .map(|kind| (kind, Graph::new(objects.iter().filter(|o| o.kind == kind))))
            .collect();
        for object in objects {
            let state = object.state();
            let count = counts.entry(object.kind).or_default();
            count.total += 1;
            // Counted as ready while they are (a suspended Alert counts as suspended).
            if object.is_static() && state == State::Ready {
                count.statics += 1;
            }
            match state {
                State::Ready => count.ready += 1,
                State::Failed | State::Stalled => count.failed += 1,
                State::Suspended => count.suspended += 1,
                State::Reconciling => count.reconciling += 1,
                State::Unknown => {}
            }
            let waiting = graphs.get(&object.kind).and_then(|g| g.waiting(object));
            let why = match state {
                State::Stalled => Some(Attention::Stalled),
                // An object that only waits for a failing dependency points at that one.
                State::Failed => Some(match &waiting {
                    Some(dep) => Attention::Waiting(dep.key.clone()),
                    None if object.kind.is_source() => Attention::NotFetched,
                    None => Attention::Failed,
                }),
                State::Suspended => Some(Attention::Suspended),
                State::Unknown | State::Reconciling
                    if object.kind.is_source()
                        && !object.is_static()
                        && object.artifact.is_none()
                        && object.kind != FluxKind::ExternalArtifact
                        && object
                            .created
                            .as_deref()
                            .and_then(|c| c.parse::<jiff::Timestamp>().ok())
                            .is_some_and(|c| now.as_second() - c.as_second() > 300) =>
                {
                    Some(Attention::NotFetched)
                }
                _ => None,
            };
            if let Some(why) = why {
                problems.push(Problem {
                    message: object.message(),
                    object: object.clone(),
                    why,
                });
            }
        }
        problems.sort_by(|a, b| {
            a.why
                .rank()
                .cmp(&b.why.rank())
                .then_with(|| a.object.kind.cmp(&b.object.kind))
                .then_with(|| a.object.key().cmp(&b.object.key()))
        });
        Summary { counts, problems }
    }

    pub fn health(&self) -> Health {
        if self.counts.is_empty() {
            return Health::Unknown;
        }
        if self.problems.iter().any(|p| {
            matches!(
                p.why,
                Attention::Failed | Attention::Stalled | Attention::NotFetched
            )
        }) {
            Health::Failing
        } else if self.problems.is_empty() {
            Health::Healthy
        } else {
            Health::Warning
        }
    }

    /// Ready over total, across every object with a status (static ones don't count).
    pub fn ready_totals(&self) -> (usize, usize) {
        self.counts.values().fold((0, 0), |(ready, total), c| {
            (ready + c.ready - c.statics, total + c.total - c.statics)
        })
    }

    /// `All 23 Flux objects are ready` / `2 failing · 1 suspended`.
    pub fn headline(&self) -> String {
        let (ready, total) = self.ready_totals();
        match self.health() {
            Health::Unknown => "Loading Flux objects…".into(),
            Health::Healthy => format!("All {total} Flux objects are ready"),
            _ => {
                let failing = self
                    .problems
                    .iter()
                    .filter(|p| {
                        matches!(
                            p.why,
                            Attention::Failed | Attention::Stalled | Attention::NotFetched
                        )
                    })
                    .count();
                let waiting = self
                    .problems
                    .iter()
                    .filter(|p| matches!(p.why, Attention::Waiting(_)))
                    .count();
                let suspended = self
                    .problems
                    .iter()
                    .filter(|p| matches!(p.why, Attention::Suspended))
                    .count();
                let mut parts = vec![format!("{ready} of {total} ready")];
                if failing > 0 {
                    parts.push(format!("{failing} failing"));
                }
                if waiting > 0 {
                    parts.push(format!("{waiting} waiting"));
                }
                if suspended > 0 {
                    parts.push(format!("{suspended} suspended"));
                }
                parts.join(" · ")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use serde_json::Value;
    use std::sync::Arc;

    fn parse(value: Value) -> FluxObject {
        FluxObject::parse(&Arc::new(value)).unwrap()
    }

    #[test]
    fn needs_attention() {
        let now: jiff::Timestamp = "2026-10-07T12:00:00Z".parse().unwrap();
        let objects = vec![
            parse(fixtures::kustomization_ready()),
            parse(fixtures::kustomization_failed()),
            parse(fixtures::kustomization_suspended()),
            parse(fixtures::kustomization_waiting()),
            parse(fixtures::helm_release_v2()),
            parse(fixtures::helm_release_stalled()),
            parse(fixtures::git_repository()),
            parse(fixtures::alert()),
        ];
        let summary = Summary::new(&objects, now);
        let ks = summary.counts[&FluxKind::Kustomization];
        assert_eq!(
            ks,
            KindCount {
                total: 4,
                ready: 1,
                failed: 2,
                suspended: 1,
                reconciling: 0,
                statics: 0,
            }
        );
        let why: Vec<(String, Attention)> = summary
            .problems
            .iter()
            .map(|p| (p.object.name.clone(), p.why.clone()))
            .collect();
        assert_eq!(
            why,
            [
                ("redis".to_string(), Attention::Stalled),
                ("broken".to_string(), Attention::Failed),
                (
                    "apps-late".to_string(),
                    Attention::Waiting("flux-demo/broken".into())
                ),
                ("paused".to_string(), Attention::Suspended),
            ]
        );
        assert!(summary.problems[1].message.contains("path not found"));
        assert_eq!(summary.health(), Health::Failing);
        // Alerts have no status and don't count towards the ready total.
        assert_eq!(summary.ready_totals(), (3, 7));
        assert_eq!(
            summary.headline(),
            "3 of 7 ready · 2 failing · 1 waiting · 1 suspended"
        );
        let healthy = Summary::new(&objects[..1], now);
        assert_eq!(healthy.health(), Health::Healthy);
        assert_eq!(healthy.headline(), "All 1 Flux objects are ready");
        assert_eq!(Summary::new(&[], now).health(), Health::Unknown);
    }

    #[test]
    fn sources_that_were_never_fetched() {
        let now: jiff::Timestamp = "2026-10-07T12:00:00Z".parse().unwrap();
        let fresh = serde_json::json!({"apiVersion": "source.toolkit.fluxcd.io/v1", "kind": "GitRepository",
            "metadata": {"name": "new", "namespace": "a", "creationTimestamp": "2026-10-07T11:59:00Z"}});
        let old = serde_json::json!({"apiVersion": "source.toolkit.fluxcd.io/v1", "kind": "Bucket",
            "metadata": {"name": "old", "namespace": "a", "creationTimestamp": "2026-10-07T10:00:00Z"}});
        let failed = serde_json::json!({"apiVersion": "source.toolkit.fluxcd.io/v1", "kind": "GitRepository",
            "metadata": {"name": "auth", "namespace": "a"},
            "status": {"conditions": [{"type": "Ready", "status": "False", "reason": "GitOperationFailed",
                "message": "failed to checkout: authentication required"}]}});
        let summary = Summary::new(&[parse(fresh), parse(old), parse(failed)], now);
        let names: Vec<_> = summary
            .problems
            .iter()
            .map(|p| (p.object.name.as_str(), p.why.clone()))
            .collect();
        assert_eq!(
            names,
            [
                ("auth", Attention::NotFetched),
                ("old", Attention::NotFetched)
            ]
        );
    }

    /// OCI HelmRepositories are static since Flux 2.3: no "Not fetched", no Unknown.
    #[test]
    fn static_sources_and_old_notification_objects() {
        let now: jiff::Timestamp = "2026-10-07T12:00:00Z".parse().unwrap();
        let oci = parse(fixtures::helm_repository_oci());
        assert!(oci.is_static() && !oci.reconcilable());
        let old_alert = parse(fixtures::alert_v1beta2());
        assert!(!old_alert.is_static());
        let summary = Summary::new(&[oci, parse(fixtures::alert()), old_alert], now);
        // The failing v1beta2 Alert is a problem; the static objects aren't.
        let names: Vec<_> = summary
            .problems
            .iter()
            .map(|p| (p.object.name.as_str(), p.why.clone()))
            .collect();
        assert_eq!(names, [("legacy", Attention::Failed)]);
        assert_eq!(summary.counts[&FluxKind::HelmRepository].statics, 1);
        // Statics don't count; the v1beta2 Alert does.
        assert_eq!(summary.ready_totals(), (0, 1));
        // A suspended static object is suspended, not a static ready one (no underflow).
        let mut paused = fixtures::alert();
        paused["spec"]["suspend"] = true.into();
        let summary = Summary::new(&[parse(paused)], now);
        assert_eq!(summary.counts[&FluxKind::Alert].statics, 0);
        assert_eq!(summary.ready_totals(), (0, 1));
        assert_eq!(summary.headline(), "0 of 1 ready · 1 suspended");
    }
}
