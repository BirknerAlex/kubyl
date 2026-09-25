//! Opt-in notifications for new Warning events in favorited namespaces
//! (`overview.notify_warnings`), throttled per namespace (`overview.notify_interval`).
//!
//! Only events that arrive after a namespace is being watched count, so connecting doesn't
//! replay an hour of history. Notifications are Kubyl toasts (the in-app notification center).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Subscription};
use kubyl_core::{ClusterId, Gvr, Notification, NotificationCenter};
use kubyl_explorer::favorites::{self, Favorites};
use kubyl_kube::ConnectionManager;
use kubyl_resources::{ResourceStores, StoreHandle, StoreKey};
use kubyl_settings::Settings;

use crate::events::model::event_row;
use crate::settings::OverviewSettings;

/// One watched favorite namespace.
struct Watch {
    store: StoreHandle,
    label: String,
    /// Event uid → count already seen.
    seen: HashMap<String, u64>,
    primed: bool,
    last_notified: Option<Instant>,
    pending: Vec<String>,
    /// A delayed flush of `pending` is scheduled (the interval wasn't over yet).
    flush_scheduled: bool,
    _observer: Subscription,
}

/// Keeps the notifier alive for the app's lifetime.
struct GlobalNotifier(#[allow(dead_code)] Entity<WarningNotifier>);

impl gpui::Global for GlobalNotifier {}

pub struct WarningNotifier {
    watches: HashMap<(ClusterId, String), Watch>,
    _subscriptions: Vec<Subscription>,
}

impl WarningNotifier {
    pub fn install(cx: &mut App) {
        let notifier = cx.new(|cx| {
            let favorites = Favorites::global(cx);
            let mut subscriptions =
                vec![cx.observe(&favorites, |this: &mut Self, _, cx| this.sync(cx))];
            let weak = cx.weak_entity();
            subscriptions.push(Settings::observe::<OverviewSettings>(cx, move |_, cx| {
                weak.update(cx, |this, cx| this.sync(cx)).ok();
            }));
            if let Some(manager) = ConnectionManager::try_global(cx) {
                subscriptions.push(cx.subscribe(&manager, |this, _, _, cx| this.sync(cx)));
            }
            let mut this = Self {
                watches: HashMap::new(),
                _subscriptions: subscriptions,
            };
            this.sync(cx);
            this
        });
        cx.set_global(GlobalNotifier(notifier));
    }

    /// Watches the events of every favorite whose cluster is connected, when enabled.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let enabled = Settings::get::<OverviewSettings>(cx).notify_warnings;
        let manager = ConnectionManager::try_global(cx);
        let mut wanted: HashMap<(ClusterId, String), String> = HashMap::new();
        if enabled {
            for favorite in Favorites::global(cx).read(cx).items() {
                let Some(cluster) = favorites::cluster_of(favorite, cx) else {
                    continue;
                };
                let connected = manager
                    .as_ref()
                    .is_some_and(|m| m.read(cx).client(&cluster).is_some());
                if connected {
                    let name = manager
                        .as_ref()
                        .map(|m| m.read(cx).display_name(&cluster).to_string())
                        .unwrap_or_else(|| cluster.to_string());
                    wanted.insert(
                        (cluster, favorite.namespace.clone()),
                        format!("{} · {name}", favorite.namespace),
                    );
                }
            }
        }
        self.watches.retain(|key, _| wanted.contains_key(key));
        for (key, label) in wanted {
            if self.watches.contains_key(&key) {
                continue;
            }
            let store = ResourceStores::acquire(
                cx,
                StoreKey::new(
                    key.0.clone(),
                    Gvr::new("", "v1", "events"),
                    Some(key.1.clone()),
                )
                .fields("type=Warning"),
            );
            let watch_key = key.clone();
            let observer = cx.observe(store.entity(), move |this, _, cx| {
                this.changed(&watch_key, cx)
            });
            self.watches.insert(
                key,
                Watch {
                    store,
                    label,
                    seen: HashMap::new(),
                    primed: false,
                    last_notified: None,
                    pending: Vec::new(),
                    flush_scheduled: false,
                    _observer: observer,
                },
            );
        }
    }

    fn changed(&mut self, key: &(ClusterId, String), cx: &mut Context<Self>) {
        let Some(watch) = self.watches.get_mut(key) else {
            return;
        };
        let store = watch.store.read(cx);
        if !store.status().is_ready() {
            return;
        }
        let mut fresh = Vec::new();
        let mut present = HashSet::new();
        for event in store.objects().values() {
            let row = event_row(event);
            let uid = row.key.to_string();
            present.insert(uid.clone());
            let previous = watch.seen.insert(uid, row.count);
            if watch.primed && previous.is_none_or(|count| count < row.count) {
                fresh.push(format!("{} {}: {}", row.reason, row.object(), row.message));
            }
        }
        watch.seen.retain(|uid, _| present.contains(uid));
        watch.primed = true;
        watch.pending.extend(fresh);
        self.flush(key, cx);
    }

    /// Shows the pending warnings of a watch, or schedules that for when the interval is over,
    /// so a warning isn't held back until the next one arrives.
    fn flush(&mut self, key: &(ClusterId, String), cx: &mut Context<Self>) {
        let interval = Duration::from_secs(Settings::get::<OverviewSettings>(cx).notify_interval);
        let Some(watch) = self.watches.get_mut(key) else {
            return;
        };
        if watch.pending.is_empty() {
            return;
        }
        let wait = watch
            .last_notified
            .map(|t| interval.saturating_sub(t.elapsed()))
            .filter(|wait| !wait.is_zero());
        if let Some(wait) = wait {
            if !watch.flush_scheduled {
                watch.flush_scheduled = true;
                let key = key.clone();
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    this.update(cx, |this, cx| {
                        if let Some(watch) = this.watches.get_mut(&key) {
                            watch.flush_scheduled = false;
                        }
                        this.flush(&key, cx);
                    })
                    .ok();
                })
                .detach();
            }
            return;
        }
        let first = watch.pending[0].clone();
        let more = watch.pending.len() - 1;
        watch.pending.clear();
        watch.last_notified = Some(Instant::now());
        let message = if more > 0 {
            format!("{first} (+{more} more)")
        } else {
            first
        };
        NotificationCenter::push(
            cx,
            Notification::warning(message).title(format!("Warning in {}", watch.label)),
        );
    }
}
