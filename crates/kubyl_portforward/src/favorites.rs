//! Saved port-forwards (`state.json`, `"port_forwards"`), optionally started when their cluster
//! connects.
//!
//! Keyed like the explorer's favorites: context name + API server URL, with the kubeconfig file
//! as a hint, so a saved forward survives kubeconfig reloads and moved files (see
//! `kubyl_explorer::favorites::resolve`).

use std::path::PathBuf;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global};
use kubyl_core::ClusterId;
use kubyl_kube::kubeconfig::ContextInfo;
use kubyl_settings::{State, StateSection};
use serde::{Deserialize, Serialize};

/// One saved forward.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedForward {
    pub context: String,
    pub server: Option<String>,
    pub file: PathBuf,
    pub namespace: String,
    /// `pods`, `services`, `deployments`, `statefulsets`, `daemonsets`.
    pub resource: String,
    pub name: String,
    /// Container port (Pod/workload forwards) or Service port; `None` = the first one.
    pub remote_port: Option<u16>,
    /// `0` = pick a free port.
    pub local_port: u16,
    pub bind_address: String,
    /// Start when the cluster connects.
    pub auto_start: bool,
}

impl Default for SavedForward {
    fn default() -> Self {
        Self {
            context: String::new(),
            server: None,
            file: PathBuf::new(),
            namespace: String::new(),
            resource: String::new(),
            name: String::new(),
            remote_port: None,
            local_port: 0,
            bind_address: "127.0.0.1".into(),
            auto_start: false,
        }
    }
}

impl SavedForward {
    /// `svc/ledger :5432 → :15432`.
    pub fn label(&self) -> String {
        let mut label = format!("{}/{}", short_kind(&self.resource), self.name);
        if let Some(port) = self.remote_port {
            label.push_str(&format!(" :{port}"));
        }
        if self.local_port == 0 {
            label.push_str(" → auto");
        } else {
            label.push_str(&format!(" → :{}", self.local_port));
        }
        label
    }

    /// Same target (context, server, file, namespace, resource, name, remote port).
    pub fn same_target(&self, other: &SavedForward) -> bool {
        self.context == other.context
            && self.server == other.server
            && self.file == other.file
            && self.namespace == other.namespace
            && self.resource == other.resource
            && self.name == other.name
            && self.remote_port == other.remote_port
    }

    /// The context this forward belongs to, among the loaded ones.
    pub fn resolve<'a>(&self, contexts: &'a [ContextInfo]) -> Option<&'a ContextInfo> {
        let favorite = kubyl_explorer::favorites::Favorite {
            context: self.context.clone(),
            server: self.server.clone(),
            file: self.file.clone(),
            namespace: self.namespace.clone(),
            kind: None,
            selector: None,
            alias: None,
        };
        kubyl_explorer::favorites::resolve(&favorite, contexts)
    }
}

/// `svc`, `pod`, `deploy`, `sts`, `ds`.
pub fn short_kind(resource: &str) -> &str {
    match resource {
        "services" => "svc",
        "pods" => "pod",
        "deployments" => "deploy",
        "statefulsets" => "sts",
        "daemonsets" => "ds",
        other => other,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedForwardsState {
    pub forwards: Vec<SavedForward>,
}

impl StateSection for SavedForwardsState {
    const KEY: &'static str = "port_forwards";
}

impl SavedForwardsState {
    /// Adds or replaces a forward with the same target.
    pub fn upsert(&mut self, forward: SavedForward) {
        match self.forwards.iter_mut().find(|f| f.same_target(&forward)) {
            Some(existing) => *existing = forward,
            None => self.forwards.push(forward),
        }
    }

    pub fn remove(&mut self, forward: &SavedForward) {
        self.forwards.retain(|f| !f.same_target(forward));
    }

    pub fn contains(&self, forward: &SavedForward) -> bool {
        self.forwards.iter().any(|f| f.same_target(forward))
    }

    /// Saved forwards to start when `cluster` connects.
    /// The forwards to start when `cluster` connects. `entry` maps a context's id to its
    /// cluster entry (grouped contexts, `ConnectionManager::resolve`).
    pub fn auto_start<'a>(
        &'a self,
        cluster: &'a ClusterId,
        contexts: &'a [ContextInfo],
        entry: impl Fn(&ClusterId) -> ClusterId + 'a,
    ) -> impl Iterator<Item = &'a SavedForward> + 'a {
        self.forwards.iter().filter(move |f| {
            f.auto_start
                && f.resolve(contexts)
                    .is_some_and(|c| &entry(&c.id) == cluster)
        })
    }
}

/// Emitted when the saved list changes.
pub struct SavedForwardsChanged;

/// The app's saved forwards, persisted in state.json.
pub struct SavedForwards {
    state: SavedForwardsState,
}

impl EventEmitter<SavedForwardsChanged> for SavedForwards {}

struct GlobalSaved(Entity<SavedForwards>);

impl Global for GlobalSaved {}

impl SavedForwards {
    pub fn install(cx: &mut App) -> Entity<Self> {
        let state = State::get::<SavedForwardsState>(cx);
        let entity = cx.new(|_| Self { state });
        cx.set_global(GlobalSaved(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalSaved>().0.clone()
    }

    pub fn state(&self) -> &SavedForwardsState {
        &self.state
    }

    pub fn update_state(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut SavedForwardsState),
    ) {
        f(&mut self.state);
        State::set(cx, &self.state);
        cx.emit(SavedForwardsChanged);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forward(name: &str, port: u16) -> SavedForward {
        SavedForward {
            context: "kind-dev".into(),
            server: Some("https://127.0.0.1:6443".into()),
            file: "/tmp/kubeconfig".into(),
            namespace: "payments".into(),
            resource: "services".into(),
            name: name.into(),
            remote_port: Some(port),
            local_port: 15432,
            ..Default::default()
        }
    }

    #[test]
    fn upsert_replaces_the_same_target() {
        let mut state = SavedForwardsState::default();
        state.upsert(forward("ledger", 5432));
        let mut replacement = forward("ledger", 5432);
        replacement.local_port = 25432;
        state.upsert(replacement);
        assert_eq!(state.forwards.len(), 1);
        assert_eq!(state.forwards[0].local_port, 25432);
        state.upsert(forward("ledger", 80));
        assert_eq!(state.forwards.len(), 2);
        state.remove(&forward("ledger", 5432));
        assert_eq!(state.forwards.len(), 1);
        assert!(state.contains(&forward("ledger", 80)));
    }

    #[test]
    fn labels_read_like_the_mockup() {
        assert_eq!(forward("ledger", 5432).label(), "svc/ledger :5432 → :15432");
        let mut auto = forward("web", 80);
        auto.remote_port = None;
        auto.local_port = 0;
        auto.resource = "deployments".into();
        assert_eq!(auto.label(), "deploy/web → auto");
    }

    #[test]
    fn old_entries_without_the_new_fields_still_load() {
        let state: SavedForwardsState = serde_json::from_value(serde_json::json!({
            "forwards": [{"cluster": "dev", "namespace": "ns", "kind": "service", "name": "x",
                          "port": 80, "local_port": 8080, "auto_start": true}]
        }))
        .unwrap();
        assert_eq!(state.forwards[0].name, "x");
        assert_eq!(state.forwards[0].local_port, 8080);
    }
}
