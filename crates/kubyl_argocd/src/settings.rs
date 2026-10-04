//! The `"argocd"` settings.json section and the confirmed installs (from `kubyl_argocd_core`),
//! read from the app's settings and state.

use gpui::App;
pub use kubyl_argocd_core::settings::*;
use kubyl_core::ClusterId;
use kubyl_kube::ConnectionManager;
use kubyl_settings::{Settings, State};

use crate::detect::Install;

/// The key of a cluster entry across restarts (see [`ContextKey::for_entry`]).
pub fn context_key(cluster: &ClusterId, cx: &App) -> Option<ContextKey> {
    let manager = ConnectionManager::try_global(cx)?;
    let info = manager.read(cx).context(cluster)?.clone();
    Some(ContextKey::for_entry(&info, &State::get::<ArgoState>(cx)))
}

/// The confirmed install of a cluster.
pub fn trusted(cluster: &ClusterId, cx: &App) -> Option<TrustedInstall> {
    let key = context_key(cluster, cx)?;
    State::get::<ArgoState>(cx)
        .trusted
        .into_iter()
        .find(|t| t.context == key)
}

/// Remembers `install` as the confirmed one of its cluster (one per cluster).
pub fn trust(cluster: &ClusterId, install: &Install, username: Option<String>, cx: &mut App) {
    let (Some(key), Some(server)) = (context_key(cluster, cx), install.server.clone()) else {
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
    let Some(key) = context_key(cluster, cx) else {
        return;
    };
    State::update::<ArgoState>(cx, |state| state.trusted.retain(|t| t.context != key));
}

pub fn get(cx: &App) -> &ArgoSettings {
    Settings::get::<ArgoSettings>(cx)
}
