//! The details of one object: the right dock panel (follows the list selection) and the
//! Details / Describe tabs.
//!
//! Summary: status pills (with +/− scaling for Deployments, StatefulSets and ReplicaSets),
//! owner chain, containers and ports (pods; ports forward with one click), usage, labels,
//! annotations,
//! conditions, related objects (selector → pods, Service → Endpoints, PVC → PV, Ingress →
//! Services/Secrets), kind-specific sections (Deployment rollout history, Node capacity and
//! taints) and recent events. Describe: `kubectl describe`-like text with events. YAML, Logs,
//! Terminal and Files are other crates' views, built through the [`ViewRegistry`].
//!
//! Related stores (events, pods, owners…) are acquired only after the selection has been
//! stable for [`SETTLE`], so scrolling through thousands of rows doesn't start watches.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use gpui::{
    AnyElement, AnyView, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle,
    Focusable, FontWeight, IntoElement, Render, SharedString, Subscription, Task, Window, div,
    prelude::*,
};
use kubyl_charts::Sparkline;
use kubyl_core::actions::{ForwardPort, OpenView, StopForward};
use kubyl_core::forwards::ActiveForwards;
use kubyl_core::{
    ChromeRegistry, ClusterCaps, ClusterId, DockPanel, DockPosition, Gvr, Notification,
    NotificationCenter, ResourceRef, TabHandle, TabView, Tone, ViewKind, ViewRegistry, ViewRequest,
};
use kubyl_kube::ConnectionManager;
use kubyl_resources::columns::{
    event_message, event_time, job_status, node_roles, node_status, pod_status, status_tone,
};
use kubyl_resources::format::{
    array_at, format_bytes, format_cpu, human_duration, int_at, map_pairs, object_age,
    parse_quantity, seconds_since, str_at, timestamp,
};
use kubyl_resources::metrics::{Metrics, SourceStatus};
use kubyl_resources::{
    ResourceSelection, ResourceStore, ResourceStores, StoreHandle, StoreKey, object_key,
};
use kubyl_ui::{
    ActiveColors, Chip, Colors, Icon, IconButton, IconName, StatusDot, StatusPill, fonts, h_flex,
    tone_color, u, v_flex,
};
use serde_json::Value;

use crate::catalog;
use crate::dialogs::{self, ConfirmSpec};

/// How long the selection must stay put before related objects are loaded.
const SETTLE: Duration = Duration::from_millis(250);
/// Clicks on +/− within this time become one scale request.
const SCALE_DEBOUNCE: Duration = Duration::from_millis(350);
/// Kinds with a `scale` subresource that the summary scales with +/−.
const SCALABLE: &[&str] = &["deployments", "statefulsets", "replicasets"];

/// What the details show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub cluster: ClusterId,
    pub gvr: Gvr,
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
}

impl Target {
    fn from_ref(target: &ResourceRef, kind: String) -> Option<Self> {
        Some(Self {
            cluster: target.cluster.clone(),
            gvr: target.gvr.clone(),
            kind,
            namespace: target.namespace.clone(),
            name: target.name.clone()?,
        })
    }

    fn key(&self) -> kubyl_resources::ObjectKey {
        object_key(self.namespace.as_deref(), &self.name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Summary,
    Describe,
    /// Inline YAML editor sub-tab (built on demand via [`ViewRegistry`]).
    Yaml,
    /// Inline logs sub-tab (built on demand via [`ViewRegistry`]).
    Logs,
    /// Inline exec terminal sub-tab (built on demand via [`ViewRegistry`]).
    Terminal,
    /// Inline file browser sub-tab (built on demand via [`ViewRegistry`]).
    Files,
}

/// Maps an extra sub-tab to the [`ViewKind`] it builds through the shared [`ViewRegistry`].
/// `Summary`/`Describe` aren't served this way, so they return `None`.
fn extra_view_kind(mode: Mode) -> Option<ViewKind> {
    match mode {
        Mode::Yaml => Some(ViewKind::Yaml),
        Mode::Logs => Some(ViewKind::Logs),
        Mode::Terminal => Some(ViewKind::Terminal),
        Mode::Files => Some(ViewKind::Files),
        Mode::Summary | Mode::Describe => None,
    }
}

/// Mirrors `kubyl_logs::logs_applicable` (`ShowLogs`): pods, and the workloads and Services
/// whose pod selector logs can follow.
fn logs_applicable(resource: &str) -> bool {
    matches!(
        resource,
        "pods"
            | "deployments"
            | "statefulsets"
            | "daemonsets"
            | "replicasets"
            | "jobs"
            | "services"
    )
}

/// Mirrors `kubyl_terminal`'s `ShowShell` availability: pods on a non-read-only cluster.
fn terminal_applicable(resource: &str, caps: &ClusterCaps) -> bool {
    resource == "pods" && !caps.read_only
}

/// Mirrors `kubyl_files`'s "Pod: Browse Files": pods on a non-read-only cluster.
fn files_applicable(resource: &str, caps: &ClusterCaps) -> bool {
    resource == "pods" && !caps.read_only
}

/// A scale request from the summary's +/− buttons, until the object shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PendingScale {
    replicas: i64,
}

/// One row of a Ports list.
struct PortRow {
    port: u16,
    /// `8080/TCP → 8080` or `8080/TCP`.
    label: String,
    /// The port name and, for pods, the container.
    detail: String,
    tcp: bool,
}

/// A cached extra sub-tab view, built once per target so switching tabs doesn't rebuild or
/// reconnect the underlying kube watch/exec/log-stream.
struct ExtraTab {
    handle: Box<dyn TabHandle>,
    _subscription: Subscription,
}

/// Stores for related objects, acquired once the selection settles.
#[derive(Default)]
struct Related {
    events: Option<StoreHandle>,
    pods: Option<StoreHandle>,
    replica_sets: Option<StoreHandle>,
    endpoints: Option<StoreHandle>,
    volume: Option<StoreHandle>,
    /// Stores of owners, one per level of the chain.
    owners: Vec<StoreHandle>,
    _observers: Vec<Subscription>,
}

/// The shared details renderer.
pub struct DetailsContent {
    target: Option<Target>,
    mode: Mode,
    object: Option<Arc<Value>>,
    /// The object no longer exists.
    gone: bool,
    source: Option<Entity<ResourceStore>>,
    own: Option<StoreHandle>,
    related: Related,
    settle: Option<Task<()>>,
    _source_observer: Option<Subscription>,
    /// Cached Yaml/Logs/Terminal sub-tab views, built lazily and kept until the target changes.
    extra: HashMap<Mode, ExtraTab>,
    /// Secret `data`/`stringData` keys whose decoded value is currently revealed.
    revealed: std::collections::HashSet<String>,
    scale_pending: Option<PendingScale>,
    scale_task: Option<Task<()>>,
    _forwards_observer: Subscription,
    _metrics_observer: Subscription,
    /// Sections other crates contribute (`ChromeRegistry::add_details_section`), built once per
    /// target.
    sections: Option<(Target, Vec<AnyView>)>,
}

impl DetailsContent {
    fn new(mode: Mode, cx: &mut Context<Self>) -> Self {
        Self {
            target: None,
            mode,
            object: None,
            gone: false,
            source: None,
            own: None,
            related: Related::default(),
            settle: None,
            _source_observer: None,
            extra: HashMap::new(),
            revealed: std::collections::HashSet::new(),
            scale_pending: None,
            scale_task: None,
            // Port rows show running forwards.
            _forwards_observer: cx.observe_global::<ActiveForwards>(|_, cx| cx.notify()),
            // Usage numbers and sparklines.
            _metrics_observer: cx.observe_global::<Metrics>(|_, cx| cx.notify()),
            sections: None,
        }
    }

