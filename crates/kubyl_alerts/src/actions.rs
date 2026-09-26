//! Keys of the Alerts view (registered in the `ActionRegistry` for the key-hint bar), the palette
//! actions, and the ways out of the view (targets, runbooks, web UIs, rules).

use gpui::{App, Context, Focusable as _, Window, actions, prelude::*};
use kubyl_core::actions::{OpenSettings, OpenView};
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ClusterId, Gvr, Notification, NotificationCenter,
    ResourceRef, ViewKind, ViewRequest, spawn_kube,
};
use kubyl_kube::ConnectionManager;
use kubyl_resources::ResourceSelection;
use secrecy::SecretString;

use crate::client::{AmConn, Via};
use crate::discover::AmTarget;
use crate::model::Target;
use crate::service::AlertsService;
use crate::view::rows::ObjectFilter;
use crate::view::{
    self, ALERTS_CONTEXT, AlertsView, Pending, RULES_CONTEXT, SILENCES_CONTEXT, Tab,
};

actions!(
    alerts,
    [
        /// Opens the details of the selected alert.
        AlertDetails,
        /// Silences the selected alert (the editor starts with its labels).
        SilenceAlert,
        /// Silences exactly the selected alert for `alerts.ack_duration`.
        Acknowledge,
        /// Opens the object the alert is about.
        GoToTarget,
        /// Opens the logs of the alert's pod.
        TargetLogs,
        /// Opens the alert's runbook in the browser.
        OpenRunbook,
        /// Copies the alert's labels as matchers.
        CopyMatchers,
        /// Focuses the filter.
        FocusFilter,
        /// Back from the filter to the list.
        BlurFilter,
        /// Next row.
        SelectNext,
        /// Previous row.
        SelectPrevious,
        /// Edits the selected silence.
        EditSilence,
        /// Extends the selected silence by an hour.
        ExtendSilence,
        /// Expires the selected silence.
        ExpireSilence,
        /// Copies the selected silence as an `amtool silence add` command.
        CopyAmtool,
        /// Opens the selected rule's details.
        RuleDetails,
        /// Opens the `PrometheusRule` of the selected rule in the YAML editor.
        EditRule,
        /// Shows the alerts of the selected rule.
        ShowRuleAlerts,
        /// Copies the selected rule's expression.
        CopyExpression,
        /// Opens the active cluster's alerts.
        ShowAlerts,
        /// Opens the alerts of every connected cluster.
        ShowAllAlerts,
        /// Opens the active cluster's silences.
        ShowSilences,
        /// Opens the silence editor.
        NewSilence,
        /// Looks for the active cluster's Alertmanager again.
        LookAgain,
        /// Stores the Authorization header for an external Alertmanager URL in the keychain.
        SetAuthorization,
        /// Removes that header.
        ClearAuthorization,
        /// Opens the alerts of the selected object.
        ShowAlertsForSelection,
    ]
);

/// Hints of writing actions, hidden on read-only clusters.
pub const WRITE_HINTS: &[&str] = &[
    "Silence…",
    "Acknowledge",
    "Edit…",
    "Extend +1h",
    "Expire…",
    "New silence…",
];

const LIST_CONTEXT: &str = "ResourceList";
const ALERT_KINDS: &[&str] = &[
    "pods",
    "deployments",
    "statefulsets",
    "daemonsets",
    "replicasets",
    "jobs",
    "cronjobs",
    "horizontalpodautoscalers",
    "persistentvolumeclaims",
    "nodes",
    "services",
    "namespaces",
];

