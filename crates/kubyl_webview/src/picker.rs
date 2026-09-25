//! Pickers: `> Web View: Open…` (web ports of the Services in the active namespace), the port
//! choice of `w` on an object with several ports, and "Open as web view…" (scheme and start
//! path for a port that doesn't look like HTTP).

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement,
    KeyBinding, Render, SharedString, Subscription, Window, actions, div, prelude::*, px,
    uniform_list,
};
use gpui_component::WindowExt as _;
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_core::{ActiveContext, Gvr, Notification, NotificationCenter, ResourceRef};
use kubyl_resources::store::{ResourceStores, StoreHandle, StoreKey};
use kubyl_ui::{ActiveColors, Button, Chip, Icon, IconName, fonts, h_flex, u, v_flex};
use serde_json::Value;

use crate::target::{Scheme, TargetKind, ports_of};
use crate::view::OpenRequest;
use crate::{backend_port, backends, cached_object, open_in, request_for};

actions!(webview_picker, [SelectNext, SelectPrevious]);

const CONTEXT: &str = "WebViewPicker";

pub(crate) fn init(cx: &mut App) {
    let input = Some("WebViewPicker > Input");
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, input),
        KeyBinding::new("up", SelectPrevious, input),
        KeyBinding::new("ctrl-n", SelectNext, input),
        KeyBinding::new("ctrl-p", SelectPrevious, input),
    ]);
}

/// One choice: a port of an object.
#[derive(Clone, Debug)]
pub struct PickerRow {
    /// `svc/grafana:80`.
    pub label: String,
    /// `http-web · appProtocol http`.
    pub detail: String,
    pub web: bool,
    pub request: OpenRequest,
}

