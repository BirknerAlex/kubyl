//! The number of applications next to Applications in the sidebar.
//!
//! No watches: once a minute, for a connected cluster, one metadata-only list per kind of the
//! objects that carry `app.kubernetes.io/instance`, grouped with the same function the tab
//! uses. The count is kept in memory only.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use kube::api::{Api, DynamicObject, ListParams};
use kubyl_apps_core::{INSTANCE, Kind, build};
use kubyl_core::{ClusterId, Gvr};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::store::{api_resource, find_resource};

const EVERY: Duration = Duration::from_secs(60);
const RETRY: Duration = Duration::from_secs(2);

struct State {
    count: Option<usize>,
    _task: Task<()>,
}

pub struct AppsService {
    clusters: HashMap<ClusterId, State>,
}

struct GlobalService(Entity<AppsService>);

impl Global for GlobalService {}

impl AppsService {
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

    /// The applications of a cluster, once counted.
    pub fn count(&self, cluster: &ClusterId) -> Option<usize> {
        self.clusters.get(cluster)?.count
    }

    #[cfg(test)]
    pub(crate) fn set_count(&mut self, cluster: &ClusterId, count: usize) {
        self.clusters.insert(
            cluster.clone(),
            State {
                count: Some(count),
                _task: Task::ready(()),
            },
        );
    }

    fn sync(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let connected = manager.read(cx).state(cluster).is_connected();
        let running = self.clusters.contains_key(cluster);
        if connected && !running {
            let id = cluster.clone();
            let task = cx.spawn(async move |this, cx| {
                loop {
                    let Ok(read) = this.update(cx, |this, cx| this.read(&id, cx)) else {
                        break;
                    };
                    // Discovery may not be in yet; look again soon so the count isn't a minute late.
                    let Some(read) = read else {
                        cx.background_executor().timer(RETRY).await;
                        continue;
                    };
                    if let Some(count) = read.await {
                        let alive = this
                            .update(cx, |this, cx| {
                                if let Some(state) = this.clusters.get_mut(&id) {
                                    state.count = Some(count);
                                }
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
                    count: None,
                    _task: task,
                },
            );
        } else if running && !connected {
            self.clusters.remove(cluster);
            kubyl_explorer::catalog::view_rows_changed(cx);
        }
    }

    /// One count on Tokio: the objects with the instance label, grouped like the tab does.
    fn read(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) -> Option<Task<Option<usize>>> {
        let manager = ConnectionManager::try_global(cx)?;
        let (client, discovery) = {
            let manager = manager.read(cx);
            (manager.client(cluster)?, manager.discovery(cluster)?)
        };
        let kinds: Vec<(Kind, kube::discovery::ApiResource)> = Kind::ALL
            .into_iter()
            .filter_map(|kind| {
                find_resource(
                    &discovery.resources,
                    &Gvr::new(kind.group(), "", kind.plural()),
                )
                .map(|info| (kind, api_resource(info)))
            })
            .collect();
        let fetch = kubyl_core::spawn_kube(cx, async move {
            let mut objects: Vec<(Kind, serde_json::Value)> = Vec::new();
            for (kind, resource) in kinds {
                let api: Api<DynamicObject> = Api::all_with(client.clone(), &resource);
                // A kind that can't be listed (RBAC) is left out; the tab says why.
                if let Ok(list) = api
                    .list_metadata(&ListParams::default().labels(INSTANCE))
                    .await
                {
                    objects.extend(
                        list.items
                            .into_iter()
                            .filter_map(|o| serde_json::to_value(o).ok().map(|v| (kind, v))),
                    );
                }
            }
            let refs: Vec<(Kind, &serde_json::Value)> =
                objects.iter().map(|(k, o)| (*k, o)).collect();
            build(&refs, &Default::default()).len()
        });
        Some(cx.spawn(async move |_, _| Some(fetch.await)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn the_sidebar_row_shows_the_count_once_read(cx: &mut gpui::TestAppContext) {
        let cluster = ClusterId::new("kind-apps@/k");
        let service = cx.update(|cx| {
            let service = AppsService::install(cx);
            let service2 = service.clone();
            kubyl_explorer::catalog::register_view_count(
                cx,
                "applications",
                std::sync::Arc::new(move |cluster, cx| service2.read(cx).count(cluster)),
            );
            service
        });
        cx.update(|cx| {
            assert_eq!(
                kubyl_explorer::catalog::view_count("applications", &cluster, cx),
                None
            );
        });
        service.update(cx, |service, _| service.set_count(&cluster, 12));
        cx.update(|cx| {
            assert_eq!(
                kubyl_explorer::catalog::view_count("applications", &cluster, cx),
                Some(12)
            );
            assert_eq!(
                kubyl_explorer::catalog::view_count("other", &cluster, cx),
                None
            );
        });
    }
}
