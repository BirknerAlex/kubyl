//! The `"webview"` section of settings.json and what web views remember in state.json: per
//! service port the scheme, start path, last page, local port and zoom, and the certificates
//! the user accepted. Nothing here is secret (see [`crate::target::remembered_path`]).

use gpui::App;
use kubyl_kube::ConnectionManager;
use kubyl_settings::{Settings, SettingsSection, State, StateSection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::target::{Scheme, TargetKind, WebTarget};

/// Where web views open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenIn {
    /// Inside Kubyl: embedded in the tab (macOS, Windows, Linux on X11), or in a window of its
    /// own on Wayland.
    #[default]
    Auto,
    /// The system browser; the tab shows the session and keeps the forward alive.
    Browser,
}

/// Settings of service web views.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct WebViewSettings {
    pub open_in: OpenIn,
    /// Stop the forward of a web view that stayed in the background this many minutes (it
    /// restarts when the tab shows again). `0` keeps forwards running.
    pub idle_stop_minutes: u32,
    /// Allow the web inspector (always allowed in debug builds).
    pub devtools: bool,
    /// New web views start private: nothing they store is kept.
    pub private_by_default: bool,
}

impl Default for WebViewSettings {
    fn default() -> Self {
        Self {
            open_in: OpenIn::Auto,
            idle_stop_minutes: 30,
            devtools: false,
            private_by_default: false,
        }
    }
}

impl SettingsSection for WebViewSettings {
    const KEY: Option<&'static str> = Some("webview");
}

impl WebViewSettings {
    pub fn get(cx: &App) -> &Self {
        Settings::get::<Self>(cx)
    }

    pub fn devtools(cx: &App) -> bool {
        cfg!(debug_assertions) || Self::get(cx).devtools
    }
}

/// A service port across restarts: the context (name + API server, like saved forwards and
/// favorites, since cluster ids contain the kubeconfig path), namespace, object and port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortKey {
    pub context: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    pub namespace: String,
    pub kind: TargetKind,
    pub name: String,
    pub port: u16,
}

impl PortKey {
    pub fn of(target: &WebTarget, cx: &App) -> Option<Self> {
        let manager = ConnectionManager::try_global(cx)?;
        let context = manager.read(cx).context(&target.cluster)?;
        Some(Self {
            // A group's own key (its context names change with `oc project`).
            context: context.stable_key(),
            server: context.server.clone(),
            namespace: target.namespace.clone(),
            kind: target.kind,
            name: target.name.clone(),
            port: target.port,
        })
    }

    /// The data store of the object (all its ports): cookies and storage are never shared
    /// between clusters, namespaces or services.
    pub fn storage_id(&self) -> [u8; 16] {
        storage_id(
            &self.context,
            self.server.as_deref(),
            &self.namespace,
            self.kind,
            &self.name,
        )
    }

    fn same_object(&self, other: &PortKey) -> bool {
        self.context == other.context
            && self.server == other.server
            && self.namespace == other.namespace
            && self.kind == other.kind
            && self.name == other.name
    }
}

/// A stable 16-byte id (an RFC 9562 version 8 UUID) for a (cluster, namespace, object).
pub fn storage_id(
    context: &str,
    server: Option<&str>,
    namespace: &str,
    kind: TargetKind,
    name: &str,
) -> [u8; 16] {
    let mut hasher = Sha256::new();
    for part in [
        "kubyl-webview",
        context,
        server.unwrap_or_default(),
        namespace,
        kind.resource(),
        name,
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    id[6] = (id[6] & 0x0f) | 0x80;
    id[8] = (id[8] & 0x3f) | 0x80;
    id
}

/// What Kubyl remembers about one service port.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PortMemory {
    pub key: Option<PortKey>,
    /// `http`/`https` when the user chose one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<Scheme>,
    /// Where a new web view starts (`/graph`, `/grafana/`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_path: Option<String>,
    /// The last page (path and query, see [`crate::target::remembered_path`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_path: Option<String>,
    /// The local port of the last forward, reused when free: the page keeps its origin, so
    /// its local storage survives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    /// Open this port's web views private.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub private: bool,
}

/// A leaf certificate the user accepted for a service port (`Proceed` on the interstitial).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AcceptedCert {
    pub key: PortKey,
    /// SHA-256 of the DER certificate, hex.
    pub sha256: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebViewState {
    pub ports: Vec<PortMemory>,
    pub accepted_certs: Vec<AcceptedCert>,
}

impl StateSection for WebViewState {
    const KEY: &'static str = "web_views";
}

/// The most ports remembered (oldest are dropped).
const MAX_PORTS: usize = 500;

impl WebViewState {
    pub fn port(&self, key: &PortKey) -> PortMemory {
        self.ports
            .iter()
            .find(|p| p.key.as_ref() == Some(key))
            .cloned()
            .unwrap_or_else(|| PortMemory {
                key: Some(key.clone()),
                ..Default::default()
            })
    }