/// The ports of a Service or Pod, or the backends of an Ingress, as picker rows.
pub fn rows_for_object(target: &ResourceRef, object: &Value, cx: &App) -> Vec<PickerRow> {
    match target.gvr.resource.as_str() {
        "services" | "pods" => {
            let Some(kind) = TargetKind::from_resource(&target.gvr.resource) else {
                return Vec::new();
            };
            ports_of(kind, object)
                .into_iter()
                .filter_map(|port| {
                    let request = request_for(target, port.port, Some(object))?;
                    Some(PickerRow {
                        label: request.target.to_string(),
                        detail: match &port.reason {
                            Some(reason) => format!("{} · {reason}", port.label()),
                            None => format!("{} · not HTTP", port.label()),
                        },
                        web: port.web,
                        request,
                    })
                })
                .collect()
        }
        "ingresses" => backends(object)
            .into_iter()
            .filter_map(|backend| {
                let service_ref = ResourceRef::object(
                    target.cluster.clone(),
                    Gvr::new("", "v1", "services"),
                    target.namespace.clone(),
                    backend.service.clone(),
                );
                let service = cached_object(&service_ref, cx);
                let port = backend_port(&backend.port, service.as_deref())?;
                let mut request = request_for(&service_ref, port, service.as_deref())?;
                if backend.path != "/" {
                    request.path = Some(backend.path.clone());
                }
                Some(PickerRow {
                    label: request.target.to_string(),
                    detail: format!("{}{}", backend.host.as_deref().unwrap_or("*"), backend.path),
                    web: true,
                    request,
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A picker over fixed rows.
pub fn open_rows(title: String, rows: Vec<PickerRow>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| Picker::new(title.into(), Source::Rows(rows), window, cx));
    show(view, window, cx);
}

/// `> Web View: Open…`: the web ports of the Services in the active namespace (all namespaces
/// when none is chosen).
pub fn open_namespace_picker(window: &mut Window, cx: &mut App) {
    let context = ActiveContext::global(cx).clone();
    let Some(cluster) = context.cluster else {
        NotificationCenter::push(cx, Notification::info("Choose a cluster first."));
        return;
    };
    let namespace = context.namespace.map(|n| n.to_string());
    let title = format!(
        "Open a web view · {}",
        namespace.clone().unwrap_or_else(|| "all namespaces".into())
    );
    let store = ResourceStores::acquire(
        cx,
        StoreKey::new(
            cluster.id.clone(),
            Gvr::new("", "v1", "services"),
            namespace,
        ),
    );
    let view = cx.new(|cx| {
        cx.observe(store.entity(), |this: &mut Picker, _, cx| {
            this.refresh(cx);
            cx.notify();
        })
        .detach();
        Picker::new(title.into(), Source::Services(store), window, cx)
    });
    show(view, window, cx);
}

fn show(view: Entity<Picker>, window: &mut Window, cx: &mut App) {
    let colors = cx.colors().clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(560.0))
            .margin_top(px(90.0))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            .on_ok(|_, _, _| false)
            .content({
                let view = view.clone();
                move |content, _, _| content.child(view.clone())
            })
    });
}

enum Source {
    Rows(Vec<PickerRow>),
    Services(StoreHandle),
}

struct Picker {
    title: SharedString,
    source: Source,
    rows: Vec<PickerRow>,
    filtered: Vec<usize>,
    selected: usize,
    filter: Entity<InputState>,
    focus_filter: bool,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl Picker {
    fn new(
        title: SharedString,
        source: Source,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter ports…"));
        let subscription =
            cx.subscribe_in(&filter, window, |this, _, event, window, cx| match event {
                InputEvent::Change => {
                    this.apply_filter(cx);
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.confirm(window, cx),
                _ => {}
            });
        let mut this = Self {
            title,
            source,
            rows: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            filter,
            // Focused in render: a dialog's focus trap ignores focus before it's rendered.
            focus_filter: true,
            focus: cx.focus_handle(),
            _subscription: subscription,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.rows = match &self.source {
            Source::Rows(rows) => rows.clone(),
            Source::Services(store) => {
                let store = store.read(cx);
                let mut rows: Vec<PickerRow> = Vec::new();
                for service in store.objects().values() {
                    let (Some(name), Some(namespace)) = (
                        service.pointer("/metadata/name").and_then(Value::as_str),
                        service
                            .pointer("/metadata/namespace")
                            .and_then(Value::as_str),
                    ) else {
                        continue;
                    };
                    let target = ResourceRef::object(
                        store.key().cluster.clone(),
                        Gvr::new("", "v1", "services"),
                        Some(namespace.to_string()),
                        name.to_string(),
                    );
                    rows.extend(
                        rows_for_object(&target, service, cx)
                            .into_iter()
                            .filter(|r| r.web)
                            .map(|mut r| {
                                if store.key().namespace.is_none() {
                                    r.detail = format!("{namespace} · {}", r.detail);
                                }
                                r
                            }),
                    );
                }
                rows.sort_by(|a, b| a.label.cmp(&b.label));
                rows
            }
        };
        self.apply_filter(cx);
    }

    fn apply_filter(&mut self, cx: &App) {
        let query = self.filter.read(cx).value().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        self.filtered = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                let text = format!("{} {}", row.label, row.detail).to_lowercase();
                words.iter().all(|w| text.contains(w))
            })
            .map(|(i, _)| i)
            .collect();
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self
            .filtered
            .get(self.selected)
            .and_then(|i| self.rows.get(*i))
            .cloned()
        else {
            return;
        };
        window.close_dialog(cx);
        if row.web {
            open_in(row.request, window, cx);
        } else {
            open_as(row.request, window, cx);
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.filtered.is_empty() {
            return;
        }
        let len = self.filtered.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        cx.notify();
    }
}

impl Focusable for Picker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Picker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_filter) {
            self.filter.update(cx, |input, cx| input.focus(window, cx));
        }
        let colors = cx.colors().clone();
        let loading = match &self.source {
            Source::Services(store) => !store.read(cx).status().is_ready(),
            Source::Rows(_) => false,
        };
        let empty = if loading {
            "Loading Services…"
        } else if self.rows.is_empty() {
            "No web ports here. Ports named http, web or ui, appProtocol http and well-known \
             ports (80, 443, 8080, 3000, 9090…) count."
        } else {
            "Nothing matches."
        };
        let count = self.filtered.len();
        let selected = self.selected;
        let rows: Vec<PickerRow> = self
            .filtered
            .iter()
            .filter_map(|i| self.rows.get(*i).cloned())
            .collect();
        let entity = cx.entity().downgrade();
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_selection(-1, cx)))
            .child(
                h_flex()
                    .px(u(14.0))
                    .py(u(10.0))
                    .gap(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(Icon::new(IconName::Globe).size(14.0).color(colors.accent))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.title.clone()),
                    ),
            )
            .child(
                div()
                    .px(u(14.0))
                    .py(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(Input::new(&self.filter).appearance(false)),
            )
            .child(if count == 0 {
                div()
                    .px(u(14.0))
                    .py(u(16.0))
                    .text_size(u(12.5))
                    .text_color(colors.text_dim)
                    .child(empty)
                    .into_any_element()
            } else {
                uniform_list("web-picker-rows", count, move |range, _, cx| {
                    let colors = cx.colors().clone();
                    range
                        .filter_map(|ix| {
                            let row = rows.get(ix)?.clone();
                            let entity = entity.clone();
                            Some(
                                h_flex()
                                    .id(("web-picker-row", ix))
                                    .w_full()
                                    .h(u(40.0))
                                    .px(u(14.0))
                                    .gap(u(10.0))
                                    .cursor_pointer()
                                    .when(ix == selected, |this| this.bg(colors.selection))
                                    .hover(|s| s.bg(colors.hover))
                                    .child(Icon::new(IconName::Globe).size(13.0).color(
                                        if row.web {
                                            colors.accent
                                        } else {
                                            colors.text_faint
                                        },
                                    ))
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .font_family(fonts::MONO)
                                                    .text_size(u(12.0))
                                                    .child(row.label.clone()),
                                            )
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(u(11.5))
                                                    .text_color(colors.text_dim)
                                                    .child(row.detail.clone()),
                                            ),
                                    )
                                    .when(!row.web, |this| this.child(Chip::new("as web view…")))
                                    .on_click(move |_, window, cx| {
                                        entity
                                            .update(cx, |this, cx| {
                                                this.selected = ix;
                                                this.confirm(window, cx);
                                            })
                                            .ok();
                                    }),
                            )
                        })
                        .collect()
                })
                .h(u((count.min(8) as f32) * 40.0))
                .into_any_element()
            })
            .child(
                h_flex()
                    .px(u(14.0))
                    .py(u(8.0))
                    .gap(u(12.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .text_size(u(11.5))
                    .text_color(colors.text_dim)
                    .child("↑↓ choose · ↵ open · esc close")
                    .child(div().flex_1())
                    .child("Opens over a temporary loopback forward"),
            )
    }
}

