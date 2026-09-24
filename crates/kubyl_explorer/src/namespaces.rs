//! The namespace switcher (`SwitchNamespace`, title bar): namespaces of the active cluster,
//! "all namespaces", and a star per row to add or remove favorites.

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, Render,
    SharedString, Subscription, Window, actions, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_core::{ActiveContext, ClusterId, Notification, NotificationCenter};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Icon, IconButton, IconName, Kbd, fonts, h_flex, u, v_flex};

use crate::favorites::{Favorite, Favorites};

actions!(
    namespace_switcher,
    [SelectNext, SelectPrevious, Confirm, ToggleFavorite]
);

const CONTEXT: &str = "NamespaceSwitcher";

pub(crate) fn init(cx: &mut App) {
    let in_input = format!("{CONTEXT} > Input");
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("up", SelectPrevious, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm, Some(CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(&in_input)),
        KeyBinding::new("up", SelectPrevious, Some(&in_input)),
        KeyBinding::new("secondary-s", ToggleFavorite, Some(CONTEXT)),
    ]);
}

/// Opens the switcher for the active cluster.
pub fn open(window: &mut Window, cx: &mut App) {
    let Some(cluster) = ActiveContext::global(cx)
        .cluster
        .as_ref()
        .map(|c| c.id.clone())
    else {
        NotificationCenter::push(cx, Notification::info("Pick a cluster first."));
        return;
    };
    let view = cx.new(|cx| NamespaceSwitcher::new(cluster, window, cx));
    let colors = cx.colors().clone();
    let dialog_view = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(480.))
            .margin_top(px(60.))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            .child(dialog_view.clone())
    });
    let focus = view.read(cx).query.read(cx).focus_handle(cx);
    window.focus(&focus, cx);
}

pub struct NamespaceSwitcher {
    cluster: ClusterId,
    query: Entity<InputState>,
    selected: usize,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl NamespaceSwitcher {
    fn new(cluster: ClusterId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Switch to namespace…"));
        let subscriptions =
            vec![
                cx.subscribe_in(&query, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => {
                            this.selected = 0;
                            cx.notify();
                        }
                        InputEvent::PressEnter { .. } => this.confirm(window, cx),
                        _ => {}
                    }
                }),
                cx.observe(&Favorites::global(cx), |_, _, cx| cx.notify()),
            ];
        let mut this = Self {
            cluster,
            query,
            selected: 0,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        let current = ActiveContext::global(cx).namespace.clone();
        if let Some(ix) = this.items(cx).iter().position(|n| *n == current) {
            this.selected = ix;
        }
        this
    }

    /// `None` = all namespaces, then the names matching the query.
    fn items(&self, cx: &App) -> Vec<Option<SharedString>> {
        let query = self.query.read(cx).value().to_lowercase();
        let names = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).namespaces(&self.cluster).names)
            .unwrap_or_default();
        let mut items = Vec::new();
        if query.is_empty() {
            items.push(None);
        }
        items.extend(
            names
                .into_iter()
                .filter(|n| query.is_empty() || n.to_lowercase().contains(&query))
                .map(|n| Some(SharedString::from(n))),
        );
        // A typed name that isn't listed (listing forbidden) can still be used.
        if !query.is_empty() && !items.iter().any(|i| i.as_deref() == Some(query.as_str())) {
            items.push(Some(query.into()));
        }
        items
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.items(cx);
        let Some(namespace) = items.get(self.selected).cloned() else {
            return;
        };
        window.close_dialog(cx);
        let active = ActiveContext::global(cx).clone();
        ActiveContext::set(
            cx,
            ActiveContext {
                namespace,
                ..active
            },
        );
    }

    fn toggle_favorite(&mut self, namespace: &str, cx: &mut Context<Self>) {
        let Some(context) = ConnectionManager::global(cx)
            .read(cx)
            .context(&self.cluster)
            .cloned()
        else {
            return;
        };
        let favorite = Favorite::new(&context, namespace);
        Favorites::global(cx).update(cx, |f, cx| {
            f.toggle(favorite, cx);
        });
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.items(cx).len();
        if count > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(count as isize) as usize;
            cx.notify();
        }
    }
}

impl Focusable for NamespaceSwitcher {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for NamespaceSwitcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let items = self.items(cx);
        self.selected = self.selected.min(items.len().saturating_sub(1));
        let manager = ConnectionManager::global(cx);
        let (cluster_name, listed) = {
            let m = manager.read(cx);
            (
                m.display_name(&self.cluster),
                m.namespaces(&self.cluster).listed,
            )
        };
        let context = manager.read(cx).context(&self.cluster).cloned();
        let current = ActiveContext::global(cx).namespace.clone();
        let favorites = Favorites::global(cx);
        let mut list = v_flex().p(u(6.0));
        for (ix, item) in items.iter().enumerate() {
            let selected = ix == self.selected;
            let hover = colors.hover;
            let is_current = *item == current;
            let starred = match (item, &context) {
                (Some(ns), Some(context)) => favorites.read(cx).contains_namespace(context, ns),
                _ => false,
            };
            let label = item.clone().unwrap_or_else(|| "All namespaces".into());
            let row = h_flex()
                .id(("namespace", ix))
                .h(u(30.0))
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
                .child(
                    Icon::new(if item.is_some() {
                        IconName::Folder
                    } else {
                        IconName::Layers
                    })
                    .size(14.0),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(item.is_some(), |this| {
                            this.font_family(fonts::MONO).text_size(u(12.5))
                        })
                        .when(is_current, |this| this.text_color(colors.accent))
                        .child(label),
                );
            let row = match item.clone() {
                Some(ns) => row.child(
                    IconButton::new(
                        ("star", ix),
                        if starred {
                            IconName::StarFilled
                        } else {
                            IconName::Star
                        },
                    )
                    .icon_size(13.0)
                    .toggled(starred)
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_favorite(&ns, cx))),
                ),
                None => row,
            };
            list = list.child(row);
        }
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| this.confirm(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleFavorite, _, cx| {
                if let Some(Some(ns)) = this.items(cx).get(this.selected).cloned() {
                    this.toggle_favorite(&ns, cx);
                }
            }))
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(
                div()
                    .p(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        Input::new(&self.query)
                            .prefix(Icon::new(IconName::Folder).size(13.0).color(colors.accent))
                            .appearance(false),
                    ),
            )
            .child(
                div()
                    .id("namespace-list")
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
                            .child(Kbd::keystroke("secondary-s"))
                            .child("favorite"),
                    )
                    .child(div().flex_1())
                    .child(if listed {
                        format!("{cluster_name}")
                    } else {
                        format!("{cluster_name} · listing forbidden, type a name")
                    }),
            )
    }
}
