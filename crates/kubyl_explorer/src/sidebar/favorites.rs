//! The Favorites section: `namespace · cluster` rows (or `view namespace · cluster` for a saved
//! tab) with the cluster's color dot, a tooltip with the kubeconfig file, drag to reorder, rename
//! and remove from the context menu.

use gpui::{
    App, AppContext as _, Context, FocusHandle, Focusable, IntoElement, Render, SharedString,
    Subscription, Window, div, prelude::*,
};
use gpui_component::menu::{ContextMenuExt as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActiveContext, Notification, NotificationCenter, ResourceRef, ViewKind, ViewRegistry,
    ViewRequest,
};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_settings::State;
use kubyl_ui::{ActiveColors, Icon, IconName, SectionHeader, StatusDot, fonts, h_flex, u, v_flex};

use crate::favorites::{self, Favorite, Favorites};
use crate::list::PendingFilter;
use crate::settings::TreeState;

/// Payload while a favorite is dragged.
#[derive(Clone)]
struct DraggedFavorite {
    index: usize,
    label: SharedString,
}

struct DragPreview {
    label: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors();
        h_flex()
            .px(u(8.0))
            .py(u(3.0))
            .gap(u(6.0))
            .rounded(u(4.0))
            .bg(colors.elevated)
            .border_1()
            .border_color(colors.accent)
            .text_size(u(12.0))
            .text_color(colors.text)
            .child(
                Icon::new(IconName::StarFilled)
                    .size(12.0)
                    .color(colors.yellow),
            )
            .child(self.label.clone())
    }
}

pub struct FavoritesSection {
    collapsed: bool,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl FavoritesSection {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let favorites = Favorites::global(cx);
        let mut subscriptions = vec![
            cx.observe(&favorites, |_, _, cx| cx.notify()),
            cx.observe_global::<ActiveContext>(|_, cx| cx.notify()),
        ];
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.subscribe(&manager, |_, _, event: &ConnectionEvent, cx| {
                if matches!(
                    event,
                    ConnectionEvent::ContextsChanged | ConnectionEvent::StateChanged(_)
                ) {
                    cx.notify();
                }
            }));
        }
        Self {
            collapsed: State::get::<TreeState>(cx)
                .collapsed_sections
                .contains("favorites"),
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }
}

/// Opens a favorite: connects its cluster, makes it active, selects the namespace and opens the
/// list (with the favorite's label selector as the filter).
pub fn open_favorite(favorite: &Favorite, window: &mut Window, cx: &mut App) {
    let Some(cluster) = activate_favorite(favorite, cx) else {
        return;
    };
    // A saved tab: reopen its view (an open tab for the same request is activated).
    if let Some(request) = favorite.request(&cluster) {
        window.dispatch_action(Box::new(OpenView(request)), cx);
        return;
    }
    let gvr = favorite.gvr();
    if let Some(selector) = &favorite.selector {
        cx.set_global(PendingFilter(Some((
            cluster.clone(),
            gvr.clone(),
            selector.clone(),
        ))));
    }
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(
            ViewKind::Table,
            ResourceRef::list(cluster, gvr, None),
        ))),
        cx,
    );
}

/// Opens the namespace variant of the Overview for a favorite.
pub fn open_favorite_overview(favorite: &Favorite, window: &mut Window, cx: &mut App) {
    open_favorite_view(favorite, ViewKind::Overview, window, cx);
}

/// Opens a cluster-level view of `kind` scoped to the favorite's namespace (the namespace
/// Overview, a crate's [`crate::catalog::NamespaceView`]).
pub fn open_favorite_view(favorite: &Favorite, kind: ViewKind, window: &mut Window, cx: &mut App) {
    let Some(cluster) = activate_favorite(favorite, cx) else {
        return;
    };
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(
            kind,
            ResourceRef::list(
                cluster,
                kubyl_core::Gvr::new("", "", ""),
                favorite.namespace.clone(),
            ),
        ))),
        cx,
    );
}