/// "Open as web view…": a port that doesn't look like HTTP; the user picks the scheme and
/// where to start.
pub fn open_as(request: OpenRequest, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| OpenAs::new(request, window, cx));
    let colors = cx.colors().clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(460.0))
            .margin_top(px(90.0))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            .on_ok(|_, _, _| false)
            .content({
                let view = view.clone();
                move |content, _, _| content.child(view.clone())
            })
    });
}

struct OpenAs {
    request: OpenRequest,
    scheme: Scheme,
    path: Entity<InputState>,
    focus_path: bool,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl OpenAs {
    fn new(request: OpenRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let path = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("/")
                .default_value(request.path.clone().unwrap_or_else(|| "/".into()))
        });
        let subscription = cx.subscribe_in(&path, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(window, cx);
            }
        });
        Self {
            scheme: request.detected,
            request,
            path,
            focus_path: true,
            focus: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut request = self.request.clone();
        let path = self.path.read(cx).value().trim().to_string();
        request.path = Some(if path.is_empty() { "/".into() } else { path });
        request.scheme = Some(self.scheme);
        window.close_dialog(cx);
        open_in(request, window, cx);
    }
}

impl Focusable for OpenAs {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for OpenAs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_path) {
            self.path.update(cx, |input, cx| input.focus(window, cx));
        }
        let colors = cx.colors().clone();
        let scheme_button = |scheme: Scheme, cx: &mut Context<Self>| {
            let selected = self.scheme == scheme;
            Button::new(scheme.as_str())
                .label(scheme.as_str().to_uppercase())
                .map(|b| if selected { b.primary() } else { b })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.scheme = scheme;
                    cx.notify();
                }))
        };
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(
                div()
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Open {} as a web view", self.request.target)),
            )
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(12.0))
                    .child(
                        div()
                            .text_size(u(12.5))
                            .text_color(colors.text_dim)
                            .child("This port doesn't look like HTTP. Pick how the page talks to it; Kubyl remembers the choice for this port."),
                    )
                    .child(
                        h_flex()
                            .gap(u(6.0))
                            .child(div().w(u(80.0)).text_color(colors.text_dim).child("Scheme"))
                            .child(scheme_button(Scheme::Http, cx))
                            .child(scheme_button(Scheme::Https, cx)),
                    )
                    .child(
                        h_flex()
                            .gap(u(6.0))
                            .child(div().w(u(80.0)).text_color(colors.text_dim).child("Start at"))
                            .child(
                                div()
                                    .flex_1()
                                    .px(u(8.0))
                                    .h(u(28.0))
                                    .flex()
                                    .items_center()
                                    .rounded(u(5.0))
                                    .border_1()
                                    .border_color(colors.border)
                                    .bg(colors.input_background)
                                    .font_family(fonts::MONO)
                                    .text_size(u(12.0))
                                    .child(Input::new(&self.path).appearance(false)),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap(u(8.0))
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(
                        Button::new("cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("open")
                            .primary()
                            .icon(IconName::Globe)
                            .label("Open")
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