    /// Changes the memory of `key` (moving it to the end, most recent).
    pub fn update_port(&mut self, key: &PortKey, f: impl FnOnce(&mut PortMemory)) {
        let mut memory = self.port(key);
        f(&mut memory);
        self.ports.retain(|p| p.key.as_ref() != Some(key));
        self.ports.push(memory);
        if self.ports.len() > MAX_PORTS {
            let excess = self.ports.len() - MAX_PORTS;
            self.ports.drain(..excess);
        }
    }

    /// Fingerprints accepted for `key`.
    pub fn accepted(&self, key: &PortKey) -> Vec<[u8; 32]> {
        self.accepted_certs
            .iter()
            .filter(|c| &c.key == key)
            .filter_map(|c| parse_hex(&c.sha256))
            .collect()
    }

    pub fn accept(&mut self, key: PortKey, fingerprint: &[u8; 32]) {
        let sha256 = hex(fingerprint);
        if !self
            .accepted_certs
            .iter()
            .any(|c| c.key == key && c.sha256 == sha256)
        {
            self.accepted_certs.push(AcceptedCert { key, sha256 });
        }
    }

    /// Forgets the accepted certificates of every port of `key`'s object.
    pub fn forget_certs(&mut self, key: &PortKey) {
        self.accepted_certs.retain(|c| !c.key.same_object(key));
    }
}

/// Reads, changes and stores the web-view state.
pub fn update(cx: &mut App, f: impl FnOnce(&mut WebViewState)) {
    State::update::<WebViewState>(cx, f);
}

pub fn get(cx: &App) -> WebViewState {
    State::get::<WebViewState>(cx)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

pub fn init(cx: &mut App) {
    Settings::register::<WebViewSettings>(cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(context: &str, name: &str, port: u16) -> PortKey {
        PortKey {
            context: context.into(),
            server: Some("https://127.0.0.1:6443".into()),
            namespace: "monitoring".into(),
            kind: TargetKind::Service,
            name: name.into(),
            port,
        }
    }

    #[test]
    fn storage_is_per_cluster_namespace_and_object() {
        let a = key("kind-a", "grafana", 80).storage_id();
        assert_eq!(a, key("kind-a", "grafana", 3000).storage_id());
        assert_ne!(a, key("kind-b", "grafana", 80).storage_id());
        assert_ne!(a, key("kind-a", "prometheus", 80).storage_id());
        let mut other_ns = key("kind-a", "grafana", 80);
        other_ns.namespace = "default".into();
        assert_ne!(a, other_ns.storage_id());
        let mut other_server = key("kind-a", "grafana", 80);
        other_server.server = Some("https://10.0.0.1:6443".into());
        assert_ne!(a, other_server.storage_id());
        // A valid version 8 UUID.
        assert_eq!(a[6] >> 4, 8);
        assert_eq!(a[8] >> 6, 0b10);
    }

    #[test]
    fn port_memory_updates_and_moves_to_the_end() {
        let mut state = WebViewState::default();
        let grafana = key("kind", "grafana", 80);
        let prometheus = key("kind", "prometheus", 9090);
        state.update_port(&grafana, |m| m.start_path = Some("/d/pods".into()));
        state.update_port(&prometheus, |m| m.local_port = Some(52000));
        state.update_port(&grafana, |m| m.last_path = Some("/explore".into()));
        assert_eq!(state.ports.len(), 2);
        assert_eq!(state.ports[1].key.as_ref(), Some(&grafana));
        let memory = state.port(&grafana);
        assert_eq!(memory.start_path.as_deref(), Some("/d/pods"));
        assert_eq!(memory.last_path.as_deref(), Some("/explore"));
        assert_eq!(state.port(&key("kind", "other", 1)).start_path, None);
    }

    #[test]
    fn accepted_certificates_are_per_port_and_round_trip() {
        let mut state = WebViewState::default();
        let argo = key("kind", "argocd-server", 443);
        let fingerprint = [7u8; 32];
        state.accept(argo.clone(), &fingerprint);
        state.accept(argo.clone(), &fingerprint);
        assert_eq!(state.accepted_certs.len(), 1);
        assert_eq!(state.accepted(&argo), vec![fingerprint]);
        assert!(state.accepted(&key("kind", "argocd-server", 80)).is_empty());
        assert!(
            state
                .accepted(&key("other", "argocd-server", 443))
                .is_empty()
        );
        let json = serde_json::to_value(&state).unwrap();
        let back: WebViewState = serde_json::from_value(json).unwrap();
        assert_eq!(back, state);
        state.forget_certs(&key("kind", "argocd-server", 80));
        assert!(state.accepted_certs.is_empty());
    }
}
