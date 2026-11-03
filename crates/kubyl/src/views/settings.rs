//! The Settings tab: every setting of settings.json, one page per section, like Zed's settings
//! editor. Pages, controls and descriptions come from the settings schema
//! ([`kubyl_settings::catalog`]), so new fields show up without changes here. What can't be
//! edited in place (maps, lists of objects) links to settings.json.

use std::collections::HashMap;
use std::ops::Range;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight,
    HighlightStyle, IntoElement, Render, ScrollHandle, SharedString, StyledText, Subscription,
    Window, div, prelude::*, px,
};
use gpui_component::button::Button as MenuButton;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::switch::Switch;
use kubyl_core::{TabView, ViewKind, ViewRequest};
use kubyl_settings::Settings;
use kubyl_settings::catalog::{Control, Group, Item, Page};
use kubyl_ui::{
    ActiveColors, Button, Colors, Icon, IconButton, IconName, fonts, h_flex, u, v_flex,
};
use serde_json::Value;

/// The view kind of the Settings tab.
pub fn settings_view_kind() -> ViewKind {
    ViewKind::Custom("settings".into())
}

pub struct SettingsView {
    focus: FocusHandle,
    search: Entity<InputState>,
    pages: Vec<Page>,
    /// The page shown while the search is empty.
    page: usize,
    /// Text inputs of number, text and list settings, by key (`logs.tail`).
    inputs: HashMap<String, Entity<InputState>>,
    /// Why the last value typed into a setting was refused, by key.
    errors: HashMap<String, String>,
    scroll: ScrollHandle,
    nav_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings…"));
        let subscriptions = vec![
            cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.scroll.set_offset(gpui::point(px(0.), px(0.)));
                    cx.notify();
                }
            }),
            cx.observe_global_in::<Settings>(window, |this, window, cx| this.refresh(window, cx)),
        ];
        let mut this = Self {
            focus: cx.focus_handle(),
            search,
            pages: Vec::new(),
            page: 0,
            inputs: HashMap::new(),
            errors: HashMap::new(),
            scroll: ScrollHandle::new(),
            nav_scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.refresh(window, cx);
        this
    }

    /// Re-reads every setting and updates the inputs that don't have focus.
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pages = Settings::catalog(cx);
        self.page = self.page.min(self.pages.len().saturating_sub(1));
        let items: Vec<Item> = self
            .pages
            .iter()
            .flat_map(|p| &p.groups)
            .flat_map(|g| &g.items)
            .filter(|i| uses_input(&i.control))
            .cloned()
            .collect();
        for item in items {
            let key = item.key();
            let text = input_text(&item.value);
            if let Some(input) = self.inputs.get(&key) {
                let focused = input.read(cx).focus_handle(cx).is_focused(window);
                if !focused && input.read(cx).value().as_ref() != text {
                    input.update(cx, |input, cx| input.set_value(text, window, cx));
                }
                continue;
            }
            let placeholder = match input_text(&item.default) {
                d if d.is_empty() => "not set".to_string(),
                d => d,
            };
            let input = cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                state.set_value(text, window, cx);
                state
            });
            let path = item.path.clone();
            self._subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |this, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        let text = input.read(cx).value().to_string();
                        this.commit_text(&path, &text, window, cx);
                    }
                },
            ));
            self.inputs.insert(key, input);
        }
        cx.notify();
    }

    fn item(&self, path: &[String]) -> Option<&Item> {
        self.pages
            .iter()
            .flat_map(|p| &p.groups)
            .flat_map(|g| &g.items)
            .find(|i| i.path == path)
    }

    /// Parses what was typed into the input of `path` and saves it.
    fn commit_text(
        &mut self,
        path: &[String],
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.item(path).cloned() else {
            return;
        };
        let key = item.key();
        // Untouched text (Enter or leaving the field): nothing to save. Parsing it again could
        // change a value, e.g. split a list entry that contains a comma.
        if text.trim() == input_text(&item.value).trim() {
            self.errors.remove(&key);
            cx.notify();
            return;
        }
        match parse_input(&item.control, text.trim()) {
            Ok(value) => {
                let unchanged = match &value {
                    None => !item.set,
                    Some(value) => *value == item.value,
                };
                if unchanged {
                    self.errors.remove(&key);
                    cx.notify();
                    return;
                }
                self.set(item.path.clone(), value, cx);
            }
            Err(err) => {
                self.errors.insert(key, err);
                cx.notify();
            }
        }
    }

    /// Saves `value` (or resets with `None`) and remembers a refusal next to the setting.
    fn set(&mut self, path: Vec<String>, value: Option<Value>, cx: &mut Context<Self>) {
        let key = path.join(".");
        match Settings::set_value(cx, &path, value) {
            Ok(()) => {
                self.errors.remove(&key);
            }
            Err(err) => {
                self.errors.insert(key, err);
            }
        }
        cx.notify();
    }

    fn render_nav(&self, query: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let searching = !query.is_empty();
        let rows: Vec<AnyElement> = self
            .pages
            .iter()
            .enumerate()
            .map(|(ix, page)| {
                let matches = if searching {
                    page.groups
                        .iter()
                        .flat_map(|g| &g.items)
                        .filter(|i| i.matches(query))
                        .count()
                } else {
                    0
                };
                let selected = !searching && ix == self.page;
                let hover = colors.hover;
                h_flex()
                    .id(("settings-page", ix))
                    .flex_none()
                    .h(u(30.0))
                    .px(u(10.0))
                    .gap(u(8.0))
                    .rounded(u(6.0))
                    .cursor_pointer()
                    .text_size(u(13.0))
                    .when(selected, |this| {
                        this.bg(colors.selection).text_color(colors.text)
                    })
                    .when(!selected, |this| {
                        this.text_color(colors.text_muted)
                            .hover(move |s| s.bg(hover))
                    })
                    .when(searching && matches == 0, |this| this.opacity(0.45))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(page.title.clone()),
                    )
                    .when(searching && matches > 0, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .px(u(6.0))
                                .rounded(u(4.0))
                                .bg(colors.chip_background)
                                .text_size(u(11.0))
                                .text_color(colors.text_muted)
                                .child(matches.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.page = ix;
                        this.scroll.set_offset(gpui::point(px(0.), px(0.)));
                        this.search
                            .update(cx, |search, cx| search.set_value("", window, cx));
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();

        v_flex()
            .w(u(240.0))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(colors.border_variant)
            .bg(colors.panel)
            .child(
                div().flex_none().p(u(12.0)).child(
                    Input::new(&self.search)
                        .prefix(Icon::new(IconName::Search).size(12.0))
                        .cleanable(true),
                ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .id("settings-nav")
                            .track_scroll(&self.nav_scroll)
                            .size_full()
                            .overflow_y_scroll()
                            .px(u(8.0))
                            .pb(u(12.0))
                            .gap(u(1.0))
                            .children(rows),
                    )
                    .vertical_scrollbar(&self.nav_scroll),
            )
    }

    fn render_content(&self, query: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let mut body: Vec<AnyElement> = Vec::new();
        if query.is_empty() {
            if let Some(page) = self.pages.get(self.page) {
                body.push(page_header(page, &colors));
                for group in &page.groups {
                    body.push(self.render_group(group, group.items.iter().collect(), cx));
                }
            }
        } else {
            for page in &self.pages {
                let groups: Vec<(&Group, Vec<&Item>)> = page
                    .groups
                    .iter()
                    .map(|g| (g, g.items.iter().filter(|i| i.matches(query)).collect()))
                    .filter(|(_, items): &(&Group, Vec<&Item>)| !items.is_empty())
                    .collect();
                if groups.is_empty() {
                    continue;
                }
                body.push(page_header(page, &colors));
                for (group, items) in groups {
                    body.push(self.render_group(group, items, cx));
                }
            }
            if body.is_empty() {
                body.push(
                    div()
                        .pt(u(24.0))
                        .text_color(colors.text_dim)
                        .child(format!("No setting matches “{query}”."))
                        .into_any_element(),
                );
            }
        }

        let top = h_flex()
            .flex_none()
            .h(u(52.0))
            .px(u(28.0))
            .gap(u(10.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(settings_path_label(cx)),
            )
            .child(
                Button::new("open-settings-json")
                    .icon(IconName::Code)
                    .label("Edit in settings.json")
                    .on_click(|_, _, cx| open_settings_json(cx)),
            );

        v_flex().flex_1().min_w_0().h_full().child(top).child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    v_flex()
                        .id("settings-content")
                        .track_scroll(&self.scroll)
                        .size_full()
                        .overflow_y_scroll()
                        .px(u(28.0))
                        .pt(u(8.0))
                        .pb(u(32.0))
                        .child(v_flex().w_full().max_w(u(860.0)).children(body)),
                )
                .vertical_scrollbar(&self.scroll),
        )
    }

    fn render_group(&self, group: &Group, items: Vec<&Item>, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        v_flex()
            .flex_none()
            .pt(u(18.0))
            .when_some(group.title.clone(), |this, title| {
                this.child(
                    v_flex()
                        .pb(u(6.0))
                        .border_b_1()
                        .border_color(colors.border_variant)
                        .child(
                            div()
                                .text_size(u(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_dim)
                                .child(title.to_uppercase()),
                        )
                        .children(group.description.clone().map(|d| {
                            div()
                                .pt(u(2.0))
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child(rich_text(&d, &colors))
                        })),
                )
            })
            .children(items.into_iter().map(|item| self.render_item(item, cx)))
            .into_any_element()
    }

    fn render_item(&self, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let key = item.key();
        let error = self.errors.get(&key).cloned();
        let reset_path = item.path.clone();
        let title = h_flex()
            .gap(u(6.0))
            .child(
                div()
                    .text_size(u(13.5))
                    .text_color(colors.text)
                    .child(item.title.clone()),
            )
            .when(item.set, |this| {
                this.child(
                    div()
                        .id(SharedString::from(format!("reset-tip-{key}")))
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new("Reset to default")
                                .build(window, cx)
                        })
                        .child(
                            IconButton::new(
                                SharedString::from(format!("reset-{key}")),
                                IconName::RotateCcw,
                            )
                            .icon_size(11.0)
                            .on_click(cx.listener(
                                move |this, _: &ClickEvent, _, cx| {
                                    this.set(reset_path.clone(), None, cx);
                                },
                            )),
                        ),
                )
            });
        let wide = matches!(item.control, Control::Text { .. } | Control::StringList);
        let text = v_flex()
            .flex_1()
            .min_w_0()
            .gap(u(3.0))
            .child(title)
            .children(item.description.as_ref().map(|d| {
                div()
                    .text_size(u(12.0))
                    .line_height(u(18.0))
                    .text_color(colors.text_dim)
                    .child(rich_text(d, &colors))
            }))
            .child(
                div()
                    .font_family(fonts::MONO)
                    .text_size(u(11.0))
                    .text_color(colors.text_faint)
                    .child(key.clone()),
            )
            // Text fields need the width: they go under the description, not beside it.
            .when(wide, |this| {
                this.child(
                    div()
                        .pt(u(4.0))
                        .w_full()
                        .max_w(u(480.0))
                        .child(self.render_control(item, cx)),
                )
            })
            .children(error.map(|e| div().text_size(u(12.0)).text_color(colors.red).child(e)));
        h_flex()
            .id(SharedString::from(format!("setting-{key}")))
            .flex_none()
            .items_start()
            .gap(u(24.0))
            .py(u(14.0))
            .border_b_1()
            .border_color(colors.row_border)
            .child(text)
            .when(!wide, |this| {
                this.child(
                    div()
                        .flex_none()
                        .pt(u(2.0))
                        .child(self.render_control(item, cx)),
                )
            })
            .into_any_element()
    }

    fn render_control(&self, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let key = item.key();
        let path = item.path.clone();
        match &item.control {
            Control::Toggle => {
                let on = item.value.as_bool().unwrap_or(false);
                Switch::new(SharedString::from(format!("toggle-{key}")))
                    .checked(on)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        this.set(path.clone(), Some(Value::Bool(*checked)), cx);
                    }))
                    .into_any_element()
            }
            Control::Choice { options, nullable } => {
                let current = item.value.as_str().map(str::to_string);
                let weak = cx.weak_entity();
                let options = options.clone();
                let nullable = *nullable;
                MenuButton::new(SharedString::from(format!("choice-{key}")))
                    .outline()
                    .compact()
                    .child(
                        h_flex()
                            .gap(u(6.0))
                            .text_size(u(12.5))
                            .child(
                                div()
                                    .max_w(u(220.0))
                                    .truncate()
                                    .child(current.clone().unwrap_or_else(|| "not set".into())),
                            )
                            .child(Icon::new(IconName::ChevronDown).size(11.0)),
                    )
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.max_h(px(420.0)).scrollable(true);
                        let pick = |value: Option<String>| {
                            let weak = weak.clone();
                            let path = path.clone();
                            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                let value = value.clone().map(Value::String);
                                weak.update(cx, |this, cx| this.set(path.clone(), value, cx))
                                    .ok();
                            }
                        };
                        if nullable {
                            menu = menu.item(
                                PopupMenuItem::new("not set")
                                    .checked(current.is_none())
                                    .on_click(pick(None)),
                            );
                        }
                        for option in &options {
                            let label = match &option.description {
                                Some(d) => format!("{} — {}", option.value, strip_ticks(d)),
                                None => option.value.clone(),
                            };
                            menu = menu.item(
                                PopupMenuItem::new(label)
                                    .checked(current.as_deref() == Some(option.value.as_str()))
                                    .on_click(pick(Some(option.value.clone()))),
                            );
                        }
                        menu
                    })
                    .into_any_element()
            }
            Control::Integer { .. } | Control::Number { .. } => match self.inputs.get(&key) {
                Some(input) => div()
                    .w(u(120.0))
                    .child(Input::new(input))
                    .into_any_element(),
                None => div().into_any_element(),
            },
            Control::Text { .. } | Control::StringList => match self.inputs.get(&key) {
                Some(input) => div().w_full().child(Input::new(input)).into_any_element(),
                None => div().into_any_element(),
            },
            Control::Json => h_flex()
                .gap(u(10.0))
                .child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child(json_summary(&item.value)),
                )
                .child(
                    Button::new(SharedString::from(format!("json-{key}")))
                        .ghost()
                        .label("Edit in settings.json")
                        .on_click(|_, _, cx| open_settings_json(cx)),
                )
                .into_any_element(),
        }
    }
}

