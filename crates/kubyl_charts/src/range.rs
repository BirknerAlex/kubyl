//! Time ranges (15m/1h/6h/24h/7d) and the segmented picker from the overview header.

use std::rc::Rc;

use gpui::{App, ClickEvent, IntoElement, RenderOnce, SharedString, Window, div, prelude::*};
use kubyl_ui::{ActiveColors, h_flex, u};

pub use kubyl_charts_core::time::TimeRange;

type OnChange = Rc<dyn Fn(TimeRange, &mut Window, &mut App)>;

/// The segmented `15m 1h 6h 24h 7d` control.
#[derive(IntoElement)]
pub struct TimeRangePicker {
    id: SharedString,
    selected: TimeRange,
    disabled: bool,
    compact: bool,
    on_change: Option<OnChange>,
}

impl TimeRangePicker {
    pub fn new(id: impl Into<SharedString>, selected: TimeRange) -> Self {
        Self {
            id: id.into(),
            selected,
            disabled: false,
            compact: false,
            on_change: None,
        }
    }

    /// Smaller, for section headers in the details dock.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// Greyed out (no history source).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_change(mut self, f: impl Fn(TimeRange, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for TimeRangePicker {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors().clone();
        h_flex()
            .flex_none()
            .border_1()
            .border_color(colors.border)
            .rounded(u(5.0))
            .overflow_hidden()
            .text_size(u(if self.compact { 11.0 } else { 12.0 }))
            .children(TimeRange::ALL.into_iter().map(|range| {
                let selected = range == self.selected && !self.disabled;
                let on_change = self.on_change.clone();
                div()
                    .id(SharedString::from(format!("{}-{}", self.id, range.label())))
                    .px(u(if self.compact { 5.0 } else { 9.0 }))
                    .py(u(if self.compact { 1.0 } else { 3.0 }))
                    .map(|this| {
                        if selected {
                            this.bg(colors.chip_selected_background)
                                .text_color(colors.chip_selected_text)
                        } else if self.disabled {
                            this.text_color(colors.text_faint)
                        } else {
                            this.text_color(colors.text_dim)
                                .hover(|this| this.bg(colors.hover))
                                .cursor_pointer()
                        }
                    })
                    .when(!self.disabled, |this| {
                        this.on_click(move |_: &ClickEvent, window, cx| {
                            if let Some(on_change) = &on_change {
                                on_change(range, window, cx);
                            }
                        })
                    })
                    .child(range.label())
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_have_about_120_points() {
        assert_eq!(TimeRange::M15.step().as_secs(), 15);
        assert_eq!(TimeRange::H1.step().as_secs(), 30);
        assert_eq!(TimeRange::D7.step().as_secs(), 5040);
        assert_eq!(TimeRange::from_label("24h"), Some(TimeRange::H24));
        assert_eq!(TimeRange::from_label("2h"), None);
    }

    #[test]
    fn windows_end_now_in_15s_buckets() {
        let (start, end, step) = TimeRange::D7.window(1_000_017.0);
        assert_eq!(step, 5040.0);
        assert_eq!(end, 1_000_005.0);
        assert_eq!(end - start, 7.0 * 86_400.0);
        assert_eq!(TimeRange::D7.window(1_000_019.0), (start, end, step));
    }
}