pub(crate) fn init(cx: &mut App) {
    let keyed: Vec<(ActionSpec, &str, &str)> = vec![
        (
            ActionSpec::new("Alerts: Details", AlertDetails).hint("Details"),
            "enter",
            ALERTS_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: Silence…", SilenceAlert).hint("Silence…"),
            "s",
            ALERTS_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: Acknowledge", Acknowledge).hint("Acknowledge"),
            "a",
            ALERTS_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: Go to Target", GoToTarget).hint("Go to target"),
            "o",
            ALERTS_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: Logs of Target", TargetLogs).hint("Logs"),
            "l",
            ALERTS_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: Open Runbook", OpenRunbook).hint("Runbook"),
            "r",
            ALERTS_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: Copy Labels as Matchers", CopyMatchers).hint("Copy matchers"),
            "y",
            ALERTS_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: Edit Silence…", EditSilence).hint("Edit…"),
            "enter",
            SILENCES_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: Extend Silence by 1h", ExtendSilence).hint("Extend +1h"),
            "e",
            SILENCES_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: Expire Silence…", ExpireSilence).hint("Expire…"),
            "ctrl-d",
            SILENCES_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: New Silence…", NewSilence).hint("New silence…"),
            "n",
            SILENCES_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: Copy Silence as amtool", CopyAmtool).hint("Copy as amtool"),
            "c",
            SILENCES_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: Rule Details", RuleDetails).hint("Details"),
            "enter",
            RULES_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: Edit PrometheusRule", EditRule).hint("Edit PrometheusRule"),
            "e",
            RULES_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: Show the Rule's Alerts", ShowRuleAlerts)
                .hint("Show its alerts"),
            "a",
            RULES_CONTEXT,
        ),
        (
            ActionSpec::new("Alerts: Copy Expression", CopyExpression).hint("Copy expression"),
            "y",
            RULES_CONTEXT,
        ),
    ];
    for (spec, keys, context) in keyed {
        ActionRegistry::register(cx, spec.bind(keys, Some(context)));
    }
    let nav = format!("{ALERTS_CONTEXT} || {SILENCES_CONTEXT} || {RULES_CONTEXT}");
    cx.bind_keys([
        gpui::KeyBinding::new("j", SelectNext, Some(nav.as_str())),
        gpui::KeyBinding::new("down", SelectNext, Some(nav.as_str())),
        gpui::KeyBinding::new("k", SelectPrevious, Some(nav.as_str())),
        gpui::KeyBinding::new("up", SelectPrevious, Some(nav.as_str())),
        gpui::KeyBinding::new("/", FocusFilter, Some(nav.as_str())),
        gpui::KeyBinding::new("escape", BlurFilter, Some("AlertsView > Input")),
        gpui::KeyBinding::new("down", BlurFilter, Some("AlertsView > Input")),
    ]);
    for spec in [
        ActionSpec::new("Alerts: Show Alerts", ShowAlerts),
        ActionSpec::new("Alerts: Show Alerts in All Clusters", ShowAllAlerts),
        ActionSpec::new("Alerts: Show Silences", ShowSilences),
        ActionSpec::new("Alerts: New Silence…", NewSilence),
        ActionSpec::new("Alerts: Look for Alertmanager Again", LookAgain),
        ActionSpec::new(
            "Alerts: Set Alertmanager Authorization Header…",
            SetAuthorization,
        ),
        ActionSpec::new(
            "Alerts: Clear Alertmanager Authorization Header",
            ClearAuthorization,
        ),
    ] {
        ActionRegistry::register(cx, spec);
    }
    let mut for_selection =
        ActionSpec::new("Alerts: Show Alerts for Selection", ShowAlertsForSelection)
            .available_when(|target, _| {
                target.is_object() && ALERT_KINDS.contains(&target.gvr.resource.as_str())
            });
    for_selection.context = Some(LIST_CONTEXT.into());
    ActionRegistry::register(cx, for_selection);

    cx.on_action(|_: &ShowAlerts, cx| {
        if let Some(cluster) = active_cluster(cx) {
            with_window(cx, move |window, cx| view::open(&cluster, None, window, cx));
        }
    });
    cx.on_action(|_: &ShowAllAlerts, cx| {
        with_window(cx, |window, cx| {
            view::open_with(None, Pending::default(), window, cx)
        });
    });
    cx.on_action(|_: &ShowSilences, cx| {
        if let Some(cluster) = active_cluster(cx) {
            with_window(cx, move |window, cx| {
                view::open(&cluster, Some(Tab::Silences), window, cx)
            });
        }
    });
    cx.on_action(|_: &NewSilence, cx| {
        if let Some(cluster) = active_cluster(cx) {
            with_window(cx, move |window, cx| {
                crate::silence::open_new(&cluster, window, cx)
            });
        }
    });
    cx.on_action(|_: &LookAgain, cx| {
        if let (Some(cluster), Some(service)) = (active_cluster(cx), AlertsService::global(cx)) {
            service.update(cx, |s, cx| s.redetect(&cluster, cx));
        }
    });
    cx.on_action(|_: &SetAuthorization, cx| {
        let Some(cluster) = active_cluster(cx) else {
            return;
        };
        let urls = external_urls(&cluster, cx);
        let Some(url) = urls.first().cloned() else {
            NotificationCenter::push(
                cx,
                Notification::error(
                    "No Alertmanager URL for this cluster: add one under alerts.clusters in settings.json first.",
                ),
            );
            return;
        };
        with_window(cx, move |window, cx| {
            kubyl_explorer::dialogs::prompt_secret(
                format!("Authorization header for {url}").into(),
                "Header value, e.g. Bearer <token> (kept in the OS keychain)",
                move |value, _, cx| {
                    let header = (!value.trim().is_empty()).then(|| SecretString::from(value));
                    save_authorization(cluster.clone(), url.clone(), header, cx);
                },
                window,
                cx,
            )
        });
    });
    cx.on_action(|_: &ClearAuthorization, cx| {
        let Some(cluster) = active_cluster(cx) else {
            return;
        };
        for url in external_urls(&cluster, cx) {
            save_authorization(cluster.clone(), url, None, cx);
        }
    });
    cx.on_action(|_: &ShowAlertsForSelection, cx| {
        let Some(selected) = ResourceSelection::global(cx)
            .primary()
            .map(|s| s.target.clone())
        else {
            return;
        };
        let Some(name) = selected.name.clone() else {
            return;
        };
        let Some(object) = ObjectFilter::for_resource(
            &selected.gvr.resource,
            selected.namespace.as_deref(),
            &name,
        ) else {
            return;
        };
        let cluster = selected.cluster.clone();
        with_window(cx, move |window, cx| {
            view::open_with(
                Some(&cluster),
                Pending {
                    tab: Some(Tab::Alerts),
                    object: Some(object),
                    ..Default::default()
                },
                window,
                cx,
            )
        });
    });
}

