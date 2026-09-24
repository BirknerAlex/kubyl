//! The cluster switcher opened from the title bar (`SwitchCluster`): contexts grouped by
//! kubeconfig source, with status dots and a filter. ↑/↓ to move, ↵ to switch, esc to close.

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement,
    KeyBinding, Render, SharedString, Subscription, Window, actions, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_core::ClusterId;
use kubyl_ui::{ActiveColors, Icon, IconName, Kbd, StatusDot, fonts, h_flex, u, v_flex};

use super::{open_clusters, switch_to};
use crate::ConnectionManager;
use crate::kubeconfig::SourceKind;
use crate::settings::display_path;

actions!(cluster_switcher, [SelectNext, SelectPrevious, Confirm]);

const CONTEXT: &str = "ClusterSwitcher";

pub(crate) fn bind_keys(cx: &mut App) {
    // The filter input has focus; `> Input` makes these win over the input's own up/down.
    let in_input = format!("{CONTEXT} > Input");
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("up", SelectPrevious, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm, Some(CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(&in_input)),
        KeyBinding::new("up", SelectPrevious, Some(&in_input)),
    ]);
}

/// Opens the switcher in `window`.
pub fn open_switcher(window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| ClusterSwitcher::new(window, cx));
    let colors = cx.colors().clone();
    let dialog_view = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(560.))
            .margin_top(px(60.))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            .child(dialog_view.clone())
    });
    let focus = view.read(cx).query.read(cx).focus_handle(cx);
    window.focus(&focus, cx);
}

struct Item {
    id: ClusterId,
    name: SharedString,
    server: String,
    source: String,
    color: gpui::Hsla,
    state_color: gpui::Hsla,
    state: String,
    production: bool,
    active: bool,
}

pub struct ClusterSwitcher {
    query: Entity<InputState>,
    selected: usize,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl ClusterSwitcher {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Switch to cluster…"));
        let subscription = cx.subscribe_in(
            &query,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.selected = 0;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.confirm(window, cx),
                _ => {}
            },
        );
        let manager = ConnectionManager::global(cx);
        let mut this = Self {
            query,
            selected: 0,
            focus: cx.focus_handle(),
            _subscription: subscription,
        };
        let active = manager.read(cx).active().cloned();
        if let Some(ix) = this
            .items(cx)
            .iter()
            .position(|i| Some(&i.id) == active.as_ref())
        {
            this.selected = ix;
        }
        this
    }

    fn items(&self, cx: &App) -> Vec<Item> {
        let query = self.query.read(cx).value().to_lowercase();
        let manager = ConnectionManager::global(cx);
        let manager = manager.read(cx);
        let colors = cx.colors();
        manager
            .contexts()
            .filter(|c| {
                query.is_empty()
                    || manager.display_name(&c.id).to_lowercase().contains(&query)
                    || c.server
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&query)
            })
            .map(|c| {
                let state = manager.state(&c.id);
                Item {
                    id: c.id.clone(),
                    name: manager.display_name(&c.id),
                    server: c.server.clone().unwrap_or_default(),
                    source: match c.source {
                        SourceKind::Env => format!("$KUBECONFIG · {}", display_path(&c.file)),
                        _ => display_path(&c.file),
                    },
                    color: manager.color(&c.id, cx),
                    state_color: state.color(colors),
                    state: state.label(),
                    production: manager.context_settings(&c.id).production,
                    active: manager.active() == Some(&c.id),
                }
            })
            .collect()
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.items(cx);
        if let Some(item) = items.get(self.selected) {
            let id = item.id.clone();
            window.close_dialog(cx);
            switch_to(&id, window, cx);
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.items(cx).len();
        if count > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(count as isize) as usize;
            cx.notify();
        }
    }
}

impl Focusable for ClusterSwitcher {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ClusterSwitcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let items = self.items(cx);
        self.selected = self.selected.min(items.len().saturating_sub(1));
        let mut list = v_flex().p(u(6.0));
        let mut last_source: Option<String> = None;
        for (ix, item) in items.iter().enumerate() {
            if last_source.as_ref() != Some(&item.source) {
                last_source = Some(item.source.clone());
                list = list.child(
                    div()
                        .px(u(12.0))
                        .pt(u(8.0))
                        .pb(u(4.0))
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(11.0))
                        .text_color(colors.text_dim)
                        .child(item.source.clone()),
                );
            }
            let selected = ix == self.selected;
            let hover = colors.hover;
            list = list.child(
                h_flex()
                    .id(("cluster", ix))
                    .h(u(32.0))
                    .px(u(12.0))
                    .gap(u(10.0))
                    .rounded(u(5.0))
                    .cursor_pointer()
                    .when(selected, |this| this.bg(colors.selection))
                    .when(!selected, |this| this.hover(move |s| s.bg(hover)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.selected = ix;
                        this.confirm(window, cx);
                    }))
                    .child(Icon::new(IconName::ShipWheel).size(14.0).color(item.color))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(if item.active {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .child(item.name.clone()),
                    )
                    .when(item.production, |this| this.child(kubyl_ui::ProdBadge))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(item.server.clone()),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(u(6.0))
                            .text_size(u(11.5))
                            .text_color(item.state_color)
                            .child(StatusDot::new(item.state_color))
                            .child(item.state.clone()),
                    ),
            );
        }
        if items.is_empty() {
            list = list.child(
                div()
                    .px(u(12.0))
                    .py(u(10.0))
                    .text_color(colors.text_dim)
                    .child("No matching cluster."),
            );
        }

        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| this.confirm(window, cx)))
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(
                div()
                    .p(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        Input::new(&self.query)
                            .prefix(
                                Icon::new(IconName::ShipWheel)
                                    .size(13.0)
                                    .color(colors.accent),
                            )
                            .appearance(false),
                    ),
            )
            .child(
                div()
                    .id("cluster-list")
                    .max_h(u(420.0))
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(
                h_flex()
                    .gap(u(16.0))
                    .px(u(14.0))
                    .py(u(8.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .text_size(u(11.5))
                    .text_color(colors.text_dim)
                    .child(
                        h_flex()
                            .gap(u(4.0))
                            .child(Kbd::keystroke("enter"))
                            .child("switch"),
                    )
                    .child(
                        h_flex()
                            .gap(u(4.0))
                            .child(Kbd::keystroke("up"))
                            .child(Kbd::keystroke("down"))
                            .child("move"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("manage-clusters")
                            .text_color(colors.accent)
                            .cursor_pointer()
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                open_clusters(window, cx);
                            })
                            .child("Manage kubeconfigs…"),
                    ),
            )
    }
}
