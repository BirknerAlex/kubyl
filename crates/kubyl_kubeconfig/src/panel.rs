//! The "Test connection" results (board 11, YAML tab and test panel): one row per step with
//! ✓/✗, details, the plain-language error and the fix it suggests.

use std::rc::Rc;

use gpui::{AnyElement, App, FontWeight, IntoElement, SharedString, Window, div, prelude::*};
use kubyl_ui::{Button, Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use crate::conntest::{Fix, Report, Status, StepKind};
use crate::state::SignInState;
use crate::widgets;

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;
type NamespaceHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// What the buttons of a report do.
#[derive(Clone, Default)]
pub struct Handlers {
    pub fetch_ca: Option<Handler>,
    pub sign_in: Option<Handler>,
    pub cancel_sign_in: Option<Handler>,
    pub retest: Option<Handler>,
    pub edit_user: Option<Handler>,
    /// A namespace chip was clicked.
    pub namespace: Option<NamespaceHandler>,
}

/// The whole report: steps, then namespaces.
pub fn report(
    id: &str,
    report: &Report,
    subtitle: Option<String>,
    sign_in: Option<&SignInState>,
    handlers: &Handlers,
    colors: &Colors,
) -> AnyElement {
    let mut steps = v_flex().gap(u(10.0));
    let skipped: Vec<&'static str> = report
        .steps
        .iter()
        .filter(|s| s.status == Status::Skipped && s.lines.iter().any(|l| l.starts_with("Skipped")))
        .map(|s| s.kind.title())
        .collect();
    for step in &report.steps {
        if step.status == Status::Skipped && step.lines.iter().any(|l| l.starts_with("Skipped")) {
            continue;
        }
        steps = steps.child(step_row(id, step, report, sign_in, handlers, colors));
    }
    if !skipped.is_empty() {
        let why = report
            .steps
            .iter()
            .find(|s| s.status == Status::Skipped)
            .and_then(|s| s.lines.first().cloned())
            .unwrap_or_default();
        steps = steps.child(
            h_flex()
                .items_start()
                .gap(u(10.0))
                .child(widgets::status_icon(Status::Skipped, colors))
                .child(
                    v_flex()
                        .gap(u(2.0))
                        .child(
                            div()
                                .text_size(u(12.5))
                                .text_color(colors.text_faint)
                                .child(why),
                        )
                        .child(
                            div()
                                .text_size(u(11.5))
                                .text_color(colors.text_faint)
                                .child(skipped.join(" · ")),
                        ),
                ),
        );
    }
    let header = h_flex()
        .gap(u(8.0))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    h_flex()
                        .gap(u(6.0))
                        .text_size(u(13.0))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Test connection"),
                        )
                        .child(div().text_color(colors.text_dim).child("·"))
                        .child(
                            div()
                                .font_family(fonts::MONO)
                                .text_size(u(12.5))
                                .truncate()
                                .child(report.context.clone()),
                        ),
                )
                .child(div().text_size(u(11.5)).text_color(colors.text_dim).child(
                    match (&subtitle, report.done) {
                        (Some(s), true) => format!("{s} · {}", duration(report.millis)),
                        (Some(s), false) => format!("{s} · testing…"),
                        (None, true) => duration(report.millis),
                        (None, false) => "testing…".into(),
                    },
                )),
        )
        .children(
            handlers
                .retest
                .clone()
                .filter(|_| report.done)
                .map(|retest| {
                    Button::new(SharedString::from(format!("{id}-retest")))
                        .ghost()
                        .icon(IconName::RefreshCw)
                        .label("Run again")
                        .on_click(move |_, window, cx| retest(window, cx))
                }),
        );
    let mut out = v_flex().gap(u(12.0)).child(header).child(steps);
    if let Some(names) = &report.namespaces {
        out = out.child(namespaces(id, names, handlers, colors));
    }
    out.into_any_element()
}

