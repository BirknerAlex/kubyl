//! What the Security Center watches: for each report kind the cluster serves, a metadata-only
//! watch (liveness, labels) and the API server's table of it (severity counts from the CRD's
//! printer columns), refetched at most every two seconds while the reports change. A report's
//! findings are never part of a list.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, Context, Entity, Subscription, Task};
use kubyl_core::{ClusterId, Gvr};
use kubyl_kube::ConnectionManager;
use kubyl_resources::object_key;
use kubyl_resources::store::find_resource;
use kubyl_resources::table::{ServerTable, fetch_table};
use kubyl_resources::{ResourceStores, StoreHandle, StoreKey, StoreStatus};
use kubyl_security_core::kinds::GROUP;
use kubyl_security_core::{Report, ReportKind};
use serde_json::Value;

const REFRESH: Duration = Duration::from_secs(2);

/// One report kind's watch and table.
pub struct Source {
    pub kind: ReportKind,
    pub gvr: Gvr,
    store: StoreHandle,
    pub table: Option<Arc<ServerTable>>,
    pub error: Option<String>,
    in_flight: bool,
    dirty: bool,
    last: Option<Instant>,
    task: Option<Task<()>>,
}

impl Source {
    pub fn status<'a>(&self, cx: &'a App) -> &'a StoreStatus {
        self.store.read(cx).status()
    }
}

/// The sources of one cluster. Notifies when reports change.
pub struct Feed {
    cluster: ClusterId,
    pub sources: Vec<Source>,
    observers: Vec<Subscription>,
}

impl Feed {
    pub fn new(cluster: ClusterId) -> Self {
        Self {
            cluster,
            sources: Vec::new(),
            observers: Vec::new(),
        }
    }

    /// The served GVR of a kind (discovery's preferred version).
    fn gvr_of(&self, kind: ReportKind, cx: &App) -> Option<Gvr> {
        let discovery = ConnectionManager::try_global(cx)?
            .read(cx)
            .discovery(&self.cluster)?;
        find_resource(&discovery.resources, &Gvr::new(GROUP, "", kind.plural()))
            .map(|info| info.gvr.clone())
    }

    /// Watches the kinds the cluster serves; only changes start or stop watches.
    pub fn sync(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(ReportKind, Gvr)> = ReportKind::ALL
            .into_iter()
            .filter_map(|kind| Some((kind, self.gvr_of(kind, cx)?)))
            .collect();
        self.set_kinds(wanted, cx);
    }

    pub fn set_kinds(&mut self, wanted: Vec<(ReportKind, Gvr)>, cx: &mut Context<Self>) {
        let current: Vec<(ReportKind, Gvr)> = self
            .sources
            .iter()
            .map(|s| (s.kind, s.gvr.clone()))
            .collect();
        if current == wanted {
            return;
        }
        self.sources = wanted
            .into_iter()
            .map(|(kind, gvr)| Source {
                kind,
                store: ResourceStores::acquire(cx, store_key(&self.cluster, &gvr)),
                gvr,
                table: None,
                error: None,
                in_flight: false,
                dirty: false,
                last: None,
                task: None,
            })
            .collect();
        self.observers = self
            .sources
            .iter()
            .enumerate()
            .map(|(ix, s)| {
                cx.observe(s.store.entity(), move |this, _, cx| {
                    this.fetch(ix, cx);
                    cx.notify();
                })
            })
            .collect();
        for ix in 0..self.sources.len() {
            self.fetch(ix, cx);
        }
        cx.notify();
    }

