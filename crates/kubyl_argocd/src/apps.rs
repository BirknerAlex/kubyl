//! Applications as the views show them: rows built from the shared watch caches, filters
//! (text, sync, health, project, destination) and sorting. No UI here.

use std::collections::BTreeSet;
use std::sync::Arc;

use gpui::App;
use kubyl_core::{ClusterId, Gvr};
use kubyl_kube::ConnectionManager;
use kubyl_resources::{ResourceStores, StoreKey, object_key};
use serde_json::Value;

use crate::links::{self, KnownCluster};
use crate::model::{Application, Health, OperationPhase, SyncStatus, short_revision};
use crate::ops::AppTarget;

/// The shared watch of every Application of a cluster.
pub fn all_key(cluster: &ClusterId, gvr: &Gvr) -> StoreKey {
    StoreKey::new(cluster.clone(), gvr.clone(), None)
}

/// The watch of the Applications in one namespace (when listing all is forbidden).
pub fn namespace_key(cluster: &ClusterId, gvr: &Gvr, namespace: &str) -> StoreKey {
    StoreKey::new(cluster.clone(), gvr.clone(), Some(namespace.to_string()))
}

/// A loaded app (from any running watch of its cluster).
pub fn find_object(
    cluster: &ClusterId,
    gvr: &Gvr,
    app: &AppTarget,
    cx: &App,
) -> Option<Arc<Value>> {
    let key = object_key(Some(&app.namespace), &app.name);
    [
        all_key(cluster, gvr),
        namespace_key(cluster, gvr, &app.namespace),
        namespace_key(cluster, gvr, &app.namespace).fields(format!("metadata.name={}", app.name)),
    ]
    .iter()
    .filter_map(|k| ResourceStores::peek(cx, k))
    .find_map(|store| store.read(cx).get(&key).cloned())
}

pub fn find_app(cluster: &ClusterId, gvr: &Gvr, app: &AppTarget, cx: &App) -> Option<Application> {
    find_object(cluster, gvr, app, cx).and_then(|o| Application::parse(&o))
}

/// Kubyl contexts with their API servers, for destination links.
pub fn known_clusters(cx: &App) -> Vec<KnownCluster> {
    let Some(manager) = ConnectionManager::try_global(cx) else {
        return Vec::new();
    };
    manager
        .read(cx)
        .contexts()
        .filter_map(|c| {
            Some(KnownCluster {
                id: c.id.clone(),
                name: c.context.clone(),
                server: c.server.clone()?,
            })
        })
        .collect()
}

/// An Application as a row.
#[derive(Clone, Debug)]
pub struct AppRow {
    /// `namespace/name`.
    pub key: Arc<str>,
    pub object: Arc<Value>,
    pub app: Arc<Application>,
    /// `apps/checkout-api @ main`, `2 sources`.
    pub source: String,
    /// The repository (first source).
    pub repo: String,
    /// Synced revision(s), short.
    pub revision: String,
    /// `in-cluster · payments`.
    pub destination: String,
    /// The Kubyl context the destination is, when known.
    pub destination_cluster: Option<ClusterId>,
}

impl AppRow {
    pub fn new(object: Arc<Value>, own: &ClusterId, known: &[KnownCluster]) -> Option<Self> {
        let app = Application::parse(&object)?;
        let sources = app.spec.all_sources();
        let source = match sources.as_slice() {
            [] => String::new(),
            [one] => format!("{} @ {}", one.what(), one.target()),
            many => format!("{} sources", many.len()),
        };
        let repo = sources.first().map(|s| s.repo_short()).unwrap_or_default();
        let revision = app
            .synced_revisions()
            .iter()
            .map(|r| short_revision(r))
            .collect::<Vec<_>>()
            .join(", ");
        let destination = app.spec.destination.label();
        let destination_cluster = links::destination_cluster(&app.spec.destination, own, known);
        Some(Self {
            key: object_key(Some(app.namespace()), app.name()),
            object,
            app: Arc::new(app),
            source,
            repo,
            revision,
            destination,
            destination_cluster,
        })
    }

    pub fn name(&self) -> &str {
        self.app.name()
    }

    pub fn namespace(&self) -> &str {
        self.app.namespace()
    }

    pub fn project(&self) -> &str {
        &self.app.spec.project
    }

    pub fn target(&self) -> AppTarget {
        AppTarget::new(self.namespace(), self.name())
    }
}

/// What the Applications view filters by.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    pub text: String,
    /// Empty: any.
    pub sync: BTreeSet<SyncStatus>,
    pub health: BTreeSet<Health>,
    pub project: Option<String>,
    pub destination: Option<String>,
}

impl Filters {
    /// Everything but the status chips (their counts are over these rows).
    pub fn matches_scope(&self, row: &AppRow) -> bool {
        let text = self.text.trim().to_lowercase();
        let text_ok = text.is_empty()
            || [
                row.name(),
                row.namespace(),
                row.project(),
                &row.source,
                &row.repo,
                &row.destination,
            ]
            .iter()
            .any(|field| field.to_lowercase().contains(&text));
        text_ok
            && self.project.as_deref().is_none_or(|p| row.project() == p)
            && self
                .destination
                .as_deref()
                .is_none_or(|d| row.destination == d)
    }

    pub fn matches(&self, row: &AppRow) -> bool {
        self.matches_scope(row)
            && (self.sync.is_empty() || self.sync.contains(&row.app.sync()))
            && (self.health.is_empty() || self.health.contains(&row.app.health()))
    }

    pub fn is_empty(&self) -> bool {
        self == &Filters::default()
    }
}

