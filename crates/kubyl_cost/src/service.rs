//! Reading OpenCost on demand, like `MetricsService`: a view says it wants a cluster's costs
//! (and for which window); a loop refreshes what's wanted once a minute, and nothing runs for
//! clusters nobody looks at. Costs are kept in memory only.
//!
//! OpenCost is found by its Service (`app.kubernetes.io/name=opencost`, else the Service named
//! `opencost`), or where `cost.clusters.<cluster>.service` says, and read through the API
//! server's service proxy (`kubyl_metrics_core::transport::Transport`): the user's own access, no
//! token goes anywhere else.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use jiff::Timestamp;
use k8s_openapi::api::core::v1::Service;
use kube::api::{Api, ListParams};
use kubyl_core::ClusterId;
use kubyl_cost_core::detect::{self, ServiceInfo};
use kubyl_cost_core::fetch::{self, CostError};
use kubyl_cost_core::settings::{CostSettings, Endpoint};
use kubyl_cost_core::summary::{self, Row, Series, Totals};
use kubyl_cost_core::window::Query;
use kubyl_kube::ConnectionManager;
use kubyl_metrics_core::transport::Transport;
use kubyl_settings::Settings;

/// A cluster's costs are refreshed this often while wanted.
pub const REFRESH: Duration = Duration::from_secs(60);
/// A want lapses after this long without a view asking again.
const WANT: Duration = Duration::from_secs(45);
const TICK: Duration = Duration::from_secs(5);

/// What the view shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Data {
    pub query: Query,
    pub totals: Totals,
    pub rows: Vec<Row>,
    pub series: Series,
    pub fetched: Timestamp,
}

/// Where OpenCost is.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    /// Not looked for yet.
    Unknown,
    Detecting,
    At(Endpoint),
    /// No Service: install it (and Prometheus).
    Missing,
    /// The settings override is wrong, or listing Services was refused.
    Failed(String),
}

pub struct ClusterCost {
    pub found: Found,
    pub data: Option<Data>,
    pub error: Option<CostError>,
    query: Option<Query>,
    wanted: Option<Instant>,
    fetched: Option<Instant>,
    busy: bool,
    task: Option<Task<()>>,
}

impl ClusterCost {
    fn new() -> Self {
        Self {
            found: Found::Unknown,
            data: None,
            error: None,
            query: None,
            wanted: None,
            fetched: None,
            busy: false,
            task: None,
        }
    }

    pub fn loading(&self) -> bool {
        self.busy
    }
}

pub struct CostService {
    clusters: HashMap<ClusterId, ClusterCost>,
    _ticker: Option<Task<()>>,
}

struct GlobalService(Entity<CostService>);

impl Global for GlobalService {}