    /// Shows `target`. `object`/`store` come from the list when available; otherwise the view
    /// watches the object itself.
    fn set_target(
        &mut self,
        target: Option<Target>,
        object: Option<Arc<Value>>,
        store: Option<Entity<ResourceStore>>,
        cx: &mut Context<Self>,
    ) {
        if target == self.target && store == self.source {
            if object.is_some() {
                self.object = object;
            }
            return;
        }
        let same_object = target == self.target;
        self.target = target.clone();
        self.object = object;
        self.gone = false;
        if !same_object {
            self.related = Related::default();
            self.extra.clear();
            self.sections = None;
            self.revealed.clear();
            self.scale_pending = None;
            self.scale_task = None;
            // Fall back to Summary if the new target doesn't offer the sub-tab that was open
            // (e.g. navigating from a Pod's Terminal tab to a ConfigMap).
            let mode_valid = match (&target, self.mode) {
                (_, Mode::Summary | Mode::Describe | Mode::Yaml) => target.is_some(),
                (None, _) => false,
                (Some(t), Mode::Logs) => logs_applicable(&t.gvr.resource),
                (Some(t), Mode::Terminal | Mode::Files) => {
                    let caps = ConnectionManager::try_global(cx)
                        .map(|m| m.read(cx).caps(&t.cluster))
                        .unwrap_or_default();
                    match self.mode {
                        Mode::Files => files_applicable(&t.gvr.resource, &caps),
                        _ => terminal_applicable(&t.gvr.resource, &caps),
                    }
                }
            };
            if !mode_valid {
                self.mode = Mode::Summary;
            }
        }
        self.own = None;
        self._source_observer = None;
        self.source = store.clone();
        let Some(target) = target else {
            self.settle = None;
            cx.notify();
            return;
        };
        let store = match store {
            Some(store) => store,
            None => {
                let key = StoreKey::new(
                    target.cluster.clone(),
                    target.gvr.clone(),
                    target.namespace.clone(),
                )
                .fields(format!("metadata.name={}", target.name));
                let handle = ResourceStores::acquire(cx, key);
                let entity = handle.entity().clone();
                self.own = Some(handle);
                entity
            }
        };
        self.source = Some(store.clone());
        self._source_observer = Some(cx.observe(&store, |this, store, cx| {
            let Some(target) = &this.target else {
                return;
            };
            let store = store.read(cx);
            match store.get(&target.key()) {
                Some(object) => {
                    this.object = Some(object.clone());
                    this.gone = false;
                }
                None if store.status().is_ready() => this.gone = true,
                None => {}
            }
            cx.notify();
        }));
        if self.object.is_none() {
            self.object = store.read(cx).get(&target.key()).cloned();
        }
        if !same_object {
            self.settle = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SETTLE).await;
                this.update(cx, |this, cx| this.load_related(cx)).ok();
            }));
        }
        cx.notify();
    }

    /// Sections from other crates for `target` (built when the target changes).
    fn contributed_sections(&mut self, target: &Target, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.sections.as_ref().map(|(t, _)| t) != Some(target) {
            let reference = ResourceRef::object(
                target.cluster.clone(),
                target.gvr.clone(),
                target.namespace.clone(),
                target.name.clone(),
            );
            let builders: Vec<_> = ChromeRegistry::global(cx).details_sections().to_vec();
            let views = builders
                .iter()
                .filter_map(|section| section.build(&reference, &target.kind, cx))
                .collect();
            self.sections = Some((target.clone(), views));
        }
        self.sections
            .as_ref()
            .map(|(_, views)| views.iter().map(|v| v.clone().into_any_element()).collect())
            .unwrap_or_default()
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }

    /// Builds (or returns the cached) view for an extra sub-tab (Yaml/Logs/Terminal), via the
    /// shared [`ViewRegistry`] rather than dispatching [`OpenView`] (which would open a
    /// separate top-level pane tab instead of rendering inline here).
    fn ensure_extra_view(
        &mut self,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyView> {
        if let Some(existing) = self.extra.get(&mode) {
            return Some(existing.handle.to_any_view());
        }
        let kind = extra_view_kind(mode)?;
        let target = self.target.as_ref()?;
        let reference = ResourceRef::object(
            target.cluster.clone(),
            target.gvr.clone(),
            target.namespace.clone(),
            target.name.clone(),
        );
        let request = ViewRequest::for_resource(kind, reference);
        let handle = ViewRegistry::build(&request, window, cx)?;
        let weak = cx.entity().downgrade();
        let subscription = handle.observe(
            cx,
            Box::new(move |cx| {
                weak.update(cx, |_, cx| cx.notify()).ok();
            }),
        );
        let view = handle.to_any_view();
        self.extra.insert(
            mode,
            ExtraTab {
                handle,
                _subscription: subscription,
            },
        );
        Some(view)
    }

    fn observe_store(&mut self, handle: &StoreHandle, cx: &mut Context<Self>) {
        self.related
            ._observers
            .push(cx.observe(handle.entity(), |_, _, cx| cx.notify()));
    }

    fn acquire(&mut self, key: StoreKey, cx: &mut Context<Self>) -> StoreHandle {
        let handle = ResourceStores::acquire(cx, key);
        self.observe_store(&handle, cx);
        handle
    }

    /// Acquires the stores for related objects of the current target.
    fn load_related(&mut self, cx: &mut Context<Self>) {
        let (Some(target), Some(object)) = (self.target.clone(), self.object.clone()) else {
            return;
        };
        let cluster = target.cluster.clone();
        let ns = target.namespace.clone();
        let core = |resource: &str| Gvr::new("", "v1", resource);

        // Events about the object.
        let events = match &ns {
            Some(ns) => StoreKey::new(cluster.clone(), core("events"), Some(ns.clone())).fields(
                format!("involvedObject.uid={}", str_at(&object, "/metadata/uid")),
            ),
            None => StoreKey::new(cluster.clone(), core("events"), None).fields(format!(
                "involvedObject.kind={},involvedObject.name={}",
                target.kind, target.name
            )),
        };
        self.related.events = Some(self.acquire(events, cx));

        // Pods selected by the object.
        if selector_of(&target.kind, &object).is_some()
            && let Some(ns) = &ns
        {
            let pods = StoreKey::new(cluster.clone(), core("pods"), Some(ns.clone()));
            self.related.pods = Some(self.acquire(pods, cx));
        }
        match target.kind.as_str() {
            "Deployment" => {
                let key = StoreKey::new(
                    cluster.clone(),
                    Gvr::new("apps", "v1", "replicasets"),
                    ns.clone(),
                );
                self.related.replica_sets = Some(self.acquire(key, cx));
            }
            "Service" => {
                let key = StoreKey::new(cluster.clone(), core("endpoints"), ns.clone())
                    .fields(format!("metadata.name={}", target.name));
                self.related.endpoints = Some(self.acquire(key, cx));
            }
            "PersistentVolumeClaim" => {
                let volume = str_at(&object, "/spec/volumeName");
                if !volume.is_empty() {
                    let key = StoreKey::new(cluster.clone(), core("persistentvolumes"), None)
                        .fields(format!("metadata.name={volume}"));
                    self.related.volume = Some(self.acquire(key, cx));
                }
            }
            _ => {}
        }
        self.load_owner_level(0, cx);
        cx.notify();
    }

    /// Watches the owner at `level` of the chain (metadata only) to find the next owner.
    fn load_owner_level(&mut self, level: usize, cx: &mut Context<Self>) {
        if level >= 3 {
            return;
        }
        let chain = self.owner_chain(cx);
        let Some(owner) = chain.get(level) else {
            return;
        };
        let Some(gvr) = owner.gvr.clone() else {
            return;
        };
        let Some(target) = &self.target else {
            return;
        };
        let key = StoreKey::new(target.cluster.clone(), gvr, target.namespace.clone()).metadata();
        let handle = ResourceStores::acquire(cx, key);
        self.related
            ._observers
            .push(cx.observe(handle.entity(), move |this, _, cx| {
                if this.related.owners.len() == level + 1 {
                    this.load_owner_level(level + 1, cx);
                }
                cx.notify();
            }));
        self.related.owners.push(handle);
    }

    /// The owner chain from the direct owner upwards: `[ReplicaSet x, Deployment y]`.
    fn owner_chain(&self, cx: &App) -> Vec<Owner> {
        let Some(object) = &self.object else {
            return Vec::new();
        };
        let Some(target) = &self.target else {
            return Vec::new();
        };
        let discovery =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).discovery(&target.cluster));
        let mut chain = Vec::new();
        let mut current = object.clone();
        for level in 0..3 {
            let Some(reference) = array_at(&current, "/metadata/ownerReferences")
                .iter()
                .find(|o| o["controller"].as_bool() == Some(true))
                .or_else(|| array_at(&current, "/metadata/ownerReferences").first())
                .cloned()
            else {
                break;
            };
            let kind = str_at(&reference, "/kind").to_string();
            let name = str_at(&reference, "/name").to_string();
            let (group, version) = match str_at(&reference, "/apiVersion").split_once('/') {
                Some((g, v)) => (g.to_string(), v.to_string()),
                None => (String::new(), str_at(&reference, "/apiVersion").to_string()),
            };
            let gvr = discovery.as_ref().and_then(|d| {
                d.by_gvk(&kubyl_core::Gvk::new(
                    group.clone(),
                    version.clone(),
                    kind.clone(),
                ))
                .map(|r| r.gvr.clone())
            });
            chain.push(Owner {
                kind,
                name: name.clone(),
                gvr: gvr.clone(),
            });
            let next = self.related.owners.get(level).and_then(|store| {
                store
                    .read(cx)
                    .get(&object_key(target.namespace.as_deref(), &name))
                    .cloned()
            });
            match next {
                Some(next) => current = next,
                None => break,
            }
        }
        chain
    }

    fn title(&self) -> SharedString {
        match &self.target {
            Some(target) => format!("{} details", target.kind).into(),
            None => "Details".into(),
        }
    }
}

#[derive(Clone, Debug)]
struct Owner {
    kind: String,
    name: String,
    gvr: Option<Gvr>,
}

/// The label selector of workloads and Services (`matchLabels` only for Services).
fn selector_of(kind: &str, object: &Value) -> Option<Vec<(String, String)>> {
    let map = match kind {
        "Service" => object.pointer("/spec/selector"),
        "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet" | "Job" => {
            object.pointer("/spec/selector/matchLabels")
        }
        _ => None,
    }?;
    let pairs: Vec<(String, String)> = map
        .as_object()?
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
        .collect();
    (!pairs.is_empty()).then_some(pairs)
}

fn matches_selector(object: &Value, selector: &[(String, String)]) -> bool {
    let labels = &object["metadata"]["labels"];
    selector
        .iter()
        .all(|(k, v)| labels[k.as_str()].as_str() == Some(v.as_str()))
}

fn open_details(target: ResourceRef, window: &mut Window, cx: &mut App) {
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(
            ViewKind::Details,
            target,
        ))),
        cx,
    );
}

// ----- Rendering -----