/// Connects the favorite's cluster, makes it active and selects its namespace.
fn activate_favorite(favorite: &Favorite, cx: &mut App) -> Option<kubyl_core::ClusterId> {
    let Some(cluster) = favorites::cluster_of(favorite, cx) else {
        NotificationCenter::push(
            cx,
            Notification::warning(format!(
                "Context {} isn't in any loaded kubeconfig ({}).",
                favorite.context,
                favorite.file.display()
            )),
        );
        return None;
    };
    let manager = ConnectionManager::global(cx);
    if manager.read(cx).active() != Some(&cluster) {
        manager.update(cx, |m, cx| m.activate(&cluster, cx));
    } else {
        manager.update(cx, |m, cx| m.ensure_connected(&cluster, cx));
    }
    // A view that isn't scoped to a namespace leaves the title bar's alone; one that shows all
    // of them switches it to "all namespaces" (a list adopts the title bar's namespace).
    if favorite.namespace.is_some() || favorite.all_namespaces {
        let active = ActiveContext::global(cx).clone();
        ActiveContext::set(
            cx,
            ActiveContext {
                namespace: favorite.namespace.clone().map(Into::into),
                ..active
            },
        );
    }
    Some(cluster)
}

impl Focusable for FavoritesSection {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for FavoritesSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let favorites = Favorites::global(cx);
        let items: Vec<Favorite> = favorites.read(cx).items().to_vec();
        let active = ActiveContext::global(cx).clone();
        let manager = ConnectionManager::try_global(cx);

