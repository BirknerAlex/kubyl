use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, Hsla, IntoElement, MouseButton, RenderOnce,
    SharedString, Stateful, Window, div, prelude::*,
};
use gpui_component::h_flex;
use smallvec::SmallVec;

use crate::{ActiveColors, Icon, StatusDot, sizes, u};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type Decorate<E> = Box<dyn FnOnce(E) -> E>;

/// One tab (`.tab`): icon, label, and a close button or a dirty dot.
#[derive(IntoElement)]
pub struct Tab {
    id: ElementId,
    label: SharedString,
    icon: Option<SharedString>,
    dot: Option<Hsla>,
    active: bool,
    dirty: bool,
    on_click: Option<ClickHandler>,
    on_close: Option<CloseHandler>,
    decorate: Option<Decorate<Stateful<Div>>>,
}

type CloseHandler = Box<dyn Fn(&mut Window, &mut App)>;

impl Tab {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            dot: None,
            active: false,
            dirty: false,
            on_click: None,
            on_close: None,
            decorate: None,
        }
    }

    /// Icon asset path (see [`crate::IconName::path`]).
    pub fn icon(mut self, path: Option<SharedString>) -> Self {
        self.icon = path;
        self
    }

    /// A colored dot before the icon (e.g. the cluster color).
    pub fn dot(mut self, color: Option<Hsla>) -> Self {
        self.dot = color;
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Unsaved changes: shows an accent dot instead of the close button (until hovered). Only
    /// for closable tabs ([`Self::on_close`]).
    pub fn dirty(mut self, dirty: bool) -> Self {
        self.dirty = dirty;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }

    /// Called by the × button and by middle-clicking the tab. Without it the tab has no ×.
    pub fn on_close(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Box::new(f));
        self
    }

    /// Adds behavior to the tab's element, e.g. dragging it and dropping onto it.
    pub fn decorate(mut self, f: impl FnOnce(Stateful<Div>) -> Stateful<Div> + 'static) -> Self {
        self.decorate = Some(Box::new(f));
        self
    }
}

impl RenderOnce for Tab {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        let group = SharedString::from(format!("tab-{}", self.id));
        let icon_color = if self.active {
            colors.accent
        } else {
            colors.text_dim
        };
        let on_close = self.on_close.map(std::rc::Rc::new);
        let close_button = {
            let on_close = on_close.clone();
            div()
                .id("close")
                .flex_none()
                .size(u(16.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(u(3.0))
                .cursor_pointer()
                .hover(|style| style.bg(colors.hover))
                .child(
                    Icon::new(crate::IconName::X)
                        .size(11.0)
                        .color(colors.text_faint),
                )
                .when_some(on_close, |this, f| {
                    this.on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        f(window, cx)
                    })
                })
        };
        // Tabs that can't be closed (dock panels) show no × at all.
        let closable = on_close.is_some();
        let trailing = if !closable {
            None
        } else if self.dirty {
            div()
                .flex_none()
                .size(u(16.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .group_hover(group.clone(), |s| s.invisible())
                        .child(StatusDot::new(colors.accent)),
                )
                .child(
                    div()
                        .absolute()
                        .invisible()
                        .group_hover(group.clone(), |s| s.visible())
                        .child(close_button),
                )
                .into_any_element()
                .into()
        } else {
            Some(close_button.into_any_element())
        };

        h_flex()
            .id(self.id)
            .group(group)
            .flex_none()
            .h_full()
            .pl(u(14.0))
            .pr(u(10.0))
            .gap(u(7.0))
            .border_r_1()
            .border_color(colors.border)
            .whitespace_nowrap()
            .cursor_pointer()
            .map(|this| {
                // The active tab merges into the content below: no bottom border.
                if self.active {
                    this.bg(colors.background).text_color(colors.text)
                } else {
                    this.border_b_1()
                        .text_color(colors.text_dim)
                        .hover(|style| style.bg(colors.hover))
                }
            })
            .when_some(self.dot, |this, color| this.child(StatusDot::new(color)))
            .when_some(self.icon, |this, path| {
                this.child(Icon::from_path(path).size(13.0).color(icon_color))
            })
            .child(self.label)
            .children(trailing)
            .when(!closable, |this| this.pr(u(14.0)))
            .when_some(self.on_click, |this, f| this.on_click(f))
            .when_some(on_close, |this, f| {
                this.on_mouse_up(MouseButton::Middle, move |_, window, cx| f(window, cx))
            })
            .when_some(self.decorate, |this, f| f(this))
    }
}

/// The tab strip (`.tabs`) with optional tool buttons on the right.
#[derive(IntoElement)]
pub struct TabBar {
    id: ElementId,
    tabs: SmallVec<[AnyElement; 8]>,
    tools: SmallVec<[AnyElement; 4]>,
    decorate_end: Option<Decorate<Div>>,
}

impl TabBar {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            tabs: SmallVec::new(),
            tools: SmallVec::new(),
            decorate_end: None,
        }
    }

    pub fn tab(mut self, tab: Tab) -> Self {
        self.tabs.push(tab.into_any_element());
        self
    }

    pub fn tabs(mut self, tabs: impl IntoIterator<Item = Tab>) -> Self {
        self.tabs
            .extend(tabs.into_iter().map(IntoElement::into_any_element));
        self
    }

    /// Adds a tool button (new tab, split, zoom…) on the right.
    pub fn tool(mut self, tool: impl IntoElement) -> Self {
        self.tools.push(tool.into_any_element());
        self
    }

    /// Adds behavior to the empty space after the last tab (e.g. a drop target).
    pub fn decorate_end(mut self, f: impl FnOnce(Div) -> Div + 'static) -> Self {
        self.decorate_end = Some(Box::new(f));
        self
    }
}

impl RenderOnce for TabBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        h_flex()
            .flex_none()
            .h(u(sizes::TAB_BAR))
            .items_stretch()
            .bg(colors.panel)
            .child(
                h_flex()
                    .id(self.id)
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .items_stretch()
                    .overflow_x_scroll()
                    .children(self.tabs)
                    .child(
                        div()
                            .flex_1()
                            .min_w(u(16.0))
                            .border_b_1()
                            .border_color(colors.border)
                            .when_some(self.decorate_end, |this, f| f(this)),
                    ),
            )
            .when(!self.tools.is_empty(), |this| {
                this.child(
                    h_flex()
                        .flex_none()
                        .px(u(8.0))
                        .gap(u(2.0))
                        .border_b_1()
                        .border_color(colors.border)
                        .children(self.tools),
                )
            })
    }
}
