use gpui::{App, FontWeight, IntoElement, RenderOnce, SharedString, Window, div, prelude::*};
use gpui_component::h_flex;

use crate::{ActiveColors, fonts, sizes, u};

/// Formats a GPUI keystroke (`cmd-k`, `ctrl-shift-p`) for display: `⌘K` on macOS, `Ctrl+K`
/// elsewhere. Single keys stay as typed (`l`, `/`).
pub fn format_keystroke(keystroke: &str) -> String {
    keystroke
        .split_whitespace()
        .map(|chord| format_chord(chord, cfg!(target_os = "macos")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_chord(chord: &str, mac: bool) -> String {
    let mut parts: Vec<&str> = chord.split('-').collect();
    // `ctrl--` means ctrl + minus.
    if chord.ends_with("--") {
        parts.retain(|p| !p.is_empty());
        parts.push("-");
    }
    let Some((key, modifiers)) = parts.split_last() else {
        return String::new();
    };
    let key = match *key {
        "escape" => "Esc".to_string(),
        "enter" => "↵".to_string(),
        "backspace" => "⌫".to_string(),
        "tab" => "⇥".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "space" => "Space".to_string(),
        k if modifiers.is_empty() => k.to_string(),
        k => k.to_uppercase(),
    };
    let mods: Vec<&str> = modifiers
        .iter()
        .map(|m| match (*m, mac) {
            ("cmd" | "super" | "win" | "secondary", true) => "⌘",
            ("secondary", false) => "Ctrl",
            ("ctrl", true) => "⌃",
            ("alt", true) => "⌥",
            ("shift", true) => "⇧",
            ("cmd" | "super" | "win", false) => "Super",
            ("ctrl", false) => "Ctrl",
            ("alt", false) => "Alt",
            ("shift", false) => "Shift",
            (other, _) => other,
        })
        .collect();
    if mac {
        format!("{}{key}", mods.concat())
    } else if mods.is_empty() {
        key
    } else {
        format!("{}+{key}", mods.join("+"))
    }
}

/// A keyboard shortcut badge (`.kbd`).
#[derive(IntoElement)]
pub struct Kbd {
    label: SharedString,
}

impl Kbd {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
        }
    }

    /// From a GPUI keystroke, formatted for the platform.
    pub fn keystroke(keystroke: &str) -> Self {
        Self::new(format_keystroke(keystroke))
    }
}

impl RenderOnce for Kbd {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        div()
            .flex_none()
            .px(u(5.0))
            .rounded(u(4.0))
            .border_1()
            .border_color(colors.border)
            .bg(colors.kbd_background)
            .font_family(fonts::MONO)
            .text_size(u(11.0))
            .line_height(u(15.0))
            .text_color(colors.text_muted)
            .child(self.label)
    }
}

/// The k9s-style key-hint bar under list views (`.hints`): `l Logs  s Shell  d Describe`.
#[derive(IntoElement, Default)]
pub struct KeyHints {
    hints: Vec<(SharedString, SharedString)>,
}

impl KeyHints {
    pub fn new(hints: impl IntoIterator<Item = (SharedString, SharedString)>) -> Self {
        Self {
            hints: hints.into_iter().collect(),
        }
    }
}

impl RenderOnce for KeyHints {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        h_flex()
            .flex_none()
            .h(u(sizes::KEY_HINTS))
            .px(u(12.0))
            .gap(u(14.0))
            .overflow_hidden()
            .border_t_1()
            .border_color(colors.border_variant)
            .bg(colors.subheader_background)
            .text_size(u(12.0))
            .text_color(colors.text_dim)
            .whitespace_nowrap()
            .children(self.hints.into_iter().map(|(key, label)| {
                h_flex()
                    .gap(u(5.0))
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.accent)
                            .child(format_keystroke(&key)),
                    )
                    .child(label)
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::format_chord;

    #[test]
    fn formats_keystrokes_per_platform() {
        assert_eq!(format_chord("cmd-k", true), "⌘K");
        assert_eq!(format_chord("ctrl-d", true), "⌃D");
        assert_eq!(format_chord("shift-f", true), "⇧F");
        assert_eq!(format_chord("l", true), "l");
        assert_eq!(format_chord("/", false), "/");
        assert_eq!(format_chord("ctrl-k", false), "Ctrl+K");
        assert_eq!(format_chord("ctrl-shift-p", false), "Ctrl+Shift+P");
        assert_eq!(format_chord("cmd--", true), "⌘-");
        assert_eq!(format_chord("shift-escape", false), "Shift+Esc");
        assert_eq!(format_chord("secondary-enter", true), "⌘↵");
        assert_eq!(format_chord("secondary-s", false), "Ctrl+S");
    }
}
