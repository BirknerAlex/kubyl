//! The main window: title bar, sidebar, center panes, right and bottom docks, status bar.

mod dock;
pub mod layout;
mod pane;
mod pane_group;
mod sidebar;

use gpui::{
    AnyElement, AnyView, App, Context, Entity, ExternalPaths, FocusHandle, Focusable, IntoElement,
    Pixels, Render, SharedString, Subscription, WeakEntity, Window, actions, div, prelude::*, px,
};
use gpui_component::resizable::{ResizableState, h_resizable, resizable_panel, v_resizable};
use kubyl_core::actions::{ActivateDockPanel, OpenSettings, OpenView, ShowNotifications};
use kubyl_core::{
    ActiveContext, ChromeRegistry, DockPosition, NotificationCenter, StatusBarPosition, TabHandle,
    ViewRegistry, ViewRequest,
};
use kubyl_settings::{Settings, State};
use kubyl_ui::{
    ActiveColors, AppearanceSettings, Button, IconButton, IconName, Modal, StatusBar,
    StatusBarText, StatusDot, ThemeChoice, TitleBar, u, v_flex,
};

pub use dock::Dock;
use layout::{DockLayout, SplitAxis, WorkspaceLayout};
pub use pane::{
    ActivateNextTab, ActivatePreviousTab, CloseActiveTab, GoBack, GoForward, NewTab, Pane,
    PaneEvent, SplitDown, SplitRight, ToggleZoom,
};
use pane_group::PaneGroup;
use sidebar::Sidebar;

use crate::views::PlaceholderView;

actions!(
    workspace,
    [
        ToggleLeftDock,
        ToggleRightDock,
        ToggleBottomDock,
        CloseWindow,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        ToggleTheme,
        About,
    ]
);

/// Builds the view for `request`, or a placeholder when no crate provides that kind yet.
pub fn build_view(request: &ViewRequest, window: &mut Window, cx: &mut App) -> Box<dyn TabHandle> {
    ViewRegistry::build(request, window, cx)
        .unwrap_or_else(|| Box::new(cx.new(|cx| PlaceholderView::for_request(request.clone(), cx))))
}

fn focus_pane(pane: &Entity<Pane>, window: &mut Window, cx: &mut App) {
    let focus = pane.read(cx).active_focus_handle(cx);
    focus.focus(window, cx);
}

enum Overlay {
    About,
    Notifications,
}

