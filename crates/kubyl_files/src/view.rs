//! `ViewKind::Files`: the pod file browser (board 9).
//!
//! A two-pane commander: this machine on one side, the container on the other. Items move by
//! F5, by dragging between the panes, or by dropping files from the OS onto the pod pane; the
//! transfer queue at the bottom shows progress, verification and history. Enter previews
//! (text and images), `e` edits a container file in place.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, ClipboardItem, Context, DispatchPhase,
    DragMoveEvent, Entity, EntityId, ExternalDragPayload, ExternalPaths, FileDragPaths,
    FocusHandle, Focusable, FontWeight, Hsla, IntoElement, KeyBinding, MouseUpEvent,
    PathPromptOptions, ScrollStrategy, SharedString, Subscription, Task, UniformListScrollHandle,
    WeakEntity, Window, actions, canvas, div, img, prelude::*, uniform_list,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{Editor, EditorState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use kubyl_core::{
    ActionRegistry, ActionSpec, ClusterId, Notification, NotificationCenter, ResourceRef, TabView,
    ViewRequest,
};
use kubyl_explorer::dialogs::{self, ConfirmSpec};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_settings::{State, StateSection};
use kubyl_ui::{
    ActiveColors, Chip, Colors, Icon, IconButton, IconName, KeyHints, ProgressBar, fonts, h_flex,
    u, v_flex,
};
use serde::{Deserialize, Serialize};

use crate::dialogs::{Resolution, conflict};
use crate::entry::{self, Entry, EntryKind, human_size, keep_both_name};
use crate::local;
use crate::mounts::{self, Mount, MountSource};
use crate::queue::{
    Transfer, TransferFinished, TransferQueue, TransferState, history, human_duration, human_speed,
};
use crate::remote::{self, RemoteTarget};
use crate::settings::FilesSettings;
use crate::transfer::{self, Direction, TransferJob, Verification};

pub(crate) const CONTEXT: &str = "FilesView";
pub(crate) const PANE_CONTEXT: &str = "FilesPane";

const ROW_HEIGHT: f32 = 28.0;
/// The cursor on the `..` row (listings never contain `..`).
const PARENT: &str = "..";
/// Files larger than this aren't opened for editing in place.
const EDIT_LIMIT: u64 = 5 * 1024 * 1024;
/// Images larger than this aren't previewed.
const IMAGE_LIMIT: u64 = 20 * 1024 * 1024;
/// Drags out of the window carry at most this much (staged when the drag starts).
const DRAG_OUT_LIMIT: u64 = 32 * 1024 * 1024;
/// Below this width (the details dock) the panes stack and the chrome gets compact.
const NARROW_WIDTH: f32 = 760.0;
/// GPUI hands drags to the OS on macOS and Wayland.
const DRAG_OUT: bool = cfg!(any(target_os = "macos", target_os = "linux"));

actions!(
    files,
    [
        CursorUp,
        CursorDown,
        ExtendUp,
        ExtendDown,
        /// Opens the folder, or previews the file.
        Open,
        GoUp,
        /// Moves focus to the other pane.
        SwitchPane,
        /// Copies the selection to the other pane's folder (F5).
        CopyToOther,
        SwapPanes,
        ToggleHidden,
        Preview,
        /// Opens a container file in an editor and uploads it on save.
        EditInPlace,
        /// Deletes the selection in the container.
        DeleteInPod,
        Rename,
        NewFolder,
        /// Changes the mode of the selection in the container.
        Chmod,
        CopyPath,
        SelectAll,
        Refresh,
        /// Uploads files or folders picked in a dialog.
        UploadFiles,
        /// Downloads the selection to a folder picked in a dialog.
        DownloadTo,
        CloseOverlay,
        SaveFile,
    ]
);

pub(crate) fn init(cx: &mut App) {
    let pane = Some(PANE_CONTEXT);
    let view = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", CursorUp, pane),
        KeyBinding::new("k", CursorUp, pane),
        KeyBinding::new("down", CursorDown, pane),
        KeyBinding::new("j", CursorDown, pane),
        KeyBinding::new("shift-up", ExtendUp, pane),
        KeyBinding::new("shift-down", ExtendDown, pane),
        KeyBinding::new("enter", Open, pane),
        KeyBinding::new("backspace", GoUp, pane),
        KeyBinding::new("left", GoUp, pane),
        KeyBinding::new("f5", CopyToOther, pane),
        KeyBinding::new("space", Preview, pane),
        KeyBinding::new("f3", Preview, pane),
        KeyBinding::new("e", EditInPlace, pane),
        KeyBinding::new("f4", EditInPlace, pane),
        KeyBinding::new("secondary-backspace", DeleteInPod, pane),
        KeyBinding::new("delete", DeleteInPod, pane),
        KeyBinding::new("f8", DeleteInPod, pane),
        KeyBinding::new("f2", Rename, pane),
        KeyBinding::new("f7", NewFolder, pane),
        KeyBinding::new("secondary-shift-n", NewFolder, pane),
        KeyBinding::new("secondary-alt-c", CopyPath, pane),
        KeyBinding::new("secondary-a", SelectAll, pane),
        KeyBinding::new("tab", SwitchPane, view),
        KeyBinding::new("secondary-shift-.", ToggleHidden, view),
        KeyBinding::new("secondary-r", Refresh, view),
        KeyBinding::new("secondary-u", UploadFiles, view),
        KeyBinding::new("secondary-shift-d", DownloadTo, view),
        KeyBinding::new("escape", CloseOverlay, view),
        KeyBinding::new("secondary-s", SaveFile, view),
    ]);
    let specs = [
        ActionSpec::new("Files: Copy to Other Pane", CopyToOther),
        ActionSpec::new("Files: Swap Panes", SwapPanes),
        ActionSpec::new("Files: Show Hidden Files", ToggleHidden),
        ActionSpec::new("Files: Preview", Preview),
        ActionSpec::new("Files: Edit in Place", EditInPlace),
        ActionSpec::new("Files: Delete in Pod…", DeleteInPod),
        ActionSpec::new("Files: Rename…", Rename),
        ActionSpec::new("Files: New Folder…", NewFolder),
        ActionSpec::new("Files: Change Mode (chmod)…", Chmod),
        ActionSpec::new("Files: Copy Path", CopyPath),
        ActionSpec::new("Files: Refresh", Refresh),
        ActionSpec::new("Files: Upload…", UploadFiles),
        ActionSpec::new("Files: Download to…", DownloadTo),
    ];
    for mut spec in specs {
        spec.context = view.map(SharedString::new_static);
        ActionRegistry::register(cx, spec);
    }
}

/// The last local folder, restored for new browsers (state.json `files`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct FilesState {
    local_dir: Option<PathBuf>,
}

impl StateSection for FilesState {
    const KEY: &'static str = "files";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Local,
    Pod,
}

impl Side {
    fn other(self) -> Side {
        match self {
            Side::Local => Side::Pod,
            Side::Pod => Side::Local,
        }
    }
}

/// Entries dragged from a pane.
#[derive(Clone)]
pub struct DraggedEntries {
    source: EntityId,
    from: Side,
    local_dir: PathBuf,
    pod_dir: String,
    entries: Vec<Entry>,
}

/// The drag preview: stacked cards with a count (board 9).
struct DragGhost {
    label: String,
    count: usize,
    folder: bool,
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let card = || {
            div()
                .absolute()
                .w(u(210.0))
                .h(u(34.0))
                .rounded(u(6.0))
                .bg(colors.elevated)
                .border_1()
                .border_color(colors.border)
        };
        div()
            .relative()
            .w(u(220.0))
            .h(u(44.0))
            .when(self.count > 2, |this| {
                this.child(card().left(u(6.0)).top(u(6.0)))
            })
            .when(self.count > 1, |this| {
                this.child(card().left(u(3.0)).top(u(3.0)))
            })
            .child(
                h_flex()
                    .absolute()
                    .w(u(210.0))
                    .h(u(34.0))
                    .px(u(10.0))
                    .gap(u(8.0))
                    .rounded(u(6.0))
                    .bg(colors.selection)
                    .border_1()
                    .border_color(colors.accent)
                    .child(
                        Icon::new(if self.folder {
                            IconName::Folder
                        } else {
                            IconName::File
                        })
                        .size(14.0)
                        .color(colors.accent),
                    )
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .text_color(colors.text)
                            .child(self.label.clone()),
                    )
                    .when(self.count > 1, |this| {
                        this.child(
                            div()
                                .px(u(6.0))
                                .rounded(u(9.0))
                                .bg(colors.accent)
                                .text_size(u(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.on_accent)
                                .child(self.count.to_string()),
                        )
                    }),
            )
    }
}

#[derive(Clone, Debug, PartialEq)]
enum RemoteState {
    NotConnected,
    Connecting(SharedString),
    Ready,
    /// The image has no shell (distroless): offer a debug container.
    NoShell,
    Failed(String),
}

struct PaneState {
    entries: Vec<Entry>,
    selected: BTreeSet<String>,
    anchor: Option<String>,
    cursor: Option<String>,
    loading: bool,
    error: Option<String>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    generation: u64,
}

impl PaneState {
    fn new(cx: &mut App) -> Self {
        Self {
            entries: Vec::new(),
            selected: BTreeSet::new(),
            anchor: None,
            cursor: None,
            loading: false,
            error: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            generation: 0,
        }
    }
}

enum PreviewContent {
    Loading,
    Text { text: String, truncated: bool },
    Image(PathBuf),
    Binary,
    Error(String),
}

struct EditSession {
    path: String,
    editor: Entity<EditorState>,
    original: String,
    /// `(mtime, size, sha256)` when the file was opened, to detect changes on save.
    baseline: (i64, u64, String),
    saving: bool,
    error: Option<String>,
}

enum Overlay {
    Preview {
        side: Side,
        entry: Entry,
        path: String,
        content: PreviewContent,
    },
    Edit(EditSession),
}

/// Where a copy goes, and what it is.
struct CopyItem {
    direction: Direction,
    /// Upload: the local path. Download: the remote path.
    source: String,
    name: String,
    is_dir: bool,
    size: u64,
}

/// A copy waiting for conflict decisions.
struct CopyPlan {
    items: Vec<CopyItem>,
    decisions: Vec<Option<Resolution>>,
    taken: HashSet<String>,
    /// Upload: the remote folder. Download: the local folder.
    destination: String,
}

/// Pod files downloaded when a drag starts, so the drag can leave the window. GPUI hands only
/// existing local files to the OS (there are no file promises), so small files are staged in a
/// temp folder and offered once they're complete; transfers rename into place, so the OS never
/// sees a partial file. The folder goes with the view.
#[derive(Default)]
struct DragStaging {
    dir: Option<tempfile::TempDir>,
    /// By remote path.
    files: HashMap<String, StagedFile>,
    next: u64,
}

struct StagedFile {
    /// `(size, modified)` of the entry when it was staged.
    fingerprint: (u64, Option<jiff::Timestamp>),
    path: PathBuf,
    ready: bool,
    failed: Option<String>,
    _task: Task<()>,
}