fn section(title: impl Into<SharedString>, colors: &Colors) -> gpui::Div {
    let title: SharedString = title.into();
    v_flex()
        .px(u(14.0))
        .py(u(12.0))
        .gap(u(8.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(
            div()
                .text_size(u(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_dim)
                .child(title.to_uppercase()),
        )
}

fn kv(rows: Vec<(&'static str, String)>, colors: &Colors) -> impl IntoElement {
    v_flex()
        .gap(u(5.0))
        .text_size(u(12.0))
        .children(rows.into_iter().map(|(k, v)| {
            h_flex()
                .gap(u(8.0))
                .child(
                    div()
                        .flex_none()
                        .w(u(104.0))
                        .text_color(colors.text_dim)
                        .child(k),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(colors.text)
                        .child(v),
                )
        }))
}

fn chips(items: Vec<String>, mono: bool) -> impl IntoElement {
    h_flex()
        .flex_wrap()
        .gap(u(4.0))
        .children(items.into_iter().map(move |item| {
            let chip = Chip::new(item);
            if mono { chip.mono() } else { chip }
        }))
}

fn link(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    target: ResourceRef,
    colors: &Colors,
) -> impl IntoElement {
    let id: SharedString = id.into();
    div()
        .id(id)
        .cursor_pointer()
        .text_color(colors.accent)
        .hover(|s| s.underline())
        .child(label.into())
        .on_click(move |_, window, cx| open_details(target.clone(), window, cx))
}

/// Wraps an element in a tooltip (the kubyl_ui buttons have none).
fn tooltip_wrap(id: &'static str, text: &'static str, child: impl IntoElement) -> impl IntoElement {
    div()
        .id(id)
        .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(text).build(window, cx))
        .child(child)
}

/// A compact bordered button for detail rows (the regular [`kubyl_ui::Button`] is taller).
fn row_button(
    id: impl Into<SharedString>,
    icon: IconName,
    label: impl Into<SharedString>,
    colors: &Colors,
) -> gpui::Stateful<gpui::Div> {
    let hover = colors.hover;
    h_flex()
        .id(id.into())
        .flex_none()
        .h(u(20.0))
        .px(u(6.0))
        .gap(u(4.0))
        .rounded(u(4.0))
        .border_1()
        .border_color(colors.border)
        .text_size(u(11.5))
        .text_color(colors.text_muted)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(Icon::new(icon).size(11.0))
        .child(label.into())
}

fn protocol(port: &Value) -> &str {
    match str_at(port, "/protocol") {
        "" => "TCP",
        p => p,
    }
}

/// Container ports of a pod, per container.
fn pod_ports(pod: &Value) -> Vec<PortRow> {
    let mut rows = Vec::new();
    for container in array_at(pod, "/spec/containers") {
        for port in array_at(container, "/ports") {
            let number = int_at(port, "/containerPort");
            let Ok(number) = u16::try_from(number) else {
                continue;
            };
            let name = str_at(port, "/name");
            let container = str_at(container, "/name");
            rows.push(PortRow {
                port: number,
                label: format!("{number}/{}", protocol(port)),
                detail: if name.is_empty() {
                    container.to_string()
                } else {
                    format!("{name} · {container}")
                },
                tcp: protocol(port) == "TCP",
            });
        }
    }
    rows
}

/// A Service's ports: `8080/TCP → 8080`.
fn service_ports(svc: &Value) -> Vec<PortRow> {
    array_at(svc, "/spec/ports")
        .iter()
        .filter_map(|port| {
            let number = u16::try_from(int_at(port, "/port")).ok()?;
            let target = port["targetPort"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| match &port["targetPort"] {
                    Value::Null => number.to_string(),
                    other => other.to_string(),
                });
            Some(PortRow {
                port: number,
                label: format!("{number}/{} → {target}", protocol(port)),
                detail: str_at(port, "/name").to_string(),
                tcp: protocol(port) == "TCP",
            })
        })
        .collect()
}

/// Placeholder shown for a masked Secret value, conceptually like `kubyl_yaml::render::MASK`
/// (not imported: replicating a one-line constant isn't worth a cross-crate dependency).
const SECRET_MASK: &str = "••••••••";

/// Decodes one Secret `data`/`stringData` entry into plain text, never raw base64. `data`
/// values are base64-encoded; `stringData` values are already plain text. Falls back to a
/// byte count when the decoded bytes aren't valid UTF-8, or when the value isn't valid base64
/// at all.
fn decode_secret_value(raw: &str, is_string_data: bool) -> String {
    if is_string_data {
        return raw.to_string();
    }
    match base64::engine::general_purpose::STANDARD.decode(raw) {
        Ok(bytes) => {
            let len = bytes.len();
            String::from_utf8(bytes).unwrap_or_else(|_| format!("<binary, {len} bytes>"))
        }
        Err(_) => "<invalid base64>".to_string(),
    }
}

fn container_state(status: Option<&Value>, now: jiff::Timestamp) -> (String, Tone) {
    let Some(status) = status else {
        return ("waiting".into(), Tone::Warning);
    };
    if let Some(running) = status.pointer("/state/running") {
        let since = timestamp(str_at(running, "/startedAt"))
            .map(|t| format!(" · {}", human_duration(seconds_since(t, now))))
            .unwrap_or_default();
        let tone = if status["ready"].as_bool() == Some(true) {
            Tone::Good
        } else {
            Tone::Warning
        };
        return (format!("running{since}"), tone);
    }
    if let Some(waiting) = status.pointer("/state/waiting") {
        let reason = str_at(waiting, "/reason");
        return (reason.to_string(), status_tone(reason));
    }
    if let Some(terminated) = status.pointer("/state/terminated") {
        let reason = str_at(terminated, "/reason");
        let tone = if terminated["exitCode"].as_i64() == Some(0) {
            Tone::Muted
        } else {
            Tone::Bad
        };
        return (format!("terminated · {reason}"), tone);
    }
    ("unknown".into(), Tone::Neutral)
}

impl DetailsContent {
    fn render_summary(
        &mut self,
        object: &Value,
        target: &Target,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let now = jiff::Timestamp::now();
        let mut out: Vec<AnyElement> = Vec::new();

        // Header: name and status pills.
        let mut pills = h_flex().flex_wrap().gap(u(6.0));
        match target.kind.as_str() {
            "Pod" => {
                let status = pod_status(object);
                let tone = status.tone();
                pills = pills.child(StatusPill::new(status.reason.clone(), tone));
                if let Some(qos) = object.pointer("/status/qosClass").and_then(Value::as_str) {
                    pills = pills.child(Chip::new(qos.to_string()));
                }
                if let Some(ip) = object.pointer("/status/podIP").and_then(Value::as_str) {
                    pills = pills.child(Chip::new(ip.to_string()));
                }
            }
            "Node" => {
                let status = node_status(object);
                let tone = if status.starts_with("Ready") {
                    Tone::Good
                } else {
                    Tone::Bad
                };
                pills = pills.child(StatusPill::new(status, tone));
                for role in node_roles(object) {
                    pills = pills.child(Chip::new(role));
                }
            }
            "Job" => {
                let status = job_status(object);
                pills = pills.child(StatusPill::new(status, status_tone(status)));
            }
            "Deployment" | "StatefulSet" | "ReplicaSet" => {
                let ready = int_at(object, "/status/readyReplicas");
                let desired = int_at(object, "/spec/replicas");
                if self.scale_pending.is_some_and(|p| p.replicas == desired) {
                    self.scale_pending = None;
                }
                let tone = if ready >= desired {
                    Tone::Good
                } else {
                    Tone::Warning
                };
                pills = pills.child(StatusPill::new(format!("{ready}/{desired} ready"), tone));
                if self.can_scale(target, cx) {
                    let step = |id: &'static str, icon: IconName, tip: &'static str| {
                        tooltip_wrap(
                            id,
                            tip,
                            h_flex()
                                .id(SharedString::from(format!("{id}-button")))
                                .size(u(20.0))
                                .justify_center()
                                .rounded(u(4.0))
                                .border_1()
                                .border_color(colors.border)
                                .cursor_pointer()
                                .hover(|s| s.bg(colors.hover))
                                .child(Icon::new(icon).size(11.0).color(colors.text_muted)),
                        )
                    };
                    pills = pills.child(
                        h_flex()
                            .gap(u(3.0))
                            .child(
                                div()
                                    .id("scale-down")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.scale_by(-1, window, cx)
                                    }))
                                    .child(step(
                                        "scale-down-tip",
                                        IconName::Minus,
                                        "Scale down by one",
                                    )),
                            )
                            .child(
                                div()
                                    .id("scale-up")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.scale_by(1, window, cx)
                                    }))
                                    .child(step("scale-up-tip", IconName::Plus, "Scale up by one")),
                            ),
                    );
                }
                if let Some(pending) = self.scale_pending {
                    pills = pills.child(
                        Chip::new(format!("scaling to {}", pending.replicas)).dot(colors.accent),
                    );
                }
            }
            _ => {
                if let Some(phase) = object.pointer("/status/phase").and_then(Value::as_str) {
                    pills = pills.child(StatusPill::new(phase.to_string(), status_tone(phase)));
                }
            }
        }
        pills = pills.child(Chip::new(format!("age {}", object_age(object, now))));
        out.push(
            v_flex()
                .px(u(14.0))
                .py(u(12.0))
                .gap(u(6.0))
                .border_b_1()
                .border_color(colors.border_variant)
                .child(
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(12.5))
                        .text_color(if self.gone {
                            colors.text_dim
                        } else {
                            colors.text
                        })
                        .child(target.name.clone()),
                )
                .when(self.gone, |this| {
                    this.child(
                        div()
                            .text_size(u(12.0))
                            .text_color(colors.red)
                            .child("Deleted"),
                    )
                })
                .when_some(target.namespace.clone(), |this, ns| {
                    this.child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(format!("namespace {ns}")),
                    )
                })
                .child(pills)
                .into_any_element(),
        );

        // Owner chain.
        let chain = self.owner_chain(cx);
        if !chain.is_empty() {
            let mut row = h_flex().flex_wrap().gap(u(6.0)).text_size(u(12.0));
            for (ix, owner) in chain.iter().rev().enumerate() {
                if ix > 0 {
                    row = row.child(div().text_color(colors.text_faint).child("›"));
                }
                match &owner.gvr {
                    Some(gvr) => {
                        let reference = ResourceRef::object(
                            target.cluster.clone(),
                            gvr.clone(),
                            target.namespace.clone(),
                            owner.name.clone(),
                        );
                        row = row.child(
                            h_flex()
                                .gap(u(4.0))
                                .child(div().text_color(colors.text_dim).child(owner.kind.clone()))
                                .child(div().font_family(fonts::MONO).text_size(u(11.5)).child(
                                    link(
                                        SharedString::from(format!("owner-{ix}")),
                                        owner.name.clone(),
                                        reference,
                                        &colors,
                                    ),
                                )),
                        );
                    }
                    None => {
                        row = row.child(format!("{} {}", owner.kind, owner.name));
                    }
                }
            }
            row = row
                .child(div().text_color(colors.text_faint).child("›"))
                .child(
                    div()
                        .text_color(colors.text_muted)
                        .child(target.kind.clone()),
                );
            out.push(
                section("Owner chain", &colors)
                    .child(row)
                    .into_any_element(),
            );
        }

        match target.kind.as_str() {
            "Pod" => out.extend(self.render_pod(object, target, &colors, cx)),
            "Deployment" => out.extend(self.render_deployment(object, target, &colors, cx)),
            "Node" => out.extend(self.render_node(object, &colors, cx)),
            "Service" => out.extend(self.render_service(object, target, &colors, cx)),
            "PersistentVolumeClaim" => out.extend(self.render_pvc(object, target, &colors, cx)),
            "Ingress" => out.extend(self.render_ingress(object, target, &colors)),
            "Secret" => out.extend(self.render_secret(object, &colors, cx)),
            _ => {}
        }
        out.extend(self.contributed_sections(target, cx));

        // Pods selected by workloads (not Deployments: their pods show via ReplicaSets too).
        if let (Some(selector), Some(pods), Some(ns)) = (
            selector_of(&target.kind, object),
            self.related.pods.as_ref(),
            target.namespace.clone(),
        ) {
            let store = pods.read(cx);
            let mut matching: Vec<&Arc<Value>> = store
                .objects()
                .values()
                .filter(|p| matches_selector(p, &selector))
                .collect();
            matching.sort_by(|a, b| str_at(a, "/metadata/name").cmp(str_at(b, "/metadata/name")));
            let mut list = v_flex().gap(u(4.0)).text_size(u(12.0));
            for (ix, pod) in matching.iter().take(12).enumerate() {
                let status = pod_status(pod);
                let name = str_at(pod, "/metadata/name").to_string();
                let reference = ResourceRef::object(
                    target.cluster.clone(),
                    Gvr::new("", "v1", "pods"),
                    Some(ns.clone()),
                    name.clone(),
                );
                list = list.child(
                    h_flex()
                        .gap(u(8.0))
                        .child(StatusDot::new(tone_color(status.tone(), &colors)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(fonts::MONO)
                                .text_size(u(11.5))
                                .child(link(
                                    SharedString::from(format!("pod-{ix}")),
                                    name,
                                    reference,
                                    &colors,
                                )),
                        )
                        .child(
                            div()
                                .text_color(tone_color(status.tone(), &colors))
                                .child(status.reason),
                        ),
                );
            }
            if matching.len() > 12 {
                list = list.child(
                    div()
                        .text_color(colors.text_dim)
                        .child(format!("… {} more", matching.len() - 12)),
                );
            }
            if matching.is_empty() {
                list = list.child(
                    div()
                        .text_color(colors.text_dim)
                        .child("No pods match the selector."),
                );
            }
            out.push(
                section(format!("Pods · {}", matching.len()), &colors)
                    .child(list)
                    .into_any_element(),
            );
        }

        // Labels, annotations, conditions.
        let labels = map_pairs(object.pointer("/metadata/labels"));
        if !labels.is_empty() {
            out.push(
                section("Labels", &colors)
                    .child(chips(labels, true))
                    .into_any_element(),
            );
        }
        let annotations: Vec<String> = map_pairs(object.pointer("/metadata/annotations"))
            .into_iter()
            .filter(|a| !a.starts_with("kubectl.kubernetes.io/last-applied-configuration"))
            .map(|a| {
                if a.chars().count() > 60 {
                    format!("{}…", a.chars().take(60).collect::<String>())
                } else {
                    a
                }
            })
            .collect();
        if !annotations.is_empty() {
            out.push(
                section(format!("Annotations · {}", annotations.len()), &colors)
                    .child(chips(annotations.into_iter().take(8).collect(), true))
                    .into_any_element(),
            );
        }
        let conditions = array_at(object, "/status/conditions");
        if !conditions.is_empty() {
            let mut grid = h_flex().flex_wrap().gap(u(4.0)).text_size(u(12.0));
            for condition in conditions {
                let ok = str_at(condition, "/status") == "True";
                let kind = str_at(condition, "/type").to_string();
                // Node pressure conditions are healthy when False.
                let healthy = if target.kind == "Node" && kind != "Ready" {
                    !ok
                } else {
                    ok
                };
                grid = grid.child(
                    h_flex()
                        .w(u(150.0))
                        .gap(u(6.0))
                        .child(
                            Icon::new(if healthy {
                                IconName::CircleCheck
                            } else {
                                IconName::CircleX
                            })
                            .size(12.0)
                            .color(if healthy {
                                colors.green
                            } else {
                                colors.red
                            }),
                        )
                        .child(div().truncate().child(kind)),
                );
            }
            out.push(
                section("Conditions", &colors)
                    .child(grid)
                    .into_any_element(),
            );
        }

        // Recent events.
        if let Some(events) = &self.related.events {
            let store = events.read(cx);
            let mut events: Vec<&Arc<Value>> = store.objects().values().collect();
            events.sort_by_key(|e| std::cmp::Reverse(event_time(e)));
            if !events.is_empty() {
                let mut list = v_flex().gap(u(6.0)).text_size(u(12.0));
                for event in events.iter().take(6) {
                    let warning = str_at(event, "/type") == "Warning";
                    let age = event_time(event)
                        .map(|t| human_duration(seconds_since(t, now)))
                        .unwrap_or_default();
                    list = list.child(
                        v_flex()
                            .child(
                                h_flex()
                                    .gap(u(6.0))
                                    .child(StatusDot::new(if warning {
                                        colors.yellow
                                    } else {
                                        colors.text_dim
                                    }))
                                    .child(
                                        div()
                                            .text_color(if warning {
                                                colors.yellow
                                            } else {
                                                colors.text
                                            })
                                            .child(str_at(event, "/reason").to_string()),
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        div()
                                            .text_color(colors.text_dim)
                                            .font_family(fonts::MONO)
                                            .text_size(u(11.0))
                                            .child(age),
                                    ),
                            )
                            .child(
                                div()
                                    .pl(u(13.0))
                                    .text_color(colors.text_muted)
                                    .child(event_message(event).to_string()),
                            ),
                    );
                }
                out.push(section("Events", &colors).child(list).into_any_element());
            }
        }
        out
    }

    fn render_pod(
        &self,
        pod: &Value,
        _: &Target,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let now = jiff::Timestamp::now();
        let mut out = Vec::new();
        let statuses = array_at(pod, "/status/containerStatuses");
        let mut list = v_flex().gap(u(8.0));
        for container in array_at(pod, "/spec/containers") {
            let name = str_at(container, "/name");
            let status = statuses.iter().find(|s| str_at(s, "/name") == name);
            let (state, tone) = container_state(status, now);
            let color = tone_color(tone, colors);
            let resources: Vec<String> = ["requests", "limits"]
                .iter()
                .filter_map(|kind| {
                    let cpu = str_at(container, &format!("/resources/{kind}/cpu"));
                    let memory = str_at(container, &format!("/resources/{kind}/memory"));
                    (!cpu.is_empty() || !memory.is_empty()).then(|| {
                        format!(
                            "{} {}",
                            &kind[..3],
                            [cpu, memory]
                                .iter()
                                .filter(|v| !v.is_empty())
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(" / ")
                        )
                    })
                })
                .collect();
            let probes: Vec<&str> = [
                ("livenessProbe", "liveness"),
                ("readinessProbe", "readiness"),
                ("startupProbe", "startup"),
            ]
            .iter()
            .filter(|(key, _)| container.get(*key).is_some())
            .map(|(_, label)| *label)
            .collect();
            let restarts = status
                .map(|s| int_at(s, "/restartCount"))
                .unwrap_or_default();
            list = list.child(
                h_flex()
                    .items_start()
                    .gap(u(8.0))
                    .child(
                        div()
                            .pt(u(2.0))
                            .child(Icon::new(IconName::Box).color(color)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .justify_between()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(name.to_string()),
                                    )
                                    .child(div().text_size(u(12.0)).text_color(color).child(state)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .font_family(fonts::MONO)
                                    .text_size(u(11.0))
                                    .text_color(colors.text_dim)
                                    .child(str_at(container, "/image").to_string()),
                            )
                            .when(
                                !resources.is_empty() || !probes.is_empty() || restarts > 0,
                                |this| {
                                    let mut parts = resources.clone();
                                    if !probes.is_empty() {
                                        parts.push(format!("probes: {}", probes.join(", ")));
                                    }
                                    if restarts > 0 {
                                        parts.push(format!("{restarts} restarts"));
                                    }
                                    this.child(
                                        div()
                                            .text_size(u(11.5))
                                            .text_color(colors.text_dim)
                                            .child(parts.join(" · ")),
                                    )
                                },
                            ),
                    ),
            );
        }
        out.push(section("Containers", colors).child(list).into_any_element());
        let ports = pod_ports(pod);
        if !ports.is_empty() {
            out.push(
                section("Ports", colors)
                    .child(self.port_list(ports, None, colors, cx))
                    .into_any_element(),
            );
        }

        out.push(self.render_pod_usage(pod, colors, cx));
        out
    }

    fn render_deployment(
        &self,
        d: &Value,
        target: &Target,
        colors: &Colors,
        cx: &App,
    ) -> Vec<AnyElement> {
        let strategy = match str_at(d, "/spec/strategy/type") {
            "" => "RollingUpdate",
            s => s,
        };
        let mut rows = vec![
            (
                "Replicas",
                format!(
                    "{} desired · {} updated · {} available",
                    int_at(d, "/spec/replicas"),
                    int_at(d, "/status/updatedReplicas"),
                    int_at(d, "/status/availableReplicas")
                ),
            ),
            ("Strategy", strategy.to_string()),
        ];
        if strategy == "RollingUpdate" {
            let value = |p: &str| {
                d.pointer(p)
                    .map(|v| v.as_str().map(String::from).unwrap_or(v.to_string()))
                    .unwrap_or_else(|| "25%".into())
            };
            rows.push((
                "Surge / unavail.",
                format!(
                    "{} / {}",
                    value("/spec/strategy/rollingUpdate/maxSurge"),
                    value("/spec/strategy/rollingUpdate/maxUnavailable")
                ),
            ));
        }
        if d.pointer("/spec/paused").and_then(Value::as_bool) == Some(true) {
            rows.push(("Rollout", "paused".into()));
        }
        let mut out = vec![
            section("Deployment", colors)
                .child(kv(rows, colors))
                .into_any_element(),
        ];
        if let Some(store) = &self.related.replica_sets {
            let uid = str_at(d, "/metadata/uid");
            let current = str_at(
                d,
                "/metadata/annotations/deployment.kubernetes.io~1revision",
            );
            let mut revisions: Vec<&Arc<Value>> = store
                .read(cx)
                .objects()
                .values()
                .filter(|rs| {
                    array_at(rs, "/metadata/ownerReferences")
                        .iter()
                        .any(|o| str_at(o, "/uid") == uid)
                })
                .collect();
            let revision = |rs: &Value| -> i64 {
                str_at(
                    rs,
                    "/metadata/annotations/deployment.kubernetes.io~1revision",
                )
                .parse()
                .unwrap_or_default()
            };
            revisions.sort_by_key(|rs| std::cmp::Reverse(revision(rs)));
            let mut list = v_flex().gap(u(4.0)).text_size(u(12.0));
            for (ix, rs) in revisions.iter().take(8).enumerate() {
                let number = revision(rs);
                let is_current = number.to_string() == current;
                let images: Vec<&str> = array_at(rs, "/spec/template/spec/containers")
                    .iter()
                    .map(|c| str_at(c, "/image"))
                    .collect();
                let name = str_at(rs, "/metadata/name").to_string();
                let reference = ResourceRef::object(
                    target.cluster.clone(),
                    Gvr::new("apps", "v1", "replicasets"),
                    target.namespace.clone(),
                    name.clone(),
                );
                list = list.child(
                    h_flex()
                        .gap(u(8.0))
                        .child(
                            div()
                                .w(u(34.0))
                                .font_family(fonts::MONO)
                                .text_color(if is_current {
                                    colors.green
                                } else {
                                    colors.text_dim
                                })
                                .child(format!("#{number}")),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().font_family(fonts::MONO).text_size(u(11.5)).child(
                                    link(
                                        SharedString::from(format!("rs-{ix}")),
                                        name,
                                        reference,
                                        colors,
                                    ),
                                ))
                                .child(
                                    div()
                                        .truncate()
                                        .font_family(fonts::MONO)
                                        .text_size(u(11.0))
                                        .text_color(colors.text_dim)
                                        .child(images.join(", ")),
                                ),
                        )
                        .child(div().text_color(colors.text_dim).child(format!(
                            "{}/{}",
                            int_at(rs, "/status/readyReplicas"),
                            int_at(rs, "/spec/replicas")
                        ))),
                );
            }
            if !revisions.is_empty() {
                out.push(
                    section("Rollout history", colors)
                        .child(list)
                        .into_any_element(),
                );
            }
        }
        out
    }

    /// Usage of the pod (board 1): current CPU and memory against requests and limits, with
    /// sparklines when the provider has history.
    fn render_pod_usage(&self, pod: &Value, colors: &Colors, cx: &App) -> AnyElement {
        let Some(target) = self.target.as_ref() else {
            return div().into_any_element();
        };
        let namespace = target.namespace.as_deref().unwrap_or_default();
        let provider = Metrics::provider(cx);
        let status = provider
            .as_ref()
            .map(|p| p.source_status(&target.cluster, cx));
        let (usage, history) = match &provider {
            Some(p) => (
                p.pod_usage(&target.cluster, namespace, &target.name, cx),
                p.pod_history(&target.cluster, namespace, &target.name, cx),
            ),
            None => (None, None),
        };
        let resource = |kind: &str, resource: &str| {
            kubyl_resources::metrics::pod_resource(pod, kind, resource)
        };
        // `USAGE · LAST 1H · Prometheus`
        let mut title = "Usage".to_string();
        if let Some(window) = history
            .as_ref()
            .map(|h| h.window.clone())
            .filter(|w| !w.is_empty())
        {
            title.push_str(&format!(" · {window}"));
        }
        let source = history
            .as_ref()
            .map(|h| h.source.clone())
            .or_else(|| match &status {
                Some(SourceStatus::Ready(label)) => Some(label.clone()),
                _ => None,
            });
        let header = h_flex()
            .gap(u(4.0))
            .text_size(u(11.0))
            .text_color(colors.text_dim)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.to_uppercase()),
            )
            .when_some(source, |this, source| {
                this.child(div().child(format!("· {source}")))
            });
        let body = match (usage, &status) {
            (Some(usage), _) => {
                let row = |label: &'static str,
                           value: String,
                           detail: String,
                           samples: Option<Vec<f64>>,
                           color| {
                    v_flex()
                        .gap(u(2.0))
                        .child(
                            h_flex()
                                .justify_between()
                                .text_size(u(12.0))
                                .child(div().text_color(colors.text_muted).child(label))
                                .child(
                                    h_flex()
                                        .font_family(fonts::MONO)
                                        .text_size(u(11.5))
                                        .child(value)
                                        .child(div().text_color(colors.text_dim).child(detail)),
                                ),
                        )
                        .when_some(samples.filter(|s| s.len() >= 2), |this, samples| {
                            this.child(Sparkline::new(samples, color).height(34.0))
                        })
                };
                let limits = |resource_name: &str, format: fn(f64) -> String| {
                    let parts: Vec<String> = [("requests", "req"), ("limits", "lim")]
                        .iter()
                        .filter_map(|(kind, short)| {
                            resource(kind, resource_name).map(|v| format!("{short} {}", format(v)))
                        })
                        .collect();
                    if parts.is_empty() {
                        String::new()
                    } else {
                        format!(" / {}", parts.join(" · "))
                    }
                };
                v_flex()
                    .gap(u(10.0))
                    .child(row(
                        "CPU",
                        format_cpu(usage.cpu),
                        limits("cpu", format_cpu),
                        history.as_ref().map(|h| h.cpu.clone()),
                        colors.accent,
                    ))
                    .child(row(
                        "Memory",
                        format_bytes(usage.memory),
                        limits("memory", format_bytes),
                        history.as_ref().map(|h| h.memory.clone()),
                        colors.purple,
                    ))
                    .into_any_element()
            }
            (None, status) => {
                let running = str_at(pod, "/status/phase") == "Running";
                let text: SharedString = match status {
                    None => "No metrics provider.".into(),
                    Some(SourceStatus::Detecting) => {
                        "Looking for metrics-server or Prometheus…".into()
                    }
                    Some(SourceStatus::Unavailable(reason)) => reason.clone(),
                    Some(SourceStatus::Ready(_)) if !running => "Not running.".into(),
                    Some(SourceStatus::Ready(_)) => "Waiting for the first sample…".into(),
                };
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(text)
                    .into_any_element()
            }
        };
        v_flex()
            .px(u(14.0))
            .py(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// Current usage of a node against its allocatable capacity.
    fn render_node_usage(&self, node: &Value, colors: &Colors, cx: &App) -> Option<AnyElement> {
        let target = self.target.as_ref()?;
        let usage = Metrics::provider(cx)?.node_usage(&target.cluster, &target.name, cx)?;
        let row = |label: &'static str,
                   used: f64,
                   allocatable: Option<f64>,
                   format: fn(f64) -> String| {
            let percent = allocatable
                .filter(|a| *a > 0.0)
                .map(|a| (used / a * 100.0) as f32);
            v_flex()
                .gap(u(3.0))
                .child(
                    h_flex()
                        .justify_between()
                        .text_size(u(12.0))
                        .child(div().text_color(colors.text_muted).child(label))
                        .child(
                            h_flex()
                                .font_family(fonts::MONO)
                                .text_size(u(11.5))
                                .child(format(used))
                                .when_some(allocatable, |this, a| {
                                    this.child(
                                        div()
                                            .text_color(colors.text_dim)
                                            .child(format!(" / {}", format(a))),
                                    )
                                })
                                .when_some(percent, |this, p| {
                                    this.child(
                                        div()
                                            .text_color(colors.text_dim)
                                            .child(format!(" · {p:.0}%")),
                                    )
                                }),
                        ),
                )
                .when_some(percent, |this, p| this.child(kubyl_ui::ProgressBar::new(p)))
        };
        let allocatable =
            |key: &str| parse_quantity(str_at(node, &format!("/status/allocatable/{key}")));
        Some(
            section("Usage", colors)
                .child(
                    v_flex()
                        .gap(u(10.0))
                        .child(row("CPU", usage.cpu, allocatable("cpu"), format_cpu))
                        .child(row(
                            "Memory",
                            usage.memory,
                            allocatable("memory"),
                            format_bytes,
                        )),
                )
                .into_any_element(),
        )
    }

    fn render_node(&self, node: &Value, colors: &Colors, cx: &App) -> Vec<AnyElement> {
        let quantity = |pointer: &str, cpu: bool| {
            let raw = str_at(node, pointer);
            match parse_quantity(raw) {
                Some(value) if cpu => format_cpu(value),
                Some(value) => format_bytes(value),
                None => raw.to_string(),
            }
        };
        let info = |key: &str| str_at(node, &format!("/status/nodeInfo/{key}")).to_string();
        let rows = vec![
            (
                "CPU",
                format!(
                    "{} allocatable of {}",
                    quantity("/status/allocatable/cpu", true),
                    quantity("/status/capacity/cpu", true)
                ),
            ),
            (
                "Memory",
                format!(
                    "{} allocatable of {}",
                    quantity("/status/allocatable/memory", false),
                    quantity("/status/capacity/memory", false)
                ),
            ),
            (
                "Pods",
                format!("{} allocatable", str_at(node, "/status/allocatable/pods")),
            ),
            ("Kubelet", info("kubeletVersion")),
            ("OS", info("osImage")),
            ("Runtime", info("containerRuntimeVersion")),
        ];
        let mut out: Vec<AnyElement> = self
            .render_node_usage(node, colors, cx)
            .into_iter()
            .collect();
        out.push(
            section("Capacity", colors)
                .child(kv(rows, colors))
                .into_any_element(),
        );
        let taints: Vec<String> = array_at(node, "/spec/taints")
            .iter()
            .map(|t| match t["value"].as_str() {
                Some(v) => format!("{}={v}:{}", str_at(t, "/key"), str_at(t, "/effect")),
                None => format!("{}:{}", str_at(t, "/key"), str_at(t, "/effect")),
            })
            .collect();
        if !taints.is_empty() {
            out.push(
                section("Taints", colors)
                    .child(chips(taints, true))
                    .into_any_element(),
            );
        }
        let addresses: Vec<(&'static str, String)> = array_at(node, "/status/addresses")
            .iter()
            .map(|a| match str_at(a, "/type") {
                "InternalIP" => ("Internal IP", str_at(a, "/address").to_string()),
                "ExternalIP" => ("External IP", str_at(a, "/address").to_string()),
                "Hostname" => ("Hostname", str_at(a, "/address").to_string()),
                _ => ("Address", str_at(a, "/address").to_string()),
            })
            .collect();
        if !addresses.is_empty() {
            out.push(
                section("Addresses", colors)
                    .child(kv(addresses, colors))
                    .into_any_element(),
            );
        }
        out
    }

    fn render_service(
        &self,
        svc: &Value,
        _: &Target,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let rows = vec![
            ("Type", str_at(svc, "/spec/type").to_string()),
            ("Cluster IP", str_at(svc, "/spec/clusterIP").to_string()),
        ];
        let ports = service_ports(svc);
        let mut out = vec![
            section("Service", colors)
                .child(kv(rows, colors))
                .when(!ports.is_empty(), |this| {
                    this.child(self.port_list(ports, Some("Ports"), colors, cx))
                })
                .into_any_element(),
        ];
        if let Some(store) = &self.related.endpoints {
            let addresses: Vec<String> = store
                .read(cx)
                .objects()
                .values()
                .flat_map(|e| array_at(e, "/subsets").to_vec())
                .flat_map(|s| {
                    let ports: Vec<i64> = array_at(&s, "/ports")
                        .iter()
                        .map(|p| int_at(p, "/port"))
                        .collect();
                    array_at(&s, "/addresses")
                        .iter()
                        .flat_map(|a| {
                            ports
                                .iter()
                                .map(move |p| format!("{}:{p}", str_at(a, "/ip")))
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            let body = if addresses.is_empty() {
                div()
                    .text_size(u(12.0))
                    .text_color(colors.yellow)
                    .child("No ready endpoints.")
                    .into_any_element()
            } else {
                chips(addresses, true).into_any_element()
            };
            out.push(section("Endpoints", colors).child(body).into_any_element());
        }
        out
    }

    fn render_pvc(
        &self,
        pvc: &Value,
        target: &Target,
        colors: &Colors,
        cx: &App,
    ) -> Vec<AnyElement> {
        let volume = str_at(pvc, "/spec/volumeName").to_string();
        let mut rows = vec![
            ("Status", str_at(pvc, "/status/phase").to_string()),
            (
                "Capacity",
                str_at(pvc, "/status/capacity/storage").to_string(),
            ),
            (
                "Storage class",
                str_at(pvc, "/spec/storageClassName").to_string(),
            ),
        ];
        if let Some(pv) = self
            .related
            .volume
            .as_ref()
            .and_then(|s| s.read(cx).get(&volume).cloned())
        {
            rows.push((
                "Reclaim policy",
                str_at(&pv, "/spec/persistentVolumeReclaimPolicy").to_string(),
            ));
        }
        let mut body = v_flex().gap(u(6.0)).child(kv(rows, colors));
        if !volume.is_empty() {
            let reference = ResourceRef::object(
                target.cluster.clone(),
                Gvr::new("", "v1", "persistentvolumes"),
                None,
                volume.clone(),
            );
            body = body.child(
                h_flex()
                    .gap(u(8.0))
                    .text_size(u(12.0))
                    .child(
                        div()
                            .w(u(104.0))
                            .text_color(colors.text_dim)
                            .child("Volume"),
                    )
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .child(link("pv", volume, reference, colors)),
                    ),
            );
        }
        vec![section("Claim", colors).child(body).into_any_element()]
    }

    fn render_ingress(&self, ing: &Value, target: &Target, colors: &Colors) -> Vec<AnyElement> {
        let mut services: Vec<String> = array_at(ing, "/spec/rules")
            .iter()
            .flat_map(|r| array_at(r, "/http/paths").to_vec())
            .filter_map(|p| {
                p.pointer("/backend/service/name")
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .chain(
                ing.pointer("/spec/defaultBackend/service/name")
                    .and_then(Value::as_str)
                    .map(String::from),
            )
            .collect();
        services.sort();
        services.dedup();
        let secrets: Vec<String> = array_at(ing, "/spec/tls")
            .iter()
            .filter_map(|t| t["secretName"].as_str().map(String::from))
            .collect();
        let mut list = v_flex().gap(u(4.0)).text_size(u(12.0));
        for (ix, service) in services.into_iter().enumerate() {
            let reference = ResourceRef::object(
                target.cluster.clone(),
                Gvr::new("", "v1", "services"),
                target.namespace.clone(),
                service.clone(),
            );
            list = list.child(
                h_flex()
                    .gap(u(6.0))
                    .child(
                        div()
                            .w(u(60.0))
                            .text_color(colors.text_dim)
                            .child("Service"),
                    )
                    .child(link(
                        SharedString::from(format!("svc-{ix}")),
                        service,
                        reference,
                        colors,
                    )),
            );
        }
        for (ix, secret) in secrets.into_iter().enumerate() {
            let reference = ResourceRef::object(
                target.cluster.clone(),
                Gvr::new("", "v1", "secrets"),
                target.namespace.clone(),
                secret.clone(),
            );
            list = list.child(
                h_flex()
                    .gap(u(6.0))
                    .child(div().w(u(60.0)).text_color(colors.text_dim).child("TLS"))
                    .child(link(
                        SharedString::from(format!("secret-{ix}")),
                        secret,
                        reference,
                        colors,
                    )),
            );
        }
        vec![section("Backends", colors).child(list).into_any_element()]
    }

    /// Secret `data`/`stringData`: decoded plain text, masked by default with a per-key reveal
    /// toggle and a copy-to-clipboard button. Never shows raw base64.
    fn render_secret(
        &self,
        secret: &Value,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut entries: Vec<(String, String)> = Vec::new();
        for (is_string_data, pointer) in [(false, "/data"), (true, "/stringData")] {
            if let Some(map) = secret.pointer(pointer).and_then(Value::as_object) {
                for (key, value) in map {
                    let raw = value.as_str().unwrap_or_default();
                    entries.push((key.clone(), decode_secret_value(raw, is_string_data)));
                }
            }
        }
        if entries.is_empty() {
            return Vec::new();
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut list = v_flex().gap(u(6.0)).text_size(u(12.0));
        for (key, decoded) in entries {
            let revealed = self.revealed.contains(&key);
            let display: SharedString = if revealed {
                decoded.clone().into()
            } else {
                SECRET_MASK.into()
            };
            let weak = cx.entity().downgrade();
            let reveal_key = key.clone();
            let copy_text = decoded.clone();
            let copy_key = key.clone();
            list = list.child(
                h_flex()
                    .gap(u(8.0))
                    .items_center()
                    .child(
                        div()
                            .w(u(140.0))
                            .flex_none()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_color(colors.text_dim)
                            .child(key.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_color(colors.text)
                            .child(display),
                    )
                    .child(
                        IconButton::new(
                            SharedString::from(format!("secret-reveal-{key}")),
                            if revealed {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            },
                        )
                        .icon_size(13.0)
                        .toggled(revealed)
                        .on_click(move |_, _, cx| {
                            weak.update(cx, |this, cx| {
                                if !this.revealed.remove(&reveal_key) {
                                    this.revealed.insert(reveal_key.clone());
                                }
                                cx.notify();
                            })
                            .ok();
                        }),
                    )
                    .child(
                        IconButton::new(
                            SharedString::from(format!("secret-copy-{copy_key}")),
                            IconName::Copy,
                        )
                        .icon_size(13.0)
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                            NotificationCenter::push(
                                cx,
                                Notification::info(format!("Copied {copy_key}")),
                            );
                        }),
                    ),
            );
        }
        vec![section("Data", colors).child(list).into_any_element()]
    }

    /// Ports with a one-click forward, or the running forward (open/copy, stop). `label`: the
    /// kv label shown left of the first row (inside a kv section).
    fn port_list(
        &self,
        ports: Vec<PortRow>,
        label: Option<&'static str>,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(target) = &self.target else {
            return div().into_any_element();
        };
        let reference = ResourceRef::object(
            target.cluster.clone(),
            target.gvr.clone(),
            target.namespace.clone(),
            target.name.clone(),
        );
        let can_forward = !ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).caps(&target.cluster))
            .unwrap_or_default()
            .read_only
            && !self.gone;
        let mut list = v_flex().gap(u(5.0)).text_size(u(12.0));
        for (ix, row) in ports.into_iter().enumerate() {
            let forward = ActiveForwards::find(cx, &reference, row.port).cloned();
            let control = if !row.tcp || !can_forward {
                None
            } else if let Some(forward) = forward {
                let id = forward.id;
                let local = forward.local.clone();
                let url = forward.url.clone();
                Some(
                    h_flex()
                        .gap(u(4.0))
                        .child(match local {
                            Some(local) => {
                                let text = url.clone().unwrap_or_else(|| local.clone());
                                div()
                                    .id(SharedString::from(format!("forward-open-{ix}")))
                                    .font_family(fonts::MONO)
                                    .text_size(u(11.5))
                                    .text_color(colors.green)
                                    .cursor_pointer()
                                    .hover(|s| s.underline())
                                    .tooltip(move |window, cx| {
                                        gpui_component::tooltip::Tooltip::new(if url.is_some() {
                                            "Open in the browser"
                                        } else {
                                            "Copy the address"
                                        })
                                        .build(window, cx)
                                    })
                                    .on_click(move |_, _, cx| {
                                        if text.starts_with("http") {
                                            cx.open_url(&text);
                                        } else {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                text.clone(),
                                            ));
                                            NotificationCenter::push(
                                                cx,
                                                Notification::info(format!("Copied {text}")),
                                            );
                                        }
                                    })
                                    .child(format!("→ {local}"))
                                    .into_any_element()
                            }
                            None => div()
                                .text_size(u(11.5))
                                .text_color(colors.text_dim)
                                .child("forwarding…")
                                .into_any_element(),
                        })
                        .child(tooltip_wrap(
                            "forward-stop-tip",
                            "Stop the port-forward",
                            IconButton::new(("forward-stop", ix), IconName::X)
                                .icon_size(11.0)
                                .on_click(move |_, window, cx| {
                                    window.dispatch_action(Box::new(StopForward(id)), cx)
                                }),
                        ))
                        .into_any_element(),
                )
            } else {
                let target = reference.clone();
                let port = row.port;
                Some(
                    row_button(
                        format!("forward-{ix}"),
                        IconName::ArrowRight,
                        "Forward",
                        colors,
                    )
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(format!(
                            "Forward port {port} to this machine"
                        ))
                        .build(window, cx)
                    })
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(
                            Box::new(ForwardPort {
                                target: target.clone(),
                                port,
                            }),
                            cx,
                        )
                    })
                    .into_any_element(),
                )
            };
            list = list.child(
                h_flex()
                    .gap(u(8.0))
                    .min_h(u(20.0))
                    .when_some(label, |this, label| {
                        this.child(
                            div()
                                .flex_none()
                                .w(u(104.0))
                                .text_color(colors.text_dim)
                                .child(if ix == 0 { label } else { "" }),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .text_color(colors.text)
                            .child(row.label),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(colors.text_dim)
                            .child(row.detail),
                    )
                    .children(control),
            );
        }
        list.into_any_element()
    }

    /// The +/− buttons: scalable kinds, a writable cluster, and no known RBAC denial.
    fn can_scale(&self, target: &Target, cx: &App) -> bool {
        if !SCALABLE.contains(&target.gvr.resource.as_str()) || self.gone {
            return false;
        }
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return false;
        };
        let manager = manager.read(cx);
        if manager.caps(&target.cluster).read_only {
            return false;
        }
        let reference = ResourceRef::object(
            target.cluster.clone(),
            target.gvr.clone(),
            target.namespace.clone(),
            target.name.clone(),
        );
        crate::actions::access_for("Workload: Scale…", &reference)
            .is_none_or(|query| manager.cached_can_i(&target.cluster, &query) != Some(false))
    }

    /// +/− one replica. Scaling to 0 asks first (typed on production clusters).
    fn scale_by(&mut self, delta: i64, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(target), Some(object)) = (self.target.clone(), self.object.clone()) else {
            return;
        };
        let current = self
            .scale_pending
            .map(|p| p.replicas)
            .unwrap_or_else(|| int_at(&object, "/spec/replicas"));
        let next = (current + delta).max(0);
        if next == current {
            return;
        }
        if next > 0 {
            self.request_scale(next, cx);
            return;
        }
        let production = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).caps(&target.cluster).production)
            .unwrap_or_default();
        let mut spec = ConfirmSpec::new(
            format!("Scale {} {} to 0?", target.kind.to_lowercase(), target.name),
            "Scale to 0",
        );
        spec.lines = vec![
            match &target.namespace {
                Some(ns) => format!("{ns}/{}", target.name),
                None => target.name.clone(),
            }
            .into(),
        ];
        spec.note = Some("All of its pods stop.".into());
        spec.danger = true;
        spec.typed = production.then(|| target.name.clone());
        let weak = cx.weak_entity();
        dialogs::confirm(
            spec,
            move |_, _, cx| {
                weak.update(cx, |this, cx| this.request_scale(0, cx)).ok();
            },
            window,
            cx,
        );
    }

    /// Shows `replicas` right away and sends it after [`SCALE_DEBOUNCE`], so a few quick clicks
    /// become one request.
    fn request_scale(&mut self, replicas: i64, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
        self.scale_pending = Some(PendingScale { replicas });
        let reference = ResourceRef::object(
            target.cluster.clone(),
            target.gvr.clone(),
            target.namespace.clone(),
            target.name.clone(),
        );
        self.scale_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SCALE_DEBOUNCE).await;
            let job = cx.update(|cx| {
                let (client, resource) = crate::actions::client_and_resource(cx, &reference)?;
                let (namespace, name) = (reference.namespace.clone(), target.name.clone());
                Some(kubyl_core::spawn_kube(cx, async move {
                    kubyl_resources::ops::scale(client, resource, namespace, name, replicas as u32)
                        .await
                }))
            });
            let result = match job {
                Some(job) => job.await,
                None => Err("the cluster isn't connected".into()),
            };
            this.update(cx, |this, cx| {
                if let Err(err) = result {
                    this.scale_pending = None;
                    NotificationCenter::push(
                        cx,
                        Notification::error(format!("Scaling {} failed: {err}", target.name)),
                    );
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_describe(&self, object: &Value, target: &Target, cx: &App) -> AnyElement {
        let colors = cx.colors();
        let events: Vec<Value> = self
            .related
            .events
            .as_ref()
            .map(|s| {
                s.read(cx)
                    .objects()
                    .values()
                    .map(|e| (**e).clone())
                    .collect()
            })
            .unwrap_or_default();
        let text = kubyl_resources::describe::describe(
            &target.kind,
            object,
            &events,
            jiff::Timestamp::now(),
        );
        v_flex()
            .p(u(12.0))
            .font_family(fonts::MONO)
            .text_size(u(12.0))
            .text_color(colors.text)
            .children(text.lines().map(|line| {
                div()
                    .whitespace_nowrap()
                    .min_h(u(17.0))
                    .child(line.to_string())
            }))
            .into_any_element()
    }
}

impl Render for DetailsContent {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let (Some(target), Some(object)) = (self.target.clone(), self.object.clone()) else {
            let message = if self.target.is_some() {
                "Loading…"
            } else {
                "Select a resource to see its details."
            };
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .text_color(colors.text_dim)
                .child(message)
                .into_any_element();
        };
        let mode = self.mode;
        let caps = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).caps(&target.cluster))
            .unwrap_or_default();
        let show_logs = logs_applicable(&target.gvr.resource);
        let show_terminal = terminal_applicable(&target.gvr.resource, &caps);
        let show_files = files_applicable(&target.gvr.resource, &caps);

        let body = match mode {
            Mode::Summary => v_flex()
                .children(self.render_summary(&object, &target, cx))
                .into_any_element(),
            Mode::Describe => self.render_describe(&object, &target, cx),
            Mode::Yaml | Mode::Logs | Mode::Terminal | Mode::Files => {
                match self.ensure_extra_view(mode, window, cx) {
                    Some(view) => div().size_full().child(view).into_any_element(),
                    None => div()
                        .p(u(14.0))
                        .text_color(colors.text_dim)
                        .child("Not available for this resource.")
                        .into_any_element(),
                }
            }
        };
        let tab = |id: &'static str, label: &'static str, mode: Mode, current: Mode| {
            let active = mode == current;
            div()
                .id(id)
                .px(u(8.0))
                .py(u(2.0))
                .rounded(u(4.0))
                .cursor_pointer()
                .when(active, |this| {
                    this.bg(colors.selection).text_color(colors.text)
                })
                .when(!active, |this| this.text_color(colors.text_dim))
                .child(label)
        };
        v_flex()
            .size_full()
            .text_size(u(13.0))
            .text_color(colors.text)
            .child(
                h_flex()
                    .flex_none()
                    .flex_wrap()
                    .pl(u(10.0))
                    // Room for the dock's pin button.
                    .pr(u(34.0))
                    .py(u(6.0))
                    .gap(u(4.0))
                    .text_size(u(12.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        tab("summary", "Summary", Mode::Summary, mode).on_click(
                            cx.listener(|this, _, _, cx| this.set_mode(Mode::Summary, cx)),
                        ),
                    )
                    .child(
                        tab("describe", "Describe", Mode::Describe, mode).on_click(
                            cx.listener(|this, _, _, cx| this.set_mode(Mode::Describe, cx)),
                        ),
                    )
                    .child(
                        tab("yaml", "YAML", Mode::Yaml, mode)
                            .on_click(cx.listener(|this, _, _, cx| this.set_mode(Mode::Yaml, cx))),
                    )
                    .when(show_logs, |this| {
                        this.child(
                            tab("logs", "Logs", Mode::Logs, mode).on_click(
                                cx.listener(|this, _, _, cx| this.set_mode(Mode::Logs, cx)),
                            ),
                        )
                    })
                    .when(show_terminal, |this| {
                        this.child(tab("terminal", "Terminal", Mode::Terminal, mode).on_click(
                            cx.listener(|this, _, _, cx| this.set_mode(Mode::Terminal, cx)),
                        ))
                    })
                    .when(show_files, |this| {
                        this.child(
                            tab("files", "Files", Mode::Files, mode).on_click(
                                cx.listener(|this, _, _, cx| this.set_mode(Mode::Files, cx)),
                            ),
                        )
                    }),
            )
            .child(
                div()
                    .id("details-scroll")
                    .flex_1()
                    .min_h_0()
                    .when(matches!(mode, Mode::Summary | Mode::Describe), |this| {
                        this.overflow_y_scroll()
                    })
                    .when(mode == Mode::Describe, |this| this.overflow_x_scroll())
                    .child(body),
            )
            .into_any_element()
    }
}

