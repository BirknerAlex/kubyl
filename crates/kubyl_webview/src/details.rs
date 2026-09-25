//! The "Web views" sections of Service, Pod and Ingress details (board 10's dock): a button
//! per port (`Web view`, `Open · 2 tabs`, `Open as web view…` for ports that don't look like
//! HTTP), the running temporary forward, the session's storage, and other web UIs in the
//! namespace.

use gpui::{
    AnyView, App, AppContext as _, Context, FontWeight, IntoElement, Render, SharedString, Window,
    div, prelude::*,
};
use kubyl_core::{DetailsSection, Gvr, ResourceRef};
use kubyl_kube::ConnectionManager;
use kubyl_portforward::manager::human_bytes;
use kubyl_resources::store::{ResourceStores, StoreHandle, StoreKey, object_key};
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, fonts, h_flex, u, v_flex};
use serde_json::Value;

use crate::forward::{ForwardStatus, WebForwards};
use crate::store::{self, PortKey, WebViewSettings};
use crate::target::{
    IngressBackend, TargetKind, WebPort, WebTarget, ingress_backends, ports_of, preset_for,
    preset_names,
};
use crate::{OpenWebView, backend_port};

/// Registered with `ChromeRegistry::add_details_section`.
pub struct WebViewDetails;

impl DetailsSection for WebViewDetails {
    fn id(&self) -> &'static str {
        "web-views"
    }

    fn order(&self) -> i32 {
        50
    }

    fn build(&self, target: &ResourceRef, kind: &str, cx: &mut App) -> Option<AnyView> {
        let mode = match (kind, target.gvr.group.as_str()) {
            ("Service", "") => Mode::Object(TargetKind::Service),
            ("Pod", "") => Mode::Object(TargetKind::Pod),
            ("Ingress", "networking.k8s.io") => Mode::Ingress,
            _ => return None,
        };
        target.namespace.as_ref()?;
        let target = target.clone();
        Some(cx.new(|cx| WebSection::new(target, mode, cx)).into())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Object(TargetKind),
    Ingress,
}

struct WebSection {
    target: ResourceRef,
    mode: Mode,
    /// The object's list (its details keep it alive too).
    objects: StoreHandle,
    /// The namespace's Services: other web UIs, and Ingress backends' named ports.
    services: Option<StoreHandle>,
}

impl WebSection {
    fn new(target: ResourceRef, mode: Mode, cx: &mut Context<Self>) -> Self {
        let objects = ResourceStores::acquire(
            cx,
            StoreKey::new(
                target.cluster.clone(),
                target.gvr.clone(),
                target.namespace.clone(),
            ),
        );
        cx.observe(objects.entity(), |_, _, cx| cx.notify())
            .detach();
        let services = match mode {
            Mode::Object(TargetKind::Pod) => None,
            _ => {
                let handle = ResourceStores::acquire(
                    cx,
                    StoreKey::new(
                        target.cluster.clone(),
                        Gvr::new("", "v1", "services"),
                        target.namespace.clone(),
                    ),
                );
                cx.observe(handle.entity(), |_, _, cx| cx.notify()).detach();
                Some(handle)
            }
        };
        if let Some(forwards) = WebForwards::try_global(cx) {
            cx.observe(&forwards, |_, _, cx| cx.notify()).detach();
        }
        Self {
            target,
            mode,
            objects,
            services,
        }
    }

    fn object(&self, cx: &App) -> Option<Value> {
        let key = object_key(
            self.target.namespace.as_deref(),
            self.target.name.as_deref()?,
        );
        self.objects
            .read(cx)
            .get(&key)
            .map(|object| object.as_ref().clone())
    }

    fn service(&self, name: &str, cx: &App) -> Option<Value> {
        let key = object_key(self.target.namespace.as_deref(), name);
        self.services
            .as_ref()?
            .read(cx)
            .get(&key)
            .map(|object| object.as_ref().clone())
    }
}

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

fn port_row(
    label: String,
    detail: String,
    button: impl IntoElement,
    colors: &Colors,
) -> impl IntoElement {
    h_flex()
        .gap(u(10.0))
        .py(u(4.0))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .truncate()
                        .child(label),
                )
                .child(
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .truncate()
                        .child(detail),
                ),
        )
        .child(button)
}

