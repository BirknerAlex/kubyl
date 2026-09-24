//! The Clusters & kubeconfigs tab (mockup board 5): kubeconfig sources on the left, the merged
//! context table, safety settings and connection details on the right.

use std::path::PathBuf;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, ExternalPaths, FocusHandle, Focusable,
    FontWeight, Hsla, IntoElement, Render, SharedString, Styled, Subscription, Window, div,
    prelude::*, px, relative,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::switch::Switch;
use kubyl_core::{ClusterId, TabView, ViewRequest};
use kubyl_ui::{
    ActiveColors, Button, Colors, Icon, IconButton, IconName, StatusDot, fonts, h_flex, u, v_flex,
};

use super::{PasteKubeconfig, browse_kubeconfigs, clusters_view_kind, open_sign_in, switch_to};
use crate::auth::{AuthMethod, store};
use crate::kubeconfig::{CaSource, ContextInfo, Source, SourceKind};
use crate::settings::{ColorTag, display_path};
use crate::{ConnectionEvent, ConnectionManager, ConnectionState};

pub struct ClustersView {
    focus: FocusHandle,
    filter: Entity<InputState>,
    default_namespace: Entity<InputState>,
    selected: Option<ClusterId>,
    _subscriptions: Vec<Subscription>,
}

impl ClustersView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let manager = ConnectionManager::global(cx);
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter contexts"));
        let default_namespace =
            cx.new(|cx| InputState::new(window, cx).placeholder("from kubeconfig"));
        let subscriptions = vec![
            cx.subscribe(&manager, |this, _, event: &ConnectionEvent, cx| {
                if matches!(event, ConnectionEvent::ContextsChanged) {
                    this.ensure_selection(cx);
                }
                cx.notify();
            }),
            cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&default_namespace, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let value = input.read(cx).value().trim().to_string();
                    this.set_default_namespace(value, cx);
                }
            }),
        ];
        let mut this = Self {
            focus: cx.focus_handle(),
            filter,
            default_namespace,
            selected: manager.read(cx).active().cloned(),
            _subscriptions: subscriptions,
        };
        this.ensure_selection(cx);
        this.sync_inputs(window, cx);
        this
    }

    fn ensure_selection(&mut self, cx: &mut Context<Self>) {
        let manager = ConnectionManager::global(cx);
        let manager = manager.read(cx);
        if self
            .selected
            .as_ref()
            .is_none_or(|id| manager.context(id).is_none())
        {
            self.selected = manager
                .active()
                .cloned()
                .or_else(|| manager.all_contexts().first().map(|c| c.id.clone()));
        }
    }

    fn select(&mut self, id: ClusterId, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_ref() != Some(&id) {
            self.selected = Some(id);
            self.sync_inputs(window, cx);
            cx.notify();
        }
    }

    fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self
            .selected
            .as_ref()
            .and_then(|id| {
                ConnectionManager::global(cx)
                    .read(cx)
                    .context_settings(id)
                    .default_namespace
            })
            .unwrap_or_default();
        self.default_namespace
            .update(cx, |input, cx| input.set_value(value, window, cx));
    }

    fn set_default_namespace(&mut self, value: String, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let value = (!value.is_empty()).then_some(value);
        ConnectionManager::global(cx).update(cx, |m, cx| {
            if m.context_settings(&id).default_namespace != value {
                m.update_context_settings(&id, cx, |s| s.default_namespace = value);
            }
        });
    }

    fn update_settings(&self, cx: &mut App, f: impl FnOnce(&mut crate::settings::ContextSettings)) {
        if let Some(id) = &self.selected {
            ConnectionManager::global(cx).update(cx, |m, cx| m.update_context_settings(id, cx, f));
        }
    }

    fn render_sources(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let manager = ConnectionManager::global(cx);
        let manager = manager.read(cx);
        let selected_source = self
            .selected
            .as_ref()
            .and_then(|id| manager.context(id))
            .map(|c| c.source_path.clone());
        let sources: Vec<Source> = manager
            .sources()
            .iter()
            .filter(|s| s.spec.kind != SourceKind::Pasted || !s.files.is_empty())
            .cloned()
            .collect();
        let first_context = |source: &Source| {
            manager
                .all_contexts()
                .iter()
                .find(|c| c.source_path == source.spec.path)
                .map(|c| c.id.clone())
        };
        let rows: Vec<AnyElement> = sources
            .iter()
            .enumerate()
            .map(|(ix, source)| {
                let selected = selected_source.as_ref() == Some(&source.spec.path);
                let target = first_context(source);
                source_row(ix, source, selected, &colors)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if let Some(id) = target.clone() {
                            this.select(id, window, cx);
                        }
                    }))
                    .into_any_element()
            })
            .collect();

        v_flex()
            .id("kubeconfig-sources")
            .w(u(330.0))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(colors.border_variant)
            .px(u(12.0))
            .py(u(16.0))
            .gap(u(4.0))
            .overflow_y_scroll()
            .child(
                h_flex()
                    .mx(u(4.0))
                    .mb(u(8.0))
                    .child(
                        div()
                            .flex_1()
                            .text_size(u(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Kubeconfig sources"),
                    )
                    .child(
                        Button::new("add-source")
                            .icon(IconName::Plus)
                            .label("Add")
                            .on_click(|_, _, cx| browse_kubeconfigs(cx)),
                    ),
            )
            .children(rows)
            .when(sources.is_empty(), |this| {
                this.child(
                    div()
                        .px(u(12.0))
                        .py(u(10.0))
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child(if manager.is_loading() {
                            "Loading kubeconfigs…"
                        } else {
                            "No kubeconfigs yet."
                        }),
                )
            })
            .child(drop_zone(&colors, cx))
            .child(div().flex_1().min_h(u(12.0)))
            .child(
                div()
                    .px(u(4.0))
                    .text_size(u(11.5))
                    .line_height(u(17.0))
                    .text_color(colors.text_dim)
                    .child(
                        "Files are never modified. Contexts from every source are merged; \
                         name collisions get the file name as suffix.",
                    ),
            )
    }

    fn render_contexts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let query = self.filter.read(cx).value().to_lowercase();
        let manager_entity = ConnectionManager::global(cx);
        let manager = manager_entity.read(cx);
        let total = manager.contexts().count();
        let source_count = manager
            .sources()
            .iter()
            .filter(|s| s.context_count() > 0)
            .count();
        let contexts: Vec<(ContextInfo, ConnectionState, Hsla, SharedString, bool)> = manager
            .all_contexts()
            .iter()
            .filter(|c| {
                query.is_empty()
                    || c.name.to_lowercase().contains(&query)
                    || c.server
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query)
                    || manager.display_name(&c.id).to_lowercase().contains(&query)
            })
            .map(|c| {
                (
                    c.clone(),
                    manager.state(&c.id),
                    manager.color(&c.id, cx),
                    manager.display_name(&c.id),
                    manager.context_settings(&c.id).hidden,
                )
            })
            .collect();

        let header = h_flex()
            .gap(u(10.0))
            .child(
                div()
                    .text_size(u(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Contexts"),
            )
            .child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(format!(
                        "{total} from {source_count} source{}",
                        if source_count == 1 { "" } else { "s" }
                    )),
            )
            .child(div().flex_1())
            .child(
                div().w(u(220.0)).child(
                    Input::new(&self.filter)
                        .prefix(Icon::new(IconName::Search).size(12.0))
                        .cleanable(true),
                ),
            );

        let th = |label: &'static str| div().min_w_0().truncate().child(label);
        let table_header = table_row(&colors)
            .h(u(28.0))
            .bg(colors.subheader_background)
            .border_b_1()
            .border_color(colors.border_variant)
            .text_size(u(11.5))
            .text_color(colors.text_dim)
            .child(div().w(u(22.0)).flex_none())
            .child(col(1.1).child(th("CONTEXT")))
            .child(col(1.3).child(th("API SERVER")))
            .child(col(1.2).child(th("AUTH")))
            .child(div().w(u(150.0)).flex_none().child(th("STATUS")));

        let rows: Vec<AnyElement> = contexts
            .into_iter()
            .map(|(info, state, color, name, hidden)| {
                let selected = self.selected.as_ref() == Some(&info.id);
                let id = info.id.clone();
                let state_color = state.color(&colors);
                table_row(&colors)
                    .id(SharedString::from(format!("ctx-{}", info.id)))
                    .h(u(34.0))
                    .border_b_1()
                    .border_color(colors.row_border)
                    .cursor_pointer()
                    .when(selected, |this| {
                        this.bg(colors.selection)
                            .border_1()
                            .border_color(colors.accent)
                    })
                    .when(!selected, |this| {
                        let hover = colors.hover;
                        this.hover(move |s| s.bg(hover))
                    })
                    .when(hidden, |this| this.opacity(0.55))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        this.select(id.clone(), window, cx);
                        if event.click_count() >= 2 {
                            switch_to(&id, window, cx);
                        }
                    }))
                    .child(div().w(u(22.0)).flex_none().child(StatusDot::new(color)))
                    .child(
                        col(1.1)
                            .font_weight(FontWeight::MEDIUM)
                            .child(div().truncate().child(name)),
                    )
                    .child(
                        col(1.3)
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .text_color(colors.text_muted)
                            .child(
                                div()
                                    .truncate()
                                    .child(info.server.clone().unwrap_or_else(|| "—".into())),
                            ),
                    )
                    .child(
                        col(1.2)
                            .text_color(colors.text_muted)
                            .child(div().truncate().child(info.auth.label())),
                    )
                    .child(
                        h_flex()
                            .w(u(150.0))
                            .flex_none()
                            .gap(u(6.0))
                            .text_color(state_color)
                            .child(StatusDot::new(state_color))
                            .child(div().truncate().child(state.label())),
                    )
                    .into_any_element()
            })
            .collect();

        let empty = rows.is_empty();
        let table = v_flex()
            .bg(colors.panel)
            .border_1()
            .border_color(colors.border)
            .rounded(u(8.0))
            .overflow_hidden()
            .child(table_header)
            .children(rows)
            .when(empty, |this| {
                this.child(
                    div()
                        .p(u(16.0))
                        .text_color(colors.text_dim)
                        .child(if total == 0 {
                            "No contexts. Add a kubeconfig on the left."
                        } else {
                            "No context matches the filter."
                        }),
                )
            });

        let selected = self
            .selected
            .clone()
            .and_then(|id| Some((manager.context(&id)?.clone(), id)));
        let cards = selected.map(|(info, id)| {
            h_flex()
                .items_start()
                .gap(u(12.0))
                .child(self.render_safety(&id, &info, cx))
                .child(render_connection(&id, &info, cx))
        });

        v_flex()
            .id("contexts")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .px(u(18.0))
            .py(u(16.0))
            .gap(u(12.0))
            .child(header)
            .child(table)
            .children(cards)
    }

    fn render_safety(
        &self,
        id: &ClusterId,
        info: &ContextInfo,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let manager = ConnectionManager::global(cx);
        let settings = manager.read(cx).context_settings(id);
        let name = manager.read(cx).display_name(id);
        let current_color = settings
            .color
            .unwrap_or_else(|| ColorTag::default_for(id.as_str(), settings.production));

        let toggle = |id_: &'static str,
                      title: &'static str,
                      sub: &'static str,
                      on: bool,
                      f: fn(&mut crate::settings::ContextSettings, bool),
                      cx: &mut Context<Self>| {
            option_row(title, sub, &colors).child(Switch::new(id_).checked(on).on_click(
                cx.listener(move |this, checked: &bool, _, cx| {
                    let checked = *checked;
                    this.update_settings(cx, |s| f(s, checked));
                }),
            ))
        };

        let color_dots = h_flex()
            .gap(u(6.0))
            .children(ColorTag::ALL.iter().map(|tag| {
                let tag = *tag;
                let selected = tag == current_color;
                div()
                    .id(SharedString::from(format!("color-{tag:?}")))
                    .size(u(16.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(selected, |this| this.border_1().border_color(colors.text))
                    .child(div().size(u(10.0)).rounded_full().bg(tag.color(&colors)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.update_settings(cx, |s| s.color = Some(tag));
                    }))
            }));

        card(&colors)
            .flex_1()
            .min_w_0()
            .child(card_title(format!("{name} · safety"), &colors))
            .child(toggle(
                "production",
                "Production cluster",
                "Red accent in title bar, typed confirmation for delete / scale to 0",
                settings.production,
                |s, v| s.production = v,
                cx,
            ))
            .child(toggle(
                "read-only",
                "Read-only mode",
                "Block all mutating requests from this app",
                settings.read_only,
                |s, v| s.read_only = v,
                cx,
            ))
            .child(toggle(
                "hidden",
                "Hide context",
                "Leave it out of the sidebar and the cluster switcher",
                settings.hidden,
                |s, v| s.hidden = v,
                cx,
            ))
            .child(
                option_row(
                    "Default namespace",
                    match &info.namespace {
                        Some(_) => "Selected when you switch to this cluster",
                        None => "Selected when you switch to this cluster",
                    },
                    &colors,
                )
                .child(div().w(u(150.0)).child(Input::new(&self.default_namespace))),
            )
            .child(
                h_flex()
                    .gap(u(12.0))
                    .pt(u(8.0))
                    .child(div().flex_1().text_size(u(12.5)).child("Color"))
                    .child(color_dots),
            )
            .into_any_element()
    }
}