// ----- Dock panel -----

/// The right-dock panel: follows [`ResourceSelection`] unless pinned.
pub struct DetailsPanel {
    content: Entity<DetailsContent>,
    pinned: bool,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl DetailsPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        let content = cx.new(|cx| DetailsContent::new(Mode::Summary, cx));
        let subscriptions = vec![
            cx.observe_global::<ResourceSelection>(|this, cx| this.selection_changed(cx)),
            cx.observe(&content, |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            content,
            pinned: false,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.selection_changed(cx);
        this
    }

    fn selection_changed(&mut self, cx: &mut Context<Self>) {
        if self.pinned {
            return;
        }
        let selection = ResourceSelection::global(cx).clone();
        let primary = selection.primary().cloned();
        self.content.update(cx, |content, cx| match primary {
            Some(selected) => {
                let target = Target::from_ref(&selected.target, selected.kind.clone());
                content.set_target(target, selected.object.clone(), selected.store.clone(), cx);
            }
            None => content.set_target(None, None, None, cx),
        });
    }
}

impl Focusable for DetailsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for DetailsPanel {
    fn tab_title(&self, cx: &App) -> SharedString {
        self.content.read(cx).title()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Info.path())
    }
}

impl Render for DetailsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let has_target = self.content.read(cx).target.is_some();
        v_flex()
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .child(self.content.clone())
            .when(has_target, |this| {
                this.child(
                    div().absolute().top(u(3.0)).right(u(8.0)).child(
                        IconButton::new(
                            "pin-details",
                            if self.pinned {
                                IconName::StarFilled
                            } else {
                                IconName::Star
                            },
                        )
                        .icon_size(13.0)
                        .toggled(self.pinned)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pinned = !this.pinned;
                            if !this.pinned {
                                this.selection_changed(cx);
                            }
                            cx.notify();
                        })),
                    ),
                )
            })
            .text_color(colors.text)
    }
}

