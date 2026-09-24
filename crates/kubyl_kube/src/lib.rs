//! Kubeconfig sources, auth, connection manager and discovery.
//!
//! # For other crates (the sidebar, resource views…)
//!
//! Everything goes through one app-wide entity, [`ConnectionManager::global`]:
//!
//! ```ignore
//! let manager = ConnectionManager::global(cx);
//! cx.subscribe(&manager, |this, _, event: &ConnectionEvent, cx| { /* re-render */ }).detach();
//!
//! // Cluster roots: every context the user didn't hide, in source order.
//! for context in manager.read(cx).contexts() {
//!     let name = manager.read(cx).display_name(&context.id);   // override or merged name
//!     let color = manager.read(cx).color(&context.id, cx);      // color tag
//!     let state = manager.read(cx).state(&context.id);           // ConnectionState
//!     let source = &context.file;                                 // kubeconfig file (Favorites)
//! }
//!
//! // Expanding a root / opening a favorite: connect lazily (no-op when connected).
//! manager.update(cx, |m, cx| m.ensure_connected(&id, cx));
//! // Making it the title bar cluster (also connects):
//! manager.update(cx, |m, cx| m.activate(&id, cx));
//!
//! // Once connected:
//! let client: Option<kube::Client> = manager.read(cx).client(&id);
//! let namespaces = manager.read(cx).namespaces(&id);     // watched, or fallbacks if forbidden
//! let discovery = manager.read(cx).discovery(&id);       // Arc<Discovery>: every GVK/GVR
//! let caps = manager.read(cx).caps(&id);                  // ClusterCaps (incl. read-only/prod)
//! let allowed = manager.update(cx, |m, cx| m.can_i(&id, AccessQuery::new("delete", &gvr, Some("ns")), cx));
//! let schema = manager.update(cx, |m, cx| m.openapi_spec(&id, OpenApiIndex::key("apps", "v1"), cx));
//! ```
//!
//! [`ConnectionEvent`]s: `ContextsChanged` (sources reloaded or overrides changed),
//! `StateChanged`, `NamespacesChanged`, `DiscoveryChanged` (also after CRDs change) and
//! `ActiveChanged`. Network work runs on the Tokio runtime; the entity only holds results.
//!
//! [`AccessQuery`]: access::AccessQuery
//! [`OpenApiIndex`]: openapi::OpenApiIndex
//!
//! # What lives where
//!
//! - [`kubeconfig`]: sources (`~/.kube/config`, `$KUBECONFIG`, added files/folders, pasted),
//!   loading and merging contexts (collisions get `@<file-stem>`).
//! - [`settings`]: the `"kubernetes"` settings.json section (sources, per-context overrides).
//! - [`auth`]: auth method detection, exec plugins, OIDC sign-in/refresh, keychain store.
//! - [`client`], [`manager`]: clients, the connection state machine, health pings, watches.
//! - [`discovery`], [`cluster_info`], [`access`], [`openapi`]: what the cluster serves.
//! - [`ui`]: the Clusters & kubeconfigs tab, switcher, sign-in and exec prompt modals.

pub mod access;
pub mod auth;
pub mod client;
pub mod cluster_info;
pub mod discovery;
pub mod kubeconfig;
pub mod manager;
pub mod openapi;
pub mod settings;
pub mod ui;
pub mod watches;

pub use manager::{Cluster, ConnectionEvent, ConnectionManager, ConnectionState, Namespaces};

use gpui::App;
use kubyl_settings::Settings;

/// Registers this crate's views, actions and chrome contributions.
pub fn init(cx: &mut App) {
    Settings::register::<settings::KubeSettings>(cx);
    auth::shell_env::prefetch();
    let pasted = kubyl_settings::config_dir().join("kubeconfigs");
    ConnectionManager::install(pasted, true, cx);
    ui::init(cx);
}
