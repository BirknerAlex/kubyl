//! The sidebar badge: the number of critical findings, while a cluster runs Trivy Operator.
//!
//! No watches: once a minute, for a connected cluster that serves report kinds, one table
//! request per kind (the same printer-column tables the tab reads), summed by
//! `kubyl_security_core::aggregate::critical_total`. Nothing runs for clusters without Trivy,
//! and the counts are kept in memory only.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use kubyl_core::{ClusterId, Gvr};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::store::find_resource;
use kubyl_resources::table::{ServerTable, fetch_table};
use kubyl_security_core::kinds::GROUP;
use kubyl_security_core::{Report, ReportKind, aggregate};
use serde_json::Value;

const EVERY: Duration = Duration::from_secs(60);
const RETRY: Duration = Duration::from_secs(2);

struct State {
    /// `None` until the first read.
    reports: Option<Arc<Vec<Report>>>,
    _task: Task<()>,
}

pub struct SecurityService {
    clusters: HashMap<ClusterId, State>,
}

struct GlobalService(Entity<SecurityService>);

impl Global for GlobalService {}

/// The reports of a kind from its table: names, namespaces and the printer columns. A table
/// has no labels, so reports of different workloads aren't told apart by subject, which the
/// badge doesn't need (images are deduplicated by their name).
pub fn reports_from_table(kind: ReportKind, table: &ServerTable) -> Vec<Report> {
    table
        .rows
        .iter()
        .filter_map(|(key, cells)| {
            let (namespace, name) = match key.split_once('/') {
                Some((ns, name)) => (Some(ns), name),
                None => (None, &key[..]),
            };
            let mut metadata = serde_json::json!({ "name": name });
            if let Some(ns) = namespace {
                metadata["namespace"] = Value::String(ns.to_string());
            }
            let meta = serde_json::json!({ "metadata": metadata });
            Report::parse(kind, &meta, |column| {
                table
                    .columns
                    .iter()
                    .position(|c| c.name == column)
                    .and_then(|ix| cells.get(ix))
            })
        })
        .collect()
}

impl SecurityService {
    pub fn install(cx: &mut App) -> Entity<Self> {
        let service = cx.new(|_| Self {
            clusters: HashMap::new(),
        });
        cx.set_global(GlobalService(service.clone()));
        if let Some(manager) = ConnectionManager::try_global(cx) {
            let weak = service.downgrade();
            cx.subscribe(&manager, move |_, event: &ConnectionEvent, cx| {
                if let ConnectionEvent::StateChanged(id) | ConnectionEvent::DiscoveryChanged(id) =
                    event
                {
                    let id = id.clone();
                    weak.update(cx, |this, cx| this.sync(&id, cx)).ok();
                }
            })
            .detach();
        }
        service
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalService>().map(|g| g.0.clone())
    }

    /// Sets what a cluster's read found (tests: nothing reads).
    #[cfg(test)]
    pub(crate) fn set_reports(
        &mut self,
        cluster: &ClusterId,
        reports: Vec<Report>,
        cx: &mut Context<Self>,
    ) {
        self.clusters.insert(
            cluster.clone(),
            State {
                reports: Some(Arc::new(reports)),
                _task: Task::ready(()),
            },
        );
        cx.notify();
    }

    /// The findings of a cluster by view, once read (of one namespace when given).
    pub fn summary(
        &self,
        cluster: &ClusterId,
        namespace: Option<&str>,
    ) -> Option<aggregate::Summary> {
        let reports = self.clusters.get(cluster)?.reports.as_ref()?;
        Some(aggregate::summary(reports, namespace))
    }

    /// The critical findings of a cluster across the views (the sidebar badge).
    pub fn criticals(&self, cluster: &ClusterId) -> Option<u32> {
        self.summary(cluster, None).map(|s| s.critical())
    }

    /// Starts or stops the reads of a cluster to match what it serves.
    fn sync(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let (connected, serves) = {
            let manager = manager.read(cx);
            (
                manager.state(cluster).is_connected(),
                manager.caps(cluster).trivy.any(),
            )
        };
        let running = self.clusters.contains_key(cluster);
        if connected && serves && !running {
            let id = cluster.clone();
            let task = cx.spawn(async move |this, cx| {
                loop {
                    let Ok(reports) = this.update(cx, |this, cx| this.read(&id, cx)) else {
                        break;
                    };
                    // Discovery may not be in yet; look again soon.
                    let Some(reports) = reports else {
                        cx.background_executor().timer(RETRY).await;
                        continue;
                    };
                    if let Some(reports) = reports.await {
                        let reports = Arc::new(reports);
                        let alive = this
                            .update(cx, |this, cx| {
                                if let Some(state) = this.clusters.get_mut(&id) {
                                    state.reports = Some(reports);
                                }
                                cx.notify();
                                kubyl_explorer::catalog::view_rows_changed(cx);
                            })
                            .is_ok();
                        if !alive {
                            break;
                        }
                    }
                    cx.background_executor().timer(EVERY).await;
                }
            });
            self.clusters.insert(
                cluster.clone(),
                State {
                    reports: None,
                    _task: task,
                },
            );
        } else if running && !(connected && serves) {
            self.clusters.remove(cluster);
            kubyl_explorer::catalog::view_rows_changed(cx);
        }
    }

