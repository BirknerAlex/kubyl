//! Dialogs: Sync (board 15), Rollback (board 14), Delete, and the API-mode sign-in.
//!
//! Rollback and delete ask for the application's name on production clusters (delete always:
//! a cascading delete removes every resource of the app). Nothing opens on read-only clusters.

use std::collections::BTreeSet;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight,
    IntoElement, Render, SharedString, Subscription, Task, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::{ClusterId, Notification, NotificationCenter, ResourceRef};
use kubyl_kube::ConnectionManager;
use kubyl_resources::{ResourceStores, StoreHandle, StoreKey};
use kubyl_ui::{
    ActiveColors, Button, Chip, Colors, Icon, IconName, ProdBadge, fonts, h_flex, u, v_flex,
};
use secrecy::SecretString;

use crate::detect::Install;
use crate::links;
use crate::model::{Application, HistoryEntry, Project, ResourceKey, SyncStatus, short_revision};
use crate::ops::{self, Cascade, SyncRequest, SyncResource};
use crate::run::{self, Op};
use crate::settings;
use crate::state::{self, ApiState, ArgoCd, Credentials};
use crate::widgets;
use crate::windows::{self, Verdict};

fn open<V: Render>(
    view: Entity<V>,
    width: f32,
    focus: Option<FocusHandle>,
    window: &mut Window,
    cx: &mut App,
) {
    let colors = cx.colors().clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(width))
            .margin_top(px(80.0))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            // Enter in an input would close the dialog first; the views submit themselves.
            .on_ok(|_, _, _| false)
            // As content, not a child, so presses on the footer reach its buttons.
            .content({
                let view = view.clone();
                move |content, _, _| content.child(view.clone())
            })
    });
    // After opening: the dialog takes focus when it opens.
    if let Some(focus) = focus {
        window.focus(&focus, cx);
    }
}

