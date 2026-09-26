use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, App, ElementId, Hsla, IntoElement, Pixels, RenderOnce,
    SharedString, Window, div, prelude::*, pulsating_between, relative,
};
use gpui_component::h_flex;
use kubyl_core::Tone;

use crate::{ActiveColors, Colors, u};

/// The theme color for a status tone.
pub fn tone_color(tone: Tone, colors: &Colors) -> Hsla {
    match tone {
        Tone::Neutral => colors.text_muted,
        Tone::Good => colors.green,
        Tone::Warning => colors.yellow,
        Tone::Bad => colors.red,
        Tone::Info => colors.accent,
        Tone::Muted => colors.text_dim,
    }
}

/// A 7px colored dot (`.dot`).
#[derive(IntoElement)]
pub struct StatusDot {
    color: Hsla,
    pulse: Option<ElementId>,
}

impl StatusDot {
    pub fn new(color: Hsla) -> Self {
        Self { color, pulse: None }
    }

    /// Fades in and out, e.g. while a cluster connects. `id` must be unique among siblings.
    pub fn pulsing(mut self, id: impl Into<ElementId>) -> Self {
        self.pulse = Some(id.into());
        self
    }
}

impl RenderOnce for StatusDot {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let dot = div().flex_none().size(u(7.0)).rounded_full().bg(self.color);
        match self.pulse {
            Some(id) => dot
                .with_animation(
                    id,
                    Animation::new(Duration::from_millis(1600))
                        .repeat_synced()
                        .with_max_fps(24.0)
                        .with_easing(pulsating_between(0.3, 1.0)),
                    |dot, alpha| dot.opacity(alpha),
                )
                .into_any_element(),
            None => dot.into_any_element(),
        }
    }
}

/// Dot plus colored label, e.g. a pod phase (`.pill`).
#[derive(IntoElement)]
pub struct StatusPill {
    label: SharedString,
    tone: Tone,
}

impl StatusPill {
    pub fn new(label: impl Into<SharedString>, tone: Tone) -> Self {
        Self {
            label: label.into(),
            tone,
        }
    }
}

impl RenderOnce for StatusPill {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = tone_color(self.tone, cx.colors());
        h_flex()
            .gap(u(6.0))
            .min_w_0()
            .child(StatusDot::new(color))
            .child(div().truncate().text_color(color).child(self.label))
    }
}

/// A thin usage bar (`.bar`). Red from 85%, yellow from 70%, accent below.
#[derive(IntoElement)]
pub struct ProgressBar {
    percent: f32,
    color: Option<Hsla>,
    width: Option<Pixels>,
}

impl ProgressBar {
    pub fn new(percent: f32) -> Self {
        Self {
            percent: percent.clamp(0.0, 100.0),
            color: None,
            width: None,
        }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }
}

impl RenderOnce for ProgressBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        let color = self.color.unwrap_or(if self.percent >= 85.0 {
            colors.red
        } else if self.percent >= 70.0 {
            colors.yellow
        } else {
            colors.accent
        });
        div()
            .h(u(5.0))
            .rounded(u(3.0))
            .bg(colors.bar_track)
            .overflow_hidden()
            .map(|this| match self.width {
                Some(width) => this.w(width),
                None => this.w_full(),
            })
            .child(
                div()
                    .h_full()
                    .rounded(u(3.0))
                    .bg(color)
                    .w(relative(self.percent / 100.0)),
            )
    }
}