fn active_cluster(cx: &App) -> Option<ClusterId> {
    ResourceSelection::global(cx)
        .primary()
        .map(|s| s.target.cluster.clone())
        .or_else(|| {
            ActiveContext::global(cx)
                .cluster
                .as_ref()
                .map(|c| c.id.clone())
        })
}

/// Runs `f` in the focused window (deferred: global handlers run inside the window update).
fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window {
            window.update(cx, |_, window, cx| f(window, cx)).ok();
        }
    });
}

/// The URLs of the cluster's external Alertmanagers (settings).
fn external_urls(cluster: &ClusterId, cx: &App) -> Vec<String> {
    let Some(service) = AlertsService::global(cx) else {
        return Vec::new();
    };
    let keys = ConnectionManager::try_global(cx)
        .map(|m| m.read(cx).settings_keys(cluster))
        .unwrap_or_default();
    service
        .read(cx)
        .settings()
        .cluster(&keys)
        .alertmanagers
        .iter()
        .filter_map(|a| a.url.clone())
        .collect()
}

/// Stores (or removes) the Authorization header of `url` in the keychain, off the UI thread.
fn save_authorization(cluster: ClusterId, url: String, header: Option<SecretString>, cx: &mut App) {
    let key = crate::service::auth_key(cluster.as_str(), &url);
    let clearing = header.is_none();
    let task = cx.background_executor().spawn(async move {
        match header {
            Some(header) => kubyl_kube::auth::store::set(&key, &header),
            None => kubyl_kube::auth::store::delete(&key),
        }
    });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            match result {
                Ok(()) => NotificationCenter::push(
                    cx,
                    Notification::success(if clearing {
                        format!("Authorization header for {url} removed.")
                    } else {
                        format!("Authorization header for {url} saved in the keychain.")
                    }),
                ),
                Err(err) => {
                    NotificationCenter::push(cx, Notification::error(format!("Keychain: {err}")))
                }
            }
            if let Some(service) = AlertsService::global(cx) {
                service.update(cx, |s, cx| s.redetect(&cluster, cx));
            }
        });
    })
    .detach();
}