pub struct DetailsDock;

impl DockPanel for DetailsDock {
    fn id(&self) -> &'static str {
        "details"
    }

    fn position(&self) -> DockPosition {
        DockPosition::Right
    }

    fn build(&self, _: &mut Window, cx: &mut App) -> Box<dyn TabHandle> {
        Box::new(cx.new(DetailsPanel::new))
    }
}

// ----- Details / Describe tabs -----

/// A tab showing one object (`ViewKind::Details`, or `describe` for the Describe mode).
pub struct DetailsView {
    target: ResourceRef,
    content: Entity<DetailsContent>,
    mode: Mode,
    focus: FocusHandle,
    _subscription: Subscription,
}

pub fn describe_view_kind() -> ViewKind {
    ViewKind::Custom("describe".into())
}

impl DetailsView {
    pub fn new(target: ResourceRef, mode: Mode, cx: &mut Context<Self>) -> Self {
        let kind = ConnectionManager::try_global(cx)
            .and_then(|m| m.read(cx).discovery(&target.cluster))
            .and_then(|d| {
                kubyl_resources::store::find_resource(&d.resources, &target.gvr)
                    .map(|r| r.gvk.kind.clone())
            })
            .unwrap_or_else(|| kind_guess(&target.gvr.resource));
        let content = cx.new(|cx| {
            let mut content = DetailsContent::new(mode, cx);
            content.set_target(Target::from_ref(&target, kind), None, None, cx);
            content
        });
        let subscription = cx.observe(&content, |_, _, cx| cx.notify());
        Self {
            target,
            content,
            mode,
            focus: cx.focus_handle(),
            _subscription: subscription,
        }
    }
}