fn step_row(
    id: &str,
    step: &crate::conntest::Step,
    report: &Report,
    sign_in: Option<&SignInState>,
    handlers: &Handlers,
    colors: &Colors,
) -> AnyElement {
    let dim = matches!(step.status, Status::Pending | Status::Skipped);
    let mut body = v_flex().flex_1().min_w_0().gap(u(2.0)).child(
        h_flex()
            .gap(u(8.0))
            .child(
                div()
                    .flex_1()
                    .text_size(u(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(dim, |this| this.text_color(colors.text_faint))
                    .child(step.kind.title()),
            )
            .children(
                step.millis
                    .filter(|_| step.kind != StepKind::Latency)
                    .map(|ms| {
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(if ms >= 1000 {
                                format!("{:.1} s", ms as f64 / 1000.0)
                            } else {
                                format!("{ms} ms")
                            })
                    }),
            ),
    );
    for line in &step.lines {
        body = body.child(
            div()
                .font_family(fonts::MONO)
                .text_size(u(11.5))
                .text_color(if dim {
                    colors.text_faint
                } else {
                    colors.text_dim
                })
                .child(line.clone()),
        );
    }
    for check in &step.checks {
        let (mark, color) = match check.allowed {
            Some(true) => ("✓", colors.green),
            Some(false) if check.optional => ("—", colors.text_faint),
            Some(false) => ("✗", colors.red),
            None => ("?", colors.text_faint),
        };
        body = body.child(
            h_flex()
                .gap(u(6.0))
                .font_family(fonts::MONO)
                .text_size(u(11.5))
                .child(div().text_color(color).child(mark))
                .child(
                    div()
                        .text_color(if check.allowed == Some(true) {
                            colors.text_dim
                        } else {
                            colors.text_faint
                        })
                        .child(check.label.clone()),
                ),
        );
    }
    if let Some(error) = &step.error {
        body = body.child(
            div()
                .pt(u(2.0))
                .text_size(u(12.0))
                .text_color(colors.text)
                .child(capitalize(error)),
        );
    }
    if let Some(stderr) = &step.stderr {
        body = body.child(
            div()
                .mt(u(4.0))
                .p(u(8.0))
                .rounded(u(5.0))
                .bg(colors.input_background)
                .font_family(fonts::MONO)
                .text_size(u(11.0))
                .text_color(colors.text_muted)
                .child(truncate(stderr, 1200)),
        );
    }
    if let Some(fix) = &step.fix {
        body = body.child(fix_row(id, fix, report, sign_in, handlers, colors));
    }
    h_flex()
        .items_start()
        .gap(u(10.0))
        .child(widgets::status_icon(step.status, colors))
        .child(body)
        .into_any_element()
}

fn fix_row(
    id: &str,
    fix: &Fix,
    report: &Report,
    sign_in: Option<&SignInState>,
    handlers: &Handlers,
    colors: &Colors,
) -> AnyElement {
    let row = h_flex().pt(u(6.0)).gap(u(8.0)).flex_wrap();
    match fix {
        Fix::FetchCa => row
            .children(handlers.fetch_ca.clone().map(|f| {
                Button::new(SharedString::from(format!("{id}-fetch-ca")))
                    .icon(IconName::Download)
                    .label("Fetch the CA from the server…")
                    .on_click(move |_, window, cx| f(window, cx))
            }))
            .into_any_element(),
        Fix::InstallHint(hint) => {
            let copy = hint.clone();
            row.child(
                v_flex()
                    .gap(u(6.0))
                    .child(
                        div()
                            .p(u(8.0))
                            .rounded(u(5.0))
                            .border_1()
                            .border_color(colors.border)
                            .text_size(u(12.0))
                            .child(hint.clone()),
                    )
                    .child(
                        Button::new(SharedString::from(format!("{id}-copy-hint")))
                            .ghost()
                            .icon(IconName::Copy)
                            .label("Copy install hint")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))
                            }),
                    ),
            )
            .into_any_element()
        }
        Fix::SignIn => match sign_in {
            Some(SignInState::WaitingForBrowser { url }) => {
                let url = url.clone();
                row.child(
                    h_flex()
                        .gap(u(8.0))
                        .child(
                            Icon::new(IconName::RefreshCw)
                                .size(13.0)
                                .color(colors.accent),
                        )
                        .child(div().text_size(u(12.0)).child("Waiting for your browser…"))
                        .child(
                            Button::new(SharedString::from(format!("{id}-reopen")))
                                .ghost()
                                .label("Reopen browser")
                                .on_click(move |_, _, _| {
                                    open::that_detached(&url).ok();
                                }),
                        )
                        .children(handlers.cancel_sign_in.clone().map(|f| {
                            Button::new(SharedString::from(format!("{id}-cancel-sign-in")))
                                .ghost()
                                .label("Cancel")
                                .on_click(move |_, window, cx| f(window, cx))
                        })),
                )
                .into_any_element()
            }
            Some(SignInState::Starting) | Some(SignInState::DeviceCode { .. }) => row
                .child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child("Signing in…"),
                )
                .into_any_element(),
            Some(SignInState::Failed(_)) | None if report.oidc.is_some() => {
                let failed = matches!(sign_in, Some(SignInState::Failed(_)));
                let err = match sign_in {
                    Some(SignInState::Failed(err)) => Some(err.clone()),
                    _ => None,
                };
                row.children(
                    err.filter(|_| failed)
                        .map(|e| div().text_size(u(12.0)).text_color(colors.red).child(e)),
                )
                .children(handlers.sign_in.clone().map(|f| {
                    Button::new(SharedString::from(format!("{id}-sign-in")))
                        .primary()
                        .icon(IconName::Key)
                        .label("Sign in…")
                        .on_click(move |_, window, cx| f(window, cx))
                }))
                .into_any_element()
            }
            _ => row.into_any_element(),
        },
        Fix::ReplaceToken => row
            .children(handlers.edit_user.clone().map(|f| {
                Button::new(SharedString::from(format!("{id}-edit-user")))
                    .label("Edit the token…")
                    .on_click(move |_, window, cx| f(window, cx))
            }))
            .into_any_element(),
    }
}