pub struct FilesView {
    focus: FocusHandle,
    request: ViewRequest,
    target: ResourceRef,
    containers: Vec<String>,
    container: Option<String>,
    remote: Option<RemoteTarget>,
    remote_state: RemoteState,
    mounts: Vec<Mount>,
    pod_dir: String,
    local_dir: PathBuf,
    local: PaneState,
    pod: PaneState,
    active: Side,
    swapped: bool,
    /// Measured each frame: the view is narrower than [`NARROW_WIDTH`].
    narrow: bool,
    show_hidden: bool,
    history_tab: bool,
    overlay: Option<Overlay>,
    drop_target: Option<Side>,
    /// Items in the drag over [`Self::drop_target`].
    drop_count: usize,
    /// Files from the OS over the pod pane.
    external_drop: Option<ExternalPaths>,
    preview_dir: Option<tempfile::TempDir>,
    staging: DragStaging,
    _connect: Option<Task<()>>,
    _connection: Option<Subscription>,
    _list: [Option<Task<()>>; 2],
    _overlay_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

fn short_pod(name: &str) -> String {
    let short = match name.rsplit_once('-') {
        Some((_, suffix)) if suffix.len() == 5 => suffix,
        _ => name,
    };
    short.to_string()
}

impl FilesView {
    pub fn new(target: Option<ResourceRef>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let has_target = target.is_some();
        let target = target.unwrap_or_else(|| {
            ResourceRef::object(
                ClusterId::new(""),
                kubyl_core::Gvr::new("", "v1", "pods"),
                None,
                String::new(),
            )
        });
        let settings = kubyl_settings::Settings::get::<FilesSettings>(cx).clone();
        let local_dir = State::get::<FilesState>(cx)
            .local_dir
            .or_else(dirs::download_dir)
            .filter(|d| d.is_dir())
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        let queue = TransferQueue::global(cx);
        let subscriptions = vec![
            cx.observe(&queue, |_, _, cx| cx.notify()),
            cx.subscribe(&queue, |this, _, event: &TransferFinished, cx| {
                this.transfer_finished(event, cx)
            }),
        ];
        let mut this = Self {
            focus: cx.focus_handle(),
            request: ViewRequest::for_resource(kubyl_core::ViewKind::Files, target.clone()),
            target,
            containers: Vec::new(),
            container: None,
            remote: None,
            remote_state: RemoteState::Connecting("connecting…".into()),
            mounts: Vec::new(),
            pod_dir: "/".into(),
            local_dir,
            local: PaneState::new(cx),
            pod: PaneState::new(cx),
            active: Side::Pod,
            swapped: false,
            narrow: false,
            show_hidden: settings.show_hidden,
            history_tab: false,
            overlay: None,
            drop_target: None,
            drop_count: 0,
            external_drop: None,
            preview_dir: None,
            staging: DragStaging::default(),
            _connect: None,
            _connection: None,
            _list: [None, None],
            _overlay_task: None,
            _subscriptions: subscriptions,
        };
        this.refresh(Side::Local, cx);
        if has_target {
            this.connect(None, cx);
        } else {
            this.remote_state = RemoteState::Failed("no pod".into());
        }
        let _ = window;
        this
    }

    fn pane(&self, side: Side) -> &PaneState {
        match side {
            Side::Local => &self.local,
            Side::Pod => &self.pod,
        }
    }

    fn pane_mut(&mut self, side: Side) -> &mut PaneState {
        match side {
            Side::Local => &mut self.local,
            Side::Pod => &mut self.pod,
        }
    }

    fn notify_error(cx: &mut App, message: impl Into<SharedString>) {
        NotificationCenter::push(cx, Notification::error(message));
    }

    // ----- Connection -----