fn settings_path_label(cx: &App) -> String {
    let path = Settings::path(cx);
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(|p| p.to_owned())) {
        Some(rel) => format!("~/{}", rel.display()),
        None => path.display().to_string(),
    }
}

/// Opens settings.json in the system's editor for JSON.
pub fn open_settings_json(cx: &mut App) {
    let path = Settings::path(cx).to_path_buf();
    cx.open_with_system(&path);
}

fn page_header(page: &Page, colors: &Colors) -> AnyElement {
    v_flex()
        .flex_none()
        .pt(u(20.0))
        .gap(u(4.0))
        .child(
            div()
                .text_size(u(20.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text)
                .child(page.title.clone()),
        )
        .children(page.description.as_ref().map(|d| {
            div()
                .text_size(u(12.5))
                .text_color(colors.text_dim)
                .child(rich_text(d, colors))
        }))
        .into_any_element()
}

/// A doc comment with its `code` spans shown as code.
fn rich_text(text: &str, colors: &Colors) -> StyledText {
    let mut plain = String::with_capacity(text.len());
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    for (i, part) in text.split('`').enumerate() {
        let start = plain.len();
        plain.push_str(part);
        if i % 2 == 1 && !part.is_empty() {
            highlights.push((
                start..plain.len(),
                HighlightStyle {
                    color: Some(colors.text_muted),
                    background_color: Some(colors.chip_background),
                    ..Default::default()
                },
            ));
        }
    }
    StyledText::new(plain).with_highlights(highlights)
}

fn strip_ticks(text: &str) -> String {
    text.replace('`', "")
}

fn uses_input(control: &Control) -> bool {
    matches!(
        control,
        Control::Integer { .. }
            | Control::Number { .. }
            | Control::Text { .. }
            | Control::StringList
    )
}

/// The text an input shows for `value`.
fn input_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Array(items) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string())
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