/// Counts for the chips.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub sync: Vec<(SyncStatus, usize)>,
    pub health: Vec<(Health, usize)>,
}

pub fn counts<'a>(rows: impl IntoIterator<Item = &'a AppRow>) -> Counts {
    let mut sync = [0usize; 3];
    let mut health = [0usize; 6];
    for row in rows {
        let s = SyncStatus::ALL
            .iter()
            .position(|x| *x == row.app.sync())
            .unwrap_or(2);
        let h = Health::ALL
            .iter()
            .position(|x| *x == row.app.health())
            .unwrap_or(5);
        sync[s] += 1;
        health[h] += 1;
    }
    Counts {
        sync: SyncStatus::ALL.iter().copied().zip(sync).collect(),
        health: Health::ALL.iter().copied().zip(health).collect(),
    }
}

/// Columns the view sorts by.
pub fn sort_rows(rows: &mut [AppRow], column: &str, ascending: bool) {
    let last = |row: &AppRow| {
        row.app
            .last_result()
            .and_then(|(_, at)| at)
            .map(|t| t.as_second())
            .unwrap_or(0)
    };
    rows.sort_by(|a, b| {
        let order = match column {
            "project" => a.project().cmp(b.project()),
            "sync" => a.app.sync().cmp(&b.app.sync()),
            "health" => b.app.health().severity().cmp(&a.app.health().severity()),
            "source" => a.source.cmp(&b.source),
            "revision" => a.revision.cmp(&b.revision),
            "destination" => a.destination.cmp(&b.destination),
            // Newest first when ascending.
            "last" => last(b).cmp(&last(a)),
            _ => std::cmp::Ordering::Equal,
        };
        let order = order
            .then_with(|| a.name().cmp(b.name()))
            .then_with(|| a.namespace().cmp(b.namespace()));
        if ascending { order } else { order.reverse() }
    });
}

/// Projects and destinations for the dropdowns.
pub fn choices(rows: &[AppRow]) -> (Vec<String>, Vec<String>) {
    let projects: BTreeSet<String> = rows.iter().map(|r| r.project().to_string()).collect();
    let destinations: BTreeSet<String> = rows.iter().map(|r| r.destination.clone()).collect();
    (
        projects.into_iter().collect(),
        destinations.into_iter().collect(),
    )
}

/// `2 out of sync`, `1 degraded` for the toolbar.
pub fn problems(rows: &[AppRow]) -> (usize, usize) {
    let out_of_sync = rows
        .iter()
        .filter(|r| r.app.sync() == SyncStatus::OutOfSync)
        .count();
    let degraded = rows
        .iter()
        .filter(|r| r.app.health() == Health::Degraded)
        .count();
    (out_of_sync, degraded)
}

/// Whether the last operation failed.
pub fn last_failed(app: &Application) -> bool {
    matches!(
        app.last_result(),
        Some((OperationPhase::Failed | OperationPhase::Error, _))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::guestbook;
    use serde_json::json;

    fn row(name: &str, project: &str, sync: &str, health: &str) -> AppRow {
        let mut object = guestbook();
        object["metadata"]["name"] = json!(name);
        object["spec"]["project"] = json!(project);
        object["status"]["sync"]["status"] = json!(sync);
        object["status"]["health"]["status"] = json!(health);
        AppRow::new(Arc::new(object), &ClusterId::new("c"), &[]).unwrap()
    }

    #[test]
    fn rows_and_filters() {
        let rows = vec![
            row("checkout", "payments", "Synced", "Healthy"),
            row("gateway", "payments", "OutOfSync", "Degraded"),
            row("ingress", "platform", "Synced", "Progressing"),
        ];
        assert_eq!(rows[0].source, "guestbook @ HEAD");
        assert_eq!(rows[0].revision, "8088f4c");
        assert_eq!(rows[0].destination, "in-cluster · guestbook");
        assert_eq!(rows[0].destination_cluster, Some(ClusterId::new("c")));
        let mut filters = Filters::default();
        assert!(rows.iter().all(|r| filters.matches(r)));
        filters.sync.insert(SyncStatus::OutOfSync);
        let names: Vec<&str> = rows
            .iter()
            .filter(|r| filters.matches(r))
            .map(|r| r.name())
            .collect();
        assert_eq!(names, ["gateway"]);
        filters.sync.clear();
        filters.project = Some("payments".into());
        filters.health.insert(Health::Healthy);
        filters.health.insert(Health::Degraded);
        assert_eq!(rows.iter().filter(|r| filters.matches(r)).count(), 2);
        filters = Filters {
            text: "PLATFORM".into(),
            ..Default::default()
        };
        assert_eq!(rows.iter().filter(|r| filters.matches(r)).count(), 1);
        let counts = counts(&rows);
        assert_eq!(counts.sync[0], (SyncStatus::Synced, 2));
        assert_eq!(counts.health[2], (Health::Degraded, 1));
        assert_eq!(problems(&rows), (1, 1));
        let (projects, destinations) = choices(&rows);
        assert_eq!(projects, ["payments", "platform"]);
        assert_eq!(destinations, ["in-cluster · guestbook"]);
    }

    #[test]
    fn sorting() {
        let mut rows = vec![
            row("b", "x", "Synced", "Healthy"),
            row("a", "y", "OutOfSync", "Degraded"),
            row("c", "x", "Unknown", "Progressing"),
        ];
        sort_rows(&mut rows, "name", true);
        assert_eq!(
            rows.iter().map(|r| r.name()).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        sort_rows(&mut rows, "health", true);
        assert_eq!(rows[0].name(), "a");
        sort_rows(&mut rows, "project", false);
        assert_eq!(rows[0].project(), "y");
    }
}
