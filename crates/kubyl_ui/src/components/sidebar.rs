use gpui::{
    AnyElement, AnyView, App, ClickEvent, ElementId, FontWeight, Hsla, IntoElement, RenderOnce,
    SharedString, Window, div, prelude::*,
};
use gpui_component::h_flex;
use smallvec::SmallVec;

use crate::{ActiveColors, Icon, IconName, fonts, sizes, u};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type TooltipBuilder = Box<dyn Fn(&mut Window, &mut App) -> AnyView>;

/// A panel title row (`.phead`): `Explorer` with icon buttons on the right.
#[derive(IntoElement)]
pub struct PanelHeader {
    title: SharedString,
    end: SmallVec<[AnyElement; 3]>,
    bordered: bool,
}

impl PanelHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            end: SmallVec::new(),
            bordered: false,
        }
    }

    /// Adds an element (usually an [`crate::IconButton`]) to the right side.
    pub fn end_child(mut self, child: impl IntoElement) -> Self {
        self.end.push(child.into_any_element());
        self
    }

    /// Draws a separator below (dock panel headers).
    pub fn bordered(mut self) -> Self {
        self.bordered = true;
        self
    }
}

impl RenderOnce for PanelHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        h_flex()
            .flex_none()
            .h(u(sizes::PANEL_HEADER))
            .pl(u(12.0))
            .pr(u(8.0))
            .gap(u(6.0))
            .text_size(u(12.0))
            .text_color(colors.text_muted)
            .when(self.bordered, |this| {
                this.border_b_1().border_color(colors.border_variant)
            })
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .child(self.title),
            )
            .children(self.end)
    }
}

/// An upper-case section title in the sidebar (`.sec`): `▾ FAVORITES  4`.
#[derive(IntoElement)]
pub struct SectionHeader {
    id: ElementId,
    title: SharedString,
    count: Option<SharedString>,
    end: SmallVec<[AnyElement; 1]>,
    collapsed: bool,
    on_toggle: Option<ClickHandler>,
}

impl SectionHeader {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            count: None,
            end: SmallVec::new(),
            collapsed: false,
            on_toggle: None,
        }
    }

    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    /// Adds an element before the count (a toggle). It should stop click propagation so it
    /// doesn't collapse the section.
    pub fn end_child(mut self, child: impl IntoElement) -> Self {
        self.end.push(child.into_any_element());
        self
    }

    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    pub fn on_toggle(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(f));
        self
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        let chevron = if self.collapsed {
            IconName::ChevronRight
        } else {
            IconName::ChevronDown
        };
        h_flex()
            .id(self.id)
            .flex_none()
            .h(u(sizes::SECTION_HEADER))
            .px(u(12.0))
            .gap(u(6.0))
            .text_size(u(sizes::LABEL_FONT))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(colors.text_dim)
            .cursor_pointer()
            .child(Icon::new(chevron).size(11.0).color(colors.text_dim))
            .child(div().flex_1().child(self.title.to_uppercase()))
            .children(self.end)
            .when_some(self.count, |this, count| {
                this.child(
                    div()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(colors.text_faint)
                        .child(count),
                )
            })
            .when_some(self.on_toggle, |this, f| this.on_click(f))
    }
}

/// One row of a tree (`.ti`): indentation, disclosure chevron, icon, label, trailing count.
#[derive(IntoElement)]
pub struct TreeRow {
    id: ElementId,
    label: SharedString,
    depth: usize,
    expanded: Option<bool>,
    icon: Option<IconName>,
    icon_color: Option<Hsla>,
    count: Option<SharedString>,
    end: SmallVec<[AnyElement; 2]>,
    selected: bool,
    root: bool,
    muted_label: bool,
    on_click: Option<ClickHandler>,
    tooltip: Option<TooltipBuilder>,
}

impl TreeRow {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            depth: 0,
            expanded: None,
            icon: None,
            icon_color: None,
            count: None,
            end: SmallVec::new(),
            selected: false,
            root: false,
            muted_label: false,
            on_click: None,
            tooltip: None,
        }
    }

    /// Indentation level; each level is 14px.
    pub fn depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }

    /// `Some(open)` shows a chevron; `None` leaves the same space empty.
    pub fn expanded(mut self, expanded: Option<bool>) -> Self {
        self.expanded = expanded;
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn icon_color(mut self, color: Hsla) -> Self {
        self.icon_color = Some(color);
        self
    }

    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    /// Adds an element between the label and the count (badges, status icons).
    pub fn end_child(mut self, child: impl IntoElement) -> Self {
        self.end.push(child.into_any_element());
        self
    }

    /// Selected rows get the focus outline (1px accent) and selection background.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Cluster roots use the primary text color and a medium weight.
    pub fn root(mut self, root: bool) -> Self {
        self.root = root;
        self
    }

    /// Dims the label (placeholders such as "14 more API groups…").
    pub fn muted_label(mut self, muted: bool) -> Self {
        self.muted_label = muted;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }

    /// Shown while the pointer rests on the row.
    pub fn tooltip(mut self, f: impl Fn(&mut Window, &mut App) -> AnyView + 'static) -> Self {
        self.tooltip = Some(Box::new(f));
        self
    }
}

impl RenderOnce for TreeRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        let text = if self.selected || self.root {
            colors.text
        } else if self.muted_label {
            colors.text_dim
        } else {
            colors.text_muted
        };
        let hover = colors.hover;
        let chevron = match self.expanded {
            Some(true) => Some(IconName::ChevronDown),
            Some(false) => Some(IconName::ChevronRight),
            None => None,
        };
        h_flex()
            .id(self.id)
            .relative()
            .flex_none()
            .h(u(sizes::TREE_ROW))
            .pl(u(8.0 + 14.0 * self.depth as f32))
            .pr(u(10.0))
            .gap(u(6.0))
            .text_color(text)
            .whitespace_nowrap()
            .when(self.root, |this| this.font_weight(FontWeight::MEDIUM))
            .map(|this| {
                if self.selected {
                    this.bg(colors.selection).child(
                        div()
                            .absolute()
                            .inset_0()
                            .border_1()
                            .border_color(colors.accent),
                    )
                } else {
                    this.hover(move |style| style.bg(hover))
                }
            })
            .map(|this| match chevron {
                Some(icon) => this.child(Icon::new(icon).size(12.0).color(colors.text_dim)),
                None => this.child(div().flex_none().w(u(12.0))),
            })
            .when_some(self.icon, |this, icon| {
                this.child(
                    Icon::new(icon)
                        .size(14.0)
                        .color(self.icon_color.unwrap_or(colors.text_dim)),
                )
            })
            .child(div().flex_1().min_w_0().truncate().child(self.label))
            .children(self.end)
            .when_some(self.count, |this, count| {
                this.child(
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(colors.text_dim)
                        .child(count),
                )
            })
            .when_some(self.on_click, |this, f| this.on_click(f))
            .when_some(self.tooltip, |this, f| this.tooltip(f))
    }
}
