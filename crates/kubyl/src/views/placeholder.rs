use gpui::{
    App, Context, FocusHandle, Focusable, IntoElement, Render, SharedString, Window, div,
    prelude::*,
};
use kubyl_core::{DockPosition, TabView, ViewKind, ViewRequest};
use kubyl_ui::{ActiveColors, Icon, IconName, u, v_flex};

/// Stands in for a view whose crate hasn't been implemented yet.
pub struct PlaceholderView {
    request: Option<ViewRequest>,
    title: SharedString,
    icon: IconName,
    message: SharedString,
    focus: FocusHandle,
}

impl PlaceholderView {
    pub fn for_request(request: ViewRequest, cx: &mut Context<Self>) -> Self {
        let (title, icon, phase) = describe(&request.kind);
        Self {
            title: title.into(),
            icon,
            message: format!("{title} arrives with {phase}.").into(),
            request: Some(request),
            focus: cx.focus_handle(),
        }
    }

    pub fn for_dock(position: DockPosition, cx: &mut Context<Self>) -> Self {
        let (title, icon, message) = match position {
            DockPosition::Right => (
                "Details",
                IconName::Info,
                "Select a resource to see its details.",
            ),
            DockPosition::Bottom => (
                "Terminal",
                IconName::Terminal,
                "Logs, shells and port-forwards open here.",
            ),
            DockPosition::Left => ("Panel", IconName::PanelLeft, ""),
        };
        Self {
            request: None,
            title: title.into(),
            icon,
            message: message.into(),
            focus: cx.focus_handle(),
        }
    }
}

fn describe(kind: &ViewKind) -> (&str, IconName, &'static str) {
    match kind {
        ViewKind::Welcome => ("Welcome", IconName::Info, "phase 00"),
        ViewKind::Table => (
            "Resources",
            IconName::List,
            "the resource explorer (phase 02)",
        ),
        ViewKind::Details => (
            "Details",
            IconName::Info,
            "the resource explorer (phase 02)",
        ),
        ViewKind::Yaml => ("YAML", IconName::Code, "the YAML editor (phase 04)"),
        ViewKind::Logs => ("Logs", IconName::Terminal, "logs and exec (phase 05)"),
        ViewKind::Terminal => ("Terminal", IconName::Terminal, "logs and exec (phase 05)"),
        ViewKind::Files => ("Files", IconName::Folder, "the file browser (phase 06)"),
        ViewKind::Overview => (
            "Overview",
            IconName::Gauge,
            "metrics and overview (phase 07)",
        ),
        ViewKind::Events => ("Events", IconName::Bell, "metrics and overview (phase 07)"),
        ViewKind::Operators => ("Operators", IconName::Blocks, "operators (phase 08)"),
        ViewKind::Updates => (
            "Cluster Updates",
            IconName::ArrowUp,
            "cluster updates (phase 09)",
        ),
        ViewKind::Settings => ("Settings", IconName::Settings, "a settings editor"),
        ViewKind::Custom(name) => (name.as_str(), IconName::File, "a later phase"),
    }
}

impl Focusable for PlaceholderView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for PlaceholderView {
    fn tab_title(&self, _: &App) -> SharedString {
        self.title.clone()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(self.icon.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        self.request.clone()
    }
}

impl Render for PlaceholderView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors();
        v_flex()
            .track_focus(&self.focus)
            .size_full()
            .items_center()
            .justify_center()
            .gap(u(10.0))
            .p(u(16.0))
            .text_color(colors.text_dim)
            .child(Icon::new(self.icon).size(28.0).color(colors.text_faint))
            .child(div().text_center().child(self.message.clone()))
    }
}