fn header(
    icon: IconName,
    title: impl Into<SharedString>,
    extra: Option<AnyElement>,
    colors: &Colors,
) -> impl IntoElement {
    h_flex()
        .gap(u(10.0))
        .px(u(16.0))
        .py(u(14.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(Icon::new(icon).size(16.0).color(colors.accent))
        .child(
            div()
                .flex_1()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .children(extra)
}

fn footer(note: Option<AnyElement>, buttons: Vec<AnyElement>, colors: &Colors) -> impl IntoElement {
    h_flex()
        .gap(u(8.0))
        .px(u(16.0))
        .py(u(12.0))
        .border_t_1()
        .border_color(colors.border_variant)
        .children(note)
        .child(div().flex_1())
        .children(buttons)
}

fn cancel_button() -> AnyElement {
    Button::new("argo-dialog-cancel")
        .ghost()
        .label("Cancel")
        .on_click(|_, window, cx| window.close_dialog(cx))
        .into_any_element()
}

fn error_line(error: &Option<String>, colors: &Colors) -> Option<AnyElement> {
    error.as_ref().map(|e| {
        div()
            .text_size(u(12.0))
            .text_color(colors.red)
            .child(e.clone())
            .into_any_element()
    })
}

/// "Kubernetes mode: …" / "API mode · as alice".
fn mode_note(cluster: &ClusterId, kubernetes: &'static str, cx: &App) -> AnyElement {
    let colors = cx.colors().clone();
    let state = ArgoCd::try_global(cx)
        .map(|a| a.read(cx).api_state(cluster))
        .unwrap_or_default();
    let (icon, text) = match state {
        ApiState::Connected { user, .. } => (IconName::Link, format!("API mode · as {user}")),
        _ => (IconName::ShipWheel, kubernetes.to_string()),
    };
    h_flex()
        .gap(u(6.0))
        .min_w_0()
        .text_size(u(11.5))
        .text_color(colors.text_dim)
        .child(Icon::new(icon).size(12.0))
        .child(div().truncate().child(text))
        .into_any_element()
}

fn production(cluster: &ClusterId, cx: &App) -> bool {
    ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(cluster).production)
}

/// A labelled text input.
fn field(
    label: &'static str,
    input: &Entity<InputState>,
    mono: bool,
    colors: &Colors,
) -> impl IntoElement {
    v_flex()
        .gap(u(4.0))
        .child(
            div()
                .text_size(u(12.0))
                .text_color(colors.text_dim)
                .child(label),
        )
        .child(
            div()
                .h(u(28.0))
                .px(u(8.0))
                .flex()
                .items_center()
                .rounded(u(5.0))
                .bg(colors.input_background)
                .border_1()
                .border_color(colors.border)
                .when(mono, |this| {
                    this.font_family(fonts::MONO).text_size(u(12.5))
                })
                .child(Input::new(input).appearance(false)),
        )
}

/// The app's object from the watches, else `None` (the dialogs need it loaded).
fn load_app(target: &ResourceRef, cx: &App) -> Option<(Arc<serde_json::Value>, Application)> {
    let app = run::app_target(target)?;
    let object = crate::apps::find_object(&target.cluster, &target.gvr, &app, cx)?;
    let parsed = Application::parse(&object)?;
    Some((object, parsed))
}

fn guard(target: &ResourceRef, cx: &mut App) -> Option<Application> {
    if run::read_only(&target.cluster, cx) {
        NotificationCenter::push(
            cx,
            Notification::error("This cluster is read-only in Kubyl."),
        );
        return None;
    }
    match load_app(target, cx) {
        Some((_, app)) => Some(app),
        None => {
            NotificationCenter::push(cx, Notification::error("The application isn't loaded yet."));
            None
        }
    }
}

// ----- Sync -----

/// Opens the Sync dialog for an app.
pub fn open_sync(target: ResourceRef, window: &mut Window, cx: &mut App) {
    let Some(app) = guard(&target, cx) else {
        return;
    };
    let view = cx.new(|cx| SyncDialog::new(target, app, window, cx));
    let focus = view.read(cx).focus.clone();
    open(view, 580.0, Some(focus), window, cx);
}

struct SyncDialog {
    target: ResourceRef,
    app: Application,
    revisions: Vec<Entity<InputState>>,
    prune: bool,
    dry_run: bool,
    apply_only: bool,
    force: bool,
    replace: bool,
    server_side: bool,
    selective: bool,
    all_resources: bool,
    selected: BTreeSet<ResourceKey>,
    project: Option<StoreHandle>,
    error: Option<String>,
    busy: bool,
    focus: FocusHandle,
    _task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl SyncDialog {
    fn new(
        target: ResourceRef,
        app: Application,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let revisions: Vec<Entity<InputState>> = app
            .spec
            .all_sources()
            .iter()
            .map(|source| {
                let value = source.target().to_string();
                cx.new(|cx| {
                    let mut state = InputState::new(window, cx);
                    state.set_value(value, window, cx);
                    state
                })
            })
            .collect();
        let subscriptions = revisions
            .iter()
            .map(|input| {
                cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.submit(window, cx);
                    }
                })
            })
            .collect();
        let options = app.policy().sync_options;
        let selected = app.out_of_sync().iter().map(|r| r.key()).collect();
        // The project, for its sync windows.
        let project = crate::state::resource(&target.cluster, "appprojects", cx).map(|(gvr, _)| {
            let namespace = app
                .status
                .controller_namespace
                .clone()
                .unwrap_or_else(|| app.namespace().to_string());
            let handle = ResourceStores::acquire(
                cx,
                StoreKey::new(target.cluster.clone(), gvr, Some(namespace))
                    .fields(format!("metadata.name={}", app.spec.project)),
            );
            cx.observe(handle.entity(), |_, _, cx| cx.notify()).detach();
            handle
        });
        Self {
            target,
            prune: false,
            dry_run: false,
            apply_only: false,
            force: false,
            replace: ops::has_option(&options, "Replace"),
            server_side: ops::has_option(&options, "ServerSideApply"),
            selective: false,
            all_resources: false,
            selected,
            project,
            app,
            revisions,
            error: None,
            busy: false,
            focus: cx.focus_handle(),
            _task: None,
            _subscriptions: subscriptions,
        }
    }

    fn window_verdict(&self, cx: &App) -> Option<(Verdict, String)> {
        let store = self.project.as_ref()?.read(cx);
        let object = store.objects().values().next()?;
        let project = Project::parse(object)?;
        let verdict = windows::verdict(
            &project.spec.sync_windows,
            &self.app,
            jiff::Timestamp::now(),
        );
        (verdict != Verdict::Allowed).then_some((verdict, project.metadata.name))
    }

    fn request(&self, cx: &App) -> SyncRequest {
        let sources = self.app.spec.all_sources();
        let revisions = self
            .revisions
            .iter()
            .zip(&sources)
            .map(|(input, source)| {
                let value = input.read(cx).value().trim().to_string();
                (!value.is_empty() && value != source.target()).then_some(value)
            })
            .collect();
        let mut options = self.app.policy().sync_options;
        ops::set_option(&mut options, "Replace", self.replace);
        ops::set_option(&mut options, "ServerSideApply", self.server_side);
        let resources = if self.selective {
            self.app
                .status
                .resources
                .iter()
                .filter(|r| self.selected.contains(&r.key()))
                .map(|r| SyncResource {
                    group: r.group.clone(),
                    kind: r.kind.clone(),
                    namespace: r.namespace.clone(),
                    name: r.name.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        SyncRequest {
            revisions,
            prune: self.prune,
            dry_run: self.dry_run,
            apply_only: self.apply_only,
            force: self.force,
            sync_options: Some(options),
            resources,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self.selective && self.selected.is_empty() {
            self.error = Some("Select at least one resource.".into());
            cx.notify();
            return;
        }
        let request = self.request(cx);
        self.busy = true;
        self.error = None;
        let task = run::run(self.target.clone(), Op::Sync(request), cx);
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(()) => window.close_dialog(cx),
                    Err(err) => this.error = Some(err),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Focusable for SyncDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SyncDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let app = self.app.clone();
        let sources = app.spec.all_sources();
        let mut revisions =
            v_flex()
                .gap(u(6.0))
                .child(div().text_size(u(12.0)).text_color(colors.text_dim).child(
                    if sources.len() > 1 {
                        "Revisions"
                    } else {
                        "Revision"
                    },
                ));
        for (source, input) in sources.iter().zip(&self.revisions) {
            revisions = revisions.child(
                h_flex()
                    .gap(u(8.0))
                    .text_size(u(12.0))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(u(6.0))
                            .child(
                                Icon::new(IconName::GitBranch)
                                    .size(12.0)
                                    .color(colors.text_dim),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .font_family(fonts::MONO)
                                    .text_size(u(11.5))
                                    .child(format!("{} · {}", source.repo_short(), source.what())),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(u(170.0))
                            .h(u(26.0))
                            .px(u(8.0))
                            .flex()
                            .items_center()
                            .rounded(u(5.0))
                            .bg(colors.input_background)
                            .border_1()
                            .border_color(colors.border)
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .child(Input::new(input).appearance(false)),
                    ),
            );
        }
        if app.policy().auto_sync() {
            revisions = revisions
                .child(div().text_size(u(11.5)).text_color(colors.text_dim).child(
                "Auto-sync is on: Argo CD only accepts the target revision (except for dry runs).",
            ));
        }
        let check = |id: &'static str,
                     on: bool,
                     label: &'static str,
                     detail: Option<&'static str>,
                     set: fn(&mut Self),
                     cx: &mut Context<Self>|
         -> AnyElement {
            widgets::checkbox(
                id,
                on,
                label,
                detail.map(SharedString::from),
                true,
                &colors,
                cx.listener(move |this, _, _, cx| {
                    set(this);
                    cx.notify();
                }),
            )
            .into_any_element()
        };
        let options = div()
            .grid()
            .grid_cols(2)
            .gap(u(10.0))
            .child(check(
                "sync-prune",
                self.prune,
                "Prune",
                Some("delete resources no longer in Git"),
                |t| t.prune = !t.prune,
                cx,
            ))
            .child(check(
                "sync-dry",
                self.dry_run,
                "Dry run",
                None,
                |t| t.dry_run = !t.dry_run,
                cx,
            ))
            .child(check(
                "sync-apply",
                self.apply_only,
                "Apply only",
                Some("skip hooks (kubectl apply)"),
                |t| t.apply_only = !t.apply_only,
                cx,
            ))
            .child(check(
                "sync-force",
                self.force,
                "Force",
                Some("delete and re-create when apply fails"),
                |t| t.force = !t.force,
                cx,
            ))
            .child(check(
                "sync-replace",
                self.replace,
                "Replace",
                Some("kubectl replace/create"),
                |t| t.replace = !t.replace,
                cx,
            ))
            .child(check(
                "sync-ssa",
                self.server_side,
                "Server-side apply",
                None,
                |t| t.server_side = !t.server_side,
                cx,
            ));
        let own_options: Vec<String> = app
            .policy()
            .sync_options
            .into_iter()
            .filter(|o| !o.starts_with("Replace=") && !o.starts_with("ServerSideApply="))
            .collect();

        // Selective sync.
        let resources: Vec<_> = app
            .status
            .resources
            .iter()
            .filter(|r| self.all_resources || r.sync() == SyncStatus::OutOfSync)
            .cloned()
            .collect();
        let total = app.status.resources.len();
        let mut list = v_flex()
            .id("sync-resources")
            .max_h(u(200.0))
            .overflow_y_scroll();
        for (index, resource) in resources.iter().enumerate() {
            let key = resource.key();
            let on = self.selected.contains(&key);
            list = list.child(
                h_flex()
                    .id(("sync-resource", index))
                    .gap(u(8.0))
                    .h(u(26.0))
                    .px(u(10.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .text_size(u(12.0))
                    .when(self.selective, |this| {
                        this.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.selected.remove(&key) {
                                    this.selected.insert(key.clone());
                                }
                                cx.notify();
                            }))
                    })
                    .when(!self.selective, |this| this.opacity(0.55))
                    .child(widgets::check_box(on || !self.selective, &colors))
                    .child(
                        div()
                            .flex_none()
                            .w(u(108.0))
                            .truncate()
                            .text_color(colors.text_dim)
                            .child(resource.kind.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .child(resource.name.clone()),
                    )
                    .child(widgets::sync_pill(resource.sync(), &colors)),
            );
        }
        if resources.is_empty() {
            list = list.child(
                div()
                    .px(u(10.0))
                    .py(u(6.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("Every resource is in sync."),
            );
        }
        let selective_header = h_flex()
            .gap(u(8.0))
            .h(u(30.0))
            .px(u(10.0))
            .text_size(u(12.0))
            .child(widgets::checkbox(
                "sync-selective",
                self.selective,
                "Selective sync",
                None,
                true,
                &colors,
                cx.listener(|this, _, _, cx| {
                    this.selective = !this.selective;
                    cx.notify();
                }),
            ))
            .child(div().text_color(colors.text_dim).child(if self.selective {
                format!("· {} of {total} selected", self.selected.len())
            } else {
                format!("· all {total} resources")
            }))
            .child(div().flex_1())
            .child(
                div()
                    .id("sync-oos")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.all_resources = false;
                        cx.notify();
                    }))
                    .child(Chip::new("out of sync").selected(!self.all_resources)),
            )
            .child(
                div()
                    .id("sync-all")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.all_resources = true;
                        cx.notify();
                    }))
                    .child(Chip::new("all").selected(self.all_resources)),
            );
        let window_warning = self.window_verdict(cx).map(|(verdict, project)| {
            let text = match verdict {
                Verdict::Blocked => format!(
                    "A sync window of project {project} blocks syncing now; Argo CD will refuse."
                ),
                _ => format!(
                    "A sync window of project {project} is active: only manual syncs are allowed."
                ),
            };
            widgets::warning_box(&colors)
                .child(Icon::new(IconName::Clock).size(14.0).color(colors.yellow))
                .child(div().flex_1().text_size(u(12.0)).child(text))
        });
        let submit = Button::new("sync-submit")
            .primary()
            .icon(IconName::RefreshCw)
            .label(if self.busy {
                "Starting…"
            } else if self.dry_run {
                "Dry run"
            } else {
                "Synchronize"
            })
            .disabled(self.busy)
            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)));
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(
                IconName::RefreshCw,
                format!("Sync {}", app.name()),
                Some(widgets::sync_pill(app.sync(), &colors).into_any_element()),
                &colors,
            ))
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(14.0))
                    .child(revisions)
                    .child(options)
                    .when(!own_options.is_empty(), |this| {
                        this.child(
                            h_flex()
                                .gap(u(6.0))
                                .flex_wrap()
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child("From the app:")
                                .children(own_options.into_iter().map(|o| Chip::new(o).mono())),
                        )
                    })
                    .children(window_warning)
                    .child(
                        v_flex()
                            .rounded(u(8.0))
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.background)
                            .overflow_hidden()
                            .child(selective_header)
                            .child(list),
                    )
                    .children(error_line(&self.error, &colors)),
            )
            .child(footer(
                Some(mode_note(
                    &self.target.cluster,
                    "Kubernetes mode: writes the Application's operation, like argocd app sync --core",
                    cx,
                )),
                vec![cancel_button(), submit.into_any_element()],
                &colors,
            ))
    }
}