    /// Probes the container (the pod's default one unless `container` is given) and lists the
    /// start folder.
    fn connect(&mut self, container: Option<String>, cx: &mut Context<Self>) {
        let Some(client) = ConnectionManager::global(cx)
            .read(cx)
            .client(&self.target.cluster)
        else {
            self.remote_state = RemoteState::NotConnected;
            self.wait_for_cluster(container, cx);
            return;
        };
        self.remote = None;
        self.remote_state = RemoteState::Connecting("probing the container…".into());
        let cluster = self.target.cluster.clone();
        let namespace = self.target.namespace.clone().unwrap_or_default();
        let pod_name = self.target.name.clone().unwrap_or_default();
        let task = kubyl_core::spawn_kube(cx, async move {
            let api: Api<Pod> = Api::namespaced(client.clone(), &namespace);
            let pod = api.get(&pod_name).await?;
            let info = kubyl_terminal::exec::PodInfo::of(&pod);
            let containers: Vec<String> = info
                .containers
                .iter()
                .filter(|c| !c.ephemeral)
                .map(|c| c.name.clone())
                .collect();
            let container = container
                .or(info.default.clone())
                .ok_or_else(|| anyhow::anyhow!("the pod has no containers"))?;
            let working_dir = pod
                .spec
                .as_ref()
                .and_then(|s| s.containers.iter().find(|c| c.name == container))
                .and_then(|c| c.working_dir.clone());
            let mounts = mounts::mounts(&pod, &container);
            let target =
                remote::open(cluster, client, namespace, pod_name, container.clone()).await?;
            anyhow::Ok((containers, container, working_dir, mounts, target))
        });
        self._connect = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((containers, container, working_dir, mounts, target)) => {
                        this.containers = containers;
                        this.container = Some(container);
                        this.mounts = mounts;
                        let shell = target.caps.shell;
                        this.remote = Some(target);
                        if shell {
                            this.remote_state = RemoteState::Ready;
                            this.pod_dir = working_dir.unwrap_or_else(|| "/".into());
                            this.refresh(Side::Pod, cx);
                        } else {
                            this.remote_state = RemoteState::NoShell;
                        }
                    }
                    Err(err) => this.remote_state = RemoteState::Failed(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Restored tabs are built before their cluster connects: connect, then probe.
    fn wait_for_cluster(&mut self, container: Option<String>, cx: &mut Context<Self>) {
        if self._connection.is_some() {
            return;
        }
        let manager = ConnectionManager::global(cx);
        let cluster = self.target.cluster.clone();
        manager.update(cx, |manager, cx| manager.ensure_connected(&cluster, cx));
        self._connection = Some(cx.subscribe(
            &manager,
            move |this, manager, event: &ConnectionEvent, cx| {
                if let ConnectionEvent::StateChanged(id) = event
                    && *id == cluster
                    && manager.read(cx).state(id).is_connected()
                    && this.remote_state == RemoteState::NotConnected
                {
                    this.connect(container.clone(), cx);
                    cx.notify();
                }
            },
        ));
    }

    /// Distroless: browse through an ephemeral debug container.
    fn use_debug_container(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.remote.clone() else {
            return;
        };
        let image = kubyl_settings::Settings::get::<FilesSettings>(cx)
            .debug_image
            .clone();
        self.remote_state = RemoteState::Connecting(format!("starting {image}…").into());
        let task = kubyl_core::spawn_kube(cx, async move {
            remote::through_debug_container(&target, &image).await
        });
        self._connect = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(target) => {
                        this.remote = Some(target);
                        this.remote_state = RemoteState::Ready;
                        this.pod_dir = "/".into();
                        this.refresh(Side::Pod, cx);
                    }
                    Err(err) => this.remote_state = RemoteState::Failed(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    // ----- Listings -----

    fn refresh(&mut self, side: Side, cx: &mut Context<Self>) {
        let pane = self.pane_mut(side);
        pane.loading = true;
        pane.generation += 1;
        let generation = pane.generation;
        let task: Task<Result<Vec<Entry>, String>> = match side {
            Side::Local => {
                let dir = self.local_dir.clone();
                cx.background_executor()
                    .spawn(async move { local::list(&dir).map_err(|e| e.to_string()) })
            }
            Side::Pod => {
                let Some(remote) = self.remote.clone().filter(|r| r.caps.shell) else {
                    self.pod.loading = false;
                    return;
                };
                let dir = self.pod_dir.clone();
                let task = kubyl_core::spawn_kube(cx, async move { remote.list(&dir).await });
                cx.background_executor()
                    .spawn(async move { task.await.map_err(|e| format!("{e:#}")) })
            }
        };
        let slot = match side {
            Side::Local => 0,
            Side::Pod => 1,
        };
        self._list[slot] = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let pane = this.pane_mut(side);
                if pane.generation != generation {
                    return;
                }
                pane.loading = false;
                match result {
                    Ok(entries) => {
                        pane.error = None;
                        let names: HashSet<&str> =
                            entries.iter().map(|e| e.name.as_str()).collect();
                        pane.selected.retain(|n| names.contains(n.as_str()));
                        if pane
                            .cursor
                            .as_ref()
                            .is_some_and(|c| !names.contains(c.as_str()))
                        {
                            pane.cursor = None;
                        }
                        pane.entries = entries;
                    }
                    Err(err) => {
                        pane.error = Some(err);
                        pane.entries.clear();
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn navigate_local(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let previous = self
            .local_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        let going_up = dir.as_path() == self.local_dir.parent().unwrap_or(&self.local_dir);
        self.local_dir = dir;
        self.local.entries.clear();
        self.local.selected.clear();
        self.local.cursor = if going_up { previous } else { None };
        State::set(
            cx,
            &FilesState {
                local_dir: Some(self.local_dir.clone()),
            },
        );
        self.refresh(Side::Local, cx);
    }

    fn navigate_pod(&mut self, dir: String, cx: &mut Context<Self>) {
        let dir = entry::normalize(&dir);
        let going_up = entry::parent(&self.pod_dir) == dir && self.pod_dir != "/";
        let previous = entry::file_name(&self.pod_dir).to_string();
        self.pod_dir = dir;
        self.pod.entries.clear();
        self.pod.selected.clear();
        self.pod.cursor = going_up.then_some(previous);
        self.refresh(Side::Pod, cx);
    }

    /// The listing shown in a pane (hidden files filtered).
    fn visible(&self, side: Side) -> Vec<&Entry> {
        self.pane(side)
            .entries
            .iter()
            .filter(|e| self.show_hidden || !e.is_hidden())
            .collect()
    }

    fn has_parent(&self, side: Side) -> bool {
        match side {
            Side::Local => self.local_dir.parent().is_some(),
            Side::Pod => self.pod_dir != "/",
        }
    }

    fn row_count(&self, side: Side) -> usize {
        self.visible(side).len() + self.has_parent(side) as usize
    }

    /// The entry of a row (`None` for the `..` row).
    fn row_entry(&self, side: Side, row: usize) -> Option<&Entry> {
        let offset = self.has_parent(side) as usize;
        row.checked_sub(offset)
            .and_then(|ix| self.visible(side).get(ix).copied())
    }

    fn cursor_row(&self, side: Side) -> Option<usize> {
        let pane = self.pane(side);
        let cursor = pane.cursor.as_ref()?;
        let offset = self.has_parent(side) as usize;
        if cursor == PARENT {
            return (offset == 1).then_some(0);
        }
        self.visible(side)
            .iter()
            .position(|e| &e.name == cursor)
            .map(|ix| ix + offset)
    }

    fn set_cursor_row(&mut self, side: Side, row: usize, extend: bool, cx: &mut Context<Self>) {
        let name = self.row_entry(side, row).map(|e| e.name.clone());
        let order: Vec<String> = self.visible(side).iter().map(|e| e.name.clone()).collect();
        let pane = self.pane_mut(side);
        match &name {
            Some(name) if extend => {
                let anchor = pane
                    .anchor
                    .clone()
                    .or(pane.cursor.clone())
                    .unwrap_or(name.clone());
                pane.anchor = Some(anchor.clone());
                let a = order.iter().position(|n| *n == anchor).unwrap_or(0);
                let b = order.iter().position(|n| n == name).unwrap_or(0);
                pane.selected = order[a.min(b)..=a.max(b)].iter().cloned().collect();
            }
            Some(name) => {
                pane.anchor = Some(name.clone());
                pane.selected = [name.clone()].into();
            }
            None => {
                pane.anchor = None;
                pane.selected.clear();
            }
        }
        pane.cursor = Some(name.unwrap_or_else(|| PARENT.into()));
        pane.scroll.scroll_to_item(row, ScrollStrategy::Top);
        cx.notify();
    }

    fn move_cursor(&mut self, side: Side, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let count = self.row_count(side);
        if count == 0 {
            return;
        }
        let current = self
            .cursor_row(side)
            .unwrap_or(if delta > 0 { usize::MAX } else { 0 });
        let next = if current == usize::MAX {
            0
        } else {
            (current as isize + delta).clamp(0, count as isize - 1) as usize
        };
        self.set_cursor_row(side, next, extend, cx);
    }

    fn click_row(
        &mut self,
        side: Side,
        row: usize,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active = side;
        self.pane(side).focus.clone().focus(window, cx);
        let modifiers = event.modifiers();
        if event.click_count() >= 2 {
            self.open_row(side, row, window, cx);
            return;
        }
        if modifiers.secondary()
            && let Some(name) = self.row_entry(side, row).map(|e| e.name.clone())
        {
            let pane = self.pane_mut(side);
            if !pane.selected.remove(&name) {
                pane.selected.insert(name.clone());
            }
            pane.cursor = Some(name.clone());
            pane.anchor = Some(name);
            cx.notify();
            return;
        }
        self.set_cursor_row(side, row, modifiers.shift, cx);
    }

    fn open_row(&mut self, side: Side, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.row_entry(side, row).cloned() {
            None => self.go_up(side, cx),
            Some(entry) if entry.is_dir() => match side {
                Side::Local => {
                    let dir = self.local_dir.join(&entry.name);
                    self.navigate_local(dir, cx)
                }
                Side::Pod => {
                    let dir = entry::join(&self.pod_dir, &entry.name);
                    self.navigate_pod(dir, cx)
                }
            },
            Some(entry) => self.preview(side, entry, window, cx),
        }
    }

    fn open_cursor(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.cursor_row(side).unwrap_or(0);
        if self.row_count(side) > 0 {
            self.open_row(side, row, window, cx);
        }
    }

    fn go_up(&mut self, side: Side, cx: &mut Context<Self>) {
        match side {
            Side::Local => {
                if let Some(parent) = self.local_dir.parent().map(Path::to_path_buf) {
                    self.navigate_local(parent, cx);
                }
            }
            Side::Pod => {
                if self.pod_dir != "/" {
                    let parent = entry::parent(&self.pod_dir);
                    self.navigate_pod(parent, cx);
                }
            }
        }
    }

    /// The selected entries, or the one under the cursor.
    fn selection(&self, side: Side) -> Vec<Entry> {
        let pane = self.pane(side);
        let entries = self.visible(side);
        let selected: Vec<Entry> = entries
            .iter()
            .filter(|e| pane.selected.contains(&e.name))
            .map(|e| (*e).clone())
            .collect();
        if !selected.is_empty() {
            return selected;
        }
        pane.cursor
            .as_ref()
            .and_then(|c| entries.iter().find(|e| &e.name == c))
            .map(|e| vec![(*e).clone()])
            .unwrap_or_default()
    }

    fn select_all(&mut self, side: Side, cx: &mut Context<Self>) {
        let names: BTreeSet<String> = self.visible(side).iter().map(|e| e.name.clone()).collect();
        self.pane_mut(side).selected = names;
        cx.notify();
    }

    // ----- Copies -----

    fn transfer_settings(&self, cx: &App) -> (bool, u64) {
        let settings = kubyl_settings::Settings::get::<FilesSettings>(cx);
        (settings.verify, settings.chunk_size_mb.max(1) * 1024 * 1024)
    }

    /// Copies the selection of `from` into the other pane's folder.
    fn copy_selection(
        &mut self,
        from: Side,
        keep_both: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = self.selection(from);
        if entries.is_empty() {
            return;
        }
        match from {
            Side::Local => {
                let paths = entries
                    .iter()
                    .map(|e| (self.local_dir.join(&e.name), e.is_dir(), e.size))
                    .collect();
                self.upload(paths, keep_both, window, cx);
            }
            Side::Pod => {
                let dir = self.local_dir.clone();
                let items = entries
                    .iter()
                    .map(|e| (entry::join(&self.pod_dir, &e.name), e.is_dir(), e.size))
                    .collect();
                self.download(items, dir, keep_both, window, cx);
            }
        }
    }

    /// Uploads local paths (`(path, is_dir, size)`) into the pod pane's folder.
    fn upload(
        &mut self,
        paths: Vec<(PathBuf, bool, u64)>,
        keep_both: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.remote.is_none() || self.remote_state != RemoteState::Ready {
            Self::notify_error(cx, "The container isn't ready.");
            return;
        }
        let destination = self.pod_dir.clone();
        let items: Vec<CopyItem> = paths
            .into_iter()
            .map(|(path, is_dir, size)| CopyItem {
                direction: Direction::Upload,
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                source: path.display().to_string(),
                is_dir,
                size: if is_dir { 0 } else { size },
            })
            .collect();
        if let Some(reason) = mounts::write_blocked(&self.mounts, &destination) {
            // Shown in the queue like any failed transfer (board 9).
            let jobs: Vec<TransferJob> = items
                .iter()
                .filter_map(|item| self.job(item, &destination, item.name.clone(), cx))
                .collect();
            let queue = TransferQueue::global(cx);
            queue.update(cx, |queue, cx| {
                for job in jobs {
                    queue.reject(job, reason.clone(), cx);
                }
            });
            Self::notify_error(cx, format!("Can't write to {destination}: {reason}"));
            return;
        }
        let taken: HashSet<String> = self.pod.entries.iter().map(|e| e.name.clone()).collect();
        self.plan(items, taken, destination, keep_both, window, cx);
    }

    /// Downloads remote paths (`(path, is_dir, size)`) into a local folder.
    fn download(
        &mut self,
        paths: Vec<(String, bool, u64)>,
        dir: PathBuf,
        keep_both: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items: Vec<CopyItem> = paths
            .into_iter()
            .map(|(path, is_dir, size)| CopyItem {
                direction: Direction::Download,
                name: entry::file_name(&path).to_string(),
                source: path,
                is_dir,
                size: if is_dir { 0 } else { size },
            })
            .collect();
        let taken: HashSet<String> = if dir == self.local_dir {
            self.local.entries.iter().map(|e| e.name.clone()).collect()
        } else {
            std::fs::read_dir(&dir)
                .map(|items| {
                    items
                        .filter_map(Result::ok)
                        .map(|i| i.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default()
        };
        self.plan(
            items,
            taken,
            dir.display().to_string(),
            keep_both,
            window,
            cx,
        );
    }

    fn plan(
        &mut self,
        items: Vec<CopyItem>,
        taken: HashSet<String>,
        destination: String,
        keep_both: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let decisions = items
            .iter()
            .map(|item| {
                if !taken.contains(&item.name) {
                    Some(Resolution::Overwrite)
                } else if keep_both {
                    Some(Resolution::KeepBoth)
                } else {
                    None
                }
            })
            .collect();
        let plan = CopyPlan {
            items,
            decisions,
            taken,
            destination,
        };
        if plan.decisions.iter().all(Option::is_some) {
            self.execute(plan, cx);
        } else {
            Self::resolve(cx.weak_entity(), plan, window, cx);
        }
    }

    /// Asks about the next undecided conflict, or starts the copies once all are decided. Runs
    /// outside of the view's update (from the dialog's callbacks).
    fn resolve(view: WeakEntity<Self>, plan: CopyPlan, window: &mut Window, cx: &mut App) {
        let Some(ix) = plan.decisions.iter().position(Option::is_none) else {
            view.update(cx, |this, cx| this.execute(plan, cx)).ok();
            return;
        };
        let remaining = plan.decisions.iter().filter(|d| d.is_none()).count() - 1;
        let name = plan.items[ix].name.clone();
        let destination = match plan.items[ix].direction {
            Direction::Download => local::display(Path::new(&plan.destination)),
            Direction::Upload => plan.destination.clone(),
        };
        let plan = Rc::new(std::cell::RefCell::new(Some(plan)));
        conflict(
            name,
            destination,
            remaining,
            move |resolution, all, window, cx| {
                let Some(mut plan) = plan.borrow_mut().take() else {
                    return;
                };
                plan.decisions[ix] = Some(resolution);
                if all {
                    for decision in plan.decisions.iter_mut().filter(|d| d.is_none()) {
                        *decision = Some(resolution);
                    }
                }
                Self::resolve(view.clone(), plan, window, cx);
            },
            window,
            cx,
        );
    }

    fn job(
        &self,
        item: &CopyItem,
        destination: &str,
        dest_name: String,
        cx: &App,
    ) -> Option<TransferJob> {
        let target = self.remote.clone()?;
        let (verify, chunk_size) = self.transfer_settings(cx);
        Some(match item.direction {
            Direction::Upload => TransferJob {
                direction: Direction::Upload,
                target,
                remote: destination.to_string(),
                local: PathBuf::from(&item.source),
                dest_name,
                is_dir: item.is_dir,
                size: item.size,
                verify,
                chunk_size,
            },
            Direction::Download => TransferJob {
                direction: Direction::Download,
                target,
                remote: item.source.clone(),
                local: PathBuf::from(destination),
                dest_name,
                is_dir: item.is_dir,
                size: item.size,
                verify,
                chunk_size,
            },
        })
    }

    fn execute(&mut self, mut plan: CopyPlan, cx: &mut Context<Self>) {
        let mut jobs = Vec::new();
        for (item, decision) in plan.items.iter().zip(&plan.decisions) {
            let dest_name = match decision {
                Some(Resolution::Skip) | None => continue,
                Some(Resolution::Overwrite) => item.name.clone(),
                Some(Resolution::KeepBoth) => {
                    let taken = plan.taken.clone();
                    keep_both_name(&item.name, &|n| taken.contains(n))
                }
            };
            plan.taken.insert(dest_name.clone());
            if let Some(job) = self.job(item, &plan.destination, dest_name, cx) {
                jobs.push(job);
            }
        }
        if jobs.is_empty() {
            return;
        }
        let queue = TransferQueue::global(cx);
        queue.update(cx, |queue, cx| {
            for job in jobs {
                queue.enqueue(job, cx);
            }
        });
        self.history_tab = false;
        cx.notify();
    }

    fn transfer_finished(&mut self, event: &TransferFinished, cx: &mut Context<Self>) {
        if Some(event.pod.as_str()) != self.target.name.as_deref() {
            return;
        }
        match event.direction {
            Direction::Upload if event.remote_dir == self.pod_dir => self.refresh(Side::Pod, cx),
            Direction::Download if event.local_dir == self.local_dir => {
                self.refresh(Side::Local, cx)
            }
            _ => {}
        }
    }

    fn drop_entries(
        &mut self,
        to: Side,
        drag: &DraggedEntries,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drop_target = None;
        if drag.source != cx.entity_id() || drag.from == to {
            cx.notify();
            return;
        }
        let keep_both = window.modifiers().alt;
        match to {
            Side::Pod => {
                let paths = drag
                    .entries
                    .iter()
                    .map(|e| (drag.local_dir.join(&e.name), e.is_dir(), e.size))
                    .collect();
                self.upload(paths, keep_both, window, cx);
            }
            Side::Local => {
                let items = drag
                    .entries
                    .iter()
                    .map(|e| (entry::join(&drag.pod_dir, &e.name), e.is_dir(), e.size))
                    .collect();
                let dir = self.local_dir.clone();
                self.download(items, dir, keep_both, window, cx);
            }
        }
    }

    fn drop_paths(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_target = None;
        let keep_both = window.modifiers().alt;
        let paths = paths
            .paths()
            .iter()
            .map(|p| {
                let meta = std::fs::metadata(p).ok();
                (
                    p.clone(),
                    meta.as_ref().is_some_and(|m| m.is_dir()),
                    meta.map(|m| m.len()).unwrap_or(0),
                )
            })
            .collect();
        self.upload(paths, keep_both, window, cx);
    }

    fn upload_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Upload".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let paths = paths
                    .into_iter()
                    .map(|p| {
                        let meta = std::fs::metadata(&p).ok();
                        (
                            p,
                            meta.as_ref().is_some_and(|m| m.is_dir()),
                            meta.map(|m| m.len()).unwrap_or(0),
                        )
                    })
                    .collect();
                this.upload(paths, false, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn download_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.selection(Side::Pod);
        if entries.is_empty() {
            return;
        }
        let items: Vec<(String, bool, u64)> = entries
            .iter()
            .map(|e| (entry::join(&self.pod_dir, &e.name), e.is_dir(), e.size))
            .collect();
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Download here".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut dirs))) = paths.await else {
                return;
            };
            let Some(dir) = dirs.pop() else { return };
            this.update_in(cx, |this, window, cx| {
                this.download(items, dir, false, window, cx)
            })
            .ok();
        })
        .detach();
    }

    // ----- Drag out to the OS -----

    /// Why the dragged entries can't leave the window, if they can't.
    fn drag_out_blocked(&self, drag: &DraggedEntries) -> Option<String> {
        if drag
            .entries
            .iter()
            .any(|e| e.is_dir() || e.kind == EntryKind::Other)
        {
            return Some(
                "Folders can't be dragged out of Kubyl yet: use Download to… (⌘⇧D).".into(),
            );
        }
        if drag.entries.iter().map(|e| e.size).sum::<u64>() > DRAG_OUT_LIMIT {
            return Some(format!(
                "Only files up to {} can be dragged out of Kubyl: use Download to… (⌘⇧D).",
                human_size(DRAG_OUT_LIMIT)
            ));
        }
        let secret = drag.entries.iter().any(|e| {
            mounts::mount_for(&self.mounts, &entry::join(&drag.pod_dir, &e.name))
                .is_some_and(|m| m.source == MountSource::Secret)
        });
        // Staging would write Secret data to disk before the user decided to.
        secret.then(|| "Secret files aren't dragged out of Kubyl: use Download to… (⌘⇧D).".into())
    }

    /// Starts downloading the dragged pod files into the staging folder.
    fn stage_for_drag(&mut self, drag: &DraggedEntries, cx: &mut Context<Self>) {
        if !DRAG_OUT || drag.from != Side::Pod || self.drag_out_blocked(drag).is_some() {
            return;
        }
        let Some(target) = self.remote.clone() else {
            return;
        };
        let (verify, chunk_size) = self.transfer_settings(cx);
        for entry in &drag.entries {
            let remote = entry::join(&drag.pod_dir, &entry.name);
            let fingerprint = (entry.size, entry.modified);
            if self
                .staging
                .files
                .get(&remote)
                .is_some_and(|f| f.fingerprint == fingerprint && f.failed.is_none())
            {
                continue;
            }
            if self.staging.dir.is_none() {
                match tempfile::Builder::new().prefix("kubyl-drag-").tempdir() {
                    Ok(dir) => self.staging.dir = Some(dir),
                    Err(err) => {
                        tracing::warn!(%err, "no temp folder for dragging files out");
                        return;
                    }
                }
            }
            let Some(dir) = self.staging.dir.as_ref().map(|d| d.path().to_path_buf()) else {
                return;
            };
            self.staging.next += 1;
            let folder = dir.join(self.staging.next.to_string());
            let path = folder.join(&entry.name);
            let job = TransferJob {
                direction: Direction::Download,
                target: target.clone(),
                remote: remote.clone(),
                local: folder.clone(),
                dest_name: entry.name.clone(),
                is_dir: false,
                size: entry.size,
                verify,
                chunk_size,
            };
            let run = kubyl_core::spawn_kube(cx, async move {
                tokio::fs::create_dir_all(&folder).await?;
                let (progress, _) = futures::channel::mpsc::unbounded();
                transfer::run(job, progress).await
            });
            let key = remote.clone();
            let task = cx.spawn(async move |this, cx| {
                let result = run.await;
                this.update(cx, |this, _| {
                    if let Some(file) = this.staging.files.get_mut(&key) {
                        match result {
                            Ok(_) => file.ready = true,
                            Err(err) => file.failed = Some(format!("{err:#}")),
                        }
                    }
                })
                .ok();
            });
            self.staging.files.insert(
                remote,
                StagedFile {
                    fingerprint,
                    path,
                    ready: false,
                    failed: None,
                    _task: task,
                },
            );
        }
    }

    /// The staged files for a drag leaving the window, or why there are none (a toast).
    fn drag_out_payload(
        &mut self,
        drag: &DraggedEntries,
        cx: &mut Context<Self>,
    ) -> Option<ExternalDragPayload> {
        if drag.from != Side::Pod || drag.source != cx.entity_id() {
            return None;
        }
        if let Some(reason) = self.drag_out_blocked(drag) {
            NotificationCenter::push(cx, Notification::info(reason));
            return None;
        }
        let mut paths = Vec::new();
        for entry in &drag.entries {
            let remote = entry::join(&drag.pod_dir, &entry.name);
            match self.staging.files.get(&remote) {
                Some(file) if file.ready => paths.push((file.path.clone(), false)),
                Some(StagedFile {
                    failed: Some(err), ..
                }) => {
                    Self::notify_error(cx, format!("Couldn't download {}: {err}", entry.name));
                    return None;
                }
                _ => {
                    NotificationCenter::push(
                        cx,
                        Notification::info(format!(
                            "Still downloading {} for the drag: drag it out again in a moment.",
                            entry.name
                        )),
                    );
                    return None;
                }
            }
        }
        tracing::debug!(files = paths.len(), "offering staged files to the OS drag");
        Some(ExternalDragPayload::Files(FileDragPaths::new(paths)))
    }

    // ----- Actions on entries -----

    fn remote_op(
        &mut self,
        label: &'static str,
        op: impl FnOnce(RemoteTarget) -> futures::future::BoxFuture<'static, anyhow::Result<()>>,
        cx: &mut Context<Self>,
    ) {
        let Some(remote) = self.remote.clone() else {
            return;
        };
        let task = kubyl_core::spawn_kube(cx, op(remote));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if let Err(err) = result {
                    Self::notify_error(cx, format!("{label} failed: {err:#}"));
                }
                this.refresh(Side::Pod, cx);
            })
            .ok();
        })
        .detach();
    }

    fn delete_in_pod(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.selection(Side::Pod);
        if entries.is_empty() || self.remote.is_none() {
            return;
        }
        if let Some(reason) = mounts::write_blocked(&self.mounts, &self.pod_dir) {
            Self::notify_error(cx, format!("Can't delete in {}: {reason}", self.pod_dir));
            return;
        }
        let paths: Vec<String> = entries
            .iter()
            .map(|e| entry::join(&self.pod_dir, &e.name))
            .collect();
        let production = ConnectionManager::global(cx)
            .read(cx)
            .caps(&self.target.cluster)
            .production;
        let mut spec = ConfirmSpec::new(
            match paths.len() {
                1 => format!("Delete {}?", entries[0].name),
                n => format!("Delete {n} items?"),
            },
            "Delete",
        );
        spec.lines = paths
            .iter()
            .map(|p| SharedString::from(p.clone()))
            .collect();
        spec.note = Some(
            format!(
                "In {} · folders are deleted with their contents.",
                self.target.name.clone().unwrap_or_default()
            )
            .into(),
        );
        spec.danger = true;
        spec.typed = production.then(|| entries[0].name.clone());
        let weak = cx.weak_entity();
        dialogs::confirm(
            spec,
            move |_, _, cx| {
                let paths = paths.clone();
                weak.update(cx, |this, cx| {
                    this.remote_op(
                        "Delete",
                        move |remote| {
                            Box::pin(async move {
                                let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
                                remote.delete(&refs).await
                            })
                        },
                        cx,
                    )
                })
                .ok();
            },
            window,
            cx,
        );
    }

    fn rename(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.selection(side).into_iter().next() else {
            return;
        };
        let weak = cx.weak_entity();
        let old = entry.name.clone();
        dialogs::prompt_text(
            format!("Rename {old}").into(),
            "New name",
            old.clone(),
            move |name, _, cx| {
                if name.is_empty() || name == old || name.contains('/') {
                    return;
                }
                let old = old.clone();
                weak.update(cx, |this, cx| match side {
                    Side::Local => {
                        let from = this.local_dir.join(&old);
                        let to = this.local_dir.join(&name);
                        if to.exists() {
                            Self::notify_error(cx, format!("{name} already exists."));
                        } else if let Err(err) = std::fs::rename(from, to) {
                            Self::notify_error(cx, format!("Rename failed: {err}"));
                        }
                        this.local.cursor = Some(name);
                        this.refresh(Side::Local, cx);
                    }
                    Side::Pod => {
                        let from = entry::join(&this.pod_dir, &old);
                        let to = entry::join(&this.pod_dir, &name);
                        this.pod.cursor = Some(name);
                        this.remote_op(
                            "Rename",
                            move |remote| Box::pin(async move { remote.rename(&from, &to).await }),
                            cx,
                        );
                    }
                })
                .ok();
            },
            window,
            cx,
        );
    }

    fn new_folder(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        if side == Side::Pod
            && let Some(reason) = mounts::write_blocked(&self.mounts, &self.pod_dir)
        {
            Self::notify_error(
                cx,
                format!("Can't create a folder in {}: {reason}", self.pod_dir),
            );
            return;
        }
        let weak = cx.weak_entity();
        dialogs::prompt_text(
            "New folder".into(),
            "Name",
            String::new(),
            move |name, _, cx| {
                if name.is_empty() || name.contains('/') {
                    return;
                }
                weak.update(cx, |this, cx| match side {
                    Side::Local => {
                        if let Err(err) = std::fs::create_dir(this.local_dir.join(&name)) {
                            Self::notify_error(cx, format!("New folder failed: {err}"));
                        }
                        this.local.cursor = Some(name);
                        this.refresh(Side::Local, cx);
                    }
                    Side::Pod => {
                        let path = entry::join(&this.pod_dir, &name);
                        this.pod.cursor = Some(name);
                        this.remote_op(
                            "New folder",
                            move |remote| Box::pin(async move { remote.mkdir(&path).await }),
                            cx,
                        );
                    }
                })
                .ok();
            },
            window,
            cx,
        );
    }

    fn chmod(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.selection(Side::Pod);
        if entries.is_empty() {
            return;
        }
        let paths: Vec<String> = entries
            .iter()
            .map(|e| entry::join(&self.pod_dir, &e.name))
            .collect();
        let current = format!("{:o}", entries[0].mode & 0o7777);
        let weak = cx.weak_entity();
        dialogs::prompt_text(
            format!(
                "Change mode of {}",
                if entries.len() == 1 {
                    entries[0].name.clone()
                } else {
                    format!("{} items", entries.len())
                }
            )
            .into(),
            "Mode (755, u+x, go-w…)",
            current,
            move |mode, _, cx| {
                if mode.is_empty() {
                    return;
                }
                let paths = paths.clone();
                weak.update(cx, |this, cx| {
                    this.remote_op(
                        "chmod",
                        move |remote| {
                            Box::pin(async move {
                                let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
                                remote.chmod(&mode, &refs).await
                            })
                        },
                        cx,
                    )
                })
                .ok();
            },
            window,
            cx,
        );
    }

    fn copy_path(&mut self, side: Side, cx: &mut Context<Self>) {
        let entries = self.selection(side);
        let paths: Vec<String> = if entries.is_empty() {
            vec![match side {
                Side::Local => self.local_dir.display().to_string(),
                Side::Pod => self.pod_dir.clone(),
            }]
        } else {
            entries
                .iter()
                .map(|e| match side {
                    Side::Local => self.local_dir.join(&e.name).display().to_string(),
                    Side::Pod => entry::join(&self.pod_dir, &e.name),
                })
                .collect()
        };
        cx.write_to_clipboard(ClipboardItem::new_string(paths.join("\n")));
    }

    // ----- Preview and edit in place -----

    fn preview(&mut self, side: Side, entry: Entry, _: &mut Window, cx: &mut Context<Self>) {
        if entry.is_dir() {
            return;
        }
        let limit = kubyl_settings::Settings::get::<FilesSettings>(cx).preview_limit_kb * 1024;
        let path = match side {
            Side::Local => self.local_dir.join(&entry.name).display().to_string(),
            Side::Pod => entry::join(&self.pod_dir, &entry.name),
        };
        // Pod images are previewed from a temp file: never for Secret data.
        let secret = side == Side::Pod
            && mounts::mount_for(&self.mounts, &path)
                .is_some_and(|m| m.source == MountSource::Secret);
        let image = is_image(&entry.name) && !secret;
        self.overlay = Some(Overlay::Preview {
            side,
            entry: entry.clone(),
            path: path.clone(),
            content: PreviewContent::Loading,
        });
        if image && entry.size > IMAGE_LIMIT {
            self.set_preview(
                PreviewContent::Error("the image is too large to preview".into()),
                cx,
            );
            return;
        }
        let temp = self
            .preview_dir
            .get_or_insert_with(|| tempfile::tempdir().expect("temp dir"))
            .path()
            .to_path_buf();
        let task: Task<anyhow::Result<PreviewContent>> = match side {
            Side::Local => {
                let path = PathBuf::from(&path);
                cx.background_executor().spawn(async move {
                    if image {
                        return Ok(PreviewContent::Image(path));
                    }
                    let mut file = std::fs::File::open(&path)?;
                    let mut buf = vec![0u8; limit as usize];
                    let n = std::io::Read::read(&mut file, &mut buf)?;
                    buf.truncate(n);
                    Ok(text_content(buf, entry.size, limit))
                })
            }
            Side::Pod => {
                let Some(remote) = self.remote.clone() else {
                    return;
                };
                let name = entry.name.clone();
                let task = kubyl_core::spawn_kube(cx, async move {
                    if image {
                        let bytes = remote.read(&path).await?;
                        let file = temp.join(&name);
                        tokio::fs::write(&file, bytes).await?;
                        return Ok(PreviewContent::Image(file));
                    }
                    let bytes = remote.head(&path, limit).await?;
                    Ok(text_content(bytes, entry.size, limit))
                });
                cx.background_executor().spawn(task)
            }
        };
        self._overlay_task = Some(cx.spawn(async move |this, cx| {
            let content = task
                .await
                .unwrap_or_else(|e| PreviewContent::Error(format!("{e:#}")));
            this.update(cx, |this, cx| this.set_preview(content, cx))
                .ok();
        }));
        cx.notify();
    }

    fn set_preview(&mut self, content: PreviewContent, cx: &mut Context<Self>) {
        if let Some(Overlay::Preview { content: slot, .. }) = &mut self.overlay {
            *slot = content;
            cx.notify();
        }
    }

    fn edit_in_place(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.selection(side).into_iter().next() else {
            return;
        };
        if entry.is_dir() {
            return;
        }
        if side == Side::Local {
            // Local files open in their app.
            cx.open_with_system(&self.local_dir.join(&entry.name));
            return;
        }
        if let Some(reason) = mounts::write_blocked(&self.mounts, &self.pod_dir) {
            Self::notify_error(cx, format!("Can't edit {}: {reason}", entry.name));
            return;
        }
        if entry.size > EDIT_LIMIT {
            Self::notify_error(
                cx,
                format!(
                    "{} is too large to edit here ({}).",
                    entry.name,
                    human_size(entry.size)
                ),
            );
            return;
        }
        let Some(remote) = self.remote.clone() else {
            return;
        };
        let path = entry::join(&self.pod_dir, &entry.name);
        let language = language_for(&entry.name);
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .line_number(true)
                .searchable(true)
                .soft_wrap(false)
        });
        self.overlay = Some(Overlay::Edit(EditSession {
            path: path.clone(),
            editor,
            original: String::new(),
            baseline: (0, 0, String::new()),
            saving: true,
            error: None,
        }));
        let task = kubyl_core::spawn_kube(cx, async move {
            let (mtime, size) = remote.stat(&path).await?;
            let bytes = remote.read(&path).await?;
            if bytes.contains(&0) {
                anyhow::bail!("this looks like a binary file");
            }
            let text = String::from_utf8(bytes)
                .map_err(|_| anyhow::anyhow!("the file isn't UTF-8 text"))?;
            let sha = local::sha256_bytes(text.as_bytes());
            anyhow::Ok((text, (mtime, size, sha)))
        });
        self._overlay_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                let Some(Overlay::Edit(session)) = &mut this.overlay else {
                    return;
                };
                session.saving = false;
                match result {
                    Ok((text, baseline)) => {
                        session
                            .editor
                            .update(cx, |state, cx| state.set_value(text.clone(), window, cx));
                        session.original = text;
                        session.baseline = baseline;
                        let focus = session.editor.read(cx).focus_handle(cx);
                        focus.focus(window, cx);
                    }
                    Err(err) => session.error = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn save_edit(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Edit(session)) = &mut self.overlay else {
            return;
        };
        if session.saving {
            return;
        }
        let dir = entry::parent(&session.path);
        if let Some(reason) = mounts::write_blocked(&self.mounts, &dir) {
            session.error = Some(format!("Can't save: {reason}"));
            cx.notify();
            return;
        }
        let Some(remote) = self.remote.clone() else {
            return;
        };
        let text = session.editor.read(cx).text().to_string();
        let path = session.path.clone();
        let baseline = session.baseline.clone();
        session.saving = true;
        session.error = None;
        let task = kubyl_core::spawn_kube(cx, async move {
            if !force {
                // Conflict check: did the file change in the container since it was opened?
                let (mtime, size) = remote.stat(&path).await?;
                if (mtime, size) != (baseline.0, baseline.1) {
                    let current = remote.read(&path).await?;
                    if local::sha256_bytes(&current) != baseline.2 {
                        return Ok(None);
                    }
                }
            }
            remote.write(&path, text.clone().into_bytes()).await?;
            let (mtime, size) = remote.stat(&path).await?;
            let sha = local::sha256_bytes(text.as_bytes());
            anyhow::Ok(Some((text, (mtime, size, sha))))
        });
        self._overlay_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                let Some(Overlay::Edit(session)) = &mut this.overlay else {
                    return;
                };
                session.saving = false;
                match result {
                    Ok(Some((text, baseline))) => {
                        session.original = text;
                        session.baseline = baseline;
                        NotificationCenter::push(
                            cx,
                            Notification::success(format!("Saved {}", session.path)),
                        );
                        this.refresh(Side::Pod, cx);
                    }
                    Ok(None) => {
                        let weak = cx.weak_entity();
                        let mut spec =
                            ConfirmSpec::new("The file changed in the container", "Overwrite");
                        spec.lines = vec![session.path.clone().into()];
                        spec.note = Some(
                            "It was modified since you opened it. Overwrite it with your version?"
                                .into(),
                        );
                        spec.danger = true;
                        dialogs::confirm(
                            spec,
                            move |_, window, cx| {
                                weak.update(cx, |this, cx| this.save_edit(true, window, cx))
                                    .ok();
                            },
                            window,
                            cx,
                        );
                    }
                    Err(err) => session.error = Some(format!("Save failed: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Edit(session)) = &self.overlay
            && *session.editor.read(cx).text() != session.original
            && !session.saving
        {
            let weak = cx.weak_entity();
            let mut spec = ConfirmSpec::new("Discard your changes?", "Discard");
            spec.lines = vec![session.path.clone().into()];
            spec.danger = true;
            dialogs::confirm(
                spec,
                move |_, window, cx| {
                    weak.update(cx, |this, cx| {
                        this.overlay = None;
                        this.pane(this.active).focus.clone().focus(window, cx);
                        cx.notify();
                    })
                    .ok();
                },
                window,
                cx,
            );
            return;
        }
        self.overlay = None;
        self._overlay_task = None;
        self.pane(self.active).focus.clone().focus(window, cx);
        cx.notify();
    }
}

fn is_image(name: &str) -> bool {
    let lower = name.to_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".bmp", ".ico",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

fn language_for(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        "yaml"
    } else {
        "text"
    }
}

fn text_content(bytes: Vec<u8>, size: u64, limit: u64) -> PreviewContent {
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return PreviewContent::Binary;
    }
    PreviewContent::Text {
        text: String::from_utf8_lossy(&bytes).into_owned(),
        truncated: size > limit,
    }
}

/// `10:31`, `yesterday`, `Sep 21`, `2024-09-21`.
fn short_time(ts: Option<jiff::Timestamp>) -> String {
    let Some(ts) = ts else { return String::new() };
    let tz = jiff::tz::TimeZone::system();
    let at = ts.to_zoned(tz.clone());
    let today = jiff::Timestamp::now().to_zoned(tz).date();
    if at.date() == today {
        at.strftime("%H:%M").to_string()
    } else if today.yesterday().ok() == Some(at.date()) {
        "yesterday".into()
    } else if at.year() == today.year() {
        at.strftime("%b %-d").to_string()
    } else {
        at.strftime("%Y-%m-%d").to_string()
    }
}

/// `3d`, `1h`, `12m` since a time.
fn age(ts: Option<jiff::Timestamp>) -> String {
    let Some(ts) = ts else { return String::new() };
    let secs = jiff::Timestamp::now().as_second() - ts.as_second();
    match secs {
        i64::MIN..60 => "now".into(),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

// ----- Rendering -----

impl FilesView {
    fn pane_header(&self, side: Side, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let weak = cx.weak_entity();
        let crumb_parts: Vec<(String, String)> = match side {
            Side::Local => {
                let display = local::display(&self.local_dir);
                let mut parts = Vec::new();
                let mut path = PathBuf::new();
                for component in self.local_dir.components() {
                    path.push(component);
                    let label = match component {
                        std::path::Component::RootDir => continue,
                        other => other.as_os_str().to_string_lossy().into_owned(),
                    };
                    parts.push((label, path.display().to_string()));
                }
                if display.starts_with('~') {
                    // Collapse the home folder into `~`.
                    let home_depth = dirs::home_dir()
                        .map(|h| h.components().count() - 1)
                        .unwrap_or(0);
                    let home = dirs::home_dir()
                        .map(|h| h.display().to_string())
                        .unwrap_or_default();
                    let mut collapsed = vec![("~".to_string(), home)];
                    collapsed.extend(parts.into_iter().skip(home_depth));
                    collapsed
                } else {
                    let mut all = vec![("/".to_string(), "/".to_string())];
                    all.extend(parts);
                    all
                }
            }
            Side::Pod => entry::crumbs(&self.pod_dir),
        };
        let count = crumb_parts.len();
        let from_root = crumb_parts.first().is_some_and(|(label, _)| label == "/");
        let crumbs = h_flex()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .gap(u(4.0))
            .font_family(fonts::MONO)
            .text_size(u(12.0))
            .children(
                crumb_parts
                    .into_iter()
                    .enumerate()
                    .map(|(ix, (label, path))| {
                        let last = ix + 1 == count;
                        h_flex()
                            .gap(u(4.0))
                            .when(ix > 1 || (ix == 1 && !from_root), |this| {
                                this.child(div().text_color(colors.text_faint).child("/"))
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!("crumb-{side:?}-{ix}")))
                                    .cursor_pointer()
                                    .text_color(if last { colors.text } else { colors.text_dim })
                                    .hover(|s| s.text_color(colors.accent))
                                    .on_click(cx.listener(move |this, _, _, cx| match side {
                                        Side::Local => {
                                            this.navigate_local(PathBuf::from(&path), cx)
                                        }
                                        Side::Pod => this.navigate_pod(path.clone(), cx),
                                    }))
                                    .child(label),
                            )
                    }),
            );
        let selected = self.pane(side).selected.len();
        let header = h_flex()
            .flex_none()
            .h(u(36.0))
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .bg(colors.panel);
        match side {
            Side::Local => {
                let bookmarks =
                    local::bookmarks(&kubyl_settings::Settings::get::<FilesSettings>(cx).bookmarks);
                let places = MenuButton::new("files-places")
                    .ghost()
                    .compact()
                    .child(
                        h_flex()
                            .gap(u(6.0))
                            .child(
                                Icon::new(IconName::Server)
                                    .size(13.0)
                                    .color(colors.text_dim),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(colors.text)
                                    .child(this_machine()),
                            )
                            .child(Icon::new(IconName::ChevronDown).size(10.0)),
                    )
                    .dropdown_menu(move |menu, _, _| {
                        let mut menu = menu;
                        for (label, path) in &bookmarks {
                            let weak = weak.clone();
                            let path = path.clone();
                            menu = menu.item(PopupMenuItem::new(label.clone()).on_click(
                                move |_, _, cx| {
                                    let path = path.clone();
                                    weak.update(cx, |this, cx| this.navigate_local(path, cx))
                                        .ok();
                                },
                            ));
                        }
                        menu
                    });
                header
                    .child(places)
                    .child(crumbs)
                    .when(selected > 0, |this| {
                        this.child(
                            div()
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child(format!("{selected} selected")),
                        )
                    })
                    .into_any_element()
            }
            Side::Pod => {
                let mount = mounts::mount_for(&self.mounts, &self.pod_dir).cloned();
                header
                    .child(Icon::new(IconName::Box).size(13.0).color(colors.accent))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.container.clone().unwrap_or_default()),
                    )
                    .child(crumbs)
                    .when_some(mount, |this, mount| {
                        this.child(
                            Chip::new(SharedString::from(mount.label()))
                                .when(!mount.writable(), |chip| chip.icon(IconName::Lock)),
                        )
                    })
                    .when(selected > 0, |this| {
                        this.child(
                            div()
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child(format!("{selected} selected")),
                        )
                    })
                    .when(!self.narrow, |this| {
                        this.child(
                            kubyl_ui::Button::new("files-new-folder")
                                .ghost()
                                .icon(IconName::Plus)
                                .label("New folder")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.new_folder(Side::Pod, window, cx)
                                })),
                        )
                        .child(
                            kubyl_ui::Button::new("files-download")
                                .ghost()
                                .icon(IconName::Download)
                                .label("Download")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.download_dialog(window, cx)
                                })),
                        )
                    })
                    .when(self.narrow, |this| {
                        this.child(
                            IconButton::new("files-new-folder", IconName::Plus)
                                .icon_size(13.0)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.new_folder(Side::Pod, window, cx)
                                })),
                        )
                        .child(
                            IconButton::new("files-download", IconName::Download)
                                .icon_size(13.0)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.download_dialog(window, cx)
                                })),
                        )
                    })
                    .into_any_element()
            }
        }
    }

    fn table_header(&self, side: Side, colors: &Colors) -> AnyElement {
        let cell = |label: &'static str, width: Option<f32>| {
            let d = div().child(label);
            match width {
                Some(w) => d.flex_none().w(u(w)),
                None => d.flex_1().min_w_0(),
            }
        };
        h_flex()
            .flex_none()
            .h(u(28.0))
            .px(u(12.0))
            .gap(u(8.0))
            .text_size(u(11.5))
            .text_color(colors.text_dim)
            .border_b_1()
            .border_color(colors.border_variant)
            .bg(colors.subheader_background)
            .child(cell("NAME", None))
            .child(cell("SIZE", Some(78.0)))
            // Narrow: names get the room.
            .when(side == Side::Pod && !self.narrow, |this| {
                this.child(cell("MODE", Some(96.0)))
                    .child(cell("OWNER · MODIFIED", Some(118.0)))
            })
            .when(side == Side::Local && !self.narrow, |this| {
                this.child(cell("MODIFIED", Some(104.0)))
            })
            .into_any_element()
    }

    fn render_row(
        &self,
        side: Side,
        row: usize,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let entry = self.row_entry(side, row).cloned();
        let pane = self.pane(side);
        let (name, is_dir, selected, cursor) = match &entry {
            None => (
                PARENT.to_string(),
                true,
                false,
                pane.cursor.as_deref() == Some(PARENT),
            ),
            Some(e) => (
                e.name.clone(),
                e.is_dir(),
                pane.selected.contains(&e.name),
                pane.cursor.as_deref() == Some(e.name.as_str()),
            ),
        };
        let icon_color = if is_dir {
            colors.accent
        } else {
            colors.text_dim
        };
        let mono = |text: String, width: f32| {
            div()
                .flex_none()
                .w(u(width))
                .truncate()
                .font_family(fonts::MONO)
                .text_size(u(12.0))
                .text_color(colors.text_muted)
                .child(text)
        };
        let path = entry.as_ref().map(|e| match side {
            Side::Local => self.local_dir.join(&e.name).display().to_string(),
            Side::Pod => entry::join(&self.pod_dir, &e.name),
        });
        let mount = match (side, &path) {
            (Side::Pod, Some(path)) => mounts::mount_at(&self.mounts, path).cloned(),
            _ => None,
        };
        let mut element = h_flex()
            .id(SharedString::from(format!("files-row-{side:?}-{row}")))
            .w_full()
            .h(u(ROW_HEIGHT))
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.row_border)
            .when(selected, |this| this.bg(colors.chip_selected_background))
            .when(cursor && focused, |this| {
                this.border_1().border_color(colors.accent)
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.click_row(side, row, event, window, cx)
            }))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(u(8.0))
                    .child(
                        Icon::new(if is_dir {
                            IconName::Folder
                        } else {
                            IconName::File
                        })
                        .size(14.0)
                        .color(icon_color),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .text_color(colors.text)
                            .child(if is_dir && entry.is_some() {
                                format!("{name}/")
                            } else {
                                name.clone()
                            }),
                    )
                    .when_some(
                        entry.as_ref().and_then(|e| e.link_target.clone()),
                        |this, target| {
                            this.child(
                                div()
                                    .truncate()
                                    .text_size(u(11.0))
                                    .text_color(colors.text_faint)
                                    .child(format!("→ {target}")),
                            )
                        },
                    )
                    .when_some(mount, |this, mount| {
                        this.child(
                            Chip::new(SharedString::from(mount.label()))
                                .when(!mount.writable(), |chip| chip.icon(IconName::Lock)),
                        )
                    }),
            );
        if let Some(e) = &entry {
            let size = if e.is_dir() {
                "—".to_string()
            } else {
                human_size(e.size)
            };
            element = element.child(mono(size, 78.0));
            element = match side {
                _ if self.narrow => element,
                Side::Pod => element.child(mono(e.mode_string(), 96.0)).child(mono(
                    format!(
                        "{} · {}",
                        e.owner.clone().unwrap_or_default(),
                        age(e.modified)
                    ),
                    118.0,
                )),
                Side::Local => element.child(mono(short_time(e.modified), 104.0)),
            };
            // Dragging: the selection if the row is in it, else just this row.
            let dragged: Vec<Entry> = if selected {
                self.selection(side)
            } else {
                vec![e.clone()]
            };
            let count = dragged.len();
            let label = if count > 1 {
                format!("{} +{}", dragged[0].name, count - 1)
            } else {
                dragged[0].name.clone()
            };
            let folder = dragged[0].is_dir();
            let drag = DraggedEntries {
                source: cx.entity_id(),
                from: side,
                local_dir: self.local_dir.clone(),
                pod_dir: self.pod_dir.clone(),
                entries: dragged,
            };
            let weak = cx.weak_entity();
            element = element
                .on_drag(drag, {
                    let weak = weak.clone();
                    move |drag: &DraggedEntries, _, _, cx| {
                        weak.update(cx, |this, cx| this.stage_for_drag(drag, cx))
                            .ok();
                        let label = label.clone();
                        cx.new(|_| DragGhost {
                            label,
                            count,
                            folder,
                        })
                    }
                })
                .external_drag_payload(move |drag: &DraggedEntries, _, cx| {
                    weak.update(cx, |this, cx| this.drag_out_payload(drag, cx))
                        .ok()
                        .flatten()
                });
        } else {
            element = element.child(mono(String::new(), 78.0));
        }
        element.into_any_element()
    }

    fn drop_summary(&self, side: Side, cx: &App) -> Option<(String, Option<String>)> {
        if !cx.has_active_drag() || self.drop_target != Some(side) {
            return None;
        }
        let destination = match side {
            Side::Pod => self.pod_dir.clone(),
            Side::Local => local::display(&self.local_dir),
        };
        let verb = match side {
            Side::Pod => "upload",
            Side::Local => "download",
        };
        let blocked = (side == Side::Pod)
            .then(|| mounts::write_blocked(&self.mounts, &self.pod_dir))
            .flatten();
        let items = match self.drop_count {
            1 => "1 item".to_string(),
            n => format!("{n} items"),
        };
        Some((
            format!("Drop to {verb} {items} into {destination}"),
            blocked,
        ))
    }

    fn render_pane(
        &mut self,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let focused = self.pane(side).focus.contains_focused(window, cx);
        let header = self.pane_header(side, cx);
        let table_header = self.table_header(side, &colors);
        let count = self.row_count(side);
        let pane = self.pane(side);
        let focus = pane.focus.clone();
        let scroll = pane.scroll.clone();
        let message: Option<SharedString> = if side == Side::Pod {
            match &self.remote_state {
                RemoteState::NotConnected => Some("The cluster isn't connected.".into()),
                RemoteState::Connecting(step) => Some(step.clone()),
                RemoteState::Failed(err) => {
                    Some(format!("Couldn't open the container: {err}").into())
                }
                _ => None,
            }
        } else {
            None
        }
        .or_else(|| pane.error.clone().map(SharedString::from))
        .or_else(|| (pane.loading && pane.entries.is_empty()).then(|| "Loading…".into()));
        let no_shell = side == Side::Pod && self.remote_state == RemoteState::NoShell;
        let summary = self.drop_summary(side, cx);
        let rows = uniform_list(
            SharedString::from(format!("files-rows-{side:?}")),
            count,
            cx.processor(
                move |this: &mut Self, range: std::ops::Range<usize>, _, cx| {
                    range
                        .map(|row| this.render_row(side, row, focused, cx))
                        .collect()
                },
            ),
        )
        .track_scroll(&scroll)
        .flex_1();
        let body = div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .when(message.is_none() && !no_shell, |this| this.child(rows))
            .when_some(message, |this, message| {
                this.child(div().p(u(16.0)).text_color(colors.text_dim).child(message))
            })
            .when(no_shell, |this| {
                let image = kubyl_settings::Settings::get::<FilesSettings>(cx).debug_image.clone();
                this.child(
                    v_flex()
                        .p(u(16.0))
                        .gap(u(10.0))
                        .text_color(colors.text_muted)
                        .child("This container has no shell (a distroless image?), so its files can't be listed directly.")
                        .child(div().text_size(u(12.0)).text_color(colors.text_dim).child(format!(
                            "Kubyl can add an ephemeral {image} container that shares its processes and reads its files at /proc/1/root. Ephemeral containers stay in the pod until it's deleted."
                        )))
                        .child(h_flex().child(
                            kubyl_ui::Button::new("files-debug-container")
                                .primary()
                                .label("Browse through a debug container")
                                .on_click(cx.listener(|this, _, _, cx| this.use_debug_container(cx)))),
                        ),
                )
            });
        let flex = match side {
            Side::Local => 1.0,
            Side::Pod => 1.25,
        };
        let mut container =
            v_flex()
                .id(SharedString::from(format!("files-pane-{side:?}")))
                .key_context(PANE_CONTEXT)
                .track_focus(&focus)
                .relative()
                .when(!self.narrow, |this| {
                    this.flex_grow(1.0)
                        .flex_basis(gpui::relative(flex / 2.25))
                        .min_w_0()
                        .h_full()
                })
                // Narrow (the details dock): the panes stack, half the height each.
                .when(self.narrow, |this| {
                    this.flex_1()
                        .flex_basis(gpui::relative(0.))
                        .min_h_0()
                        .w_full()
                })
                .on_action(cx.listener(move |this, _: &CursorUp, _, cx| {
                    this.move_cursor(side, -1, false, cx)
                }))
                .on_action(cx.listener(move |this, _: &CursorDown, _, cx| {
                    this.move_cursor(side, 1, false, cx)
                }))
                .on_action(cx.listener(move |this, _: &ExtendUp, _, cx| {
                    this.move_cursor(side, -1, true, cx)
                }))
                .on_action(cx.listener(move |this, _: &ExtendDown, _, cx| {
                    this.move_cursor(side, 1, true, cx)
                }))
                .on_action(
                    cx.listener(move |this, _: &Open, window, cx| {
                        this.open_cursor(side, window, cx)
                    }),
                )
                .on_action(cx.listener(move |this, _: &GoUp, _, cx| this.go_up(side, cx)))
                .on_action(cx.listener(move |this, _: &CopyToOther, window, cx| {
                    this.copy_selection(side, false, window, cx)
                }))
                .on_action(cx.listener(move |this, _: &Preview, window, cx| {
                    if let Some(entry) = this.selection(side).into_iter().next() {
                        this.preview(side, entry, window, cx);
                    }
                }))
                .on_action(cx.listener(move |this, _: &EditInPlace, window, cx| {
                    this.edit_in_place(side, window, cx)
                }))
                .on_action(
                    cx.listener(move |this, _: &Rename, window, cx| this.rename(side, window, cx)),
                )
                .on_action(cx.listener(move |this, _: &NewFolder, window, cx| {
                    this.new_folder(side, window, cx)
                }))
                .on_action(cx.listener(move |this, _: &CopyPath, _, cx| this.copy_path(side, cx)))
                .on_action(cx.listener(move |this, _: &SelectAll, _, cx| this.select_all(side, cx)))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.active = side;
                        cx.notify();
                    }),
                )
                .on_drag_move(cx.listener(
                    move |this, event: &DragMoveEvent<DraggedEntries>, _, cx| {
                        let drag = event.drag(cx);
                        // Only the other pane of this browser takes the drop.
                        let inside = event.bounds.contains(&event.event.position)
                            && drag.from != side
                            && drag.source == cx.entity_id();
                        this.drop_count = drag.entries.len();
                        let target = inside.then_some(side);
                        if inside && this.drop_target != target {
                            this.drop_target = target;
                            cx.notify();
                        } else if !inside && this.drop_target == Some(side) {
                            this.drop_target = None;
                            cx.notify();
                        }
                    },
                ))
                .on_drop(cx.listener(move |this, drag: &DraggedEntries, window, cx| {
                    this.drop_entries(side, drag, window, cx)
                }))
                .child(header)
                .child(table_header)
                .child(body);
        if side == Side::Pod {
            container = container
                .on_drag_move(
                    cx.listener(|this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                        let inside = event.bounds.contains(&event.event.position);
                        let paths = event.drag(cx).clone();
                        this.drop_count = paths.paths().len();
                        this.external_drop = inside.then_some(paths);
                        let target = inside.then_some(Side::Pod);
                        if this.drop_target != target {
                            this.drop_target = target;
                            cx.notify();
                        }
                    }),
                )
                // Not `on_drop` (nor `capture_any_mouse_up`): GPUI hit-tests those with the hover
                // state, which stays off after keyboard input because OS drags don't count as
                // mouse input. A raw capture listener takes the drop before the workspace's
                // kubeconfig drop sees it.
                .child(
                    canvas(|_, _, _| {}, {
                        let view = cx.weak_entity();
                        move |bounds, _, window, _| {
                            let view = view.clone();
                            window.on_mouse_event(
                                move |event: &MouseUpEvent, phase, window, cx| {
                                    if phase != DispatchPhase::Capture
                                        || !bounds.contains(&event.position)
                                    {
                                        return;
                                    }
                                    view.update(cx, |this, cx| {
                                        let Some(paths) = this.external_drop.take() else {
                                            return;
                                        };
                                        if cx.has_active_drag() {
                                            cx.stop_active_drag(window);
                                            cx.stop_propagation();
                                            this.drop_paths(&paths, window, cx);
                                        }
                                    })
                                    .ok();
                                },
                            );
                        }
                    })
                    .absolute()
                    .size_full(),
                );
        }
        if let Some((text, blocked)) = summary {
            container = container.child(
                div()
                    .absolute()
                    .left(u(8.0))
                    .right(u(8.0))
                    .top(u(44.0))
                    .bottom(u(8.0))
                    .rounded(u(8.0))
                    .border_2()
                    .border_dashed()
                    .border_color(if blocked.is_some() {
                        colors.red
                    } else {
                        colors.accent
                    })
                    .bg(colors.accent.opacity(0.07))
                    .flex()
                    .items_end()
                    .justify_center()
                    .pb(u(26.0))
                    .child(
                        h_flex()
                            .gap(u(10.0))
                            .px(u(16.0))
                            .py(u(10.0))
                            .rounded(u(8.0))
                            .bg(colors.panel)
                            .border_1()
                            .border_color(if blocked.is_some() {
                                colors.red
                            } else {
                                colors.accent
                            })
                            .child(
                                Icon::new(if side == Side::Pod {
                                    IconName::Upload
                                } else {
                                    IconName::Download
                                })
                                .size(16.0)
                                .color(colors.accent),
                            )
                            .child(
                                v_flex().child(div().text_size(u(13.0)).child(text)).child(
                                    div()
                                        .text_size(u(11.5))
                                        .text_color(if blocked.is_some() {
                                            colors.red
                                        } else {
                                            colors.yellow
                                        })
                                        .child(blocked.unwrap_or_else(|| {
                                            "hold ⌥ to keep both when names exist".into()
                                        })),
                                ),
                            ),
                    ),
            );
        }
        let first = (side == Side::Local) != self.swapped;
        container
            .when(first && !self.narrow, |this| {
                this.border_r_1().border_color(colors.border)
            })
            .when(first && self.narrow, |this| {
                this.border_b_1().border_color(colors.border)
            })
            .into_any_element()
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let weak = cx.weak_entity();
        let pod = self.target.name.clone().unwrap_or_default();
        let containers = self.containers.clone();
        let current = self.container.clone();
        let container_menu = menu_button(
            "files-container",
            IconName::Box,
            current.clone().unwrap_or_else(|| "container".into()),
            &colors,
        )
        .dropdown_menu(move |menu, _, _| {
            let mut menu = menu;
            for name in &containers {
                let weak = weak.clone();
                let pick = name.clone();
                menu = menu.item(
                    PopupMenuItem::new(name.clone())
                        .checked(current.as_deref() == Some(name))
                        .on_click(move |_, _, cx| {
                            let pick = pick.clone();
                            weak.update(cx, |this, cx| this.connect(Some(pick), cx))
                                .ok();
                        }),
                );
            }
            menu
        });
        let (chip_color, chip_label) = match (&self.remote_state, &self.remote) {
            (RemoteState::Ready, Some(remote)) if remote.is_debug() => (
                colors.yellow,
                format!(
                    "debug container {} · {}",
                    remote.exec_container,
                    remote.caps.summary()
                ),
            ),
            (RemoteState::Ready, Some(remote)) => (
                if remote.caps.complete() {
                    colors.green
                } else {
                    colors.yellow
                },
                remote.caps.summary(),
            ),
            (RemoteState::NoShell, _) => (colors.yellow, "no shell".into()),
            (RemoteState::Failed(_), _) => (colors.red, "unavailable".into()),
            _ => (colors.text_faint, "probing…".into()),
        };
        h_flex()
            .flex_none()
            .flex_wrap()
            .min_h(u(kubyl_ui::sizes::TOOLBAR))
            .px(u(12.0))
            .py(u(4.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border)
            .bg(colors.panel)
            .child(
                h_flex()
                    .gap(u(6.0))
                    .child(Icon::new(IconName::Box).size(14.0).color(colors.accent))
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .child(pod),
                    ),
            )
            .child(container_menu)
            .child(Chip::new(SharedString::from(chip_label)).dot(chip_color))
            .child(div().flex_1())
            .child(
                div()
                    .id("files-hidden")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_hidden = !this.show_hidden;
                        cx.notify();
                    }))
                    .child(Chip::new("Show hidden").selected(self.show_hidden)),
            )
            .when(!self.narrow, |this| {
                this.child(
                    kubyl_ui::Button::new("files-upload")
                        .icon(IconName::Upload)
                        .label("Upload…")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.upload_dialog(window, cx)),
                        ),
                )
                .child(
                    kubyl_ui::Button::new("files-swap")
                        .icon(IconName::Columns)
                        .label("Swap panes")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.swapped = !this.swapped;
                            cx.notify();
                        })),
                )
            })
            .when(self.narrow, |this| {
                this.child(
                    IconButton::new("files-upload", IconName::Upload)
                        .icon_size(13.0)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.upload_dialog(window, cx)),
                        ),
                )
                .child(
                    IconButton::new("files-swap", IconName::Rows)
                        .icon_size(13.0)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.swapped = !this.swapped;
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    fn transfer_row(
        transfer: &Transfer,
        narrow: bool,
        colors: &Colors,
        cx: &Context<Self>,
    ) -> AnyElement {
        let job = &transfer.job;
        let id = transfer.id;
        let pod = short_pod(&job.target.pod);
        let detail = match job.direction {
            Direction::Download => format!(
                "{pod}:{} → {}",
                entry::parent(&job.remote),
                local::display(&job.local)
            ),
            Direction::Upload => format!(
                "{} → {pod}:{}",
                job.local.parent().map(local::display).unwrap_or_default(),
                job.remote
            ),
        };
        let name = if job.dest_name != job.source_name() {
            format!("{} → {}", job.source_name(), job.dest_name)
        } else if job.is_dir {
            format!("{}/", job.dest_name)
        } else {
            job.dest_name.clone()
        };
        let (state, color): (String, Hsla) = match &transfer.state {
            TransferState::Queued => ("queued".into(), colors.text_dim),
            TransferState::Running => {
                let mut parts = Vec::new();
                if let Some(pct) = transfer.percent() {
                    parts.push(format!("{pct:.0}%"));
                } else {
                    parts.push(human_size(transfer.done_bytes));
                }
                if transfer.speed > 0.0 {
                    parts.push(human_speed(transfer.speed));
                }
                if let Some(eta) = transfer.eta() {
                    parts.push(format!("{} left", human_duration(eta)));
                }
                (parts.join(" · "), colors.accent)
            }
            TransferState::Done(Verification::Verified) => ("done · verified".into(), colors.green),
            TransferState::Done(Verification::Unverified(reason)) => {
                (format!("done · unverified ({reason})"), colors.yellow)
            }
            TransferState::Failed(err) => (err.clone(), colors.red),
            TransferState::Cancelled => ("cancelled".into(), colors.text_dim),
        };
        let percent = match &transfer.state {
            TransferState::Running => Some(transfer.percent().unwrap_or(0.0)),
            TransferState::Done(_) => Some(100.0),
            _ => None,
        };
        let finished = transfer.state.is_finished();
        let retry = matches!(
            transfer.state,
            TransferState::Failed(_) | TransferState::Cancelled
        );
        h_flex()
            .id(("transfer", id as usize))
            .h(u(34.0))
            .px(u(14.0))
            .gap(u(12.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(
                Icon::new(match job.direction {
                    Direction::Download => IconName::Download,
                    Direction::Upload => IconName::Upload,
                })
                .size(14.0)
                .color(color),
            )
            .child(
                div()
                    .when(!narrow, |this| this.flex_none().w(u(340.0)))
                    .when(narrow, |this| this.flex_1().min_w_0())
                    .truncate()
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .child(name),
            )
            .when(!narrow, |this| {
                this.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child(detail),
                )
            })
            .child(
                div()
                    .flex_none()
                    .w(u(if narrow { 60.0 } else { 180.0 }))
                    .when_some(percent, |this, pct| {
                        this.child(ProgressBar::new(pct).color(color))
                    }),
            )
            .child(
                div()
                    .id(("transfer-state", id as usize))
                    .flex_none()
                    .w(u(if narrow { 110.0 } else { 200.0 }))
                    .truncate()
                    .text_align(gpui::TextAlign::Right)
                    .text_size(u(12.0))
                    .text_color(color)
                    .tooltip({
                        let state = state.clone();
                        move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(state.clone()).build(window, cx)
                        }
                    })
                    .child(state),
            )
            .when(retry, |this| {
                this.child(
                    IconButton::new(("transfer-retry", id as usize), IconName::RefreshCw)
                        .icon_size(12.0)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            TransferQueue::global(cx).update(cx, |q, cx| q.retry(id, cx));
                        })),
                )
            })
            .child(
                IconButton::new(("transfer-x", id as usize), IconName::X)
                    .icon_size(12.0)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        TransferQueue::global(cx).update(cx, |q, cx| {
                            if finished {
                                q.dismiss(id, cx)
                            } else {
                                q.cancel(id, cx)
                            }
                        });
                    })),
            )
            .into_any_element()
    }

    fn render_queue(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let queue = TransferQueue::global(cx).read(cx);
        let active = queue.active_count();
        let shown: Vec<&Transfer> = if self.history_tab {
            history(queue.transfers())
        } else {
            queue
                .transfers()
                .iter()
                .filter(|t| !t.dismissed)
                .rev()
                .collect()
        };
        let rows: Vec<AnyElement> = shown
            .into_iter()
            .map(|t| Self::transfer_row(t, self.narrow, &colors, cx))
            .collect();
        let tab = |id: &'static str, label: &'static str, icon: IconName, on: bool| {
            h_flex()
                .id(id)
                .h_full()
                .px(u(14.0))
                .gap(u(7.0))
                .cursor_pointer()
                .border_r_1()
                .border_color(colors.border)
                .when(on, |this| this.bg(colors.background))
                .text_color(if on { colors.text } else { colors.text_dim })
                .child(Icon::new(icon).size(13.0).color(if on {
                    colors.accent
                } else {
                    colors.text_dim
                }))
                .child(label)
        };
        v_flex()
            .flex_none()
            .h(u(if self.narrow { 150.0 } else { 210.0 }))
            .border_t_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .flex_none()
                    .h(u(32.0))
                    .border_b_1()
                    .border_color(colors.border)
                    .bg(colors.panel)
                    .child(
                        tab(
                            "files-transfers-tab",
                            "Transfers",
                            IconName::RefreshCw,
                            !self.history_tab,
                        )
                        .when(active > 0, |this| {
                            this.child(Chip::new(SharedString::from(format!("{active} active"))))
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.history_tab = false;
                            cx.notify();
                        })),
                    )
                    .child(
                        tab(
                            "files-history-tab",
                            "History",
                            IconName::Clock,
                            self.history_tab,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.history_tab = true;
                            cx.notify();
                        })),
                    )
                    .child(div().flex_1())
                    .when(!self.narrow, |this| {
                        this.child(
                            div()
                                .px(u(12.0))
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child("tar over exec · resumable chunks · sha256 verified"),
                        )
                    })
                    .when(self.history_tab, |this| {
                        this.child(
                            kubyl_ui::Button::new("files-clear-history")
                                .ghost()
                                .label("Clear")
                                .on_click(cx.listener(|_, _, _, cx| {
                                    TransferQueue::global(cx)
                                        .update(cx, |q, cx| q.clear_history(cx));
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .id("files-transfer-rows")
                    .flex_1()
                    .overflow_y_scroll()
                    .when(rows.is_empty(), |this| {
                        this.child(div().p(u(14.0)).text_color(colors.text_faint).child(
                            if self.history_tab {
                                "No finished transfers yet."
                            } else {
                                "Drag files between the panes, drop them from Finder, or press F5."
                            },
                        ))
                    })
                    .children(rows),
            )
            .into_any_element()
    }

    fn render_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let colors = cx.colors().clone();
        let overlay = self.overlay.as_ref()?;
        let (title, subtitle, body, edit): (String, String, AnyElement, bool) = match overlay {
            Overlay::Preview {
                side,
                entry,
                path,
                content,
            } => {
                let body = match content {
                    PreviewContent::Loading => div()
                        .p(u(16.0))
                        .text_color(colors.text_dim)
                        .child("Loading…")
                        .into_any_element(),
                    PreviewContent::Error(err) => div()
                        .p(u(16.0))
                        .text_color(colors.red)
                        .child(err.clone())
                        .into_any_element(),
                    PreviewContent::Binary => div()
                        .p(u(16.0))
                        .text_color(colors.text_dim)
                        .child("Binary file: no preview. Download it or open it in a terminal.")
                        .into_any_element(),
                    PreviewContent::Image(path) => div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(u(16.0))
                        .child(img(path.clone()).max_w_full().max_h_full())
                        .into_any_element(),
                    PreviewContent::Text { text, truncated } => div()
                        .id("files-preview-text")
                        .flex_1()
                        .overflow_y_scroll()
                        .p(u(12.0))
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .line_height(u(18.0))
                        .text_color(colors.text)
                        .whitespace_normal()
                        .child(text.clone())
                        .when(*truncated, |this| {
                            this.child(
                                div()
                                    .pt(u(8.0))
                                    .text_color(colors.text_faint)
                                    .child("… preview truncated"),
                            )
                        })
                        .into_any_element(),
                };
                (
                    entry.name.clone(),
                    format!("{} · {}", path, human_size(entry.size)),
                    body,
                    *side == Side::Pod
                        && !is_image(&entry.name)
                        && entry.size <= EDIT_LIMIT
                        && mounts::write_blocked(&self.mounts, &entry::parent(path)).is_none(),
                )
            }
            Overlay::Edit(session) => {
                let editor = Editor::new(&session.editor)
                    .h_full()
                    .bordered(false)
                    .font_family(fonts::MONO)
                    .text_size(u(13.0))
                    .bg(colors.background);
                let status = if session.saving {
                    "working…".to_string()
                } else if *session.editor.read(cx).text() != session.original {
                    "modified · ⌘S saves to the container".to_string()
                } else {
                    "saved".to_string()
                };
                let body = v_flex()
                    .flex_1()
                    .min_h_0()
                    .when_some(session.error.clone(), |this, err| {
                        this.child(
                            div()
                                .px(u(12.0))
                                .py(u(6.0))
                                .bg(colors.error_row_background)
                                .text_color(colors.red)
                                .child(err),
                        )
                    })
                    .child(div().flex_1().min_h_0().child(editor))
                    .into_any_element();
                (
                    entry::file_name(&session.path).to_string(),
                    format!("{} · {status}", session.path),
                    body,
                    false,
                )
            }
        };
        let editing = matches!(overlay, Overlay::Edit(_));
        let _ = window;
        Some(
            v_flex()
                .absolute()
                .inset_0()
                .bg(colors.background)
                .child(
                    h_flex()
                        .flex_none()
                        .h(u(38.0))
                        .px(u(12.0))
                        .gap(u(8.0))
                        .border_b_1()
                        .border_color(colors.border)
                        .bg(colors.panel)
                        .child(
                            Icon::new(if editing {
                                IconName::Code
                            } else {
                                IconName::Eye
                            })
                            .size(14.0)
                            .color(colors.accent),
                        )
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .font_family(fonts::MONO)
                                .child(title),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child(subtitle),
                        )
                        .when(edit, |this| {
                            this.child(
                                kubyl_ui::Button::new("files-preview-edit")
                                    .icon(IconName::Code)
                                    .label("Edit in place")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.overlay = None;
                                        this.edit_in_place(Side::Pod, window, cx);
                                    })),
                            )
                        })
                        .when(editing, |this| {
                            this.child(
                                kubyl_ui::Button::new("files-save")
                                    .primary()
                                    .label("Save")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_edit(false, window, cx)
                                    })),
                            )
                        })
                        .child(
                            IconButton::new("files-overlay-close", IconName::X)
                                .icon_size(13.0)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_overlay(window, cx)
                                })),
                        ),
                )
                .child(body)
                .into_any_element(),
        )
    }
}

