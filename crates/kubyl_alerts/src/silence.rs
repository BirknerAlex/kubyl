//! Silences: the editor (matchers with completion, duration, comment, creator, live preview),
//! the summary every create and edit shows first (typed cluster name on PROD when it matters),
//! acknowledge, extend, expire and recreate, and the Undo toast.
//!
//! Guards: nothing here opens on read-only clusters; a silence always has a comment and at least
//! one matcher that doesn't match the empty string; the cluster's `matchers` (a central
//! Alertmanager) are always added; writes with a service-account token need consent.

use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight,
    IntoElement, Render, SharedString, Subscription, Task, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use jiff::Timestamp;
use kubyl_core::{ClusterId, Notification, NotificationCenter, NotificationLevel};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, ProdBadge, fonts, h_flex, u, v_flex};

use crate::client;
use crate::matchers::{self, MatchOp, Matcher};
use crate::model::{Alert, AlertState, Severity, Silence};
use crate::service::{AlertsService, Pace, created_by};
use crate::view::widgets::{self, severity_color};

/// Durations offered in the editor.
const DURATIONS: [(&str, u64); 5] = [
    ("1h", 3600),
    ("2h", 7200),
    ("4h", 14_400),
    ("1d", 86_400),
    ("1w", 604_800),
];
/// On PROD, the summary asks for the typed cluster name above this many matched alerts.
const MANY: usize = 10;

fn production(cluster: &ClusterId, cx: &App) -> bool {
    ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(cluster).production)
}

fn read_only(cluster: &ClusterId, cx: &App) -> bool {
    ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(cluster).read_only)
}

fn cluster_name(cluster: &ClusterId, cx: &App) -> String {
    ConnectionManager::try_global(cx)
        .map(|m| m.read(cx).display_name(cluster).to_string())
        .unwrap_or_else(|| cluster.to_string())
}

/// The alerts of a cluster (for the preview and completion).
fn cluster_alerts(cluster: &ClusterId, cx: &App) -> Arc<Vec<Alert>> {
    AlertsService::global(cx)
        .and_then(|s| {
            s.read(cx)
                .cluster(cluster, Pace::Background)
                .map(|c| c.alerts.clone())
        })
        .unwrap_or_default()
}

fn cluster_matchers(cluster: &ClusterId, cx: &App) -> Vec<Matcher> {
    AlertsService::global(cx)
        .and_then(|s| {
            s.read(cx)
                .cluster(cluster, Pace::Background)
                .map(|c| c.matchers.clone())
        })
        .unwrap_or_default()
}

/// Alertmanagers of the cluster: label and service account (writes with its token need
/// consent).
fn alertmanagers(cluster: &ClusterId, cx: &App) -> Vec<(String, Option<String>)> {
    AlertsService::global(cx)
        .and_then(|s| {
            s.read(cx).cluster(cluster, Pace::Background).map(|c| {
                c.sources
                    .iter()
                    .map(|s| (s.label(), s.conn.service_account()))
                    .collect()
            })
        })
        .unwrap_or_default()
}

fn guard(cluster: &ClusterId, cx: &mut App) -> bool {
    if read_only(cluster, cx) {
        NotificationCenter::push(
            cx,
            Notification::error("This cluster is read-only in Kubyl."),
        );
        return false;
    }
    if alertmanagers(cluster, cx).is_empty() {
        NotificationCenter::push(
            cx,
            Notification::error(
                "Silences need an Alertmanager; none is connected for this cluster.",
            ),
        );
        return false;
    }
    true
}

/// What a silence covers, as the summary shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Impact {
    pub matched: Vec<Alert>,
    pub critical: usize,
    pub warning: usize,
    pub other: usize,
}

/// Alerts the matchers select now (active ones: firing, pending, already silenced or inhibited).
pub fn impact(matchers: &[Matcher], alerts: &[Alert]) -> Impact {
    let matched: Vec<Alert> = alerts
        .iter()
        .filter(|a| a.state != AlertState::Resolved)
        .filter(|a| matchers.iter().all(|m| m.matches(&a.labels)))
        .cloned()
        .collect();
    let mut impact = Impact::default();
    for alert in &matched {
        match alert.severity {
            Severity::Critical => impact.critical += 1,
            Severity::Warning => impact.warning += 1,
            _ => impact.other += 1,
        }
    }
    impact.matched = matched;
    impact
}

/// On PROD, the typed cluster name is needed when the silence matches a critical alert, more
/// than [`MANY`] alerts, or has no `alertname` matcher.
pub fn needs_typed_name(production: bool, matchers: &[Matcher], impact: &Impact) -> bool {
    production
        && (impact.critical > 0
            || impact.matched.len() > MANY
            || !matchers.iter().any(|m| m.name == "alertname"))
}

/// A silence being written.
#[derive(Clone, Debug)]
struct Draft {
    cluster: ClusterId,
    /// Editing (or extending) this silence.
    id: Option<String>,
    /// The Alertmanager it goes to.
    source: Option<String>,
    matchers: Vec<Matcher>,
    starts_at: Timestamp,
    ends_at: Timestamp,
    comment: String,
    created_by: String,
    /// For the summary line ("From KubePodCrashLooping on pod/…").
    from: Option<Alert>,
    /// The id of the silence this replaces, for "recreate" and undo.
    replaces_expired: bool,
}

// ----- Entry points -----

/// "New silence…" (empty).
pub fn open_new(cluster: &ClusterId, window: &mut Window, cx: &mut App) {
    if !guard(cluster, cx) {
        return;
    }
    open_editor(cluster.clone(), None, Vec::new(), None, window, cx);
}