// ----- Rollback -----

/// Opens the Rollback dialog for history entry `id` (the previous deployment when `None`).
pub fn open_rollback(target: ResourceRef, id: Option<i64>, window: &mut Window, cx: &mut App) {
    let Some(app) = guard(&target, cx) else {
        return;
    };
    let current = app.current_history_id();
    let entry = match id {
        Some(id) => app.status.history.iter().find(|h| h.id == id).cloned(),
        None => app
            .history_newest_first()
            .into_iter()
            .find(|h| Some(h.id) != current),
    };
    let Some(entry) = entry else {
        NotificationCenter::push(
            cx,
            Notification::error(format!(
                "{} has no earlier deployment to roll back to.",
                app.name()
            )),
        );
        return;
    };
    let view = cx.new(|cx| RollbackDialog::new(target, app, entry, window, cx));
    let focus = {
        let dialog = view.read(cx);
        if dialog.production {
            dialog.typed.read(cx).focus_handle(cx)
        } else {
            dialog.focus.clone()
        }
    };
    open(view, 560.0, Some(focus), window, cx);
}

struct RollbackDialog {
    target: ResourceRef,
    app: Application,
    entry: HistoryEntry,
    disable_auto_sync: bool,
    prune: bool,
    dry_run: bool,
    typed: Entity<InputState>,
    production: bool,
    error: Option<String>,
    busy: bool,
    focus: FocusHandle,
    _task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl RollbackDialog {
    fn new(
        target: ResourceRef,
        app: Application,
        entry: HistoryEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let typed = cx.new(|cx| InputState::new(window, cx).placeholder(app.name().to_string()));
        let subscriptions =
            vec![
                cx.subscribe_in(&typed, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::PressEnter { .. } => this.submit(window, cx),
                        InputEvent::Change => cx.notify(),
                        _ => {}
                    }
                }),
            ];
        let production = production(&target.cluster, cx);
        Self {
            target,
            app,
            entry,
            disable_auto_sync: true,
            prune: false,
            dry_run: false,
            typed,
            production,
            error: None,
            busy: false,
            focus: cx.focus_handle(),
            _task: None,
            _subscriptions: subscriptions,
        }
    }

    fn ready(&self, cx: &App) -> bool {
        let typed_ok = !self.production || self.typed.read(cx).value().trim() == self.app.name();
        let auto_ok = !self.app.policy().auto_sync() || self.disable_auto_sync || self.dry_run;
        typed_ok && auto_ok && self.entry.can_roll_back() && !self.busy
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            if self.production && self.typed.read(cx).value().trim() != self.app.name() {
                self.error = Some("Type the application's name to confirm.".into());
                cx.notify();
            }
            return;
        }
        let op = Op::Rollback {
            id: self.entry.id,
            prune: self.prune,
            dry_run: self.dry_run,
            disable_auto_sync: self.app.policy().auto_sync()
                && self.disable_auto_sync
                && !self.dry_run,
        };
        self.busy = true;
        self.error = None;
        let task = run::run(self.target.clone(), op, cx);
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(()) => window.close_dialog(cx),
                    Err(err) => this.error = Some(err),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn pick(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(entry) = self.app.status.history.iter().find(|h| h.id == id) {
            self.entry = entry.clone();
            cx.notify();
        }
    }
}