/// What an input's text means: `None` resets the setting (an empty input).
fn parse_input(control: &Control, text: &str) -> Result<Option<Value>, String> {
    if text.is_empty() {
        return Ok(None);
    }
    match control {
        Control::Integer { min, max, .. } => {
            let n: i64 = text
                .parse()
                .map_err(|_| format!("“{text}” is not a whole number."))?;
            if let Some(min) = min
                && (n as f64) < *min
            {
                return Err(format!("The smallest value is {min}."));
            }
            if let Some(max) = max
                && (n as f64) > *max
            {
                return Err(format!("The largest value is {max}."));
            }
            Ok(Some(Value::from(n)))
        }
        Control::Number { .. } => {
            let n: f64 = text
                .parse()
                .map_err(|_| format!("“{text}” is not a number."))?;
            serde_json::Number::from_f64(n)
                .map(|n| Some(Value::Number(n)))
                .ok_or_else(|| format!("“{text}” is not a number."))
        }
        Control::StringList => Ok(Some(Value::Array(
            text.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| Value::String(s.to_string()))
                .collect(),
        ))),
        _ => Ok(Some(Value::String(text.to_string()))),
    }
}

/// "3 entries", "not set"… for settings edited in settings.json.
fn json_summary(value: &Value) -> String {
    match value {
        Value::Null => "not set".into(),
        Value::Object(map) if map.is_empty() => "none".into(),
        Value::Array(items) if items.is_empty() => "none".into(),
        Value::Object(map) => plural(map.len(), "entry", "entries"),
        Value::Array(items) => plural(items.len(), "item", "items"),
        other => other.to_string(),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for SettingsView {
    fn tab_title(&self, _: &App) -> SharedString {
        "Settings".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Settings.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(ViewRequest::new(settings_view_kind()))
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let query = self.search.read(cx).value().trim().to_string();
        h_flex()
            .id("settings-view")
            .key_context("SettingsView")
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .child(self.render_nav(&query, cx))
            .child(self.render_content(&query, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_parse_by_control() {
        let int = Control::Integer {
            min: Some(0.0),
            max: Some(10.0),
            nullable: false,
        };
        assert_eq!(parse_input(&int, "4"), Ok(Some(Value::from(4))));
        assert!(parse_input(&int, "11").is_err());
        assert!(parse_input(&int, "x").is_err());
        assert_eq!(parse_input(&int, ""), Ok(None));
        assert_eq!(
            parse_input(&Control::StringList, "a, b,,c"),
            Ok(Some(serde_json::json!(["a", "b", "c"])))
        );
        assert_eq!(
            input_text(&serde_json::json!(["a", "b"])),
            "a, b".to_string()
        );
        assert_eq!(json_summary(&serde_json::json!({"a": 1})), "1 entry");
    }
}
