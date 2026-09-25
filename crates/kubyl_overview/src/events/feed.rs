//! A live events stream for one cluster (all namespaces or one), shared by the dock panel and
//! the Events view.
//!
//! Watches `events.k8s.io/v1` (falling back to core `v1` when the group isn't served or is
//! forbidden) plus the pods of the scope, whose status yields `OOMKilled` warnings right away.
//! Both are shared [`ResourceStores`] watches, so the pods list, the overview and every feed use
//! one watch per scope. Rows are rebuilt at most every [`THROTTLE`] while events pour in.

use std::sync::Arc;
use std::time::Duration;

use gpui::{App, Context, Subscription};
use kubyl_core::{ClusterId, Gvr, ResourceRef};
use kubyl_kube::ConnectionManager;
use kubyl_resources::{ResourceStores, StoreHandle, StoreKey, StoreStatus};
use kubyl_settings::Settings;

use super::model::{self, EventRow};
use crate::settings::OverviewSettings;

/// Rebuild rows at most this often.
const THROTTLE: Duration = Duration::from_millis(200);

/// Where the stream is.
#[derive(Clone, Debug, PartialEq)]
pub enum FeedStatus {
    NoCluster,
    Loading,
    Live,
    Paused { new: usize },
    Forbidden,
    Error(String),
}

pub struct EventsFeed {
    cluster: Option<ClusterId>,
    namespace: Option<String>,
    events: Option<StoreHandle>,
    /// Using core `v1` Events.
    core: bool,
    pods: Option<StoreHandle>,
    fold: bool,
    paused: bool,
    live: Arc<Vec<EventRow>>,
    shown: Arc<Vec<EventRow>>,
    rebuild_pending: bool,
    _observers: Vec<Subscription>,
    _settings: Subscription,
}

impl EventsFeed {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // The derived-events setting decides whether pods are watched: re-acquire on change.
        let weak = cx.weak_entity();
        let settings = Settings::observe::<OverviewSettings>(cx, move |_, cx| {
            weak.update(cx, |this, cx| {
                this.events = None;
                let (cluster, namespace) = (this.cluster.clone(), this.namespace.clone());
                this.set_scope(cluster, namespace, cx);
            })
            .ok();
        });
        Self {
            cluster: None,
            namespace: None,
            events: None,
            core: false,
            pods: None,
            fold: true,
            paused: false,
            live: Arc::default(),
            shown: Arc::default(),
            rebuild_pending: false,
            _observers: Vec::new(),
            _settings: settings,
        }
    }

    pub fn cluster(&self) -> Option<&ClusterId> {
        self.cluster.as_ref()
    }

    /// `None`: all namespaces.
    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    /// Watches `cluster` (in `namespace`, or everywhere).
    pub fn set_scope(
        &mut self,
        cluster: Option<ClusterId>,
        namespace: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if cluster == self.cluster && namespace == self.namespace && self.events.is_some() {
            return;
        }
        self.cluster = cluster;
        self.namespace = namespace;
        self.core = false;
        self.paused = false;
        self.acquire(cx);
    }

    fn acquire(&mut self, cx: &mut Context<Self>) {
        self._observers.clear();
        self.events = None;
        self.pods = None;
        self.live = Arc::default();
        if !self.paused {
            self.shown = Arc::default();
        }
        let Some(cluster) = self.cluster.clone() else {
            cx.notify();
            return;
        };
        let gvr = if self.core {
            Gvr::new("", "v1", "events")
        } else {
            Gvr::new("events.k8s.io", "v1", "events")
        };
        let events = ResourceStores::acquire(
            cx,
            StoreKey::new(cluster.clone(), gvr, self.namespace.clone()),
        );
        self._observers
            .push(cx.observe(events.entity(), |this, _, cx| this.events_changed(cx)));
        self.events = Some(events);
        if Settings::get::<OverviewSettings>(cx).derived_events {
            let pods = ResourceStores::acquire(
                cx,
                StoreKey::new(cluster, Gvr::new("", "v1", "pods"), self.namespace.clone()),
            );
            self._observers
                .push(cx.observe(pods.entity(), |this, _, cx| this.schedule_rebuild(cx)));
            self.pods = Some(pods);
        }
        self.rebuild(cx);
    }

    fn events_changed(&mut self, cx: &mut Context<Self>) {
        let status = self.events.as_ref().map(|e| e.read(cx).status().clone());
        // The events.k8s.io group isn't served or allowed: use core v1 Events.
        if !self.core
            && matches!(
                status,
                Some(StoreStatus::Unsupported | StoreStatus::Forbidden)
            )
        {
            self.core = true;
            self.acquire(cx);
            return;
        }
        self.schedule_rebuild(cx);
    }

    fn schedule_rebuild(&mut self, cx: &mut Context<Self>) {
        if self.rebuild_pending {
            return;
        }
        self.rebuild_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(THROTTLE).await;
            this.update(cx, |this, cx| {
                this.rebuild_pending = false;
                this.rebuild(cx);
            })
            .ok();
        })
        .detach();
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let rows = {
            let events = self.events.as_ref().map(|e| e.read(cx).objects().values());
            let pods = self.pods.as_ref().map(|p| p.read(cx).objects().values());
            model::rows(
                events.into_iter().flatten(),
                pods.into_iter().flatten(),
                self.fold,
            )
        };
        self.live = Arc::new(rows);
        if !self.paused {
            self.shown = self.live.clone();
        }
        cx.notify();
    }

    /// Rows on screen (frozen while paused), newest first.
    pub fn rows(&self) -> Arc<Vec<EventRow>> {
        self.shown.clone()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Freezes the rows on screen; the watch keeps running.
    pub fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        self.paused = paused;
        if !paused {
            self.shown = self.live.clone();
        }
        cx.notify();
    }

    pub fn is_folded(&self) -> bool {
        self.fold
    }

    /// Groups repeated events into one row (`×N`).
    pub fn set_folded(&mut self, fold: bool, cx: &mut Context<Self>) {
        self.fold = fold;
        self.rebuild(cx);
    }

    pub fn status(&self, cx: &App) -> FeedStatus {
        let Some(events) = &self.events else {
            return FeedStatus::NoCluster;
        };
        match events.read(cx).status() {
            StoreStatus::Waiting | StoreStatus::Loading => FeedStatus::Loading,
            StoreStatus::Forbidden => FeedStatus::Forbidden,
            StoreStatus::Error(err) => FeedStatus::Error(err.clone()),
            _ if self.paused => FeedStatus::Paused {
                new: self
                    .live
                    .iter()
                    .filter(|r| self.shown.first().is_none_or(|newest| r.last > newest.last))
                    .count(),
            },
            _ => FeedStatus::Live,
        }
    }
}

/// The resource an event row is about, resolved through discovery.
pub fn object_ref(row: &EventRow, cluster: &ClusterId, cx: &App) -> Option<ResourceRef> {
    let discovery = ConnectionManager::try_global(cx)?
        .read(cx)
        .discovery(cluster)?;
    let group = row
        .api_version
        .rsplit_once('/')
        .map(|(g, _)| g)
        .unwrap_or_default();
    let info = discovery
        .resources
        .iter()
        .filter(|r| r.gvk.kind == row.kind.as_ref() && r.gvk.group == group)
        .max_by_key(|r| r.preferred)?;
    Some(ResourceRef::object(
        cluster.clone(),
        info.gvr.clone(),
        row.namespace
            .as_ref()
            .filter(|_| info.namespaced)
            .map(|n| n.to_string()),
        row.name.to_string(),
    ))
}