/// Binds the view's key actions.
pub(crate) fn bind_view_actions(this: gpui::Div, cx: &mut Context<AlertsView>) -> gpui::Div {
    this.on_action(cx.listener(|view, _: &FocusFilter, window, cx| {
        let focus = match view.tab {
            Tab::Alerts => view.filter_input.read(cx).focus_handle(cx),
            Tab::Silences => view.silence_filter.read(cx).focus_handle(cx),
            Tab::Rules => view.rules_filter.read(cx).focus_handle(cx),
        };
        focus.focus(window, cx);
    }))
    .on_action(cx.listener(|view, _: &BlurFilter, window, cx| {
        view.focus.focus(window, cx);
    }))
    .on_action(cx.listener(|view, _: &SelectNext, _, cx| match view.tab {
        Tab::Alerts => view.move_selection(1, cx),
        Tab::Silences => view.move_silence(1, cx),
        Tab::Rules => view.move_rule(1, cx),
    }))
    .on_action(
        cx.listener(|view, _: &SelectPrevious, _, cx| match view.tab {
            Tab::Alerts => view.move_selection(-1, cx),
            Tab::Silences => view.move_silence(-1, cx),
            Tab::Rules => view.move_rule(-1, cx),
        }),
    )
    .on_action(cx.listener(|view, _: &AlertDetails, _, cx| {
        view.details_open = !view.details_open || view.selected.is_none();
        cx.notify();
    }))
    .on_action(cx.listener(|view, _: &SilenceAlert, window, cx| {
        if let Some(entry) = view.selected_entry() {
            crate::silence::open_for_alert(&entry.cluster, &entry.alert, window, cx);
        }
    }))
    .on_action(cx.listener(|view, _: &Acknowledge, window, cx| {
        if let Some(entry) = view.selected_entry() {
            crate::silence::acknowledge(&entry.cluster, &entry.alert, window, cx);
        }
    }))
    .on_action(cx.listener(|view, _: &GoToTarget, window, cx| {
        if let Some(entry) = view.selected_entry()
            && let Some(target) = &entry.alert.target
        {
            open_target(&entry.cluster, target, false, window, cx);
        }
    }))
    .on_action(cx.listener(|view, _: &TargetLogs, window, cx| {
        if let Some(entry) = view.selected_entry()
            && let Some(target) = &entry.alert.target
        {
            open_target(&entry.cluster, target, true, window, cx);
        }
    }))
    .on_action(cx.listener(|view, _: &OpenRunbook, _, cx| {
        if let Some(url) = view
            .selected_entry()
            .and_then(|e| e.alert.runbook_url().map(str::to_string))
        {
            crate::view::widgets::open_url(&url, cx);
        }
    }))
    .on_action(cx.listener(|view, _: &CopyMatchers, _, cx| {
        if let Some(entry) = view.selected_entry() {
            crate::view::widgets::copy(
                crate::matchers::format(&entry.alert.matchers()),
                "the labels",
                cx,
            );
        }
    }))
    .on_action(cx.listener(|view, _: &EditSilence, window, cx| {
        if let Some(entry) = view.selected_silence_entry(cx) {
            crate::silence::open_edit(&entry.cluster, &entry.silence, window, cx);
        }
    }))
    .on_action(cx.listener(|view, _: &ExtendSilence, window, cx| {
        if let Some(entry) = view.selected_silence_entry(cx)
            && entry.silence.state != "expired"
        {
            crate::silence::extend(&entry.cluster, &entry.silence, 1, window, cx);
        }
    }))
    .on_action(cx.listener(|view, _: &ExpireSilence, window, cx| {
        if let Some(entry) = view.selected_silence_entry(cx)
            && entry.silence.state != "expired"
        {
            crate::silence::confirm_expire(&entry.cluster, &entry.silence, window, cx);
        }
    }))
    .on_action(cx.listener(|view, _: &NewSilence, window, cx| {
        if let Some(cluster) = view.cluster().cloned() {
            crate::silence::open_new(&cluster, window, cx);
        }
    }))
    .on_action(cx.listener(|view, _: &CopyAmtool, _, cx| {
        if let Some(entry) = view.selected_silence_entry(cx) {
            let command = format!(
                "amtool silence add {} --comment={}",
                crate::matchers::amtool_args(&entry.silence.matchers),
                shell_quote(&entry.silence.comment)
            );
            crate::view::widgets::copy(command, "the amtool command", cx);
        }
    }))
    .on_action(cx.listener(|view, _: &RuleDetails, _, cx| {
        view.rule_details_open = true;
        cx.notify();
    }))
    .on_action(cx.listener(|view, _: &EditRule, window, cx| {
        if let Some((cluster, rule)) = view.selected_rule(cx)
            && let Some((ns, name)) = view
                .rule_objects
                .find(&cluster, &rule.group, &rule.name, cx)
        {
            edit_prometheus_rule(&cluster, &ns, &name, window, cx);
        }
    }))
    .on_action(cx.listener(|view, _: &ShowRuleAlerts, window, cx| {
        if let Some((_, rule)) = view.selected_rule(cx) {
            view.show_rule_alerts(&rule.name, window, cx);
        }
    }))
    .on_action(cx.listener(|view, _: &CopyExpression, _, cx| {
        if let Some((_, rule)) = view.selected_rule(cx) {
            crate::view::widgets::copy(rule.query, "the expression", cx);
        }
    }))
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Opens the object an alert is about (details, or a pod's logs).
pub fn open_target(
    cluster: &ClusterId,
    target: &Target,
    logs: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let (group, version, resource) = target.kind.gvr();
    let object = ResourceRef::object(
        cluster.clone(),
        Gvr::new(group, version, resource),
        target
            .namespace
            .clone()
            .filter(|_| target.kind.namespaced()),
        target.name.clone(),
    );
    let kind = if logs && target.kind == crate::model::TargetKind::Pod {
        ViewKind::Logs
    } else {
        ViewKind::Details
    };
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(kind, object))),
        cx,
    );
}

