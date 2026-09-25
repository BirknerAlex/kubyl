//! The `"argocd"` section of settings.json, and what Kubyl remembers in state.json: the Argo CD
//! installs the user confirmed for API mode (per context: namespace, Service and its UID). The
//! API-mode token is in the OS keychain, never in either file.

use gpui::App;
use kubyl_core::ClusterId;
use kubyl_kube::ConnectionManager;
use kubyl_settings::{Settings, SettingsSection, State, StateSection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::detect::Install;

/// How API mode reaches `argocd-server`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApiTransport {
    /// The API server's service proxy, else a temporary port-forward.
    #[default]
    Auto,
    /// Only the API server's service proxy (needs `get`/`create` on `services/proxy`).
    Proxy,
    /// Only a temporary loopback port-forward (needs `create` on `pods/portforward`).
    Forward,
}

/// Argo CD settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ArgoSettings {
    pub api_transport: ApiTransport,
    /// Connect API mode by itself when an install was confirmed and a token is stored.
    pub auto_connect: bool,
}

impl Default for ArgoSettings {
    fn default() -> Self {
        Self {
            api_transport: ApiTransport::Auto,
            auto_connect: true,
        }
    }
}

impl SettingsSection for ArgoSettings {
    const KEY: Option<&'static str> = Some("argocd");
}

/// A context across restarts: its name and API server (cluster ids contain the kubeconfig
/// path, like favorites and saved forwards).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContextKey {
    pub context: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

impl ContextKey {
    pub fn of(cluster: &ClusterId, cx: &App) -> Option<Self> {
        let manager = ConnectionManager::try_global(cx)?;
        let info = manager.read(cx).context(cluster)?.clone();
        Some(Self {
            context: info.context,
            server: info.server,
        })
    }
}

/// An install the user confirmed for API mode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedInstall {
    #[serde(flatten)]
    pub context: ContextKey,
    pub namespace: String,
    pub service: String,
    /// The Service's UID when confirmed: a re-created Service must be confirmed again.
    pub uid: String,
    /// The last user who signed in (not secret; prefills the dialog).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

impl TrustedInstall {
    /// Whether `install` is the one the user confirmed.
    pub fn matches(&self, install: &Install) -> bool {
        install.namespace == self.namespace
            && install
                .server
                .as_ref()
                .is_some_and(|s| s.name == self.service && s.uid == self.uid)
    }
}

/// `state.json` → `argocd`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArgoState {
    pub trusted: Vec<TrustedInstall>,
}

impl StateSection for ArgoState {
    const KEY: &'static str = "argocd";
}

/// The confirmed install of a cluster.
pub fn trusted(cluster: &ClusterId, cx: &App) -> Option<TrustedInstall> {
    let key = ContextKey::of(cluster, cx)?;
    State::get::<ArgoState>(cx)
        .trusted
        .into_iter()
        .find(|t| t.context == key)
}

/// Remembers `install` as the confirmed one of its cluster (one per cluster).
pub fn trust(cluster: &ClusterId, install: &Install, username: Option<String>, cx: &mut App) {
    let (Some(key), Some(server)) = (ContextKey::of(cluster, cx), install.server.clone()) else {
        return;
    };
    State::update::<ArgoState>(cx, |state| {
        state.trusted.retain(|t| t.context != key);
        state.trusted.push(TrustedInstall {
            context: key,
            namespace: install.namespace.clone(),
            service: server.name,
            uid: server.uid,
            username,
        });
    });
}

/// Forgets the confirmed install of a cluster.
pub fn forget(cluster: &ClusterId, cx: &mut App) {
    let Some(key) = ContextKey::of(cluster, cx) else {
        return;
    };
    State::update::<ArgoState>(cx, |state| state.trusted.retain(|t| t.context != key));
}

/// The keychain entry of an install's API token.
pub fn token_key(key: &ContextKey, namespace: &str, service: &str) -> String {
    format!(
        "argocd/{}/{}/{namespace}/{service}",
        key.server.as_deref().unwrap_or("-"),
        key.context
    )
}

pub fn get(cx: &App) -> &ArgoSettings {
    Settings::get::<ArgoSettings>(cx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::ServerService;

    #[test]
    fn trust_needs_the_same_service() {
        let trusted = TrustedInstall {
            context: ContextKey {
                context: "kind-kubyl-dev".into(),
                server: Some("https://127.0.0.1:1".into()),
            },
            namespace: "argocd".into(),
            service: "argocd-server".into(),
            uid: "u1".into(),
            username: Some("admin".into()),
        };
        let mut install = Install {
            namespace: "argocd".into(),
            server: Some(ServerService {
                name: "argocd-server".into(),
                uid: "u1".into(),
                https_port: Some(443),
                http_port: Some(80),
            }),
            ..Default::default()
        };
        assert!(trusted.matches(&install));
        // Re-created (someone deleted and created a Service with the same name).
        install.server.as_mut().unwrap().uid = "u2".into();
        assert!(!trusted.matches(&install));
        install.server.as_mut().unwrap().uid = "u1".into();
        install.namespace = "evil".into();
        assert!(!trusted.matches(&install));
        // The state holds no token.
        let json = serde_json::to_string(&ArgoState {
            trusted: vec![trusted.clone()],
        })
        .unwrap();
        assert!(!json.contains("token"));
        assert_eq!(
            token_key(&trusted.context, "argocd", "argocd-server"),
            "argocd/https://127.0.0.1:1/kind-kubyl-dev/argocd/argocd-server"
        );
    }
}