fn namespaces(id: &str, names: &[String], handlers: &Handlers, colors: &Colors) -> AnyElement {
    let shown: Vec<String> = names.iter().take(12).cloned().collect();
    let more = names.len().saturating_sub(shown.len());
    v_flex()
        .gap(u(8.0))
        .p(u(10.0))
        .rounded(u(6.0))
        .border_1()
        .border_color(colors.border_variant)
        .child(
            h_flex()
                .child(
                    div()
                        .flex_1()
                        .text_size(u(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_dim)
                        .child(format!("NAMESPACES · {}", names.len())),
                )
                .children(handlers.namespace.as_ref().map(|_| {
                    div()
                        .text_size(u(11.0))
                        .text_color(colors.text_dim)
                        .child("click one: use as default")
                })),
        )
        .child(
            h_flex()
                .flex_wrap()
                .gap(u(6.0))
                .children(shown.into_iter().enumerate().map(|(ix, name)| {
                    let on_click = handlers.namespace.clone();
                    let value = name.clone();
                    div()
                        .id(SharedString::from(format!("{id}-ns-{ix}")))
                        .px(u(7.0))
                        .h(u(20.0))
                        .flex()
                        .items_center()
                        .rounded(u(4.0))
                        .bg(colors.chip_background)
                        .text_size(u(11.5))
                        .text_color(colors.text_muted)
                        .when(on_click.is_some(), |this| {
                            this.cursor_pointer().hover(|s| s.bg(colors.hover))
                        })
                        .on_click(move |_, window, cx| {
                            if let Some(f) = &on_click {
                                f(&value, window, cx);
                            }
                        })
                        .child(name)
                }))
                .when(more > 0, |this| {
                    this.child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(format!("+{more} more")),
                    )
                }),
        )
        .into_any_element()
}

/// `320 ms`, `1.2 s`.
fn duration(millis: u64) -> String {
    if millis < 1000 {
        format!("{millis} ms")
    } else {
        format!("{:.1} s", millis as f64 / 1000.0)
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

/// A compact status for list rows: ✓, ✗ or a spinner.
pub fn badge(report: &Report, colors: &Colors) -> AnyElement {
    let status = if !report.done {
        Status::Running
    } else if report.passed() {
        Status::Ok
    } else {
        Status::Fail
    };
    div()
        .flex_none()
        .child(match status {
            Status::Ok => Icon::new(IconName::Check).size(12.0).color(colors.green),
            Status::Fail => Icon::new(IconName::CircleX).size(12.0).color(colors.red),
            _ => Icon::new(IconName::RefreshCw)
                .size(12.0)
                .color(colors.accent),
        })
        .into_any_element()
}