/// `pods` → `Pod`, for tabs opened before discovery finished.
fn kind_guess(resource: &str) -> String {
    let singular = resource
        .strip_suffix("ies")
        .map(|s| format!("{s}y"))
        .or_else(|| resource.strip_suffix("sses").map(|s| format!("{s}ss")))
        .or_else(|| resource.strip_suffix('s').map(String::from))
        .unwrap_or_else(|| resource.to_string());
    let mut chars = singular.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

impl Focusable for DetailsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for DetailsView {
    fn tab_title(&self, _: &App) -> SharedString {
        let name = self.target.name.clone().unwrap_or_default();
        match self.mode {
            Mode::Describe => format!("{name} · describe").into(),
            // `DetailsView` is only ever constructed with `Summary` or `Describe` (the two
            // top-level entry `ViewKind`s); the extra sub-tabs live inside `DetailsContent`.
            Mode::Summary | Mode::Yaml | Mode::Logs | Mode::Terminal | Mode::Files => name.into(),
        }
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(catalog::icon_for(&self.target.gvr.group, &self.target.gvr.resource).path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        let kind = match self.mode {
            Mode::Describe => describe_view_kind(),
            Mode::Summary | Mode::Yaml | Mode::Logs | Mode::Terminal | Mode::Files => {
                ViewKind::Details
            }
        };
        Some(ViewRequest::for_resource(kind, self.target.clone()))
    }
}

impl Render for DetailsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        v_flex()
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.background)
            .child(div().flex_1().min_h_0().child(self.content.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn logs_and_terminal_applicability_mirrors_context_menu_actions() {
        assert!(logs_applicable("pods"));
        assert!(logs_applicable("deployments"));
        assert!(logs_applicable("statefulsets"));
        assert!(logs_applicable("daemonsets"));
        assert!(logs_applicable("jobs"));
        assert!(logs_applicable("replicasets"));
        assert!(logs_applicable("services"));
        assert!(!logs_applicable("configmaps"));

        let writable = ClusterCaps::default();
        let read_only = ClusterCaps {
            read_only: true,
            ..Default::default()
        };
        assert!(terminal_applicable("pods", &writable));
        assert!(!terminal_applicable("pods", &read_only));
        assert!(!terminal_applicable("deployments", &writable));
        assert!(files_applicable("pods", &writable));
        assert!(!files_applicable("pods", &read_only));
        assert!(!files_applicable("services", &writable));
    }

    #[test]
    fn ports_of_pods_and_services() {
        let pod = json!({"spec": {"containers": [
            {"name": "api", "ports": [
                {"containerPort": 8080, "name": "http"},
                {"containerPort": 53, "protocol": "UDP"}
            ]},
            {"name": "sidecar", "ports": [{"containerPort": 9090}]}
        ]}});
        let ports = pod_ports(&pod);
        assert_eq!(ports.len(), 3);
        assert_eq!((ports[0].port, ports[0].label.as_str()), (8080, "8080/TCP"));
        assert_eq!(ports[0].detail, "http · api");
        assert!(!ports[1].tcp, "UDP ports can't be forwarded");
        assert_eq!(ports[2].detail, "sidecar");

        let svc = json!({"spec": {"ports": [
            {"port": 80, "targetPort": "http", "name": "web"},
            {"port": 5432, "targetPort": 5432},
            {"port": 9000}
        ]}});
        let ports = service_ports(&svc);
        assert_eq!(ports[0].label, "80/TCP → http");
        assert_eq!(ports[0].detail, "web");
        assert_eq!(ports[1].label, "5432/TCP → 5432");
        assert_eq!(ports[2].label, "9000/TCP → 9000");
    }

    #[test]
    fn extra_view_kind_maps_sub_tabs_only() {
        assert_eq!(extra_view_kind(Mode::Yaml), Some(ViewKind::Yaml));
        assert_eq!(extra_view_kind(Mode::Logs), Some(ViewKind::Logs));
        assert_eq!(extra_view_kind(Mode::Terminal), Some(ViewKind::Terminal));
        assert_eq!(extra_view_kind(Mode::Files), Some(ViewKind::Files));
        assert_eq!(extra_view_kind(Mode::Summary), None);
        assert_eq!(extra_view_kind(Mode::Describe), None);
    }

    #[test]
    fn decodes_secret_values_without_ever_showing_raw_base64() {
        // `data`: base64-decoded to plain text.
        assert_eq!(decode_secret_value("aGVsbG8=", false), "hello");
        // `stringData`: already plain text, passed through.
        assert_eq!(decode_secret_value("hello", true), "hello");
        // Invalid UTF-8 after decoding: a byte count, not mojibake.
        let binary = base64::engine::general_purpose::STANDARD.encode([0xff, 0xfe, 0x00]);
        assert_eq!(decode_secret_value(&binary, false), "<binary, 3 bytes>");
        // Not valid base64 at all.
        assert_eq!(
            decode_secret_value("not base64!!", false),
            "<invalid base64>"
        );
    }

    #[test]
    fn guesses_kinds() {
        assert_eq!(kind_guess("pods"), "Pod");
        assert_eq!(kind_guess("ingresses"), "Ingress");
        assert_eq!(kind_guess("networkpolicies"), "Networkpolicy");
        assert_eq!(kind_guess("storageclasses"), "Storageclass");
    }

    #[test]
    fn selectors_match_pods() {
        let deployment = json!({"spec": {"selector": {"matchLabels": {"app": "web"}}}});
        let selector = selector_of("Deployment", &deployment).unwrap();
        assert!(matches_selector(
            &json!({"metadata": {"labels": {"app": "web", "x": "y"}}}),
            &selector
        ));
        assert!(!matches_selector(
            &json!({"metadata": {"labels": {"app": "api"}}}),
            &selector
        ));
        assert!(selector_of("ConfigMap", &deployment).is_none());
        let service = json!({"spec": {"selector": {"app": "web"}}});
        assert_eq!(selector_of("Service", &service).unwrap().len(), 1);
    }

    #[test]
    fn container_states() {
        let now = jiff::Timestamp::now();
        let waiting = json!({"state": {"waiting": {"reason": "CrashLoopBackOff"}}});
        assert_eq!(
            container_state(Some(&waiting), now),
            ("CrashLoopBackOff".into(), Tone::Bad)
        );
        let done = json!({"state": {"terminated": {"reason": "Completed", "exitCode": 0}}});
        assert_eq!(container_state(Some(&done), now).1, Tone::Muted);
    }
}