/// Opens a `PrometheusRule` in the YAML editor.
pub fn edit_prometheus_rule(
    cluster: &ClusterId,
    namespace: &str,
    name: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let object = ResourceRef::object(
        cluster.clone(),
        Gvr::new("monitoring.coreos.com", "v1", "prometheusrules"),
        Some(namespace.to_string()),
        name.to_string(),
    );
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(ViewKind::Yaml, object))),
        cx,
    );
}

/// Opens settings (where Alertmanagers are named).
pub fn open_settings(_cluster: &ClusterId, window: &mut Window, cx: &mut App) {
    window.dispatch_action(Box::new(OpenSettings), cx);
}

/// A Service port by name or number, looked up off the UI thread; then `f` in the window.
fn with_port(
    cluster: &ClusterId,
    namespace: &str,
    service: &str,
    port: &str,
    window: &mut Window,
    cx: &mut App,
    f: impl FnOnce(u16, &mut Window, &mut App) + 'static,
) {
    if let Ok(number) = port.parse::<u16>() {
        return f(number, window, cx);
    }
    let Some(client) = ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(cluster))
    else {
        return;
    };
    let (namespace, service, name) = (namespace.to_string(), service.to_string(), port.to_string());
    let task = spawn_kube(cx, async move {
        let request =
            http::Request::get(format!("/api/v1/namespaces/{namespace}/services/{service}"))
                .body(Vec::new())
                .ok()?;
        let svc: serde_json::Value = client.request(request).await.ok()?;
        svc.pointer("/spec/ports")?
            .as_array()?
            .iter()
            .find(|p| p["name"].as_str() == Some(name.as_str()))
            .and_then(|p| p["port"].as_u64())
            .and_then(|p| u16::try_from(p).ok())
    });
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        if let Some(number) = task.await {
            handle
                .update(cx, |_, window, cx| f(number, window, cx))
                .ok();
        }
    })
    .detach();
}

/// "Open Alertmanager UI": a web view on the Service, or the URL in the browser.
pub fn open_alertmanager_ui(cluster: &ClusterId, conn: &AmConn, window: &mut Window, cx: &mut App) {
    match (&conn.target, &conn.via) {
        (_, Via::Route { url }) => crate::view::widgets::open_url(url, cx),
        (AmTarget::Url { url, .. }, _) => crate::view::widgets::open_url(url, cx),
        (
            AmTarget::Service {
                namespace,
                service,
                port,
                path,
                ..
            },
            _,
        ) => {
            let target = ResourceRef::object(
                cluster.clone(),
                Gvr::new("", "v1", "services"),
                Some(namespace.clone()),
                service.clone(),
            );
            let path = (!path.is_empty()).then(|| format!("{path}/"));
            with_port(
                cluster,
                namespace,
                service,
                port,
                window,
                cx,
                move |port, window, cx| {
                    window.dispatch_action(
                        Box::new(kubyl_webview::OpenWebView {
                            target,
                            port,
                            path,
                            ask: false,
                        }),
                        cx,
                    );
                },
            );
        }
    }
}

/// "Open in Prometheus": a web view on the Prometheus Service at the generator URL's path and
/// query, else the URL itself in the browser.
pub fn open_generator(cluster: &ClusterId, url: &str, window: &mut Window, cx: &mut App) {
    let prometheus = kubyl_metrics::MetricsService::global(cx)
        .and_then(|m| m.read(cx).prometheus(cluster))
        .map(|p| p.target().clone());
    let parsed = url::Url::parse(url).ok();
    match (prometheus, parsed) {
        (
            Some(kubyl_metrics::prometheus::Target::Service {
                namespace,
                service,
                port,
                ..
            }),
            Some(parsed),
        ) => {
            let path = match parsed.query() {
                Some(query) => format!("{}?{query}", parsed.path()),
                None => parsed.path().to_string(),
            };
            let target = ResourceRef::object(
                cluster.clone(),
                Gvr::new("", "v1", "services"),
                Some(namespace.clone()),
                service.clone(),
            );
            with_port(
                cluster,
                &namespace,
                &service,
                &port,
                window,
                cx,
                move |port, window, cx| {
                    window.dispatch_action(
                        Box::new(kubyl_webview::OpenWebView {
                            target,
                            port,
                            path: Some(path),
                            ask: false,
                        }),
                        cx,
                    );
                },
            );
        }
        _ => crate::view::widgets::open_url(url, cx),
    }
}