pub struct Workspace {
    center: PaneGroup,
    active_pane: Entity<Pane>,
    zoomed: Option<WeakEntity<Pane>>,
    sidebar: Entity<Sidebar>,
    right_dock: Entity<Dock>,
    bottom_dock: Entity<Dock>,
    docks: DockSizes,
    body_state: Entity<ResizableState>,
    center_state: Entity<ResizableState>,
    status_left: Vec<AnyView>,
    status_right: Vec<AnyView>,
    toasts_shown: u64,
    history_seen: u64,
    overlay: Option<Overlay>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

struct DockSizes {
    sidebar: DockLayout,
    right: DockLayout,
    bottom: DockLayout,
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Workspace {
    pub fn new(layout: WorkspaceLayout, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut panes = Vec::new();
        let center = PaneGroup::from_layout(&layout.center, &mut |tabs, active| {
            let pane = Self::new_pane(window, cx);
            pane.update(cx, |pane, cx| {
                for request in tabs {
                    let item = build_view(request, window, cx);
                    pane.add_item(item, false, window, cx);
                }
                pane.activate(active, false, window, cx);
            });
            panes.push(pane.clone());
            pane
        });
        let active_pane = panes.first().cloned().expect("layout has a pane");

        let sidebar = cx.new(|cx| Sidebar::new(window, cx));
        let right_dock =
            cx.new(|cx| Dock::new(DockPosition::Right, Box::new(ToggleRightDock), window, cx));
        let bottom_dock =
            cx.new(|cx| Dock::new(DockPosition::Bottom, Box::new(ToggleBottomDock), window, cx));

        let registry: Vec<_> = ChromeRegistry::global(cx)
            .status_items(StatusBarPosition::Left)
            .cloned()
            .collect();
        let status_left = registry.iter().map(|item| item.build(window, cx)).collect();
        let registry: Vec<_> = ChromeRegistry::global(cx)
            .status_items(StatusBarPosition::Right)
            .cloned()
            .collect();
        let status_right = registry.iter().map(|item| item.build(window, cx)).collect();

        let latest = NotificationCenter::global(cx).latest_id();
        let subscriptions = vec![
            cx.observe_global_in::<NotificationCenter>(window, Self::show_new_notifications),
            cx.observe_global::<ActiveContext>(|_, cx| cx.notify()),
            cx.observe_window_bounds(window, |this, window, cx| this.save_layout(window, cx)),
            cx.observe_window_appearance(window, |_, _, cx| kubyl_ui::apply_theme(cx)),
        ];

        let this = Self {
            center,
            active_pane,
            zoomed: None,
            sidebar,
            right_dock,
            bottom_dock,
            docks: DockSizes {
                sidebar: layout.sidebar,
                right: layout.right_dock,
                bottom: layout.bottom_dock,
            },
            body_state: cx.new(|_| ResizableState::default()),
            center_state: cx.new(|_| ResizableState::default()),
            status_left,
            status_right,
            toasts_shown: latest,
            history_seen: latest,
            overlay: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        focus_pane(&this.active_pane, window, cx);
        this
    }

    fn new_pane(window: &mut Window, cx: &mut Context<Self>) -> Entity<Pane> {
        let pane = cx.new(|cx| Pane::new(window, cx));
        cx.subscribe_in(&pane, window, Self::handle_pane_event)
            .detach();
        pane
    }

    fn handle_pane_event(
        &mut self,
        pane: &Entity<Pane>,
        event: &PaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PaneEvent::Focused => {
                if &self.active_pane != pane {
                    self.active_pane = pane.clone();
                    cx.notify();
                }
            }
            PaneEvent::Split(axis) => self.split(pane, *axis, window, cx),
            PaneEvent::Emptied => {
                if self.center.remove(pane) {
                    if self.zoomed.as_ref().and_then(|z| z.upgrade()).as_ref() == Some(pane) {
                        self.zoomed = None;
                    }
                    self.active_pane = self.center.panes()[0].clone();
                    focus_pane(&self.active_pane, window, cx);
                    self.save_layout(window, cx);
                    cx.notify();
                }
            }
            PaneEvent::ToggleZoom => self.toggle_zoom(pane, cx),
            PaneEvent::Changed => self.save_layout(window, cx),
        }
    }

    fn split(
        &mut self,
        pane: &Entity<Pane>,
        axis: SplitAxis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_pane = Self::new_pane(window, cx);
        // The new pane starts with a copy of the active tab's request, like Zed.
        let request = pane
            .read(cx)
            .active_item()
            .and_then(|item| item.view_request(cx));
        if let Some(request) = request {
            let item = build_view(&request, window, cx);
            new_pane.update(cx, |p, cx| p.add_item(item, false, window, cx));
        }
        if self.center.split(pane, new_pane.clone(), axis) {
            self.zoomed = None;
            self.active_pane = new_pane.clone();
            focus_pane(&new_pane, window, cx);
            self.save_layout(window, cx);
            cx.notify();
        }
    }

    fn toggle_zoom(&mut self, pane: &Entity<Pane>, cx: &mut Context<Self>) {
        let zoomed = self.zoomed.as_ref().and_then(|z| z.upgrade());
        if let Some(zoomed) = &zoomed {
            zoomed.update(cx, |p, cx| p.set_zoomed(false, cx));
        }
        if zoomed.as_ref() == Some(pane) {
            self.zoomed = None;
        } else {
            pane.update(cx, |p, cx| p.set_zoomed(true, cx));
            self.zoomed = Some(pane.downgrade());
        }
        cx.notify();
    }

    /// Opens `request` in the active pane (or activates the tab that already shows it).
    pub fn open(&mut self, request: &ViewRequest, window: &mut Window, cx: &mut Context<Self>) {
        let item = build_view(request, window, cx);
        self.active_pane
            .update(cx, |pane, cx| pane.add_item(item, true, window, cx));
    }

    fn show_new_notifications(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let new: Vec<_> = NotificationCenter::global(cx)
            .since(self.toasts_shown)
            .cloned()
            .collect();
        for (id, notification) in new {
            kubyl_ui::show_notification(&notification, window, cx);
            self.toasts_shown = id;
        }
        cx.notify();
    }

    fn toggle_dock(&mut self, position: DockPosition, window: &mut Window, cx: &mut Context<Self>) {
        let dock = match position {
            DockPosition::Left => &mut self.docks.sidebar,
            DockPosition::Right => &mut self.docks.right,
            DockPosition::Bottom => &mut self.docks.bottom,
        };
        dock.visible = !dock.visible;
        let visible = dock.visible;
        if self.zoomed.take().and_then(|z| z.upgrade()).is_some() {
            for pane in self.center.panes() {
                pane.update(cx, |p, cx| p.set_zoomed(false, cx));
            }
        }
        // Moving focus away from a hidden dock keeps keyboard input working.
        let focus = match (position, visible) {
            (DockPosition::Left, true) => Some(self.sidebar.focus_handle(cx)),
            (DockPosition::Right, true) => Some(self.right_dock.focus_handle(cx)),
            (DockPosition::Bottom, true) => Some(self.bottom_dock.focus_handle(cx)),
            _ => None,
        };
        match focus {
            Some(focus) => focus.focus(window, cx),
            None => focus_pane(&self.active_pane, window, cx),
        }
        self.save_layout(window, cx);
        cx.notify();
    }

    /// Shows the dock holding the panel `id`, activates the panel and focuses it.
    fn activate_dock_panel(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        for position in [DockPosition::Right, DockPosition::Bottom] {
            let dock = match position {
                DockPosition::Bottom => self.bottom_dock.clone(),
                _ => self.right_dock.clone(),
            };
            if !dock.update(cx, |dock, cx| dock.activate_panel(id, cx)) {
                continue;
            }
            let layout = match position {
                DockPosition::Bottom => &mut self.docks.bottom,
                _ => &mut self.docks.right,
            };
            let was_visible = std::mem::replace(&mut layout.visible, true);
            // A zoomed pane hides every dock, visible or not: un-zoom so the panel shows.
            let unzoomed = self.zoomed.take().and_then(|z| z.upgrade()).is_some();
            if unzoomed {
                for pane in self.center.panes() {
                    pane.update(cx, |p, cx| p.set_zoomed(false, cx));
                }
            }
            if !was_visible || unzoomed {
                self.save_layout(window, cx);
            }
            let focus = dock.read(cx).active_focus_handle(cx);
            focus.focus(window, cx);
            cx.notify();
            return;
        }
        tracing::warn!(panel = id, "no dock panel with this id");
    }

    fn rem_scale(window: &Window) -> f32 {
        f32::from(window.rem_size()) / 16.0
    }

    /// Reads dock sizes back from the resizable groups after the user dragged a handle.
    fn sync_dock_sizes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scale = Self::rem_scale(window);
        let body = self.body_state.read(cx).sizes().clone();
        if let [sidebar, _, right] = body.as_slice() {
            if self.docks.sidebar.visible && *sidebar > px(0.0) {
                self.docks.sidebar.size = f32::from(*sidebar) / scale;
            }
            if self.docks.right.visible && *right > px(0.0) {
                self.docks.right.size = f32::from(*right) / scale;
            }
        }
        let center = self.center_state.read(cx).sizes().clone();
        if let [_, bottom] = center.as_slice()
            && self.docks.bottom.visible
            && *bottom > px(0.0)
        {
            self.docks.bottom.size = f32::from(*bottom) / scale;
        }
        self.save_layout(window, cx);
    }