        let mut list = v_flex().track_focus(&self.focus).w_full().child(
            SectionHeader::new("favorites", "Favorites")
                .count(items.len().to_string())
                .collapsed(self.collapsed)
                .on_toggle(cx.listener(|this, _, _, cx| {
                    this.collapsed = !this.collapsed;
                    let collapsed = this.collapsed;
                    State::update::<TreeState>(cx, |s| {
                        if collapsed {
                            s.collapsed_sections.insert("favorites".into());
                        } else {
                            s.collapsed_sections.remove("favorites");
                        }
                    });
                    cx.notify();
                })),
        );
        if self.collapsed {
            return list;
        }
        if items.is_empty() {
            return list.child(
                div()
                    .px(u(12.0))
                    .py(u(4.0))
                    .text_size(u(12.0))
                    .text_color(colors.text_faint)
                    .child(
                        "Star a namespace in the namespace switcher, or use the star in the \
                         tab bar to pin the current view here.",
                    ),
            );
        }
        for (index, favorite) in items.iter().enumerate() {
            let cluster = favorites::cluster_of(favorite, cx);
            let (cluster_name, color, state) = match (&cluster, &manager) {
                (Some(id), Some(m)) => (
                    m.read(cx).display_name(id).to_string(),
                    m.read(cx).color(id, cx),
                    m.read(cx).state(id),
                ),
                _ => (
                    favorite.context.clone(),
                    colors.text_faint,
                    kubyl_kube::ConnectionState::Disconnected,
                ),
            };
            // Faint while the cluster isn't connected; the tooltip says why.
            let connected = state.is_connected();
            // A saved view can't tell from the title bar whether it's the one on screen.
            let selected = cluster.is_some()
                && favorite.view.is_none()
                && active.cluster.as_ref().map(|c| &c.id) == cluster.as_ref()
                && active.namespace.as_deref() == favorite.namespace.as_deref();
            let missing = cluster.is_none();
            let label: SharedString = favorite.label().to_string().into();
            let tooltip: SharedString = format!(
                "{} · {}{}{}\n{}",
                favorite.context,
                favorite.file.display(),
                favorite
                    .view
                    .as_ref()
                    .zip(favorite.namespace.as_ref())
                    .map(|(_, namespace)| format!(" · {namespace}"))
                    .unwrap_or_default(),
                favorite
                    .selector
                    .as_ref()
                    .map(|s| format!(" · {s}"))
                    .unwrap_or_default(),
                if missing {
                    "The context isn't in any loaded kubeconfig.".to_string()
                } else {
                    super::clusters::state_line(&state)
                }
            )
            .into();
            let hover = colors.hover;
            let accent = colors.accent;
            let open = favorite.clone();
            let menu_favorite = favorite.clone();
            let drag = DraggedFavorite {
                index,
                label: label.clone(),
            };
            let row = h_flex()
                .id(("favorite", index))
                .relative()
                .h(u(kubyl_ui::sizes::TREE_ROW))
                .pl(u(12.0))
                .pr(u(10.0))
                .gap(u(6.0))
                .whitespace_nowrap()
                .cursor_pointer()
                .map(|this| {
                    if selected {
                        this.bg(colors.selection).child(
                            div()
                                .absolute()
                                .inset_0()
                                .border_1()
                                .border_color(colors.accent),
                        )
                    } else {
                        this.hover(move |s| s.bg(hover))
                    }
                })
                .child(
                    Icon::new(IconName::StarFilled)
                        .size(13.0)
                        .color(if missing {
                            colors.text_faint
                        } else {
                            colors.yellow
                        }),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(u(4.0))
                        .overflow_hidden()
                        .child(
                            div()
                                .flex_none()
                                .text_color(if missing || !connected {
                                    colors.text_dim
                                } else {
                                    colors.text
                                })
                                .child(label.clone()),
                        )
                        // The kind of a namespace favorite, or the namespace of a saved view.
                        .when_some(
                            favorite
                                .kind
                                .clone()
                                .or_else(|| favorite.view.as_ref().and(favorite.namespace.clone())),
                            |this, chip| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .font_family(fonts::MONO)
                                        .text_size(u(11.0))
                                        .text_color(colors.text_dim)
                                        .child(chip),
                                )
                            },
                        )
                        .child(
                            div()
                                .truncate()
                                .text_color(if connected {
                                    colors.text_dim
                                } else {
                                    colors.text_faint
                                })
                                .child(format!("· {cluster_name}")),
                        )
                        .when(missing, |this| {
                            this.child(
                                div()
                                    .text_size(u(11.0))
                                    .text_color(colors.red)
                                    .child("missing"),
                            )
                        }),
                )
                .child(StatusDot::new(color))
                .tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                })
                .on_click(move |_, window, cx| open_favorite(&open, window, cx))
                .on_drag(drag, |drag, _, _, cx| {
                    cx.new(|_| DragPreview {
                        label: drag.label.clone(),
                    })
                })
                .drag_over::<DraggedFavorite>(move |style, _, _, _| {
                    style.border_t_2().border_color(accent)
                })
                .on_drop(cx.listener(move |_, drag: &DraggedFavorite, _, cx| {
                    Favorites::global(cx).update(cx, |f, cx| f.move_item(drag.index, index, cx));
                }))
                .context_menu(move |menu, _, cx| {
                    let open = menu_favorite.clone();
                    let in_workspace = !Favorites::global(cx)
                        .read(cx)
                        .is_excluded_from_workspace(index);
                    let alias = menu_favorite.alias.clone().unwrap_or_default();
                    let overview = open.clone();
                    // Namespace favorites open the namespace's views; a saved view is one view.
                    let namespace_favorite = open.view.is_none();
                    let has_overview =
                        namespace_favorite && ViewRegistry::is_registered(cx, &ViewKind::Overview);
                    menu.item(
                        PopupMenuItem::new("Open")
                            .on_click(move |_, window, cx| open_favorite(&open, window, cx)),
                    )
                    .when(has_overview, |menu| {
                        menu.item(PopupMenuItem::new("Open Namespace Overview").on_click(
                            move |_, window, cx| open_favorite_overview(&overview, window, cx),
                        ))
                    })
                    .map(|mut menu| {
                        if !namespace_favorite {
                            return menu;
                        }
                        for view in crate::catalog::namespace_views(cx) {
                            let favorite = menu_favorite.clone();
                            menu = menu.item(PopupMenuItem::new(view.label).on_click(
                                move |_, window, cx| {
                                    open_favorite_view(&favorite, view.kind.clone(), window, cx)
                                },
                            ));
                        }
                        menu.item(PopupMenuItem::new("Open Favorites Workspace").on_click(
                            |_, window, cx| {
                                window.dispatch_action(Box::new(crate::OpenFavoritesWorkspace), cx)
                            },
                        ))
                        .item(
                            PopupMenuItem::new("Include in Workspace")
                                .checked(in_workspace)
                                .on_click(move |_, _, cx| {
                                    Favorites::global(cx).update(cx, |f, cx| {
                                        f.set_in_workspace(index, !in_workspace, cx)
                                    });
                                }),
                        )
                    })
                    .separator()
                    .item(
                        PopupMenuItem::new("Rename…").on_click(move |_, window, cx| {
                            crate::dialogs::prompt_text(
                                "Rename favorite".into(),
                                "Alias (empty: the default name)",
                                alias.clone(),
                                move |text, _, cx| {
                                    Favorites::global(cx)
                                        .update(cx, |f, cx| f.rename(index, Some(text), cx));
                                },
                                window,
                                cx,
                            );
                        }),
                    )
                    .item(PopupMenuItem::new("Remove").on_click(move |_, _, cx| {
                        Favorites::global(cx).update(cx, |f, cx| f.remove(index, cx));
                    }))
                });
            list = list.child(row);
        }
        list
    }
}
