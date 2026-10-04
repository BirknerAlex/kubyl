//! Schemas from the cluster's OpenAPI v3 documents (the model is in `kubyl_yaml_core`), and
//! [`Schemas`], the per-cluster cache all editors share.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use kubyl_core::ClusterId;
use kubyl_kube::openapi::OpenApiIndex;
use kubyl_kube::{ConnectionEvent, ConnectionManager};
pub use kubyl_yaml_core::schema::*;
use serde_json::Value;

// ----- Cache -----

enum Entry {
    Loading(#[allow(dead_code)] Task<()>),
    Ready(Arc<Value>),
    Failed(String),
}

/// OpenAPI documents per (cluster, group-version), loaded on demand and shared by all editors.
/// Observe the entity to hear when one finishes loading.
pub struct Schemas {
    docs: HashMap<(ClusterId, String), Entry>,
}

struct GlobalSchemas(Entity<Schemas>);

impl Global for GlobalSchemas {}

impl Schemas {
    pub(crate) fn install(cx: &mut App) {
        let entity = cx.new(|cx| {
            if let Some(manager) = ConnectionManager::try_global(cx) {
                cx.subscribe(&manager, |this: &mut Schemas, _, event, cx| {
                    if let ConnectionEvent::DiscoveryChanged(cluster) = event {
                        // CRDs may have been added, changed or removed.
                        this.docs.retain(|(c, _), _| c != cluster);
                        cx.notify();
                    }
                })
                .detach();
            }
            Schemas {
                docs: HashMap::new(),
            }
        });
        cx.set_global(GlobalSchemas(entity));
    }

    pub fn global(cx: &App) -> Option<Entity<Schemas>> {
        cx.try_global::<GlobalSchemas>().map(|g| g.0.clone())
    }

    /// The document for `group/version`, or `None` while it loads (observers are notified).
    pub fn get(
        &mut self,
        cluster: &ClusterId,
        group: &str,
        version: &str,
        cx: &mut Context<Self>,
    ) -> Result<Option<Arc<Value>>, String> {
        let key = (cluster.clone(), OpenApiIndex::key(group, version));
        match self.docs.get(&key) {
            Some(Entry::Ready(doc)) => return Ok(Some(doc.clone())),
            Some(Entry::Failed(err)) => return Err(err.clone()),
            Some(Entry::Loading(_)) => return Ok(None),
            None => {}
        }
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return Err("not connected".into());
        };
        if manager.read(cx).client(cluster).is_none() {
            return Ok(None);
        }
        let load = manager.update(cx, |m, cx| m.openapi_spec(cluster, key.1.clone(), cx));
        let task_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                let entry = match result {
                    Ok(doc) => Entry::Ready(Arc::new(doc)),
                    Err(err) => {
                        tracing::warn!(path = %task_key.1, "OpenAPI schema: {err}");
                        Entry::Failed(err)
                    }
                };
                this.docs.insert(task_key, entry);
                cx.notify();
            })
            .ok();
        });
        self.docs.insert(key, Entry::Loading(task));
        Ok(None)
    }

    /// Forgets a failed or stale document so the next [`Self::get`] fetches it again.
    pub fn invalidate(&mut self, cluster: &ClusterId, group: &str, version: &str) {
        self.docs
            .remove(&(cluster.clone(), OpenApiIndex::key(group, version)));
    }
}
