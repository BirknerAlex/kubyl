//! The `"argocd"` settings.json section and the confirmed installs (from `kubyl_argocd_core`),
//! read from the app's settings and state.

use gpui::App;
pub use kubyl_argocd_core::settings::*;
use kubyl_core::ClusterId;
use kubyl_kube::ConnectionManager;
use kubyl_settings::{Settings, State};

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

pub fn get(cx: &App) -> &ArgoSettings {
    Settings::get::<ArgoSettings>(cx)
}