/// The button of one port: `Web view`, `Open · N tabs`, or `Open as web view…`.
fn port_button(
    id: SharedString,
    target: &ResourceRef,
    port: &WebPort,
    tabs: usize,
    path: Option<String>,
    colors: &Colors,
) -> gpui::AnyElement {
    let action = OpenWebView {
        target: target.clone(),
        port: port.port,
        path,
        // Another tab of an open port keeps its scheme; only a first open asks.
        ask: !port.web && tabs == 0,
    };
    let label = match (port.web, tabs) {
        (_, 1) => "Open · 1 tab".to_string(),
        (_, n) if n > 1 => format!("Open · {n} tabs"),
        (false, _) => "Open as web view…".to_string(),
        (true, _) => "Web view".to_string(),
    };
    let accent = colors.accent;
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .gap(u(5.0))
        .h(u(24.0))
        .px(u(8.0))
        .rounded(u(5.0))
        .border_1()
        .text_size(u(12.0))
        .cursor_pointer()
        .hover(|s| s.bg(colors.hover))
        .map(|this| {
            if tabs > 0 {
                this.border_color(accent).text_color(accent)
            } else if port.web {
                this.border_color(colors.border)
                    .bg(colors.button_background)
                    .text_color(colors.text)
            } else {
                this.border_color(gpui::transparent_black())
                    .text_color(colors.text_muted)
            }
        })
        .when(port.web || tabs > 0, |this| {
            this.child(Icon::new(IconName::Globe).size(12.0).color(if tabs > 0 {
                accent
            } else {
                colors.text_dim
            }))
        })
        .child(label)
        .on_click(move |_, window, cx| window.dispatch_action(Box::new(action.clone()), cx))
        .into_any_element()
}

impl WebSection {
    fn render_object(&self, kind: TargetKind, object: &Value, cx: &mut Context<Self>) -> gpui::Div {
        let colors = cx.colors().clone();
        let forwards = WebForwards::try_global(cx);
        let ports = ports_of(kind, object);
        let names = preset_names(object);
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let memory = store::get(cx);
        let target_of = |port: u16| WebTarget::new(&self.target, port);
        let mut container = v_flex();

        // Ports.
        let mut ports_section = section("Web views", &colors);
        if ports.is_empty() {
            ports_section = ports_section.child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("No TCP ports."),
            );
        }
        for (index, port) in ports.iter().enumerate() {
            let target = target_of(port.port);
            let tabs = match (&forwards, &target) {
                (Some(f), Some(t)) => f.read(cx).tab_count(t),
                _ => 0,
            };
            let remembered = target
                .as_ref()
                .and_then(|t| PortKey::of(t, cx))
                .map(|k| memory.port(&k));
            let start = remembered
                .as_ref()
                .and_then(|m| m.start_path.clone())
                .or_else(|| preset_for(&names, port.port).map(|p| p.start_path.to_string()));
            let mut detail = port
                .reason
                .clone()
                .unwrap_or_else(|| "not HTTP".to_string());
            if port.web
                && let Some(start) = start.filter(|s| s != "/")
            {
                detail.push_str(&format!(" · start path {start}"));
            }
            ports_section = ports_section.child(port_row(
                port.label(),
                detail,
                port_button(
                    format!("web-port-{index}").into(),
                    &self.target,
                    port,
                    tabs,
                    None,
                    &colors,
                ),
                &colors,
            ));
        }
        container = container.child(ports_section);