/// "Silence…" on an alert: its labels, with the name, namespace and target labels ticked.
pub fn open_for_alert(cluster: &ClusterId, alert: &Alert, window: &mut Window, cx: &mut App) {
    if !guard(cluster, cx) {
        return;
    }
    let ticked = [
        "alertname",
        "namespace",
        "pod",
        "deployment",
        "statefulset",
        "daemonset",
        "job_name",
        "cronjob",
        "persistentvolumeclaim",
        "node",
        "horizontalpodautoscaler",
        "exported_namespace",
        "exported_pod",
    ];
    let target_label: Option<&str> = alert.target.as_ref().map(|t| match t.kind {
        crate::model::TargetKind::Service => "service",
        crate::model::TargetKind::ClusterOperator => "name",
        crate::model::TargetKind::Node => "instance",
        _ => "",
    });
    let rows: Vec<(bool, Matcher)> = alert
        .labels
        .iter()
        .map(|(k, v)| {
            let on = ticked.contains(&k.as_str()) || target_label == Some(k.as_str());
            (on, Matcher::new(k, MatchOp::Equal, v))
        })
        .collect();
    // Alert name first.
    let mut rows = rows;
    rows.sort_by_key(|(_, m)| m.name != "alertname");
    open_editor(cluster.clone(), None, rows, Some(alert.clone()), window, cx);
}

/// Edit an active or pending silence.
pub fn open_edit(cluster: &ClusterId, silence: &Silence, window: &mut Window, cx: &mut App) {
    if !guard(cluster, cx) {
        return;
    }
    if silence.state == "expired" {
        return open_recreate(cluster, silence, window, cx);
    }
    let rows = own_matchers(cluster, &silence.matchers, cx)
        .into_iter()
        .map(|m| (true, m))
        .collect();
    let view = cx.new(|cx| {
        let mut editor = SilenceDialog::new(cluster.clone(), rows, None, window, cx);
        editor.editing = Some(silence.clone());
        editor.source = Some(silence.source.clone());
        editor
            .comment
            .update(cx, |c, cx| c.set_value(silence.comment.clone(), window, cx));
        editor.custom_end = silence.ends_at;
        editor
    });
    show(view, window, cx);
}

/// Recreate an expired silence (same matchers and comment, from now for its old length).
pub fn open_recreate(cluster: &ClusterId, silence: &Silence, window: &mut Window, cx: &mut App) {
    if !guard(cluster, cx) {
        return;
    }
    let rows = own_matchers(cluster, &silence.matchers, cx)
        .into_iter()
        .map(|m| (true, m))
        .collect();
    let view = cx.new(|cx| {
        let mut editor = SilenceDialog::new(cluster.clone(), rows, None, window, cx);
        editor.source = Some(silence.source.clone());
        editor
            .comment
            .update(cx, |c, cx| c.set_value(silence.comment.clone(), window, cx));
        editor
    });
    show(view, window, cx);
}

/// A silence's matchers without the cluster's fixed ones (the editor adds those back).
fn own_matchers(cluster: &ClusterId, matchers: &[Matcher], cx: &App) -> Vec<Matcher> {
    let fixed = cluster_matchers(cluster, cx);
    matchers
        .iter()
        .filter(|m| !fixed.contains(m))
        .cloned()
        .collect()
}

/// Acknowledge: a silence of exactly this alert's labels for `alerts.ack_duration`, after the
/// summary.
pub fn acknowledge(cluster: &ClusterId, alert: &Alert, window: &mut Window, cx: &mut App) {
    if !guard(cluster, cx) {
        return;
    }
    let settings = AlertsService::global(cx)
        .map(|s| s.read(cx).settings().clone())
        .unwrap_or_default();
    let (user, _) = created_by(cluster, cx);
    let now = Timestamp::now();
    let mut matchers = cluster_matchers(cluster, cx);
    matchers.extend(
        alert
            .matchers()
            .into_iter()
            .filter(|m| !matchers.contains(m))
            .collect::<Vec<_>>(),
    );
    let draft = Draft {
        cluster: cluster.clone(),
        id: None,
        source: None,
        matchers,
        starts_at: now,
        ends_at: now
            .checked_add(jiff::SignedDuration::from_secs(
                settings.ack().as_secs() as i64
            ))
            .unwrap_or(now),
        comment: format!(
            "Acknowledged in Kubyl by {}",
            if user.is_empty() {
                "a Kubyl user"
            } else {
                user.as_str()
            }
        ),
        created_by: user,
        from: Some(alert.clone()),
        replaces_expired: false,
    };
    let view = cx.new(|cx| SilenceDialog::review(draft, window, cx));
    show(view, window, cx);
}

/// Extend a silence by `hours` (an edit: the summary shows first).
pub fn extend(
    cluster: &ClusterId,
    silence: &Silence,
    hours: i64,
    window: &mut Window,
    cx: &mut App,
) {
    if !guard(cluster, cx) {
        return;
    }
    let now = Timestamp::now();
    let base = silence.ends_at.filter(|t| *t > now).unwrap_or(now);
    let draft = Draft {
        cluster: cluster.clone(),
        id: Some(silence.id.clone()),
        source: Some(silence.source.clone()),
        matchers: silence.matchers.clone(),
        starts_at: silence.starts_at.unwrap_or(now),
        ends_at: base
            .checked_add(jiff::SignedDuration::from_hours(hours))
            .unwrap_or(base),
        comment: silence.comment.clone(),
        created_by: silence.created_by.clone(),
        from: None,
        replaces_expired: false,
    };
    let view = cx.new(|cx| SilenceDialog::review(draft, window, cx));
    show(view, window, cx);
}