impl Focusable for RollbackDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RollbackDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let app = self.app.clone();
        let entry = self.entry.clone();
        let current = app
            .current_history_id()
            .and_then(|id| app.status.history.iter().find(|h| h.id == id).cloned());
        let ago = |e: &HistoryEntry| widgets::age_of(e.deployed());
        let by = |e: &HistoryEntry| {
            e.initiated_by
                .clone()
                .map(|i| i.label())
                .unwrap_or_else(|| "unknown".into())
        };

        // The entry picker.
        let entries: Vec<HistoryEntry> = app
            .history_newest_first()
            .into_iter()
            .filter(|h| Some(h.id) != app.current_history_id())
            .collect();
        let weak = cx.entity().downgrade();
        let picker = MenuButton::new("rollback-entry")
            .ghost()
            .compact()
            .child(
                h_flex()
                    .gap(u(4.0))
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .child(format!("#{}", entry.id))
                    .child(Icon::new(IconName::ChevronDown).size(11.0)),
            )
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for e in &entries {
                    let weak = weak.clone();
                    let id = e.id;
                    let label = format!(
                        "#{} · {} · {} ago",
                        e.id,
                        e.all_revisions()
                            .first()
                            .map(|r| short_revision(r))
                            .unwrap_or_default(),
                        widgets::age_of(e.deployed())
                    );
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        weak.update(cx, |this, cx| this.pick(id, cx)).ok();
                    }));
                }
                menu
            });

        // Revision and source changes.
        let from = current
            .as_ref()
            .map(|c| c.all_revisions())
            .unwrap_or_default();
        let to = entry.all_revisions();
        let from_sources = current
            .as_ref()
            .map(|c| c.all_sources())
            .unwrap_or_default();
        let to_sources = entry.all_sources();
        let mut card = v_flex()
            .gap(u(6.0))
            .p(u(10.0))
            .rounded(u(8.0))
            .border_1()
            .border_color(colors.border)
            .bg(colors.background)
            .text_size(u(12.0));
        for (index, target_revision) in to.iter().enumerate() {
            let old = from.get(index).cloned().unwrap_or_default();
            let repo = to_sources
                .get(index)
                .map(|s| s.repo_url.clone())
                .unwrap_or_default();
            let compare = links::compare_url(&repo, &old, target_revision);
            let link = links::commit_url(&repo, target_revision);
            card = card.child(
                h_flex()
                    .gap(u(10.0))
                    .child(
                        div()
                            .flex_none()
                            .w(u(70.0))
                            .text_color(colors.text_dim)
                            .child(if to.len() > 1 {
                                format!("Revision {}", index + 1)
                            } else {
                                "Revision".into()
                            }),
                    )
                    .child(widgets::mono(if old.is_empty() {
                        "—".into()
                    } else {
                        short_revision(&old)
                    }))
                    .child(
                        Icon::new(IconName::ArrowRight)
                            .size(11.0)
                            .color(colors.text_dim),
                    )
                    .child(match link {
                        Some(url) => widgets::url_link(
                            ("rollback-rev", index),
                            short_revision(target_revision),
                            url,
                            &colors,
                        )
                        .into_any_element(),
                        None => widgets::mono(short_revision(target_revision)),
                    })
                    .child(div().flex_1())
                    .when_some(compare, |this, url| {
                        this.child(widgets::url_link(
                            ("rollback-compare", index),
                            "compare ↗",
                            url,
                            &colors,
                        ))
                    }),
            );
        }
        let source_changed = from_sources != to_sources;
        card = card.child(
            h_flex()
                .gap(u(10.0))
                .child(
                    div()
                        .flex_none()
                        .w(u(70.0))
                        .text_color(colors.text_dim)
                        .child("Source"),
                )
                .child(div().child(if source_changed {
                    "changed"
                } else {
                    "unchanged"
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(
                            to_sources
                                .iter()
                                .map(|s| {
                                    format!("{} · {} @ {}", s.repo_short(), s.what(), s.target())
                                })
                                .collect::<Vec<_>>()
                                .join(", "),
                        ),
                ),
        );
        if let Some(current) = &current {
            card = card.child(
                h_flex()
                    .gap(u(10.0))
                    .child(
                        div()
                            .flex_none()
                            .w(u(70.0))
                            .text_color(colors.text_dim)
                            .child("Current"),
                    )
                    .child(format!(
                        "#{} · deployed {} ago by {}",
                        current.id,
                        ago(current),
                        by(current)
                    )),
            );
        }

        let auto_on = app.policy().auto_sync();
        let auto_warning = auto_on.then(|| {
            let target = app.spec.all_sources().first().map(|s| s.target().to_string()).unwrap_or_default();
            widgets::warning_box(&colors)
                .child(Icon::new(IconName::TriangleAlert).size(15.0).color(colors.yellow))
                .child(
                    v_flex()
                        .flex_1()
                        .gap(u(8.0))
                        .child(
                            div()
                                .text_size(u(12.5))
                                .child(format!("Auto-sync is on. Argo CD would sync back to {target} right away, so rollback needs it off.")),
                        )
                        .child(widgets::checkbox(
                            "rollback-disable-auto",
                            self.disable_auto_sync,
                            "Turn off auto-sync first",
                            Some("Removes spec.syncPolicy.automated; turn it on again from the summary.".into()),
                            true,
                            &colors,
                            cx.listener(|this, _, _, cx| {
                                this.disable_auto_sync = !this.disable_auto_sync;
                                cx.notify();
                            }),
                        )),
                )
        });
        let options = h_flex()
            .gap(u(18.0))
            .child(widgets::checkbox(
                "rollback-prune",
                self.prune,
                "Prune",
                Some(
                    format!(
                        "delete resources that {} doesn't have",
                        to.first().map(|r| short_revision(r)).unwrap_or_default()
                    )
                    .into(),
                ),
                true,
                &colors,
                cx.listener(|this, _, _, cx| {
                    this.prune = !this.prune;
                    cx.notify();
                }),
            ))
            .child(widgets::checkbox(
                "rollback-dry",
                self.dry_run,
                "Dry run",
                None,
                true,
                &colors,
                cx.listener(|this, _, _, cx| {
                    this.dry_run = !this.dry_run;
                    cx.notify();
                }),
            ));
        let typed = self.production.then(|| {
            v_flex()
                .gap(u(6.0))
                .child(
                    h_flex()
                        .gap(u(6.0))
                        .text_size(u(12.0))
                        .text_color(colors.text_muted)
                        .child("This is a production cluster. Type")
                        .child(
                            div()
                                .font_family(fonts::MONO)
                                .text_color(colors.text)
                                .child(app.name().to_string()),
                        )
                        .child("to confirm."),
                )
                .child(field("Confirmation", &self.typed, true, &colors))
        });
        let blocked = !entry.can_roll_back();
        let submit = Button::new("rollback-submit")
            .danger()
            .icon(IconName::RotateCcw)
            .label(if self.busy {
                "Rolling back…".to_string()
            } else {
                format!("Roll back to #{}", entry.id)
            })
            .disabled(!self.ready(cx))
            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)));
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(
                IconName::RotateCcw,
                format!("Roll back {}", app.name()),
                self.production.then(|| ProdBadge.into_any_element()),
                &colors,
            ))
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(14.0))
                    .child(
                        h_flex()
                            .gap(u(6.0))
                            .text_size(u(12.5))
                            .child(div().text_color(colors.text_muted).child("History entry"))
                            .child(picker)
                            .child(div().text_color(colors.text_dim).child(format!(
                                "synced {} ago by {}",
                                ago(&entry),
                                by(&entry)
                            ))),
                    )
                    .child(div().text_size(u(12.5)).text_color(colors.text_muted).child(
                        "Rollback syncs the revision and source this entry deployed, like argocd app rollback.",
                    ))
                    .child(card)
                    .children(auto_warning)
                    .when(blocked, |this| {
                        this.child(div().text_size(u(12.0)).text_color(colors.red).child(
                            "This entry was deployed by Argo CD 0.11 or older and has no source: sync to its revision instead.",
                        ))
                    })
                    .child(options)
                    .children(typed)
                    .children(error_line(&self.error, &colors)),
            )
            .child(footer(
                Some(mode_note(
                    &self.target.cluster,
                    "Kubernetes mode: writes operation.sync with the entry's revision and source",
                    cx,
                )),
                vec![cancel_button(), submit.into_any_element()],
                &colors,
            ))
    }
}