fn render_connection(
    id: &ClusterId,
    info: &ContextInfo,
    cx: &mut Context<ClustersView>,
) -> AnyElement {
    let colors = cx.colors().clone();
    let manager = ConnectionManager::global(cx);
    let m = manager.read(cx);
    let state = m.state(id);
    let cluster = m.cluster(id);
    let mut rows: Vec<(&'static str, AnyElement)> = Vec::new();
    let text = |s: String| div().truncate().child(s).into_any_element();
    let mono = |s: String| {
        div()
            .truncate()
            .font_family(fonts::MONO)
            .text_size(u(11.5))
            .child(s)
            .into_any_element()
    };
    let dim = |s: &'static str| {
        div()
            .text_color(colors.text_dim)
            .child(s)
            .into_any_element()
    };

    rows.push(("Source", mono(display_path(&info.file))));
    if info.name != info.context {
        rows.push(("Context", mono(info.context.clone())));
    }
    rows.push((
        "Server",
        mono(info.server.clone().unwrap_or_else(|| "—".into())),
    ));
    let auth = match &info.auth {
        AuthMethod::Oidc(params) => format!(
            "OIDC · {} · client {}",
            params.issuer_host(),
            params.client_id
        ),
        AuthMethod::Exec(exec) if exec.interactive => {
            format!("{} · interactive", info.auth.label())
        }
        other => other.label(),
    };
    rows.push(("Auth", text(auth)));
    let token = match (&info.auth, cluster.and_then(|c| c.credential_expires_at)) {
        (AuthMethod::Exec(_) | AuthMethod::Oidc(_), Some(at)) => Some(expiry_label(at)),
        (AuthMethod::Exec(_), None) if state.is_connected() => Some("cached · no expiry".into()),
        (AuthMethod::Oidc(_), None) => Some(format!("stored in {}", store::store_name())),
        _ => None,
    };
    if let Some(token) = token {
        rows.push(("Token", text(token)));
    }
    let ca = if info.insecure_skip_tls_verify {
        h_flex()
            .gap(u(6.0))
            .text_color(colors.yellow)
            .child(
                Icon::new(IconName::TriangleAlert)
                    .size(12.0)
                    .color(colors.yellow),
            )
            .child("TLS verification disabled")
            .into_any_element()
    } else {
        match &info.ca {
            CaSource::Inline => text("inline certificate".into()),
            CaSource::File(path) => mono(display_path(path)),
            CaSource::System => text("system trust store".into()),
        }
    };
    rows.push(("CA", ca));
    if let Some(name) = &info.tls_server_name {
        rows.push(("TLS name", mono(name.clone())));
    }
    let proxy = cluster
        .and_then(|c| c.proxy.clone())
        .or_else(|| info.proxy_url.clone());
    rows.push((
        "Proxy",
        match proxy {
            Some(proxy) => mono(proxy),
            None => dim("none"),
        },
    ));
    if let Some(cluster_info) = cluster.and_then(|c| c.info.as_ref()) {
        rows.push(("Version", text(cluster_info.summary())));
        if let Some(user) = &cluster_info.user {
            rows.push(("User", mono(user.clone())));
        }
    }
    if let Some(cluster) = cluster.filter(|_| state.is_connected()) {
        let discovery = match (&cluster.discovery, cluster.crd_count) {
            (Some(d), Some(crds)) => format!("{} kinds · {crds} CRDs", d.kind_count()),
            (Some(d), None) => format!("{} kinds", d.kind_count()),
            (None, _) => "running…".into(),
        };
        rows.push(("Discovery", text(discovery)));
        let ns = &cluster.namespaces;
        rows.push((
            "Namespaces",
            if ns.listed {
                text(format!("{} (watched)", ns.names.len()))
            } else {
                text(format!("{} (listing not allowed)", ns.names.len()))
            },
        ));
    }

    let error = match &state {
        ConnectionState::AuthRequired {
            message, detail, ..
        } => Some((message.clone(), detail.clone())),
        ConnectionState::Unreachable { message, .. } | ConnectionState::Forbidden(message) => {
            Some((message.clone(), None))
        }
        _ => info.error.clone().map(|e| (e, None)),
    };
    let sign_in = matches!(state, ConnectionState::AuthRequired { sign_in: true, .. })
        || (info.auth.is_oidc() && !state.is_connected() && state != ConnectionState::Connecting);

    let id_switch = id.clone();
    let id_connect = id.clone();
    let id_sign_in = id.clone();
    let connected = state.is_connected();
    let busy = state == ConnectionState::Connecting;
    let buttons = h_flex()
        .gap(u(6.0))
        .pt(u(10.0))
        .child(
            Button::new("switch-to")
                .primary()
                .label("Switch to")
                .on_click(move |_, window, cx| switch_to(&id_switch, window, cx)),
        )
        .child(
            Button::new("connect")
                .icon(if connected {
                    IconName::X
                } else {
                    IconName::RefreshCw
                })
                .label(if connected {
                    "Disconnect"
                } else if busy {
                    "Connecting…"
                } else {
                    "Connect"
                })
                .disabled(busy)
                .on_click(move |_, _, cx| {
                    ConnectionManager::global(cx).update(cx, |m, cx| {
                        if m.state(&id_connect).is_connected() {
                            m.disconnect(&id_connect, cx);
                        } else {
                            m.connect(&id_connect, cx);
                        }
                    })
                }),
        )
        .when(sign_in, |this| {
            this.child(
                Button::new("sign-in")
                    .icon(IconName::Key)
                    .label("Sign in…")
                    .on_click(move |_, window, cx| open_sign_in(id_sign_in.clone(), window, cx)),
            )
        });

    card(&colors)
        .flex_1()
        .min_w_0()
        .child(card_title("Connection".into(), &colors))
        .child(
            v_flex()
                .gap(u(5.0))
                .text_size(u(12.0))
                .children(rows.into_iter().map(|(label, value)| {
                    h_flex()
                        .gap(u(8.0))
                        .child(
                            div()
                                .w(u(104.0))
                                .flex_none()
                                .text_color(colors.text_dim)
                                .child(label),
                        )
                        .child(div().flex_1().min_w_0().child(value))
                })),
        )
        .children(error.map(|(message, detail)| {
            v_flex()
                .mt(u(10.0))
                .p(u(8.0))
                .gap(u(4.0))
                .rounded(u(6.0))
                .bg(colors.error_row_background)
                .text_size(u(12.0))
                .text_color(colors.red)
                .child(message)
                .children(detail.map(|detail| {
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(11.0))
                        .text_color(colors.text_muted)
                        .child(detail)
                }))
        }))
        .child(buttons)
        .into_any_element()
}