/// Expire a silence (confirmed), with Undo.
pub fn confirm_expire(cluster: &ClusterId, silence: &Silence, window: &mut Window, cx: &mut App) {
    if read_only(cluster, cx) {
        return;
    }
    let alerts = cluster_alerts(cluster, cx);
    let matched = alerts
        .iter()
        .filter(|a| a.silenced_by.contains(&silence.id))
        .count();
    let cluster = cluster.clone();
    let silence = silence.clone();
    let colors = cx.colors().clone();
    let message = if matched == 0 {
        "It silences no alert right now.".to_string()
    } else {
        format!(
            "{matched} alert{} it silences notif{} again.",
            if matched == 1 { "" } else { "s" },
            if matched == 1 { "ies" } else { "y" }
        )
    };
    window.open_dialog(cx, move |dialog, _, _| {
        let cluster = cluster.clone();
        let silence = silence.clone();
        let message = message.clone();
        dialog
            .w(px(460.0))
            .margin_top(px(120.0))
            .bg(colors.panel)
            .close_button(false)
            .content(move |content, _, _| {
                let cluster = cluster.clone();
                let silence = silence.clone();
                content.child(
                    v_flex()
                        .gap(u(10.0))
                        .text_size(u(12.5))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(u(14.0))
                                .child(format!("Expire silence {}?", short_id(&silence.id))),
                        )
                        .child(
                            div()
                                .font_family(fonts::MONO)
                                .text_size(u(11.5))
                                .child(matchers::format(&silence.matchers)),
                        )
                        .child(div().text_color(colors.text_muted).child(message.clone()))
                        .child(
                            h_flex()
                                .gap(u(8.0))
                                .pt(u(6.0))
                                .child(div().flex_1())
                                .child(
                                    Button::new("expire-cancel")
                                        .ghost()
                                        .label("Cancel")
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    Button::new("expire-confirm")
                                        .danger()
                                        .label("Expire")
                                        .on_click(move |_, window, cx| {
                                            window.close_dialog(cx);
                                            expire(&cluster, &silence, cx);
                                        }),
                                ),
                        ),
                )
            })
    });
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn expire(cluster: &ClusterId, silence: &Silence, cx: &mut App) {
    let Some(service) = AlertsService::global(cx) else {
        return;
    };
    let task = service.update(cx, |s, cx| {
        s.expire_silence(cluster, &silence.source, &silence.id, cx)
    });
    let cluster = cluster.clone();
    let silence = silence.clone();
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| match result {
            Ok(()) => {
                let undo_cluster = cluster.clone();
                let undo = silence.clone();
                NotificationCenter::push(
                    cx,
                    Notification::success(format!("Silence {} expired.", short_id(&silence.id)))
                        .action("Undo", move |_, cx| recreate_now(&undo_cluster, &undo, cx)),
                );
            }
            Err(err) => NotificationCenter::push(
                cx,
                Notification::error(format!("Expiring the silence failed: {err}")),
            ),
        });
    })
    .detach();
}

/// Undo of expire: the same silence again until its old end (at least an hour).
fn recreate_now(cluster: &ClusterId, silence: &Silence, cx: &mut App) {
    let now = Timestamp::now();
    let ends = silence.ends_at.filter(|t| *t > now).unwrap_or_else(|| {
        now.checked_add(jiff::SignedDuration::from_hours(1))
            .unwrap_or(now)
    });
    let body = client::silence_body(
        None,
        &silence.matchers,
        now,
        ends,
        &silence.created_by,
        &silence.comment,
    );
    post(
        cluster.clone(),
        Some(silence.source.clone()),
        body,
        None,
        false,
        cx,
    );
}

/// Called with the outcome of a write.
type Done = Box<dyn FnOnce(Result<(), String>, &mut App)>;

/// Writes a silence and shows the result (with Undo for new ones).
fn post(
    cluster: ClusterId,
    source: Option<String>,
    body: serde_json::Value,
    done: Option<Done>,
    undoable: bool,
    cx: &mut App,
) {
    let Some(service) = AlertsService::global(cx) else {
        return;
    };
    let task = service.update(cx, |s, cx| {
        s.post_silence(&cluster, source.as_deref(), body, cx)
    });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            match &result {
                Ok(id) => {
                    let mut toast =
                        Notification::success(format!("Silence {} created.", short_id(id)));
                    if undoable {
                        let undo_cluster = cluster.clone();
                        let undo_source = source.clone().unwrap_or_default();
                        let undo_id = id.clone();
                        toast = toast.action("Undo", move |_, cx| {
                            if let Some(service) = AlertsService::global(cx) {
                                let cluster = undo_cluster.clone();
                                let source = if undo_source.is_empty() {
                                    alertmanagers(&cluster, cx)
                                        .first()
                                        .map(|(l, _)| l.clone())
                                        .unwrap_or_default()
                                } else {
                                    undo_source.clone()
                                };
                                let id = undo_id.clone();
                                service
                                    .update(cx, |s, cx| {
                                        s.expire_silence(&cluster, &source, &id, cx)
                                    })
                                    .detach();
                            }
                        });
                    }
                    NotificationCenter::push(cx, toast);
                }
                Err(err) => NotificationCenter::push(
                    cx,
                    Notification::new(
                        NotificationLevel::Error,
                        format!("The silence wasn't created: {err}"),
                    ),
                ),
            }
            if let Some(done) = done {
                done(result.map(|_| ()), cx);
            }
        });
    })
    .detach();
}

fn open_editor(
    cluster: ClusterId,
    source: Option<String>,
    rows: Vec<(bool, Matcher)>,
    from: Option<Alert>,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| {
        let mut editor = SilenceDialog::new(cluster, rows, from, window, cx);
        editor.source = source;
        editor
    });
    show(view, window, cx);
}

fn show(view: Entity<SilenceDialog>, window: &mut Window, cx: &mut App) {
    let colors = cx.colors().clone();
    let focus = view.read(cx).initial_focus(cx);
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(600.0))
            .margin_top(px(60.0))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            .on_ok(|_, _, _| false)
            .content({
                let view = view.clone();
                move |content, _, _| content.child(view.clone())
            })
    });
    window.focus(&focus, cx);
}

// ----- The dialog -----

struct MatcherRow {
    on: bool,
    name: Entity<InputState>,
    op: MatchOp,
    value: Entity<InputState>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Edit,
    Review,
}