fn this_machine() -> &'static str {
    if cfg!(target_os = "macos") {
        "This Mac"
    } else {
        "This computer"
    }
}

/// The toolbar dropdown button look (`.btn`, 24px): icon, label, chevron.
fn menu_button(
    id: &'static str,
    icon: IconName,
    label: impl Into<SharedString>,
    colors: &Colors,
) -> MenuButton {
    MenuButton::new(id).ghost().compact().p_0().child(
        h_flex()
            .h(u(24.0))
            .px(u(8.0))
            .gap(u(5.0))
            .rounded(u(5.0))
            .border_1()
            .border_color(colors.border)
            .bg(colors.button_background)
            .text_size(u(12.0))
            .text_color(colors.text)
            .child(Icon::new(icon).size(12.0))
            .child(label.into())
            .child(Icon::new(IconName::ChevronDown).size(11.0)),
    )
}

impl Focusable for FilesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.pod.focus.clone()
    }
}

impl TabView for FilesView {
    fn tab_title(&self, _: &App) -> SharedString {
        format!(
            "{} · files",
            short_pod(&self.target.name.clone().unwrap_or_default())
        )
        .into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Folder.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(self.request.clone())
    }
}

impl Render for FilesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let toolbar = self.render_toolbar(cx);
        let local = self.render_pane(Side::Local, window, cx);
        let pod = self.render_pane(Side::Pod, window, cx);
        let queue = self.render_queue(cx);
        let overlay = self.render_overlay(window, cx);
        let (first, second) = if self.swapped {
            (pod, local)
        } else {
            (local, pod)
        };
        let hints = KeyHints::new([
            ("enter".into(), "Open / preview".into()),
            ("e".into(), "Edit in place".into()),
            ("f5".into(), "Copy to other pane".into()),
            ("secondary-backspace".into(), "Delete in pod".into()),
            ("alt".into(), "drag: keep both".into()),
            ("tab".into(), "Other pane".into()),
            ("secondary-shift-d".into(), "Download to…".into()),
        ]);
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .on_action(cx.listener(|this, _: &SwitchPane, window, cx| {
                this.active = this.active.other();
                this.pane(this.active).focus.clone().focus(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SwapPanes, _, cx| {
                this.swapped = !this.swapped;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleHidden, _, cx| {
                this.show_hidden = !this.show_hidden;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| {
                this.refresh(Side::Local, cx);
                this.refresh(Side::Pod, cx);
            }))
            .on_action(
                cx.listener(|this, _: &UploadFiles, window, cx| this.upload_dialog(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &DownloadTo, window, cx| this.download_dialog(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &DeleteInPod, window, cx| this.delete_in_pod(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Chmod, window, cx| this.chmod(window, cx)))
            .on_action(cx.listener(|this, _: &CloseOverlay, window, cx| {
                if this.overlay.is_some() {
                    this.close_overlay(window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &SaveFile, window, cx| {
                if matches!(this.overlay, Some(Overlay::Edit(_))) {
                    this.save_edit(false, window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .child(toolbar)
            .child(
                div()
                    .flex()
                    .when(self.narrow, |this| this.flex_col())
                    .flex_1()
                    .min_h_0()
                    .child(first)
                    .child(second),
            )
            .child(queue)
            .when(!self.narrow, |this| this.child(hints))
            .children(overlay)
            // Measures the width for the narrow layout (applied from the next frame).
            .child(
                canvas(
                    {
                        let view = cx.weak_entity();
                        move |bounds, window, cx| {
                            let narrow =
                                bounds.size.width < u(NARROW_WIDTH).to_pixels(window.rem_size());
                            let view = view.clone();
                            cx.defer(move |cx| {
                                view.update(cx, |this, cx| {
                                    if this.narrow != narrow {
                                        this.narrow = narrow;
                                        cx.notify();
                                    }
                                })
                                .ok();
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
    }
}

/// Opens the browser for a pod (the `f` key on pods).
pub fn open(target: ResourceRef, window: &mut Window, cx: &mut App) {
    window.dispatch_action(
        Box::new(kubyl_core::actions::OpenView(ViewRequest::for_resource(
            kubyl_core::ViewKind::Files,
            target,
        ))),
        cx,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_pod_names() {
        assert_eq!(short_pod("checkout-api-7d9f8c6b5-x2kqp"), "x2kqp");
        assert_eq!(short_pod("ledger-writer-0"), "ledger-writer-0");
    }

    #[test]
    fn previews_detect_binary_and_truncation() {
        assert!(matches!(
            text_content(vec![0, 1, 2], 3, 10),
            PreviewContent::Binary
        ));
        match text_content(b"hello".to_vec(), 100, 5) {
            PreviewContent::Text { text, truncated } => {
                assert_eq!(text, "hello");
                assert!(truncated);
            }
            _ => panic!("text"),
        }
        assert!(is_image("logo.PNG"));
        assert!(!is_image("app.yaml"));
        assert_eq!(language_for("values.yml"), "yaml");
    }

    #[test]
    fn ages_are_compact() {
        let now = jiff::Timestamp::now();
        assert_eq!(age(Some(now)), "now");
        assert_eq!(age(Some(now - jiff::SignedDuration::from_hours(3))), "3h");
        assert_eq!(age(Some(now - jiff::SignedDuration::from_hours(72))), "3d");
        assert_eq!(age(None), "");
    }

    fn open_view(
        cx: &mut gpui::TestAppContext,
    ) -> (tempfile::TempDir, gpui::WindowHandle<FilesView>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("certs")).unwrap();
        std::fs::write(dir.path().join("a.yaml"), b"a: 1\n").unwrap();
        std::fs::write(dir.path().join("b.log"), b"line\n").unwrap();
        let config = dir.path().join(".config");
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, &config);
            kubyl_ui::init(cx);
            kubyl_settings::Settings::register::<FilesSettings>(cx);
            TransferQueue::install(cx);
        });
        let local = dir.path().to_path_buf();
        let window = cx.add_window(|window, cx| {
            let mut view = FilesView::new(None, window, cx);
            view.navigate_local(local, cx);
            view
        });
        cx.run_until_parked();
        (dir, window)
    }

    #[gpui::test]
    fn keyboard_cursor_walks_the_parent_row_and_entries(cx: &mut gpui::TestAppContext) {
        let (dir, window) = open_view(cx);
        window
            .update(cx, |view, _, cx| {
                let names: Vec<_> = view
                    .visible(Side::Local)
                    .iter()
                    .map(|e| e.name.clone())
                    .collect();
                assert_eq!(
                    names,
                    ["certs", "a.yaml", "b.log"],
                    "hidden files are filtered"
                );
                // `..` first, then the entries; the cursor never sticks on the parent row.
                view.move_cursor(Side::Local, 1, false, cx);
                assert_eq!(view.local.cursor.as_deref(), Some(PARENT));
                assert!(view.selection(Side::Local).is_empty());
                view.move_cursor(Side::Local, 1, false, cx);
                view.move_cursor(Side::Local, 1, false, cx);
                assert_eq!(view.local.cursor.as_deref(), Some("a.yaml"));
                view.move_cursor(Side::Local, 1, true, cx);
                let selected: Vec<_> = view
                    .selection(Side::Local)
                    .into_iter()
                    .map(|e| e.name)
                    .collect();
                assert_eq!(selected, ["a.yaml", "b.log"]);
                view.move_cursor(Side::Local, -3, false, cx);
                assert_eq!(view.local.cursor.as_deref(), Some(PARENT));
            })
            .unwrap();
        // Going up puts the cursor on the folder we came from.
        window
            .update(cx, |view, window, cx| {
                view.open_cursor(Side::Local, window, cx)
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, _| {
                assert_eq!(view.local_dir, dir.path().parent().unwrap());
                assert_eq!(
                    view.local.cursor.as_deref(),
                    dir.path().file_name().and_then(|n| n.to_str())
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn copies_without_conflicts_start_right_away(cx: &mut gpui::TestAppContext) {
        let (dir, window) = open_view(cx);
        // Runs inside the view's update: must not re-enter it (it used to panic).
        window
            .update(cx, |view, window, cx| {
                view.download(
                    vec![("/tmp/new.log".into(), false, 4)],
                    dir.path().to_path_buf(),
                    false,
                    window,
                    cx,
                )
            })
            .unwrap();
    }

    #[gpui::test]
    fn drag_out_takes_small_files_but_not_folders_or_secrets(cx: &mut gpui::TestAppContext) {
        let (_dir, window) = open_view(cx);
        window
            .update(cx, |view, _, cx| {
                view.mounts = vec![Mount {
                    path: "/app/secrets".into(),
                    source: MountSource::Secret,
                    object: Some("db".into()),
                    read_only: true,
                }];
                let drag = |dir: &str, entries: Vec<Entry>| DraggedEntries {
                    source: cx.entity_id(),
                    from: Side::Pod,
                    local_dir: PathBuf::new(),
                    pod_dir: dir.into(),
                    entries,
                };
                let mut log = Entry::new("app.log", EntryKind::File);
                log.size = 1024;
                assert!(
                    view.drag_out_blocked(&drag("/tmp", vec![log.clone()]))
                        .is_none()
                );
                let mut dump = log.clone();
                dump.size = DRAG_OUT_LIMIT + 1;
                assert!(view.drag_out_blocked(&drag("/tmp", vec![dump])).is_some());
                let folder = Entry::new("dumps", EntryKind::Dir);
                assert!(view.drag_out_blocked(&drag("/tmp", vec![folder])).is_some());
                let secret = view
                    .drag_out_blocked(&drag("/app/secrets", vec![log]))
                    .unwrap();
                assert!(secret.contains("Secret"), "{secret}");
            })
            .unwrap();
    }
}