fn expiry_label(at: jiff::Timestamp) -> String {
    let secs = at.as_second() - jiff::Timestamp::now().as_second();
    if secs <= 0 {
        "expired".into()
    } else if secs < 120 {
        format!("cached · expires in {secs}s")
    } else if secs < 7200 {
        format!("cached · expires in {}m", secs / 60)
    } else {
        format!("cached · expires in {}h", secs / 3600)
    }
}

fn col(weight: f32) -> gpui::Div {
    let mut col = div().min_w_0().overflow_hidden().pr(u(10.0));
    let style = col.style();
    style.flex_grow = Some(weight);
    style.flex_shrink = Some(1.0);
    style.flex_basis = Some(relative(0.).into());
    col
}

fn table_row(_colors: &Colors) -> gpui::Div {
    h_flex().px(u(12.0)).whitespace_nowrap()
}

fn card(colors: &Colors) -> gpui::Div {
    v_flex()
        .bg(colors.panel)
        .border_1()
        .border_color(colors.border)
        .rounded(u(8.0))
        .px(u(16.0))
        .py(u(14.0))
}

fn card_title(title: String, colors: &Colors) -> impl IntoElement {
    div()
        .mb(u(8.0))
        .text_size(u(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_dim)
        .child(title.to_uppercase())
}

fn option_row(title: &'static str, sub: &'static str, colors: &Colors) -> gpui::Div {
    h_flex()
        .gap(u(12.0))
        .py(u(8.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_size(u(12.5)).child(title))
                .child(
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(sub),
                ),
        )
}

fn source_row(
    ix: usize,
    source: &Source,
    selected: bool,
    colors: &Colors,
) -> gpui::Stateful<gpui::Div> {
    let icon = match source.spec.kind {
        SourceKind::Env => IconName::Zap,
        SourceKind::Pasted => IconName::Copy,
        _ if source.spec.is_dir => IconName::Folder,
        _ => IconName::File,
    };
    let errors: Vec<&str> = source.errors().collect();
    let count = source.context_count();
    let contexts = format!("{count} context{}", if count == 1 { "" } else { "s" });
    let sub = match source.spec.kind {
        SourceKind::Env => format!("merged · {} files · {contexts}", source.spec.files.len()),
        _ if source.spec.is_dir => format!("{} files · {contexts} · watched", source.files.len()),
        _ if source.has_oidc() => format!("{contexts} · OIDC"),
        _ => format!("{contexts} · watched for changes"),
    };
    let user_source = source.spec.kind == SourceKind::User;
    let path = source.spec.path.clone();
    let hover = colors.hover;
    h_flex()
        .id(("source", ix))
        .group("source")
        .items_start()
        .gap(u(10.0))
        .px(u(12.0))
        .py(u(10.0))
        .rounded(u(6.0))
        .cursor_pointer()
        .when(selected, |this| {
            this.bg(colors.selection)
                .border_1()
                .border_color(colors.accent)
        })
        .when(!selected, |this| this.hover(move |s| s.bg(hover)))
        .child(
            div()
                .pt(u(1.0))
                .child(Icon::new(icon).size(15.0).color(if selected {
                    colors.accent
                } else {
                    colors.text_dim
                })),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .text_color(colors.text)
                        .child(source.spec.label()),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(sub),
                )
                .children(errors.first().map(|err| {
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.red)
                        .child(format!(
                            "{}{}",
                            err,
                            if errors.len() > 1 {
                                format!(" (+{} more)", errors.len() - 1)
                            } else {
                                String::new()
                            }
                        ))
                })),
        )
        .when(!errors.is_empty(), |this| {
            this.child(
                Icon::new(IconName::TriangleAlert)
                    .size(13.0)
                    .color(colors.red),
            )
        })
        .when(errors.is_empty() && source.has_oidc(), |this| {
            this.child(Icon::new(IconName::Key).size(13.0).color(colors.yellow))
        })
        .when(user_source, |this| {
            this.child(
                div()
                    .invisible()
                    .group_hover("source", |s| s.visible())
                    .child(
                        IconButton::new(("remove-source", ix), IconName::X)
                            .icon_size(12.0)
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let path: PathBuf = path.clone();
                                ConnectionManager::global(cx)
                                    .update(cx, |m, cx| m.remove_source(&path, cx));
                            }),
                    ),
            )
        })
}