struct SilenceDialog {
    cluster: ClusterId,
    step: Step,
    rows: Vec<MatcherRow>,
    /// The cluster's matchers (fixed).
    fixed: Vec<Matcher>,
    duration: Option<u64>,
    custom: Entity<InputState>,
    custom_end: Option<Timestamp>,
    comment: Entity<TextareaState>,
    created_by: Entity<InputState>,
    created_by_note: &'static str,
    from: Option<Alert>,
    source: Option<String>,
    editing: Option<Silence>,
    /// The draft under review.
    draft: Option<Draft>,
    /// Opened straight on the summary (acknowledge, extend): Back closes.
    review_only: bool,
    typed: Entity<InputState>,
    consent: bool,
    show_matches: bool,
    error: Option<String>,
    busy: bool,
    focus: FocusHandle,
    _task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl SilenceDialog {
    fn new(
        cluster: ClusterId,
        rows: Vec<(bool, Matcher)>,
        from: Option<Alert>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (user, note) = created_by(&cluster, cx);
        let created_by = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("you@example.com");
            input.set_value(user, window, cx);
            input
        });
        let comment = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Why? A ticket or incident helps whoever gets paged next.")
        });
        let custom =
            cx.new(|cx| InputState::new(window, cx).placeholder("3h30m or 2026-09-27 18:00"));
        let typed =
            cx.new(|cx| InputState::new(window, cx).placeholder(cluster_name(&cluster, cx)));
        let mut subscriptions =
            vec![
                cx.subscribe(&comment, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.error = None;
                        cx.notify();
                    }
                }),
                cx.subscribe(&custom, |this, input, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.custom_end = parse_end(&input.read(cx).value());
                        this.duration = None;
                        cx.notify();
                    }
                }),
                cx.subscribe_in(&typed, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::PressEnter { .. } => this.submit(window, cx),
                        InputEvent::Change => cx.notify(),
                        _ => {}
                    }
                }),
            ];
        let mut this = Self {
            fixed: cluster_matchers(&cluster, cx),
            cluster,
            step: Step::Edit,
            rows: Vec::new(),
            duration: Some(7200),
            custom,
            custom_end: None,
            comment,
            created_by,
            created_by_note: note,
            from,
            source: None,
            editing: None,
            draft: None,
            review_only: false,
            typed,
            consent: false,
            show_matches: false,
            error: None,
            busy: false,
            focus: cx.focus_handle(),
            _task: None,
            _subscriptions: Vec::new(),
        };
        for (on, matcher) in rows {
            this.push_row(on, matcher, window, cx);
        }
        if this.rows.is_empty() {
            this.push_row(
                true,
                Matcher::new("alertname", MatchOp::Equal, ""),
                window,
                cx,
            );
        }
        subscriptions.append(&mut this._subscriptions);
        this._subscriptions = subscriptions;
        this
    }

    fn review(draft: Draft, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::new(
            draft.cluster.clone(),
            Vec::new(),
            draft.from.clone(),
            window,
            cx,
        );
        this.source = draft.source.clone();
        this.draft = Some(draft);
        this.step = Step::Review;
        this.review_only = true;
        this
    }

    fn initial_focus(&self, cx: &App) -> FocusHandle {
        match self.step {
            Step::Review if self.needs_typed(cx) => self.typed.read(cx).focus_handle(cx),
            Step::Review => self.focus.clone(),
            Step::Edit => self.comment.read(cx).focus_handle(cx),
        }
    }

    fn push_row(
        &mut self,
        on: bool,
        matcher: Matcher,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("label");
            input.set_value(matcher.name.clone(), window, cx);
            input
        });
        let value = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("value");
            input.set_value(matcher.value.clone(), window, cx);
            input
        });
        for input in [&name, &value] {
            self._subscriptions
                .push(cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.error = None;
                        cx.notify();
                    }
                }));
        }
        self.rows.push(MatcherRow {
            on,
            name,
            op: matcher.op,
            value,
        });
    }

    /// The ticked matchers plus the cluster's fixed ones.
    fn matchers(&self, cx: &App) -> Vec<Matcher> {
        let mut out = self.fixed.clone();
        for row in self.rows.iter().filter(|r| r.on) {
            let name = row.name.read(cx).value().trim().to_string();
            if name.is_empty() {
                continue;
            }
            let matcher = Matcher::new(&name, row.op, &row.value.read(cx).value());
            if !out.contains(&matcher) {
                out.push(matcher);
            }
        }
        out
    }

    fn ends_at(&self, now: Timestamp) -> Option<Timestamp> {
        match self.duration {
            Some(seconds) => now
                .checked_add(jiff::SignedDuration::from_secs(seconds as i64))
                .ok(),
            None => self.custom_end,
        }
    }

    /// Builds the draft or says what's missing.
    fn build(&self, cx: &App) -> Result<Draft, String> {
        let matchers = self.matchers(cx);
        matchers::validate_silence(&matchers)?;
        let comment = self.comment.read(cx).value().trim().to_string();
        if comment.is_empty() {
            return Err("A comment is required.".into());
        }
        let created_by = self.created_by.read(cx).value().trim().to_string();
        if created_by.is_empty() {
            return Err("Say who creates it.".into());
        }
        let now = Timestamp::now();
        let starts_at = self
            .editing
            .as_ref()
            .and_then(|s| s.starts_at)
            .filter(|t| *t > now)
            .unwrap_or(now);
        let ends_at = self.ends_at(now).ok_or("Pick a duration or a valid end.")?;
        if ends_at <= starts_at {
            return Err("The end must be after the start.".into());
        }
        Ok(Draft {
            cluster: self.cluster.clone(),
            id: self.editing.as_ref().map(|s| s.id.clone()),
            source: self.source.clone(),
            matchers,
            starts_at,
            ends_at,
            comment,
            created_by,
            from: self.from.clone(),
            replaces_expired: false,
        })
    }

    fn impact(&self, cx: &App) -> Impact {
        let matchers = match (&self.step, &self.draft) {
            (Step::Review, Some(draft)) => draft.matchers.clone(),
            _ => self.matchers(cx),
        };
        impact(&matchers, &cluster_alerts(&self.cluster, cx))
    }

    fn needs_typed(&self, cx: &App) -> bool {
        let Some(draft) = &self.draft else {
            return false;
        };
        needs_typed_name(
            production(&self.cluster, cx),
            &draft.matchers,
            &self.impact(cx),
        )
    }

    /// The service account a write would use, if any.
    fn service_account(&self, cx: &App) -> Option<String> {
        let ams = alertmanagers(&self.cluster, cx);
        let target = self
            .draft
            .as_ref()
            .and_then(|d| d.source.clone())
            .or_else(|| self.source.clone());
        match target {
            Some(label) => ams
                .into_iter()
                .find(|(l, _)| *l == label)
                .and_then(|(_, sa)| sa),
            None => ams.into_iter().next().and_then(|(_, sa)| sa),
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            Step::Edit => match self.build(cx) {
                Ok(draft) => {
                    self.draft = Some(draft);
                    self.step = Step::Review;
                    self.error = None;
                    let focus = self.initial_focus(cx);
                    window.focus(&focus, cx);
                    cx.notify();
                }
                Err(err) => {
                    self.error = Some(err);
                    cx.notify();
                }
            },
            Step::Review => self.create(window, cx),
        }
    }

    fn ready(&self, cx: &App) -> bool {
        if self.busy {
            return false;
        }
        if self.service_account(cx).is_some() && !self.consent {
            return false;
        }
        !self.needs_typed(cx)
            || self.typed.read(cx).value().trim() == cluster_name(&self.cluster, cx)
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.clone() else {
            return;
        };
        if read_only(&draft.cluster, cx) {
            self.error = Some("This cluster is read-only in Kubyl.".into());
            cx.notify();
            return;
        }
        if !self.ready(cx) {
            self.error = Some(if self.service_account(cx).is_some() && !self.consent {
                "Confirm that the silence is created with the service account's token.".into()
            } else {
                format!("Type {} to confirm.", cluster_name(&self.cluster, cx))
            });
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        let body = client::silence_body(
            draft.id.as_deref(),
            &draft.matchers,
            draft.starts_at,
            draft.ends_at,
            &draft.created_by,
            &draft.comment,
        );
        let weak = cx.entity().downgrade();
        let window_handle = window.window_handle();
        let done: Done = Box::new(move |result, cx| {
            window_handle
                .update(cx, |_, window, cx| {
                    weak.update(cx, |this, cx| {
                        this.busy = false;
                        match result {
                            Ok(()) => window.close_dialog(cx),
                            Err(err) => this.error = Some(err),
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .ok();
        });
        let undoable = draft.id.is_none() && !draft.replaces_expired;
        post(
            draft.cluster.clone(),
            draft.source.clone(),
            body,
            Some(done),
            undoable,
            cx,
        );
        cx.notify();
    }

    // ----- Rendering -----

    fn header(&self, title: String, cx: &App) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        h_flex()
            .gap(u(10.0))
            .px(u(16.0))
            .py(u(14.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(Icon::new(IconName::BellOff).size(16.0).color(colors.accent))
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .when(production(&self.cluster, cx), |this| this.child(ProdBadge))
    }

    fn render_edit(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let name = cluster_name(&self.cluster, cx);
        let title = match &self.editing {
            Some(_) => format!("Edit silence · {name}"),
            None => format!("New silence · {name}"),
        };
        let alerts = cluster_alerts(&self.cluster, cx);
        let intro = match (&self.from, &self.editing) {
            (_, Some(silence)) => Some(
                div()
                    .text_color(colors.text_muted)
                    .child(format!(
                        "Editing silence {}. Alertmanager replaces a silence with a new id when its matchers or start change.",
                        short_id(&silence.id)
                    ))
                    .into_any_element(),
            ),
            (Some(alert), None) => Some(
                div()
                    .text_color(colors.text_muted)
                    .child(format!(
                        "From {}{}. Ticked labels become matchers.",
                        alert.name,
                        alert
                            .target
                            .as_ref()
                            .map(|t| format!(" on {}", t.label()))
                            .unwrap_or_default()
                    ))
                    .into_any_element(),
            ),
            _ => None,
        };
        let fixed_rows: Vec<AnyElement> = self
            .fixed
            .iter()
            .map(|m| {
                h_flex()
                    .gap(u(8.0))
                    .text_size(u(12.0))
                    .text_color(colors.text_muted)
                    .child(Icon::new(IconName::Lock).size(12.0))
                    .child(div().font_family(fonts::MONO).child(m.to_string()))
                    .child(
                        div()
                            .text_color(colors.text_dim)
                            .child("the cluster's matcher (settings)"),
                    )
                    .into_any_element()
            })
            .collect();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (i, row) in self.rows.iter().enumerate() {
            let label = row.name.read(cx).value().to_string();
            let mut values: Vec<String> = alerts
                .iter()
                .filter_map(|a| a.labels.get(&label).cloned())
                .collect();
            values.sort();
            values.dedup();
            values.truncate(30);
            let mut names: Vec<String> = alerts
                .iter()
                .flat_map(|a| a.labels.keys().cloned())
                .collect();
            names.sort();
            names.dedup();
            names.truncate(40);
            rows.push(self.render_row(i, row, names, values, &colors, cx));
        }
        let now = Timestamp::now();
        let ends = self.ends_at(now);
        let durations = h_flex()
            .gap(u(6.0))
            .children(DURATIONS.iter().map(|(label, seconds)| {
                let on = self.duration == Some(*seconds);
                let seconds = *seconds;
                div()
                    .id(SharedString::from(format!("duration-{label}")))
                    .px(u(10.0))
                    .py(u(3.0))
                    .rounded(u(4.0))
                    .cursor_pointer()
                    .text_size(u(12.0))
                    .border_1()
                    .border_color(if on { colors.accent } else { colors.border })
                    .when(on, |this| this.bg(colors.selection))
                    .child(*label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.duration = Some(seconds);
                        cx.notify();
                    }))
            }));
        let custom_on = self.duration.is_none();
        let custom = div()
            .w(u(170.0))
            .h(u(24.0))
            .px(u(8.0))
            .flex()
            .items_center()
            .rounded(u(4.0))
            .bg(colors.input_background)
            .border_1()
            .border_color(if custom_on && self.custom_end.is_none() {
                colors.red
            } else if custom_on {
                colors.accent
            } else {
                colors.border
            })
            .text_size(u(12.0))
            .child(Input::new(&self.custom).appearance(false));
        let end_text = ends
            .map(|t| format!("starts now · ends {}", widgets::local_and_utc(t)))
            .unwrap_or_else(|| "enter a duration (3h30m) or an end (2026-09-27 18:00)".into());
        let impact = self.impact(cx);
        let preview = self.render_preview(&impact, &colors, cx);
        let sources = alertmanagers(&self.cluster, cx);
        let source_label = self
            .source
            .clone()
            .or_else(|| sources.first().map(|(l, _)| l.clone()))
            .unwrap_or_default();
        let weak = cx.entity().downgrade();
        let source_picker: AnyElement = if sources.len() > 1 && self.editing.is_none() {
            let current = source_label.clone();
            MenuButton::new("silence-source")
                .ghost()
                .compact()
                .child(
                    h_flex()
                        .gap(u(4.0))
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(format!("Alertmanager {current}"))
                        .child(Icon::new(IconName::ChevronDown).size(11.0)),
                )
                .dropdown_menu(move |mut menu, _, _| {
                    for (label, _) in &sources {
                        let weak = weak.clone();
                        let value = label.clone();
                        menu = menu.item(
                            PopupMenuItem::new(label.clone())
                                .checked(*label == current)
                                .on_click(move |_, _, cx| {
                                    let value = value.clone();
                                    weak.update(cx, |this, cx| {
                                        this.source = Some(value);
                                        cx.notify();
                                    })
                                    .ok();
                                }),
                        );
                    }
                    menu
                })
                .into_any_element()
        } else {
            div()
                .text_size(u(11.5))
                .text_color(colors.text_dim)
                .child(format!("Alertmanager {source_label}"))
                .into_any_element()
        };
        v_flex()
            .child(self.header(title, cx))
            .child(
                v_flex()
                    .id("silence-editor-body")
                    .max_h(u(560.0))
                    .overflow_y_scroll()
                    .p(u(16.0))
                    .gap(u(10.0))
                    .text_size(u(12.5))
                    .children(intro)
                    .children(fixed_rows)
                    .children(rows)
                    .child(
                        h_flex()
                            .id("add-matcher")
                            .gap(u(5.0))
                            .cursor_pointer()
                            .text_color(colors.accent)
                            .child(Icon::new(IconName::Plus).size(12.0).color(colors.accent))
                            .child("Add matcher")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.push_row(
                                    true,
                                    Matcher::new("", MatchOp::Equal, ""),
                                    window,
                                    cx,
                                );
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .pt(u(6.0))
                            .text_color(colors.text_muted)
                            .child("Duration"),
                    )
                    .child(
                        h_flex()
                            .gap(u(8.0))
                            .child(durations)
                            .child(custom)
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_size(u(12.0))
                                    .text_color(colors.text_muted)
                                    .child(end_text),
                            ),
                    )
                    .child(
                        h_flex()
                            .pt(u(6.0))
                            .gap(u(4.0))
                            .child("Comment")
                            .child(div().text_color(colors.red).child("required")),
                    )
                    .child(
                        div()
                            .h(u(64.0))
                            .p(u(6.0))
                            .rounded(u(5.0))
                            .bg(colors.input_background)
                            .border_1()
                            .border_color(colors.accent)
                            .child(Textarea::new(&self.comment).appearance(false).h_full()),
                    )
                    .child(
                        h_flex()
                            .gap(u(10.0))
                            .child(
                                div()
                                    .w(u(80.0))
                                    .text_color(colors.text_muted)
                                    .child("Created by"),
                            )
                            .child(
                                h_flex()
                                    .flex_1()
                                    .h(u(28.0))
                                    .px(u(8.0))
                                    .rounded(u(5.0))
                                    .bg(colors.input_background)
                                    .border_1()
                                    .border_color(colors.border)
                                    .child(
                                        div()
                                            .flex_1()
                                            .child(Input::new(&self.created_by).appearance(false)),
                                    )
                                    .child(
                                        div()
                                            .text_size(u(11.0))
                                            .text_color(colors.text_dim)
                                            .child(self.created_by_note),
                                    ),
                            ),
                    )
                    .child(preview)
                    .when_some(self.error.clone(), |this, error| {
                        this.child(div().text_color(colors.red).child(error))
                    }),
            )
            .child(
                h_flex()
                    .gap(u(8.0))
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(source_picker)
                    .child(div().flex_1())
                    .child(
                        Button::new("silence-cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("silence-review")
                            .primary()
                            .label("Review…")
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_row(
        &self,
        index: usize,
        row: &MatcherRow,
        names: Vec<String>,
        values: Vec<String>,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let field = |input: &Entity<InputState>,
                     id: SharedString,
                     choices: Vec<String>,
                     cx: &mut Context<Self>| {
            let target = input.clone();
            let picker = (!choices.is_empty()).then(|| {
                MenuButton::new(id)
                    .ghost()
                    .compact()
                    .child(Icon::new(IconName::ChevronDown).size(11.0))
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.max_h(px(300.0)).scrollable(true);
                        for choice in &choices {
                            let target = target.clone();
                            let value = choice.clone();
                            menu = menu.item(PopupMenuItem::new(choice.clone()).on_click(
                                move |_, window, cx| {
                                    let value = value.clone();
                                    target
                                        .update(cx, |input, cx| input.set_value(value, window, cx));
                                },
                            ));
                        }
                        menu
                    })
            });
            let _ = cx;
            h_flex()
                .flex_1()
                .min_w_0()
                .h(u(28.0))
                .pl(u(8.0))
                .rounded(u(5.0))
                .bg(colors.input_background)
                .border_1()
                .border_color(colors.border)
                .font_family(fonts::MONO)
                .text_size(u(12.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(input).appearance(false)),
                )
                .children(picker)
        };
        let op = row.op;
        let weak = cx.entity().downgrade();
        let op_menu = MenuButton::new(("matcher-op", index))
            .ghost()
            .compact()
            .child(
                h_flex()
                    .w(u(44.0))
                    .gap(u(4.0))
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .child(op.symbol())
                    .child(div().flex_1())
                    .child(Icon::new(IconName::ChevronDown).size(11.0)),
            )
            .dropdown_menu(move |mut menu, _, _| {
                for choice in MatchOp::ALL {
                    let weak = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(choice.symbol())
                            .checked(choice == op)
                            .on_click(move |_, _, cx| {
                                weak.update(cx, |this, cx| {
                                    if let Some(row) = this.rows.get_mut(index) {
                                        row.op = choice;
                                    }
                                    cx.notify();
                                })
                                .ok();
                            }),
                    );
                }
                menu
            });
        let name_field = field(
            &row.name,
            SharedString::from(format!("matcher-names-{index}")),
            names,
            cx,
        );
        let value_field = field(
            &row.value,
            SharedString::from(format!("matcher-values-{index}")),
            values,
            cx,
        );
        h_flex()
            .gap(u(8.0))
            .child(
                div()
                    .id(("matcher-on", index))
                    .cursor_pointer()
                    .child(check_box(row.on, colors))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(row) = this.rows.get_mut(index) {
                            row.on = !row.on;
                        }
                        cx.notify();
                    })),
            )
            .child(div().w(u(150.0)).child(name_field))
            .child(op_menu)
            .child(value_field)
            .child(
                kubyl_ui::IconButton::new(("matcher-remove", index), IconName::X)
                    .icon_size(11.0)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if index < this.rows.len() {
                            this.rows.remove(index);
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_preview(
        &self,
        impact: &Impact,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = impact.matched.len();
        let mut parts: Vec<AnyElement> = vec![
            div()
                .child(format!(
                    "Matches {count} alert{} now",
                    if count == 1 { "" } else { "s" }
                ))
                .font_weight(FontWeight::MEDIUM)
                .into_any_element(),
        ];
        let mut detail = Vec::new();
        if impact.critical > 0 {
            detail.push(
                div()
                    .text_color(colors.red)
                    .child(format!("{} critical", impact.critical))
                    .into_any_element(),
            );
        }
        if impact.warning > 0 {
            detail.push(
                div()
                    .text_color(colors.yellow)
                    .child(format!("{} warning", impact.warning))
                    .into_any_element(),
            );
        }
        if impact.other > 0 {
            detail.push(
                div()
                    .text_color(colors.text_muted)
                    .child(format!("{} other", impact.other))
                    .into_any_element(),
            );
        }
        if !detail.is_empty() {
            parts.push(div().child(":").into_any_element());
            for (i, d) in detail.into_iter().enumerate() {
                if i > 0 {
                    parts.push(div().child(",").into_any_element());
                }
                parts.push(d);
            }
        }
        let show = self.show_matches;
        v_flex()
            .p(u(10.0))
            .gap(u(4.0))
            .rounded(u(6.0))
            .border_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .gap(u(4.0))
                    .child(Icon::new(IconName::Eye).size(13.0).color(colors.accent))
                    .children(parts)
                    .child(div().flex_1())
                    .when(count > 0, |this| {
                        this.child(
                            div()
                                .id("preview-show")
                                .cursor_pointer()
                                .text_color(colors.accent)
                                .child(if show { "Hide" } else { "Show" })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_matches = !this.show_matches;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .when(show, |this| {
                this.children(match_lines(&impact.matched, 8, colors))
            })
            .into_any_element()
    }

    fn render_review(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(draft) = self.draft.clone() else {
            return div().into_any_element();
        };
        let name = cluster_name(&self.cluster, cx);
        let title = match (&draft.id, &self.editing) {
            (Some(_), _) | (_, Some(_)) => format!("Update silence on {name}"),
            _ => format!("Create silence on {name}"),
        };
        let impact = self.impact(cx);
        let prod = production(&self.cluster, cx);
        let typed = self.needs_typed(cx);
        let now = Timestamp::now();
        let length =
            widgets::short_duration(draft.ends_at.duration_since(draft.starts_at).as_secs());
        let until = widgets::local_and_utc(draft.ends_at);
        let row = |label: &'static str, value: AnyElement| {
            h_flex()
                .items_start()
                .gap(u(12.0))
                .child(
                    div()
                        .flex_none()
                        .w(u(94.0))
                        .text_color(colors.text_muted)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(value))
        };
        let matcher_chips = h_flex()
            .flex_wrap()
            .gap(u(5.0))
            .children(draft.matchers.iter().map(|m| {
                div()
                    .px(u(6.0))
                    .py(u(1.0))
                    .rounded(u(4.0))
                    .bg(colors.chip_background)
                    .font_family(fonts::MONO)
                    .text_size(u(11.5))
                    .child(format!("{} {} {}", m.name, m.op.symbol(), m.value))
            }))
            .into_any_element();
        let count = impact.matched.len();
        let silences = v_flex()
            .gap(u(3.0))
            .child(div().font_weight(FontWeight::MEDIUM).child(format!(
                "{count} alert{} firing now",
                if count == 1 { "" } else { "s" }
            )))
            .children(match_lines(&impact.matched, 6, &colors))
            .into_any_element();
        let warning = (prod && impact.critical > 0).then(|| {
            h_flex()
                .items_start()
                .gap(u(10.0))
                .p(u(12.0))
                .rounded(u(6.0))
                .border_1()
                .border_color(colors.red.opacity(0.6))
                .bg(colors.red.opacity(0.08))
                .child(Icon::new(IconName::TriangleAlert).size(14.0).color(colors.red))
                .child(
                    div()
                        .whitespace_normal()
                        .child(format!(
                            "This silences {} on a production cluster. Nobody is paged for {} until the silence ends or is expired.",
                            if impact.critical == 1 { "a critical alert".to_string() } else { format!("{} critical alerts", impact.critical) },
                            if impact.critical == 1 { "it" } else { "them" },
                        )),
                )
        });
        let many = (prod && !typed && count > MANY).then(|| {
            div()
                .text_color(colors.yellow)
                .child(format!("This silence matches {count} alerts."))
        });
        let sa = self.service_account(cx);
        let consent = sa.clone().map(|sa| {
            let on = self.consent;
            h_flex()
                .id("sa-consent")
                .items_start()
                .gap(u(8.0))
                .cursor_pointer()
                .child(check_box(on, &colors))
                .child(div().whitespace_normal().child(format!(
                    "You sign in with a client certificate, so this silence is created as service account {sa}. \"Created by\" stays {}.",
                    draft.created_by
                )))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.consent = !this.consent;
                    cx.notify();
                }))
        });
        let typed_field = typed.then(|| {
            v_flex()
                .gap(u(6.0))
                .child(
                    h_flex()
                        .gap(u(4.0))
                        .child("Type")
                        .child(
                            div()
                                .font_family(fonts::MONO)
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(name.clone()),
                        )
                        .child("to confirm."),
                )
                .child(
                    div()
                        .h(u(30.0))
                        .px(u(8.0))
                        .flex()
                        .items_center()
                        .rounded(u(5.0))
                        .bg(colors.input_background)
                        .border_1()
                        .border_color(colors.accent)
                        .font_family(fonts::MONO)
                        .child(Input::new(&self.typed).appearance(false)),
                )
        });
        let ready = self.ready(cx);
        let busy = self.busy;
        let review_only = self.review_only;
        let _ = now;
        v_flex()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "enter" && !this.needs_typed(cx) {
                    this.submit(window, cx);
                }
            }))
            .child(self.header(title, cx))
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(10.0))
                    .text_size(u(12.5))
                    .child(row("Matchers", matcher_chips))
                    .child(row(
                        "Duration",
                        div()
                            .child(format!("{length} · until {until}"))
                            .into_any_element(),
                    ))
                    .child(row(
                        "Comment",
                        div()
                            .whitespace_normal()
                            .child(draft.comment.clone())
                            .into_any_element(),
                    ))
                    .child(row(
                        "Created by",
                        div().child(draft.created_by.clone()).into_any_element(),
                    ))
                    .child(row("Silences", silences))
                    .children(warning)
                    .children(many)
                    .children(consent)
                    .children(typed_field)
                    .when_some(self.error.clone(), |this, error| {
                        this.child(div().text_color(colors.red).child(error))
                    }),
            )
            .child(
                h_flex()
                    .gap(u(8.0))
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(div().flex_1())
                    .child(
                        Button::new("silence-back")
                            .ghost()
                            .label(if review_only { "Cancel" } else { "Back" })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if review_only {
                                    window.close_dialog(cx);
                                } else {
                                    this.step = Step::Edit;
                                    this.error = None;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        Button::new("silence-create")
                            .primary()
                            .icon(IconName::BellOff)
                            .label(if busy {
                                "Saving…"
                            } else if draft.id.is_some() {
                                "Update silence"
                            } else {
                                "Create silence"
                            })
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                    ),
            )
            .into_any_element()
    }
}