    fn save_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bounds = window.bounds();
        let layout = WorkspaceLayout {
            window_size: Some((f32::from(bounds.size.width), f32::from(bounds.size.height))),
            window_maximized: window.is_maximized(),
            sidebar: self.docks.sidebar,
            right_dock: self.docks.right,
            bottom_dock: self.docks.bottom,
            center: self.center.layout(cx),
        };
        State::set(cx, &layout);
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> StatusBar {
        let colors = cx.colors().clone();
        let context = ActiveContext::global(cx).clone();
        let cluster = match &context.cluster {
            Some(cluster) => div()
                .flex()
                .items_center()
                .gap(u(5.0))
                .text_color(colors.text)
                .child(StatusDot::new(if cluster.connected {
                    colors.green
                } else {
                    colors.red
                }))
                .child(cluster.name.clone())
                .into_any_element(),
            None => StatusBarText::new("No cluster")
                .icon(IconName::ShipWheel, None)
                .into_any_element(),
        };
        let mut bar = StatusBar::new()
            .left(
                IconButton::new("toggle-sidebar", IconName::Columns)
                    .icon_size(13.0)
                    .toggled(self.docks.sidebar.visible)
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleLeftDock), cx)),
            )
            .left(cluster)
            .left(
                StatusBarText::new(
                    context
                        .namespace
                        .clone()
                        .unwrap_or_else(|| SharedString::new_static("all namespaces")),
                )
                .icon(IconName::Folder, None),
            );
        for item in &self.status_left {
            bar = bar.left(item.clone());
        }
        for item in &self.status_right {
            bar = bar.right(item.clone());
        }
        bar.right(
            IconButton::new("toggle-bottom-dock", IconName::Terminal)
                .icon_size(12.0)
                .toggled(self.docks.bottom.visible)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleBottomDock), cx)),
        )
        .right(
            IconButton::new("toggle-right-dock", IconName::PanelRight)
                .icon_size(13.0)
                .toggled(self.docks.right.visible)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleRightDock), cx)),
        )
        .right(
            div()
                .font_family(kubyl_ui::fonts::MONO)
                .text_size(u(11.5))
                .child("kube-rs · GPUI"),
        )
    }

    fn render_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(zoomed) = self.zoomed.as_ref().and_then(|z| z.upgrade()) {
            return div().flex_1().min_h_0().child(zoomed).into_any_element();
        }
        let scale = Self::rem_scale(window);
        let size = |dock: &DockLayout| -> Pixels { px(dock.size * scale) };
        let this = cx.entity().downgrade();
        let on_resize = move |_: &Entity<ResizableState>, window: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.sync_dock_sizes(window, cx))
                .ok();
        };
        let center = v_resizable("workspace-center")
            .with_state(&self.center_state)
            .on_resize(on_resize.clone())
            .child(resizable_panel().child(self.center.render()))
            .child(
                resizable_panel()
                    .size(size(&self.docks.bottom))
                    .size_range(px(120.0 * scale)..px(900.0 * scale))
                    .flex_none()
                    .visible(self.docks.bottom.visible)
                    .child(self.bottom_dock.clone()),
            );
        h_resizable("workspace-body")
            .with_state(&self.body_state)
            .on_resize(on_resize)
            .child(
                resizable_panel()
                    .size(size(&self.docks.sidebar))
                    .size_range(px(180.0 * scale)..px(640.0 * scale))
                    .flex_none()
                    .visible(self.docks.sidebar.visible)
                    .child(self.sidebar.clone()),
            )
            .child(resizable_panel().child(center))
            .child(
                resizable_panel()
                    .size(size(&self.docks.right))
                    .size_range(px(220.0 * scale)..px(720.0 * scale))
                    .flex_none()
                    .visible(self.docks.right.visible)
                    .child(self.right_dock.clone()),
            )
            .into_any_element()
    }

    fn render_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let overlay = self.overlay.as_ref()?;
        let this = cx.entity().downgrade();
        let dismiss = move |_: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| {
                this.overlay = None;
                cx.notify();
            })
            .ok();
        };
        let close_button = {
            let dismiss = dismiss.clone();
            Button::new("close-overlay")
                .label("Close")
                .on_click(move |_, window, cx| dismiss(window, cx))
        };
        let colors = cx.colors().clone();
        Some(
            match overlay {
                Overlay::About => Modal::new("About Kubyl")
                    .child(
                        gpui::img("logo/png/app-icon-128.png")
                            .size(u(64.0))
                            .flex_none(),
                    )
                    .child(
                        div()
                            .text_color(colors.text)
                            .child(format!("Kubyl {}", env!("CARGO_PKG_VERSION"))),
                    )
                    .child("A native Kubernetes client, built with kube-rs and GPUI.")
                    .child(div().text_size(u(12.0)).text_color(colors.text_dim).child(
                        "Licensed under MIT OR Apache-2.0. IBM Plex (OFL), Lucide icons (ISC).",
                    ))
                    .footer(close_button)
                    .on_dismiss(dismiss),
                Overlay::Notifications => {
                    let recent: Vec<_> = NotificationCenter::global(cx)
                        .since(0)
                        .map(|(_, n)| n.clone())
                        .collect();
                    let mut modal = Modal::new("Notifications").width(520.0);
                    if recent.is_empty() {
                        modal = modal.child("No notifications yet.");
                    }
                    for notification in recent.into_iter().rev() {
                        let color = match notification.level {
                            kubyl_core::NotificationLevel::Info => colors.accent,
                            kubyl_core::NotificationLevel::Success => colors.green,
                            kubyl_core::NotificationLevel::Warning => colors.yellow,
                            kubyl_core::NotificationLevel::Error => colors.red,
                        };
                        modal = modal.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(u(8.0))
                                .child(StatusDot::new(color))
                                .child(notification.message),
                        );
                    }
                    modal.footer(close_button).on_dismiss(dismiss)
                }
            }
            .into_any_element(),
        )
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        kubyl_ui::apply_zoom(window, cx);
        let colors = cx.colors().clone();
        let context = ActiveContext::global(cx).clone();
        let unread = NotificationCenter::global(cx)
            .latest_id()
            .saturating_sub(self.history_seen) as usize;
        let body = self.render_body(window, cx);
        let status_bar = self.render_status_bar(cx);
        let overlay = self.render_overlay(cx);
        // gpui-component's Root leaves its overlay layers to the root view.
        let sheet_layer = gpui_component::Root::render_sheet_layer(window, cx);
        let dialog_layer = gpui_component::Root::render_dialog_layer(window, cx);
        let notification_layer = gpui_component::Root::render_notification_layer(window, cx);

        v_flex()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(kubyl_ui::fonts::UI)
            .text_size(u(kubyl_ui::sizes::UI_FONT))
            .on_action(cx.listener(|this, _: &ToggleLeftDock, window, cx| {
                this.toggle_dock(DockPosition::Left, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleRightDock, window, cx| {
                this.toggle_dock(DockPosition::Right, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleBottomDock, window, cx| {
                this.toggle_dock(DockPosition::Bottom, window, cx)
            }))
            .on_action(
                cx.listener(|this, action: &OpenView, window, cx| this.open(&action.0, window, cx)),
            )
            .on_action(cx.listener(|this, action: &ActivateDockPanel, window, cx| {
                this.activate_dock_panel(&action.0, window, cx)
            }))
            .on_action(cx.listener(|_, _: &CloseWindow, window, _| window.remove_window()))
            .on_action(|_: &ZoomIn, _, cx| kubyl_ui::zoom(cx, Some(1.0)))
            .on_action(|_: &ZoomOut, _, cx| kubyl_ui::zoom(cx, Some(-1.0)))
            .on_action(|_: &ResetZoom, _, cx| kubyl_ui::zoom(cx, None))
            .on_action(|_: &ToggleTheme, _, cx| {
                let dark = cx.global::<kubyl_ui::Theme>().dark;
                Settings::update::<AppearanceSettings>(cx, |s| {
                    s.theme = if dark {
                        ThemeChoice::Light
                    } else {
                        ThemeChoice::Dark
                    };
                });
            })
            .on_action(|_: &OpenSettings, _, cx| {
                let path = Settings::path(cx).to_path_buf();
                cx.open_with_system(&path);
            })
            // Kubeconfig files dropped anywhere on the window become sources.
            .on_drop(|paths: &ExternalPaths, _, cx| {
                kubyl_kube::ui::add_paths(paths.paths().to_vec(), cx)
            })
            .on_action(cx.listener(|this, _: &About, _, cx| {
                this.overlay = Some(Overlay::About);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShowNotifications, _, cx| {
                this.history_seen = NotificationCenter::global(cx).latest_id();
                this.overlay = Some(Overlay::Notifications);
                cx.notify();
            }))
            .child(
                TitleBar::new()
                    .cluster(context.cluster.clone())
                    .namespace(context.namespace.clone())
                    .unread(unread),
            )
            .child(div().flex_1().min_h_0().flex().child(body))
            .child(status_bar)
            .children(overlay)
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use layout::PaneLayout;

    fn init(cx: &mut TestAppContext) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            crate::app::init(cx);
        });
        dir
    }

    fn open(cx: &mut TestAppContext, layout: WorkspaceLayout) -> gpui::WindowHandle<Workspace> {
        cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                cx.new(|cx| Workspace::new(layout, window, cx))
            })
            .unwrap()
        })
    }

    #[gpui::test]
    fn default_layout_opens_the_welcome_tab(cx: &mut TestAppContext) {
        let _dir = init(cx);
        let window = open(cx, WorkspaceLayout::default());
        window
            .update(cx, |workspace, _, cx| {
                let pane = workspace.active_pane.read(cx);
                let titles: Vec<_> = pane.items().iter().map(|i| i.title(cx)).collect();
                assert_eq!(titles.len(), 1);
                assert_eq!(titles[0].as_ref(), "Welcome");
            })
            .unwrap();
    }

    #[gpui::test]
    fn split_close_and_zoom(cx: &mut TestAppContext) {
        let _dir = init(cx);
        let window = open(cx, WorkspaceLayout::default());
        window
            .update(cx, |workspace, window, cx| {
                let first = workspace.active_pane.clone();
                workspace.split(&first, SplitAxis::Horizontal, window, cx);
                assert_eq!(workspace.center.panes().len(), 2);
                let second = workspace.active_pane.clone();
                assert_ne!(first, second);
                assert!(matches!(
                    workspace.center.layout(cx),
                    PaneLayout::Split {
                        axis: SplitAxis::Horizontal,
                        ..
                    }
                ));

                workspace.toggle_zoom(&second, cx);
                assert!(workspace.zoomed.is_some());
                workspace.toggle_zoom(&second, cx);
                assert!(workspace.zoomed.is_none());

                // Closing every tab of the new pane removes it from the split.
                second.update(cx, |pane, cx| {
                    while !pane.items().is_empty() {
                        pane.close(0, window, cx);
                    }
                });
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |workspace, _, cx| {
                assert_eq!(workspace.center.panes().len(), 1);
                assert!(matches!(
                    workspace.center.layout(cx),
                    PaneLayout::Pane { .. }
                ));
            })
            .unwrap();
    }

    #[gpui::test]
    fn opening_the_same_view_twice_reuses_the_tab(cx: &mut TestAppContext) {
        let _dir = init(cx);
        let window = open(cx, WorkspaceLayout::default());
        window
            .update(cx, |workspace, window, cx| {
                let request = ViewRequest::new(kubyl_core::ViewKind::Overview);
                workspace.open(&request, window, cx);
                workspace.open(&request, window, cx);
                assert_eq!(workspace.active_pane.read(cx).items().len(), 2);
            })
            .unwrap();
    }

    #[gpui::test]
    fn back_and_forward_follow_the_active_tab(cx: &mut TestAppContext) {
        let _dir = init(cx);
        let window = open(cx, WorkspaceLayout::default());
        let active_kind = |workspace: &Workspace, cx: &App| {
            workspace
                .active_pane
                .read(cx)
                .active_item()
                .and_then(|i| i.view_request(cx))
                .map(|r| r.kind)
        };
        window
            .update(cx, |workspace, window, cx| {
                workspace.open(
                    &ViewRequest::new(kubyl_core::ViewKind::Overview),
                    window,
                    cx,
                );
                workspace.open(&ViewRequest::new(kubyl_core::ViewKind::Events), window, cx);
                let pane = workspace.active_pane.clone();
                assert!(pane.update(cx, |p, cx| p.navigate(-1, window, cx)));
                assert_eq!(
                    active_kind(workspace, cx),
                    Some(kubyl_core::ViewKind::Overview)
                );
                // Closing the tab we came from doesn't lose it: forward reopens it.
                pane.update(cx, |p, cx| {
                    let events = p
                        .items()
                        .iter()
                        .position(|i| {
                            i.view_request(cx).map(|r| r.kind) == Some(kubyl_core::ViewKind::Events)
                        })
                        .unwrap();
                    p.close(events, window, cx);
                });
                assert!(pane.update(cx, |p, cx| p.navigate(1, window, cx)));
                assert_eq!(
                    active_kind(workspace, cx),
                    Some(kubyl_core::ViewKind::Events)
                );
                assert!(!pane.update(cx, |p, cx| p.navigate(1, window, cx)));
            })
            .unwrap();
    }

    #[gpui::test]
    fn keymap_presets_name_existing_actions(cx: &mut TestAppContext) {
        let _dir = init(cx);
        cx.update(|cx| {
            for preset in [kubyl_keymap::Preset::Default, kubyl_keymap::Preset::K9s] {
                for source in kubyl_keymap::preset_sources(preset) {
                    let blocks = kubyl_keymap::parse_file(source).unwrap();
                    let mut errors = Vec::new();
                    let bindings =
                        kubyl_keymap::load_blocks(&blocks, kubyl_keymap::PRESET, cx, &mut errors);
                    assert!(errors.is_empty(), "{preset:?}: {errors:?}");
                    assert!(!bindings.is_empty());
                }
            }
        });
    }

    #[gpui::test]
    fn toggling_docks_persists_the_layout(cx: &mut TestAppContext) {
        let _dir = init(cx);
        let window = open(cx, WorkspaceLayout::default());
        window
            .update(cx, |workspace, window, cx| {
                workspace.toggle_dock(DockPosition::Bottom, window, cx);
                workspace.toggle_dock(DockPosition::Left, window, cx);
            })
            .unwrap();
        cx.update(|cx| {
            let saved = State::get::<WorkspaceLayout>(cx);
            assert!(saved.bottom_dock.visible);
            assert!(!saved.sidebar.visible);
        });
    }

    struct TestPanel;

    impl kubyl_core::DockPanel for TestPanel {
        fn id(&self) -> &'static str {
            "test-panel"
        }

        fn position(&self) -> DockPosition {
            DockPosition::Bottom
        }

        fn build(&self, _: &mut Window, cx: &mut App) -> Box<dyn TabHandle> {
            Box::new(cx.new(|cx| PlaceholderView::for_dock(DockPosition::Bottom, cx)))
        }
    }

    #[gpui::test]
    fn activating_a_dock_panel_shows_its_dock(cx: &mut TestAppContext) {
        let _dir = init(cx);
        cx.update(|cx| ChromeRegistry::add_dock_panel(cx, TestPanel));
        let window = open(cx, WorkspaceLayout::default());
        window
            .update(cx, |workspace, window, cx| {
                assert!(!workspace.docks.bottom.visible);
                workspace.activate_dock_panel("test-panel", window, cx);
                assert!(workspace.docks.bottom.visible);
                // Unknown ids leave the layout alone.
                workspace.activate_dock_panel("missing", window, cx);
                assert!(workspace.docks.bottom.visible);
                // A zoomed pane hides the dock even though it's visible: activating un-zooms.
                let pane = workspace.active_pane.clone();
                workspace.toggle_zoom(&pane, cx);
                assert!(workspace.zoomed.is_some());
                workspace.activate_dock_panel("test-panel", window, cx);
                assert!(workspace.zoomed.is_none());
            })
            .unwrap();
    }
}