impl CostService {
    /// Installs the service; `live` off keeps tests free of the refresh loop.
    pub fn install(live: bool, cx: &mut App) -> Entity<Self> {
        let service = cx.new(|cx| {
            let ticker = live.then(|| {
                cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(TICK).await;
                        if this
                            .update(cx, |this: &mut CostService, cx| this.tick(cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
            });
            Self {
                clusters: HashMap::new(),
                _ticker: ticker,
            }
        });
        cx.set_global(GlobalService(service.clone()));
        service
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalService>().map(|g| g.0.clone())
    }

    pub fn cluster(&self, id: &ClusterId) -> Option<&ClusterCost> {
        self.clusters.get(id)
    }

    /// A view wants `cluster`'s costs for `query` (`accumulate` is the service's business):
    /// call again to keep them coming; a changed query refreshes at once.
    pub fn want(&mut self, id: &ClusterId, query: Query, cx: &mut Context<Self>) {
        let state = self
            .clusters
            .entry(id.clone())
            .or_insert_with(ClusterCost::new);
        let changed = state.query != Some(query);
        state.query = Some(query);
        state.wanted = Some(Instant::now());
        if changed {
            state.fetched = None;
        }
        self.tick_cluster(id, cx);
    }

    /// Refreshes now ("Refresh").
    pub fn refresh(&mut self, id: &ClusterId, cx: &mut Context<Self>) {
        if let Some(state) = self.clusters.get_mut(id) {
            state.fetched = None;
            if matches!(state.found, Found::Missing | Found::Failed(_)) {
                state.found = Found::Unknown;
            }
        }
        self.tick_cluster(id, cx);
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<ClusterId> = self.clusters.keys().cloned().collect();
        for id in ids {
            self.tick_cluster(&id, cx);
        }
    }

    fn tick_cluster(&mut self, id: &ClusterId, cx: &mut Context<Self>) {
        let Some(state) = self.clusters.get_mut(id) else {
            return;
        };
        let wanted = state.wanted.is_some_and(|w| w.elapsed() < WANT);
        if !wanted || state.busy {
            return;
        }
        let Some(query) = state.query else {
            return;
        };
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let Some(client) = manager.read(cx).client(id) else {
            return;
        };
        match state.found.clone() {
            Found::Unknown => {
                let keys = manager.read(cx).settings_keys(id);
                let configured = Settings::get::<CostSettings>(cx).endpoint(&keys);
                match configured {
                    Some(Ok(endpoint)) => {
                        state.found = Found::At(endpoint);
                        self.tick_cluster(id, cx);
                    }
                    Some(Err(why)) => state.found = Found::Failed(why),
                    None => {
                        state.found = Found::Detecting;
                        state.busy = true;
                        let id = id.clone();
                        let find = kubyl_core::spawn_kube(cx, detect_service(client));
                        state.task = Some(cx.spawn(async move |this, cx| {
                            let found = find.await;
                            this.update(cx, |this, cx| {
                                if let Some(state) = this.clusters.get_mut(&id) {
                                    state.busy = false;
                                    state.found = match found {
                                        Ok(Some(endpoint)) => Found::At(endpoint),
                                        Ok(None) => Found::Missing,
                                        Err(why) => Found::Failed(why),
                                    };
                                }
                                this.tick_cluster(&id, cx);
                                cx.notify();
                            })
                            .ok();
                        }));
                    }
                }
                cx.notify();
            }
            Found::At(endpoint) => {
                if state.fetched.is_some_and(|at| at.elapsed() < REFRESH) {
                    return;
                }
                state.busy = true;
                let id = id.clone();
                let asked = query;
                let read = kubyl_core::spawn_kube(cx, read_costs(client, endpoint, query));
                state.task = Some(cx.spawn(async move |this, cx| {
                    let result = read.await;
                    this.update(cx, |this, cx| {
                        if let Some(state) = this.clusters.get_mut(&id) {
                            state.busy = false;
                            state.fetched = Some(Instant::now());
                            // The window changed while this read ran: read the new one at once.
                            let stale = state.query != Some(asked);
                            if stale {
                                state.fetched = None;
                            }
                            match result {
                                Ok(data) => {
                                    state.data = Some(data);
                                    state.error = None;
                                }
                                // Keep the last good numbers on screen, with the error above.
                                Err(error) => state.error = Some(error),
                            }
                            if stale {
                                this.tick_cluster(&id, cx);
                            }
                        }
                        cx.notify();
                    })
                    .ok();
                }));
                cx.notify();
            }
            Found::Detecting | Found::Missing | Found::Failed(_) => {}
        }
    }

    /// Sets what a cluster's read found (tests: nothing reads).
    #[cfg(test)]
    pub(crate) fn set(
        &mut self,
        id: &ClusterId,
        found: Found,
        data: Option<Data>,
        error: Option<CostError>,
        cx: &mut Context<Self>,
    ) {
        let state = self
            .clusters
            .entry(id.clone())
            .or_insert_with(ClusterCost::new);
        state.found = found;
        state.data = data;
        state.error = error;
        cx.notify();
    }
}

/// OpenCost's Service in the cluster: by label, else by name.
async fn detect_service(client: kube::Client) -> Result<Option<Endpoint>, String> {
    let api: Api<Service> = Api::all(client);
    let mut found: Vec<ServiceInfo> = Vec::new();
    for params in [
        ListParams::default().labels("app.kubernetes.io/name=opencost"),
        ListParams::default().fields("metadata.name=opencost"),
    ] {
        let list = api.list(&params).await.map_err(|err| {
            kubyl_resources_core::errors::describe(&err, "list", "services", None)
        })?;
        for service in list.items {
            let meta = service.metadata;
            let labels = meta.labels.unwrap_or_default();
            found.push(ServiceInfo {
                namespace: meta.namespace.unwrap_or_default(),
                name: meta.name.unwrap_or_default(),
                app_labels: ["app.kubernetes.io/name", "app"]
                    .iter()
                    .filter_map(|k| labels.get(*k).cloned())
                    .collect(),
                ports: service
                    .spec
                    .and_then(|s| s.ports)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| (p.name.unwrap_or_default(), p.port as u16))
                    .collect(),
            });
        }
        if detect::find(&found).is_some() {
            break;
        }
    }
    Ok(detect::find(&found))
}

/// The two reads of a refresh: the window's totals per namespace, and the steps for the chart.
async fn read_costs(
    client: kube::Client,
    endpoint: Endpoint,
    query: Query,
) -> Result<Data, CostError> {
    let transport = Transport::service_proxy(
        client,
        &endpoint.namespace,
        &endpoint.service,
        &endpoint.port,
        "http",
        "",
    );
    let totals_query = Query {
        accumulate: true,
        ..query
    };
    let steps_query = Query {
        accumulate: false,
        ..query
    };
    let accumulated = fetch::allocations(&transport, &endpoint, &totals_query).await?;
    let steps = fetch::allocations(&transport, &endpoint, &steps_query).await?;
    Ok(build(query, &accumulated, &steps))
}

/// The view's data from the two responses.
pub fn build(
    query: Query,
    accumulated: &[Vec<kubyl_cost_core::model::Allocation>],
    steps: &[Vec<kubyl_cost_core::model::Allocation>],
) -> Data {
    let set = accumulated.first().map(Vec::as_slice).unwrap_or(&[]);
    Data {
        query,
        totals: summary::totals(set),
        rows: summary::rows(set),
        series: summary::series(steps, query.window.step_seconds(), 6),
        fetched: Timestamp::now(),
    }
}