/// `● KubePodCrashLooping  pod/payment-gateway-…` lines, then "and N more".
fn match_lines(alerts: &[Alert], max: usize, colors: &Colors) -> Vec<AnyElement> {
    let mut lines: Vec<AnyElement> = alerts
        .iter()
        .take(max)
        .map(|a| {
            h_flex()
                .gap(u(7.0))
                .text_size(u(12.0))
                .child(kubyl_ui::StatusDot::new(severity_color(
                    &a.severity,
                    colors,
                )))
                .child(div().font_family(fonts::MONO).child(a.name.clone()))
                .child(
                    div()
                        .truncate()
                        .text_color(colors.text_muted)
                        .font_family(fonts::MONO)
                        .child(a.target.as_ref().map(|t| t.label()).unwrap_or_default()),
                )
                .into_any_element()
        })
        .collect();
    if alerts.len() > max {
        lines.push(
            div()
                .text_size(u(12.0))
                .text_color(colors.text_dim)
                .child(format!("and {} more", alerts.len() - max))
                .into_any_element(),
        );
    }
    lines
}

fn check_box(checked: bool, colors: &Colors) -> impl IntoElement {
    div()
        .flex_none()
        .mt(u(1.0))
        .size(u(15.0))
        .rounded(u(3.0))
        .flex()
        .items_center()
        .justify_center()
        .map(|this| {
            if checked {
                this.bg(colors.accent).child(
                    Icon::new(IconName::Check)
                        .size(11.0)
                        .color(colors.on_accent),
                )
            } else {
                this.border_1().border_color(colors.text_faint)
            }
        })
}