    /// One read of every served kind on Tokio.
    fn read(
        &mut self,
        cluster: &ClusterId,
        cx: &mut Context<Self>,
    ) -> Option<Task<Option<Vec<Report>>>> {
        let manager = ConnectionManager::try_global(cx)?;
        let (client, discovery) = {
            let manager = manager.read(cx);
            (manager.client(cluster)?, manager.discovery(cluster)?)
        };
        let kinds: Vec<(ReportKind, Gvr)> = ReportKind::ALL
            .into_iter()
            .filter_map(|kind| {
                find_resource(&discovery.resources, &Gvr::new(GROUP, "", kind.plural()))
                    .map(|info| (kind, info.gvr.clone()))
            })
            .collect();
        let previous = self
            .clusters
            .get(cluster)
            .and_then(|state| state.reports.clone());
        let fetch = kubyl_core::spawn_kube(cx, async move {
            let mut read = Vec::new();
            for (kind, gvr) in kinds {
                // A kind that can't be read (RBAC, a blip) isn't a clean scan: see `merge`.
                let reports = fetch_table(client.clone(), gvr, None, None, None)
                    .await
                    .ok()
                    .map(|table| reports_from_table(kind, &table));
                read.push((kind, reports));
            }
            merge(
                previous.as_deref().map(|v| &v[..]).unwrap_or_default(),
                read,
            )
        });
        Some(cx.spawn(async move |_, _| fetch.await))
    }
}

/// What a read of every kind makes of the cluster's reports: a kind that failed keeps what the
/// last read had of it (a blip must not lower the numbers), and a read where none worked is
/// `None`, so the state stays unread or as it was rather than showing zeros for a clean scan.
fn merge(previous: &[Report], read: Vec<(ReportKind, Option<Vec<Report>>)>) -> Option<Vec<Report>> {
    if read.iter().all(|(_, reports)| reports.is_none()) {
        return None;
    }
    let mut merged = Vec::new();
    for (kind, reports) in read {
        match reports {
            Some(reports) => merged.extend(reports),
            None => merged.extend(previous.iter().filter(|r| r.kind == kind).cloned()),
        }
    }
    Some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_resources::table::parse_table;
    use serde_json::json;

    fn report(kind: ReportKind, name: &str, critical: u32) -> Report {
        let mut report = Report::parse(
            kind,
            &json!({"metadata": {"name": name, "namespace": "shop"}}),
            |_| None,
        )
        .unwrap();
        report.counts.critical = critical;
        report
    }

    #[test]
    fn a_failed_kind_keeps_its_last_reports_and_all_failing_reads_nothing() {
        let before = vec![
            report(ReportKind::Vulnerability, "a", 5),
            report(ReportKind::ConfigAudit, "b", 2),
        ];
        // Config failed this time: its reports stay, the vulnerability ones are replaced.
        let merged = merge(
            &before,
            vec![
                (
                    ReportKind::Vulnerability,
                    Some(vec![report(ReportKind::Vulnerability, "a", 3)]),
                ),
                (ReportKind::ConfigAudit, None),
            ],
        )
        .unwrap();
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].counts.critical, 3);
        assert_eq!(merged[1].name, "b");
        // Nothing readable (every list forbidden) is not "0 critical".
        assert!(
            merge(
                &before,
                vec![
                    (ReportKind::Vulnerability, None),
                    (ReportKind::ConfigAudit, None)
                ]
            )
            .is_none()
        );
    }

    #[test]
    fn tables_become_reports() {
        let table = parse_table(&json!({
            "columnDefinitions": [
                {"name": "Name", "type": "string", "format": "name"}, {"name": "Repository", "type": "string"},
                {"name": "Tag", "type": "string"}, {"name": "Scanner", "type": "string"},
                {"name": "Age", "type": "date"}, {"name": "Critical", "type": "integer"}, {"name": "High", "type": "integer"}],
            "rows": [
                {"cells": ["a", "library/nginx", "1.19", "Trivy", "5m", 42, 143],
                 "object": {"metadata": {"name": "a", "namespace": "shop"}}},
                {"cells": ["b", "library/nginx", "1.19", "Trivy", "5m", 40, 150],
                 "object": {"metadata": {"name": "b", "namespace": "edge"}}}]
        }));
        let reports = reports_from_table(ReportKind::Vulnerability, &table);
        assert_eq!(reports.len(), 2);
        assert!(
            reports
                .iter()
                .all(|r| r.image.as_deref() == Some("library/nginx:1.19"))
        );
        // The same image in two workloads counts once, at its larger count.
        assert_eq!(aggregate::critical_total(&reports), 42);
    }
}