fn drop_zone(colors: &Colors, cx: &mut Context<ClustersView>) -> impl IntoElement {
    let accent = colors.accent;
    let link = |id: &'static str, label: &'static str, on_click: fn(&mut Window, &mut App)| {
        div()
            .id(id)
            .text_color(accent)
            .cursor_pointer()
            .hover(|s| s.underline())
            .on_click(move |_, window, cx| on_click(window, cx))
            .child(label)
    };
    v_flex()
        .id("drop-zone")
        .mt(u(10.0))
        .items_center()
        .gap(u(2.0))
        .px(u(14.0))
        .py(u(18.0))
        .rounded(u(8.0))
        .border_1()
        .border_dashed()
        .border_color(colors.border)
        .text_size(u(12.5))
        .line_height(u(19.0))
        .text_color(colors.text_dim)
        .drag_over::<ExternalPaths>(move |style, _, _, _| {
            style.border_color(accent).bg(accent.opacity(0.08))
        })
        .on_drop(cx.listener(|_, paths: &ExternalPaths, _, cx| {
            super::add_paths(paths.paths().to_vec(), cx);
        }))
        .child(Icon::new(IconName::Upload).size(18.0))
        .child("Drop kubeconfig files here")
        .child(
            h_flex()
                .gap(u(4.0))
                .text_size(u(12.0))
                .child("or")
                .child(link("browse", "browse", |_, cx| browse_kubeconfigs(cx)))
                .child("·")
                .child(link("paste", "paste YAML", |window, cx| {
                    window.dispatch_action(Box::new(PasteKubeconfig), cx)
                })),
        )
}

impl Focusable for ClustersView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for ClustersView {
    fn tab_title(&self, _: &App) -> SharedString {
        "Clusters & kubeconfigs".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Settings.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(ViewRequest::new(clusters_view_kind()))
    }
}

impl Render for ClustersView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let accent = colors.accent;
        h_flex()
            .id("clusters-view")
            .key_context("ClustersView")
            .track_focus(&self.focus)
            .size_full()
            .items_start()
            .bg(colors.background)
            .text_color(colors.text)
            // Kubeconfigs can be dropped anywhere on the view, not only on the drop zone.
            .drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(accent.opacity(0.04)))
            .on_drop(cx.listener(|_, paths: &ExternalPaths, _, cx| {
                super::add_paths(paths.paths().to_vec(), cx);
            }))
            .child(self.render_sources(cx))
            .child(self.render_contexts(cx))
            .child(div().w(px(0.)))
    }
}