/// A custom end: a duration (`3h30m`) from now, or a local date and time.
fn parse_end(text: &str) -> Option<Timestamp> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let now = Timestamp::now();
    if let Some(duration) = crate::settings::parse_duration(text) {
        return now
            .checked_add(jiff::SignedDuration::from_secs(duration.as_secs() as i64))
            .ok();
    }
    let civil: jiff::civil::DateTime = text.replace('T', " ").parse().ok()?;
    let zoned = civil.to_zoned(jiff::tz::TimeZone::system()).ok()?;
    Some(zoned.timestamp()).filter(|t| *t > now)
}

impl Focusable for SilenceDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SilenceDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let body = match self.step {
            Step::Edit => self.render_edit(cx),
            Step::Review => self.render_review(cx),
        };
        div()
            .text_color(colors.text)
            .text_size(u(13.0))
            .font_family(fonts::UI)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(name: &str, severity: Severity) -> Alert {
        let mut labels = std::collections::BTreeMap::new();
        labels.insert("alertname".to_string(), name.to_string());
        labels.insert("namespace".to_string(), "payments".to_string());
        Alert {
            fingerprint: name.into(),
            name: name.into(),
            labels,
            annotations: Default::default(),
            severity,
            state: AlertState::Firing,
            silenced_by: Vec::new(),
            inhibited_by: Vec::new(),
            starts_at: None,
            starts_approx: false,
            active_at: None,
            updated_at: None,
            ends_at: None,
            receivers: Vec::new(),
            generator_url: None,
            value: None,
            source: String::new(),
            target: None,
        }
    }

    #[test]
    fn typed_name_on_prod_for_critical_many_or_no_alertname() {
        let alerts = vec![
            alert("KubePodCrashLooping", Severity::Critical),
            alert("KubeJobFailed", Severity::Warning),
        ];
        let by_name = [Matcher::new("alertname", MatchOp::Equal, "KubeJobFailed")];
        let warning_only = impact(&by_name, &alerts);
        assert_eq!(warning_only.warning, 1);
        assert!(!needs_typed_name(true, &by_name, &warning_only));
        assert!(!needs_typed_name(false, &by_name, &warning_only));
        let namespace = [Matcher::new("namespace", MatchOp::Equal, "payments")];
        let both = impact(&namespace, &alerts);
        assert_eq!(both.critical, 1);
        assert!(
            needs_typed_name(true, &namespace, &both),
            "critical, and no alertname"
        );
        let many: Vec<Alert> = (0..11)
            .map(|i| alert(&format!("A{i}"), Severity::Info))
            .collect();
        let regex = [Matcher::new("alertname", MatchOp::Regex, "A.*")];
        assert!(needs_typed_name(true, &regex, &impact(&regex, &many)));
    }

    #[test]
    fn custom_ends() {
        assert!(parse_end("2h").is_some());
        assert!(parse_end("2099-01-01 10:00").is_some());
        assert!(parse_end("2001-01-01 10:00").is_none(), "in the past");
        assert!(parse_end("soon").is_none());
    }
}