// ----- Delete -----

/// Opens the Delete dialog: cascading (the default when the app has the resources finalizer)
/// or not, confirmed by typing the name.
pub fn open_delete(target: ResourceRef, window: &mut Window, cx: &mut App) {
    let Some(app) = guard(&target, cx) else {
        return;
    };
    let view = cx.new(|cx| DeleteDialog::new(target, app, window, cx));
    let focus = view.read(cx).typed.read(cx).focus_handle(cx);
    open(view, 540.0, Some(focus), window, cx);
}

struct DeleteDialog {
    target: ResourceRef,
    app: Application,
    cascade: Cascade,
    typed: Entity<InputState>,
    error: Option<String>,
    busy: bool,
    focus: FocusHandle,
    _task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl DeleteDialog {
    fn new(
        target: ResourceRef,
        app: Application,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let typed = cx.new(|cx| InputState::new(window, cx).placeholder(app.name().to_string()));
        let subscriptions =
            vec![
                cx.subscribe_in(&typed, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::PressEnter { .. } => this.submit(window, cx),
                        InputEvent::Change => cx.notify(),
                        _ => {}
                    }
                }),
            ];
        // Argo CD's default: cascade when the finalizer is set (the API's default too).
        let cascade = if app.cascades() || app.metadata.finalizers.is_empty() {
            Cascade::Foreground
        } else {
            Cascade::None
        };
        Self {
            target,
            app,
            cascade,
            typed,
            error: None,
            busy: false,
            focus: cx.focus_handle(),
            _task: None,
            _subscriptions: subscriptions,
        }
    }

    fn ready(&self, cx: &App) -> bool {
        self.typed.read(cx).value().trim() == self.app.name() && !self.busy
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready(cx) {
            self.error = Some("Type the application's name to confirm.".into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        let task = run::run(self.target.clone(), Op::Delete(self.cascade), cx);
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(()) => window.close_dialog(cx),
                    Err(err) => this.error = Some(err),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Focusable for DeleteDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DeleteDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let app = self.app.clone();
        let count = app.status.resources.len();
        let destination = app.spec.destination.label();
        let choice = |id: &'static str,
                      cascade: Cascade,
                      label: String,
                      detail: String,
                      cx: &mut Context<Self>| {
            let on = self.cascade == cascade;
            h_flex()
                .id(id)
                .items_start()
                .gap(u(10.0))
                .p(u(10.0))
                .rounded(u(7.0))
                .border_1()
                .border_color(if on { colors.accent } else { colors.border })
                .when(on, |this| this.bg(colors.selection))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.cascade = cascade;
                    cx.notify();
                }))
                .child(
                    div()
                        .flex_none()
                        .mt(u(2.0))
                        .size(u(14.0))
                        .rounded_full()
                        .border_1()
                        .border_color(if on { colors.accent } else { colors.text_faint })
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(on, |this| {
                            this.child(div().size(u(7.0)).rounded_full().bg(colors.accent))
                        }),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().text_size(u(12.5)).child(label))
                        .child(
                            div()
                                .text_size(u(11.5))
                                .text_color(colors.text_dim)
                                .child(detail),
                        ),
                )
        };
        let resources = format!("{count} resource{}", if count == 1 { "" } else { "s" });
        let submit = Button::new("delete-submit")
            .danger()
            .icon(IconName::Trash)
            .label(if self.busy { "Deleting…" } else { "Delete" })
            .disabled(!self.ready(cx))
            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)));
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(
                IconName::Trash,
                format!("Delete {}", app.name()),
                production(&self.target.cluster, cx).then(|| ProdBadge.into_any_element()),
                &colors,
            ))
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(10.0))
                    .child(choice(
                        "delete-cascade",
                        Cascade::Foreground,
                        format!("Delete the application and its {resources}"),
                        format!("Cascading: Argo CD deletes everything it deployed to {destination} first (the resources finalizer)."),
                        cx,
                    ))
                    .child(choice(
                        "delete-background",
                        Cascade::Background,
                        format!("Delete the application, its {resources} in the background"),
                        "Cascading, background propagation: the app goes first, its resources follow.".to_string(),
                        cx,
                    ))
                    .child(choice(
                        "delete-keep",
                        Cascade::None,
                        "Delete only the application, keep its resources".to_string(),
                        "Non-cascading: removes the resources finalizer; the workloads keep running, unmanaged.".to_string(),
                        cx,
                    ))
                    .child(
                        h_flex()
                            .gap(u(6.0))
                            .pt(u(4.0))
                            .text_size(u(12.0))
                            .text_color(colors.text_muted)
                            .child("Type")
                            .child(div().font_family(fonts::MONO).text_color(colors.text).child(app.name().to_string()))
                            .child("to confirm."),
                    )
                    .child(field("Confirmation", &self.typed, true, &colors))
                    .children(error_line(&self.error, &colors)),
            )
            .child(footer(
                Some(mode_note(&self.target.cluster, "Kubernetes mode: sets or removes the finalizer, then deletes", cx)),
                vec![cancel_button(), submit.into_any_element()],
                &colors,
            ))
    }
}