    /// Refetches a source's table, at most once per [`REFRESH`].
    fn fetch(&mut self, ix: usize, cx: &mut Context<Self>) {
        let cluster = self.cluster.clone();
        let Some(source) = self.sources.get_mut(ix) else {
            return;
        };
        if !source.store.read(cx).status().is_ready() {
            return;
        }
        if source.in_flight {
            source.dirty = true;
            return;
        }
        let Some(client) =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(&cluster))
        else {
            return;
        };
        let wait = source
            .last
            .map(|last| REFRESH.saturating_sub(last.elapsed()))
            .unwrap_or_default();
        let gvr = source.gvr.clone();
        source.in_flight = true;
        source.dirty = false;
        let executor = cx.background_executor().clone();
        let table = kubyl_core::spawn_kube(cx, async move {
            fetch_table(client, gvr, None, None, None).await
        });
        source.task = Some(cx.spawn(async move |this, cx| {
            if !wait.is_zero() {
                executor.timer(wait).await;
            }
            let result = table.await;
            this.update(cx, |this, cx| {
                let Some(source) = this.sources.get_mut(ix) else {
                    return;
                };
                source.in_flight = false;
                source.last = Some(Instant::now());
                match result {
                    Ok(table) => {
                        source.table = Some(table);
                        source.error = None;
                    }
                    Err(err) => {
                        source.error = Some(kubyl_resources_core::errors::describe(
                            &err,
                            "list",
                            source.kind.plural(),
                            None,
                        ))
                    }
                }
                if source.dirty {
                    this.fetch(ix, cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Sets the table of a kind's source (tests: nothing fetches).
    #[cfg(test)]
    pub(crate) fn set_table(
        &mut self,
        kind: ReportKind,
        table: ServerTable,
        cx: &mut Context<Self>,
    ) {
        if let Some(source) = self.sources.iter_mut().find(|s| s.kind == kind) {
            source.table = Some(Arc::new(table));
        }
        cx.notify();
    }

    /// Every report of the sources, as rows. Reports whose table hasn't arrived yet have
    /// zero counts until it does ([`Self::loading`]).
    pub fn reports(&self, cx: &App) -> Vec<Report> {
        let mut out = Vec::new();
        for source in &self.sources {
            let table = source.table.as_deref();
            for object in source.store.read(cx).objects().values() {
                let meta = &object.pointer("/metadata");
                let (Some(name), namespace) = (
                    meta.and_then(|m| m.get("name")).and_then(Value::as_str),
                    meta.and_then(|m| m.get("namespace"))
                        .and_then(Value::as_str),
                ) else {
                    continue;
                };
                let cells: HashMap<String, &Value> = table
                    .and_then(|t| {
                        let row = t.rows.get(&object_key(namespace, name))?;
                        Some(
                            t.columns
                                .iter()
                                .zip(row.iter())
                                .map(|(c, v)| (c.name.clone(), v))
                                .collect(),
                        )
                    })
                    .unwrap_or_default();
                if let Some(report) =
                    Report::parse(source.kind, object, |column| cells.get(column).copied())
                {
                    out.push(report);
                }
            }
        }
        out
    }

    /// Some source hasn't loaded its list or its table yet.
    pub fn loading(&self, cx: &App) -> bool {
        self.sources.iter().any(|s| {
            matches!(s.status(cx), StoreStatus::Waiting | StoreStatus::Loading)
                || (s.table.is_none() && s.error.is_none() && s.status(cx).is_ready())
        })
    }

    /// What's wrong, per kind: a refused or failing watch or table names the verb and the
    /// resource.
    pub fn problems(&self, cx: &App) -> Vec<String> {
        self.sources
            .iter()
            .filter_map(|s| match s.status(cx) {
                StoreStatus::Forbidden => Some(format!(
                    "Not allowed to list and watch {}.{GROUP} in all namespaces (RBAC verbs list, watch).",
                    s.kind.plural()
                )),
                StoreStatus::Error(err) => Some(format!(
                    "Watching {} failed: {err}",
                    s.kind.plural()
                )),
                _ => s.error.clone(),
            })
            .collect()
    }
}

/// The metadata watch of a report kind, in all namespaces.
pub fn store_key(cluster: &ClusterId, gvr: &Gvr) -> StoreKey {
    StoreKey::new(cluster.clone(), gvr.clone(), None).metadata()
}

/// The entity type views hold.
pub type FeedEntity = Entity<Feed>;