        // The running temporary forwards of this object.
        let running: Vec<(WebTarget, ForwardStatus, usize)> = forwards
            .as_ref()
            .map(|f| {
                let f = f.read(cx);
                ports
                    .iter()
                    .filter_map(|p| target_of(p.port))
                    .filter_map(|t| Some((t.clone(), f.status(&t, cx)?, f.tab_count(&t))))
                    .collect()
            })
            .unwrap_or_default();
        if !running.is_empty() {
            let mut forward_section = section("Temporary forward", &colors);
            for (index, (target, status, tabs)) in running.into_iter().enumerate() {
                let (line, detail, color) = match &status {
                    ForwardStatus::Ready { local_port, info } => (
                        format!("127.0.0.1:{local_port} → {target}"),
                        format!(
                            "{}{} · {} · {}",
                            info.pod
                                .as_ref()
                                .map(|p| format!("via pod {p} · "))
                                .unwrap_or_default(),
                            match info.connections {
                                1 => "1 connection".to_string(),
                                n => format!("{n} connections"),
                            },
                            human_bytes(info.bytes_sent + info.bytes_received),
                            match tabs {
                                1 => "1 tab".to_string(),
                                n => format!("{n} tabs"),
                            }
                        ),
                        colors.green,
                    ),
                    ForwardStatus::Starting => (
                        format!("127.0.0.1:… → {target}"),
                        "starting…".into(),
                        colors.text_dim,
                    ),
                    ForwardStatus::Failed(err) => (format!("→ {target}"), err.clone(), colors.red),
                    ForwardStatus::Disconnected => (
                        format!("→ {target}"),
                        "cluster disconnected".into(),
                        colors.yellow,
                    ),
                };
                let stop = target.clone();
                forward_section = forward_section.child(
                    h_flex()
                        .gap(u(10.0))
                        .items_start()
                        .child(Icon::new(IconName::Link).size(14.0).color(color))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .text_size(u(12.0))
                                .child(div().font_family(fonts::MONO).truncate().child(line))
                                .child(div().text_color(colors.text_dim).child(detail))
                                .child(div().mt(u(4.0)).text_color(colors.text_muted).child(
                                    "Hidden from saved forwards. Stops when the last web view \
                                     tab closes.",
                                )),
                        ),
                );
                forward_section = forward_section.child(
                    h_flex().child(
                        Button::new(("web-stop", index))
                            .danger()
                            .icon(IconName::X)
                            .label("Stop and close tabs")
                            .on_click(move |_, _, cx| {
                                let target = stop.clone();
                                if let Some(forwards) = WebForwards::try_global(cx) {
                                    forwards.update(cx, |f, cx| f.stop_and_close(&target, cx));
                                }
                            }),
                    ),
                );
            }
            container = container.child(forward_section);
        }

        // Session (storage isolation, idle stop).
        if ports.iter().any(|p| p.web)
            && let Some(first) = ports.first().and_then(|p| target_of(p.port))
        {
            container = container.child(self.render_session(&first, &colors, cx));
        }

        // Other web UIs in the namespace (Services).
        if kind == TargetKind::Service {
            container = container.children(self.render_others(&colors, cx));
        }
        container
    }

    fn render_session(
        &self,
        target: &WebTarget,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let cluster = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).display_name(&target.cluster).to_string())
            .unwrap_or_default();
        let key = PortKey::of(target, cx);
        let private = key.as_ref().is_some_and(|k| store::get(cx).port(k).private)
            || WebViewSettings::get(cx).private_by_default;
        let idle = WebViewSettings::get(cx).idle_stop_minutes;
        let tabs: Vec<_> = WebForwards::try_global(cx)
            .map(|f| {
                let f = f.read(cx);
                ports_of(target.kind, &self.object(cx).unwrap_or_default())
                    .iter()
                    .filter_map(|p| WebTarget::new(&self.target, p.port))
                    .flat_map(|t| f.tabs(&t))
                    .collect()
            })
            .unwrap_or_default();
        let kv = |label: &'static str, value: String| {
            h_flex()
                .gap(u(10.0))
                .text_size(u(12.0))
                .child(
                    div()
                        .w(u(84.0))
                        .flex_none()
                        .text_color(colors.text_dim)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().truncate().child(value))
        };
        let storage = if private {
            "private · nothing is kept".to_string()
        } else {
            format!(
                "isolated · {cluster} / {} / {}",
                target.namespace, target.name
            )
        };
        let clear_tabs = tabs.clone();
        let object_ports: Vec<u16> = ports_of(target.kind, &self.object(cx).unwrap_or_default())
            .iter()
            .map(|p| p.port)
            .collect();
        let toggle_target = self.target.clone();
        section("Session", colors)
            .child(kv("Storage", storage))
            .child(kv(
                "Cookies",
                if private {
                    "dropped when the tab closes".into()
                } else {
                    "kept between sessions".into()
                },
            ))
            .child(kv(
                "Idle stop",
                if idle == 0 {
                    "off".into()
                } else {
                    format!("after {idle} min in background")
                },
            ))
            .child(
                h_flex()
                    .gap(u(6.0))
                    .mt(u(4.0))
                    .child(
                        Button::new("web-clear")
                            .ghost()
                            .label("Clear site data")
                            .disabled(clear_tabs.is_empty())
                            .on_click(move |_, _, cx| {
                                // The store belongs to the open page; the first tab clears it.
                                if let Some(tab) = clear_tabs.first() {
                                    tab.update(cx, |tab, cx| tab.clear_site_data_from_details(cx));
                                }
                            }),
                    )
                    .child(
                        Button::new("web-private")
                            .ghost()
                            .label(if private {
                                "Leave private mode"
                            } else {
                                "Private mode"
                            })
                            .on_click(move |_, window, cx| {
                                set_private(&toggle_target, &object_ports, !private, window, cx)
                            }),
                    ),
            )
    }

    fn render_others(&self, colors: &Colors, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let services = self.services.as_ref()?;
        let mut rows: Vec<(String, WebPort)> = services
            .read(cx)
            .objects()
            .values()
            .filter_map(|svc| {
                let name = svc.pointer("/metadata/name")?.as_str()?.to_string();
                (Some(name.as_str()) != self.target.name.as_deref()).then_some(())?;
                let port = ports_of(TargetKind::Service, svc)
                    .into_iter()
                    .find(|p| p.web)?;
                Some((name, port))
            })
            .collect();
        if rows.is_empty() {
            return None;
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        let namespace = self.target.namespace.clone().unwrap_or_default();
        let mut section = section(format!("Other web UIs in {namespace}"), colors);
        for (index, (name, port)) in rows.into_iter().take(8).enumerate() {
            let target = ResourceRef::object(
                self.target.cluster.clone(),
                Gvr::new("", "v1", "services"),
                self.target.namespace.clone(),
                name.clone(),
            );
            let tabs = WebTarget::new(&target, port.port)
                .zip(WebForwards::try_global(cx))
                .map(|(t, f)| f.read(cx).tab_count(&t))
                .unwrap_or_default();
            section = section.child(port_row(
                format!("{name}:{}", port.port),
                port.reason.clone().unwrap_or_default(),
                port_button(
                    format!("web-other-{index}").into(),
                    &target,
                    &port,
                    tabs,
                    None,
                    colors,
                ),
                colors,
            ));
        }
        Some(section)
    }

    fn render_ingress(&self, object: &Value, cx: &mut Context<Self>) -> gpui::Div {
        let colors = cx.colors().clone();
        let backends = ingress_backends(object);
        let mut section = section("Web views", &colors);
        if backends.is_empty() {
            return section.child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("No Service backends."),
            );
        }
        for (index, backend) in backends.iter().enumerate() {
            section = section.child(self.backend_row(index, backend, &colors, cx));
        }
        section.child(
            div()
                .text_size(u(11.5))
                .text_color(colors.text_muted)
                .child("Opens the backend Service through a forward, even when the ingress host isn't reachable from here."),
        )
    }

    fn backend_row(
        &self,
        index: usize,
        backend: &IngressBackend,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let service = self.service(&backend.service, cx);
        let port = backend_port(&backend.port, service.as_ref());
        let label = format!(
            "{}{} → {}:{}",
            backend.host.as_deref().unwrap_or("*"),
            backend.path,
            backend.service,
            match (&backend.port, port) {
                (_, Some(port)) => port.to_string(),
                (crate::target::BackendPort::Name(name), None) => name.clone(),
                (crate::target::BackendPort::Number(n), None) => n.to_string(),
            }
        );
        let target = ResourceRef::object(
            self.target.cluster.clone(),
            Gvr::new("", "v1", "services"),
            self.target.namespace.clone(),
            backend.service.clone(),
        );
        let button = match (port, &service) {
            (Some(port), service) => {
                let web_port = service
                    .as_ref()
                    .and_then(|s| {
                        ports_of(TargetKind::Service, s)
                            .into_iter()
                            .find(|p| p.port == port)
                    })
                    .map(|mut p| {
                        // An Ingress backend serves HTTP whatever its port looks like.
                        p.web = true;
                        p
                    })
                    .unwrap_or_else(|| WebPort {
                        port,
                        name: None,
                        app_protocol: None,
                        target_port: None,
                        web: true,
                        scheme: crate::target::Scheme::Http,
                        reason: None,
                    });
                let tabs = WebTarget::new(&target, port)
                    .zip(WebForwards::try_global(cx))
                    .map(|(t, f)| f.read(cx).tab_count(&t))
                    .unwrap_or_default();
                let path = (backend.path != "/").then(|| backend.path.clone());
                port_button(
                    format!("web-backend-{index}").into(),
                    &target,
                    &web_port,
                    tabs,
                    path,
                    colors,
                )
            }
            (None, _) => div()
                .text_size(u(11.5))
                .text_color(colors.text_dim)
                .child("port not found")
                .into_any_element(),
        };
        port_row(
            label,
            if service.is_some() {
                "backend Service".into()
            } else {
                "Service not found in this namespace".into()
            },
            button,
            colors,
        )
    }
}

/// Switches the private session of every port of an object (open tabs reload into it).
fn set_private(
    target: &ResourceRef,
    ports: &[u16],
    private: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let targets: Vec<WebTarget> = ports
        .iter()
        .filter_map(|p| WebTarget::new(target, *p))
        .collect();
    for web_target in &targets {
        if let Some(key) = PortKey::of(web_target, cx) {
            store::update(cx, |state| state.update_port(&key, |m| m.private = private));
        }
    }
    let tabs: Vec<_> = WebForwards::try_global(cx)
        .map(|f| {
            let f = f.read(cx);
            targets.iter().flat_map(|t| f.tabs(t)).collect()
        })
        .unwrap_or_default();
    for tab in tabs {
        tab.update(cx, |tab, cx| tab.set_private(private, window, cx));
    }
}

impl Render for WebSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = self.object(cx) else {
            return div().into_any_element();
        };
        match self.mode {
            Mode::Object(kind) => self.render_object(kind, &object, cx).into_any_element(),
            Mode::Ingress => self.render_ingress(&object, cx).into_any_element(),
        }
    }
}
