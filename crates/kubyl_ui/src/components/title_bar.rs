use gpui::{
    Action, App, FontWeight, IntoElement, MouseButton, RenderOnce, SharedString, Window, div,
    prelude::*,
};
use gpui_component::h_flex;
use kubyl_core::ClusterBadge;
use kubyl_core::actions::{
    ShowNotifications, SwitchCluster, SwitchNamespace, ToggleCommandPalette,
};

use crate::{ActiveColors, Icon, IconName, Kbd, StatusDot, sizes, u};

/// The window title bar (`.tb`): cluster and namespace switchers, the search button, the
/// cluster meta, notifications and the account avatar.
///
/// Clicking a control dispatches an app action (`SwitchCluster`, `ToggleCommandPalette`…), so
/// whichever crate owns the feature handles it. Wraps gpui-component's `TitleBar`, which owns
/// window dragging, double-click zoom and the Windows/Linux window controls. On macOS it leaves
/// room for the native traffic lights.
#[derive(IntoElement)]
pub struct TitleBar {
    cluster: Option<ClusterBadge>,
    namespace: Option<SharedString>,
    account: Option<SharedString>,
    unread: usize,
}

impl TitleBar {
    pub fn new() -> Self {
        Self {
            cluster: None,
            namespace: None,
            account: None,
            unread: 0,
        }
    }

    pub fn cluster(mut self, cluster: Option<ClusterBadge>) -> Self {
        self.cluster = cluster;
        self
    }

    /// `None` shows "all namespaces".
    pub fn namespace(mut self, namespace: Option<SharedString>) -> Self {
        self.namespace = namespace;
        self
    }

    /// Initials for the avatar; `None` shows a person icon.
    pub fn account(mut self, initials: Option<SharedString>) -> Self {
        self.account = initials;
        self
    }

    /// Unread notifications; shows a dot on the bell.
    pub fn unread(mut self, unread: usize) -> Self {
        self.unread = unread;
        self
    }
}

impl Default for TitleBar {
    fn default() -> Self {
        Self::new()
    }
}

/// A title bar control: stops the mouse-down from starting a window drag and dispatches `action`.
fn control(id: &'static str, action: Box<dyn Action>) -> gpui::Stateful<gpui::Div> {
    h_flex()
        .id(id)
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
        .cursor_pointer()
}

impl RenderOnce for TitleBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors().clone();
        let hover = colors.title_bar_hover;
        let switcher = |id, action: Box<dyn Action>| {
            control(id, action)
                .h(u(26.0))
                .px(u(8.0))
                .gap(u(6.0))
                .rounded(u(5.0))
                .text_color(colors.text)
                .hover(move |s| s.bg(hover))
        };

        let cluster = match &self.cluster {
            Some(cluster) => switcher("cluster-switcher", Box::new(SwitchCluster))
                .child(
                    Icon::new(IconName::ShipWheel)
                        .size(15.0)
                        .color(cluster.color),
                )
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(cluster.name.clone()),
                )
                .when(cluster.production, |this| this.child(ProdBadge))
                .child(Icon::new(IconName::ChevronDown).size(12.0)),
            None => switcher("cluster-switcher", Box::new(SwitchCluster))
                .child(
                    Icon::new(IconName::ShipWheel)
                        .size(15.0)
                        .color(colors.text_dim),
                )
                .child(div().text_color(colors.text_muted).child("No cluster"))
                .child(Icon::new(IconName::ChevronDown).size(12.0)),
        };
        let namespace = switcher("namespace-switcher", Box::new(SwitchNamespace))
            .child(Icon::new(IconName::Folder).size(14.0))
            .child(
                self.namespace
                    .clone()
                    .unwrap_or_else(|| "all namespaces".into()),
            )
            .child(Icon::new(IconName::ChevronDown).size(12.0));

        let search = control("search", Box::new(ToggleCommandPalette))
            .flex_none()
            .w(u(420.0))
            .h(u(26.0))
            .px(u(8.0))
            .gap(u(8.0))
            .rounded(u(6.0))
            .bg(colors.panel)
            .border_1()
            .border_color(colors.border)
            .text_color(colors.text_dim)
            .child(Icon::new(IconName::Search).size(13.0))
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .child("Search resources, commands, contexts…"),
            )
            .child(Kbd::keystroke(if cfg!(target_os = "macos") {
                "cmd-k"
            } else {
                "ctrl-k"
            }));

        let meta = self.cluster.as_ref().and_then(|cluster| {
            let meta = cluster.meta.clone()?;
            let dot = if cluster.connected {
                colors.green
            } else {
                colors.red
            };
            Some(
                h_flex()
                    .gap(u(6.0))
                    .mr(u(6.0))
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(StatusDot::new(dot))
                    .child(meta),
            )
        });

        let bell = control("notifications", Box::new(ShowNotifications))
            .relative()
            .size(u(26.0))
            .justify_center()
            .rounded(u(5.0))
            .hover(move |s| s.bg(hover))
            .child(Icon::new(IconName::Bell).size(14.0))
            .when(self.unread > 0, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(u(5.0))
                        .right(u(5.0))
                        .child(StatusDot::new(colors.accent)),
                )
            });

        let avatar = h_flex()
            .flex_none()
            .size(u(24.0))
            .justify_center()
            .rounded_full()
            .bg(colors.avatar_background)
            .text_color(colors.avatar_text)
            .text_size(u(10.5))
            .font_weight(FontWeight::SEMIBOLD)
            .map(|this| match self.account {
                Some(initials) => this.child(initials),
                None => this.child(
                    Icon::new(IconName::User)
                        .size(13.0)
                        .color(colors.avatar_text),
                ),
            });

        gpui_component::TitleBar::new()
            .h(u(sizes::TITLE_BAR))
            .bg(colors.elevated)
            .border_color(colors.border)
            .child(
                h_flex()
                    .size_full()
                    .gap(u(6.0))
                    .pr(u(10.0))
                    .when(!cfg!(target_os = "macos"), |this| this.pl(u(4.0)))
                    .text_size(u(sizes::UI_FONT))
                    .child(cluster)
                    .child(div().text_color(colors.text_faint).child("/"))
                    .child(namespace)
                    .child(div().flex_1())
                    .child(search)
                    .child(div().flex_1())
                    .children(meta)
                    .child(bell)
                    .child(avatar),
            )
    }
}

/// The red `PROD` badge (`.prod`).
#[derive(IntoElement)]
pub struct ProdBadge;

impl RenderOnce for ProdBadge {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        div()
            .flex_none()
            .px(u(6.0))
            .py(u(1.0))
            .rounded(u(3.0))
            .bg(colors.red)
            .text_color(colors.on_accent)
            .text_size(u(10.5))
            .font_weight(FontWeight::SEMIBOLD)
            .child("PROD")
    }
}