// ----- Sign in (API mode) -----

/// Opens the API-mode sign-in for a cluster: it shows which install (namespace, Service, URL)
/// the credentials go to; signing in confirms it.
pub fn open_sign_in(cluster: ClusterId, window: &mut Window, cx: &mut App) {
    let Some(argo) = ArgoCd::try_global(cx) else {
        return;
    };
    let installs = argo.read(cx).installs(&cluster);
    if installs.is_empty() {
        argo.update(cx, |argo, cx| argo.redetect(&cluster, cx));
    }
    let view = cx.new(|cx| SignInDialog::new(cluster, window, cx));
    let focus = view.read(cx).password.read(cx).focus_handle(cx);
    open(view, 540.0, Some(focus), window, cx);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Method {
    Password,
    Token,
}

struct SignInDialog {
    cluster: ClusterId,
    install: Option<Install>,
    method: Method,
    username: Entity<InputState>,
    password: Entity<InputState>,
    token: Entity<InputState>,
    error: Option<String>,
    busy: bool,
    focus: FocusHandle,
    _task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl SignInDialog {
    fn new(cluster: ClusterId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let trusted = settings::trusted(&cluster, cx);
        let username = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("username");
            state.set_value(
                trusted
                    .as_ref()
                    .and_then(|t| t.username.clone())
                    .unwrap_or_else(|| "admin".into()),
                window,
                cx,
            );
            state
        });
        // Masked inputs mask their placeholder too.
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let token = cx.new(|cx| InputState::new(window, cx).masked(true));
        let on_enter = |this: &mut Self,
                        _: &Entity<InputState>,
                        event: &InputEvent,
                        window: &mut Window,
                        cx: &mut Context<Self>| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(window, cx);
            }
        };
        let mut subscriptions = vec![
            cx.subscribe_in(&username, window, on_enter),
            cx.subscribe_in(&password, window, on_enter),
            cx.subscribe_in(&token, window, on_enter),
        ];
        if let Some(argo) = ArgoCd::try_global(cx) {
            subscriptions.push(cx.observe(&argo, |this, _, cx| {
                if this.install.is_none() {
                    this.pick_default(cx);
                }
                cx.notify();
            }));
        }
        let mut this = Self {
            cluster,
            install: None,
            method: Method::Password,
            username,
            password,
            token,
            error: None,
            busy: false,
            focus: cx.focus_handle(),
            _task: None,
            _subscriptions: subscriptions,
        };
        this.pick_default(cx);
        this
    }

    /// The confirmed install, else the first one found.
    fn pick_default(&mut self, cx: &App) {
        let Some(argo) = ArgoCd::try_global(cx) else {
            return;
        };
        let argo = argo.read(cx);
        let trusted = settings::trusted(&self.cluster, cx);
        let installs = argo.installs(&self.cluster);
        self.install = trusted
            .and_then(|t| {
                installs
                    .iter()
                    .find(|i| i.namespace == t.namespace)
                    .cloned()
            })
            .or_else(|| installs.iter().find(|i| i.server.is_some()).cloned())
            .or_else(|| installs.first().cloned());
        if let Some(install) = &self.install
            && !install.admin_enabled
            && self.method == Method::Password
        {
            // Without the admin account, SSO users sign in with a token.
            self.method = Method::Token;
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(install) = self.install.clone() else {
            self.error = Some("No Argo CD install found on this cluster.".into());
            cx.notify();
            return;
        };
        let credentials = match self.method {
            Method::Password => {
                let username = self.username.read(cx).value().trim().to_string();
                let password = self.password.read(cx).value().to_string();
                if username.is_empty() || password.is_empty() {
                    self.error = Some("Enter a username and a password.".into());
                    cx.notify();
                    return;
                }
                Credentials::Password {
                    username,
                    password: SecretString::from(password),
                }
            }
            Method::Token => {
                let token = self.token.read(cx).value().trim().to_string();
                if token.is_empty() {
                    self.error = Some("Paste a token.".into());
                    cx.notify();
                    return;
                }
                Credentials::Token(SecretString::from(token))
            }
        };
        let Some(argo) = ArgoCd::try_global(cx) else {
            return;
        };
        self.busy = true;
        self.error = None;
        let cluster = self.cluster.clone();
        let task = argo.update(cx, |argo, cx| {
            argo.sign_in(&cluster, install, credentials, cx)
        });
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(info) => {
                        NotificationCenter::push(
                            cx,
                            Notification::info(format!(
                                "Signed in to Argo CD as {}",
                                info.username
                            )),
                        );
                        // Clear the secret inputs before the view goes.
                        this.password
                            .update(cx, |i, cx| i.set_value("", window, cx));
                        this.token.update(cx, |i, cx| i.set_value("", window, cx));
                        window.close_dialog(cx);
                    }
                    Err(err) => this.error = Some(err),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Focusable for SignInDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SignInDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let argo = ArgoCd::try_global(cx);
        let detecting = argo.as_ref().is_some_and(|a| {
            matches!(
                a.read(cx).detection(&self.cluster),
                state::Detection::Running
            )
        });
        let installs = argo
            .as_ref()
            .map(|a| a.read(cx).installs(&self.cluster))
            .unwrap_or_default();
        let problem = argo
            .as_ref()
            .and_then(|a| a.read(cx).trust_problem(&self.cluster, cx));
        let name = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).display_name(&self.cluster))
            .unwrap_or_default();
        let transport = match settings::get(cx).api_transport {
            settings::ApiTransport::Auto => {
                "the API server's service proxy (a temporary port-forward if that fails)"
            }
            settings::ApiTransport::Proxy => "the API server's service proxy",
            settings::ApiTransport::Forward => "a temporary port-forward",
        };
        let mut body = v_flex().p(u(16.0)).gap(u(14.0));
        match &self.install {
            None if detecting => body = body.child(div().text_color(colors.text_dim).child("Looking for Argo CD…")),
            None => {
                body = body.child(div().text_color(colors.red).child(
                    "No Argo CD install found. Kubyl looks for the argocd-cm ConfigMap and the argocd-server Service (you need to be allowed to read them).",
                ))
            }
            Some(install) => {
                let server = install.server.clone();
                let service = server
                    .as_ref()
                    .map(|s| {
                        let port = if install.insecure { s.http_port } else { s.https_port.or(s.http_port) };
                        format!("{}{}", s.name, port.map(|p| format!(":{p}")).unwrap_or_default())
                    })
                    .unwrap_or_else(|| "not found".into());
                let mut facts: Vec<(&'static str, AnyElement)> = vec![
                    ("Cluster", widgets::text(name.clone())),
                    ("Namespace", widgets::mono(install.namespace.clone())),
                    ("Service", widgets::mono(service)),
                    ("Through", widgets::text(transport)),
                ];
                if let Some(url) = &install.url {
                    facts.push(("Argo CD URL", widgets::mono(format!("{url} (not used)"))));
                }
                if let Some(version) = &install.version {
                    facts.push(("Version", widgets::mono(version.clone())));
                }
                body = body.child(widgets::kv(facts, &colors));
                if installs.len() > 1 {
                    let weak = cx.entity().downgrade();
                    let choices: Vec<Install> = installs.iter().cloned().collect();
                    body = body.child(
                        MenuButton::new("sign-in-install")
                            .ghost()
                            .compact()
                            .child(format!("{} installs found · choose another", installs.len()))
                            .dropdown_menu(move |menu, _, _| {
                                let mut menu = menu;
                                for install in &choices {
                                    let weak = weak.clone();
                                    let pick = install.clone();
                                    menu = menu.item(PopupMenuItem::new(install.label()).on_click(move |_, _, cx| {
                                        let pick = pick.clone();
                                        weak.update(cx, |this, cx| {
                                            this.install = Some(pick);
                                            cx.notify();
                                        })
                                        .ok();
                                    }));
                                }
                                menu
                            }),
                    );
                }
                if let Some(problem) = &problem {
                    body = body.child(
                        widgets::warning_box(&colors)
                            .child(Icon::new(IconName::TriangleAlert).size(14.0).color(colors.yellow))
                            .child(div().flex_1().text_size(u(12.0)).child(problem.clone())),
                    );
                }
                if install.sso.dex || install.sso.oidc_issuer.is_some() {
                    let issuer = install.sso.oidc_issuer.clone().unwrap_or_else(|| "Dex".into());
                    body = body.child(
                        div().text_size(u(12.0)).text_color(colors.text_dim).child(format!(
                            "This Argo CD uses SSO ({issuer}). SSO users sign in with a token: `argocd account generate-token`, or the argocd.token cookie of a browser session."
                        )),
                    );
                }
            }
        }
        let method_tab =
            |id: &'static str, method: Method, label: &'static str, cx: &mut Context<Self>| {
                let on = self.method == method;
                div()
                    .id(id)
                    .px(u(10.0))
                    .py(u(4.0))
                    .cursor_pointer()
                    .text_size(u(12.5))
                    .text_color(if on { colors.text } else { colors.text_dim })
                    .when(on, |this| this.border_b_2().border_color(colors.accent))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.method = method;
                        let focus = match method {
                            Method::Password => this.password.read(cx).focus_handle(cx),
                            Method::Token => this.token.read(cx).focus_handle(cx),
                        };
                        focus.focus(window, cx);
                        cx.notify();
                    }))
                    .child(label)
            };
        body = body
            .child(
                h_flex()
                    .gap(u(4.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(method_tab("sign-in-password", Method::Password, "Username & password", cx))
                    .child(method_tab("sign-in-token", Method::Token, "Token", cx)),
            )
            .child(match self.method {
                Method::Password => v_flex()
                    .gap(u(10.0))
                    .child(field("Username", &self.username, false, &colors))
                    .child(field("Password", &self.password, true, &colors))
                    .into_any_element(),
                Method::Token => field("API token", &self.token, true, &colors).into_any_element(),
            })
            .child(
                h_flex()
                    .items_start()
                    .gap(u(8.0))
                    .text_size(u(11.5))
                    .text_color(colors.text_dim)
                    .child(Icon::new(IconName::Lock).size(12.0))
                    .child(div().flex_1().child(format!(
                        "Signing in confirms this install: Kubyl remembers it for {name} and asks again if its Service is replaced. The token goes to the {}; your Kubernetes credentials are never sent to Argo CD.",
                        kubyl_kube::auth::store::store_name()
                    ))),
            )
            .children(error_line(&self.error, &colors));
        let submit = Button::new("sign-in-submit")
            .primary()
            .icon(IconName::Key)
            .label(if self.busy {
                "Signing in…"
            } else {
                "Sign in"
            })
            .disabled(self.busy || self.install.as_ref().is_none_or(|i| i.server.is_none()))
            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)));
        v_flex()
            .key_context("ArgoSignIn")
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(
                IconName::Key,
                "Sign in to Argo CD (API mode)",
                None,
                &colors,
            ))
            .child(body)
            .child(footer(
                None,
                vec![cancel_button(), submit.into_any_element()],
                &colors,
            ))
    }
}
