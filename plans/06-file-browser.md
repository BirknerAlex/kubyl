# Phase 06: Pod file browser, drag and drop transfers

**Status:** in progress: all tasks done except full drag-out to the OS (a staged version for small files ships; see Handoff log)
**Depends on:** 05 (exec channel), 02
**Owns:** `crates/kubyl_files`
**Mockups:** board 9 · Pod file browser

## Goal

Browse a container's filesystem next to a local folder. Upload and download files and folders by
drag and drop, both inside the app and to and from Finder/Explorer/file managers, with a
resumable, verified transfer queue.

## Tasks

### Remote filesystem
- [x] Listing via exec: prefer `ls -la --time-style=full-iso`, fall back to `find -printf` or a busybox `stat` loop. Parse into entries (name, type, size, mode, owner, mtime, symlink target)
- [x] Capability probe per container: which of `tar`, `ls`, `find`, `stat`, `sha256sum`, `/bin/sh` exist. Shown as a chip in the toolbar
- [x] Distroless fallback: offer an ephemeral debug container (`busybox`, sharing the target's process namespace) and access the target's files via `/proc/<pid>/root`
- [x] Mark mount sources (from the pod spec): ConfigMap, Secret, PVC, emptyDir, and read-only mounts. Warn before writing to ConfigMap/Secret mounts (changes are lost)

### Transfers
- [x] Download: `tar cf - <paths>` streamed over exec stdout, extracted locally on the fly. Single files can use `cat`. Large files are split into chunks with `dd`, so transfers can resume
- [x] Upload: local tar stream into `tar xf - -C <dir>` via exec stdin. Keep mode bits where possible
- [x] Verification: `sha256sum` remotely when available, compared locally. Mark "verified" / "unverified"
- [x] Transfer queue (bottom panel): direction, name, progress, speed, ETA, cancel, retry, history tab. Limited concurrency per pod
- [x] Conflicts: overwrite / keep both / skip, "apply to all". ⌥-drag = keep both (as in the mockup)

### UI
- [x] Two-pane commander: local pane (any local folder, bookmarks) and pod pane (container picker, path breadcrumb, hidden files toggle). F5 copies to the other pane, swap panes
- [x] Drag inside the app between panes, with a drop-zone highlight and a summary ("Drop to upload 3 items into /app/config")
- [x] **Drop from the OS** onto the pod pane (GPUI `ExternalPaths` drop support)
- [ ] **Drag out to the OS** to download: needs per-platform native drag sources (macOS `NSFilePromiseProvider`, Windows `IDataObject` with `CFSTR_FILEDESCRIPTOR`/`FILECONTENTS`, Linux XDND/Wayland `text/uri-list` with a temp-file fallback). GPUI doesn't provide this. Implement it behind a `platform_drag_out` module and upstream it if possible
- [x] Quick preview (text/images) and "edit in place": download to a temp file, open in the built-in editor, upload on save with a conflict check on mtime/hash
- [x] Actions: new folder, rename, delete (with confirmation), chmod, copy path

## Acceptance criteria

- Drag a folder with 100 files from Finder onto `/app/config`: uploaded and verified.
- Drag a 400 MB heap dump from the pod pane onto the desktop: streams with progress and resumes after a network blip.
- Writing to a Secret mount is blocked with a clear error. A distroless pod works via the debug container.

## Risks

- Drag-out to the OS is the hardest part (native APIs on three platforms). If it slips, ship "Download to…" and an in-app drag first, and track drag-out as a follow-up.

## Handoff log

### 2026-09-25 (branch `phase/05-finish-06-file-browser`)

Everything but full drag-out to the OS is done and checked against the kind dev cluster. The
browser opens with `f` on a pod (or "Pod: Browse Files"); tabs restore from `state.json`.

**Verification.** `cargo test --workspace`, clippy (`--all-targets`), fmt and `cargo deny` pass.
`kubyl_files` has 27 unit/GPUI tests (listing parsers for find/stat/GNU and busybox `ls`, mode
strings, mounts, keep-both names, tar channels, the pane cursor, conflict-free copies, drag-out
rules). `kubyl_files/tests/live.rs` (ignored; `KUBYL_TEST_KUBECONFIG=… cargo test -p kubyl_files
--test live -- --ignored`) runs against a busybox pod with ConfigMap, Secret and emptyDir mounts
and a `pause` pod: capability probe, mounts, a 100-file folder upload (verified, owned by the
container user), a 20 MB chunked download plus resume from a partial file, a folder download,
and the distroless path through a debug container.

Screenshot runs (`--features screenshot`) covered the UI against board 9 and the acceptance
criteria:
- Two panes (This Mac with bookmarks · the container with breadcrumb, container picker,
  capability chip "tar available · /bin/sh", mount chips "ConfigMap · read-only" /
  "Secret · read-only"), keyboard navigation (`..` row, shift-select, Tab, Backspace), hints bar.
- A folder of 100 files dropped from the OS onto `/app/config`: uploaded and verified. The
  harness has a new `filedrop=x:y:/path` step (and `drag=`/`drop=` for in-app drags).
- F5 from the pod with a name clash: conflict dialog (Skip / Keep both / Overwrite, "Apply to the
  N other conflicts", Enter = keep both); keep-both names "a (1).yaml". Three files dragged from
  the local pane (ghost with count badge, dashed drop zone, "Drop to upload 3 items into /tmp",
  ⌥ hint), two conflicts resolved at once, all verified.
- A 400 MB heap dump downloads at ~84 MB/s with percent, speed and ETA; the partial file
  `.name.kubyl-part` resumes (live test). Writing into the ConfigMap mount is refused with
  "read-only mount (ConfigMap checkout-config): change the ConfigMap instead" (toast + failed row
  in the queue, reason in a tooltip).
- Preview (text, images via a temp file), edit in place with YAML highlighting: ⌘S saves into
  the container; a file changed in the container meanwhile asks before overwriting.
- A `pause` pod: "no shell" state, "Browse through a debug container" lists the target's root
  through `kubyl-files-*` (busybox with SYS_PTRACE, reused while it runs).
- New folder (F7), rename (F2), delete with confirmation (⌘⌫, typed name on production), chmod,
  copy path.

**Drag out to the OS (follow-up).** GPUI now has `external_drag_payload`: when an in-app drag
leaves the window it can hand existing local files to the OS (macOS, Wayland; not Windows/X11,
no file promises). The browser stages dragged pod files in a temp folder when the drag starts
(regular files up to 32 MB, not from Secret mounts; downloads rename into place, so the OS never
sees a partial file) and offers them once complete. If they aren't ready yet, or the drag holds
folders or large files, a toast points to "Download to…" (⌘⇧D). The harness confirmed the
payload is built (its synthetic events can't start a real macOS drag session); the final drop
into Finder still needs a manual check with a real mouse. Missing for the full task: folders and
large files (needs `NSFilePromiseProvider` / `CFSTR_FILEDESCRIPTOR` / XDND in GPUI), Windows and
X11.

**Bugs found on the way.**
- GPUI's OS file drops (`FileDropEvent`) become mouse events without setting the input modality,
  so after any key press `on_drop` hit-testing sees nothing hovered and the drop is lost. The pod
  pane takes OS drops with a raw capture listener; the workspace's kubeconfig drop
  (`crates/kubyl/src/workspace`) still has this problem. Worth reporting upstream.
- Enter in a gpui-component dialog also triggers the dialog's Confirm binding, which closed
  `kubyl_explorer::dialogs` prompts before their input's `PressEnter` arrived (Enter cancelled
  every `prompt_text`). Fixed in `dialogs::open` (`on_ok` returns false; the views submit).

**Shared-crate changes** (own commit): screenshot harness steps `drag=`, `drop=`, `filedrop=`
(`crates/kubyl/src/app.rs`); the dialog Enter fix in `kubyl_explorer`; `script/dev-cluster.sh`
mounts `checkout-config` (ConfigMap, `/app/config/flags`), `checkout-db` (Secret, read-only,
`/app/config/secrets`) and an emptyDir into `checkout-api`, as on board 9.

**Deviations from board 9.** The editor for "edit in place" is an overlay over the browser (not
a separate tab). Upload/download progress for folders counts bytes against `du`/local sizes, so
percentages of tar streams are approximate. The queue lives in the browser, not the bottom dock.

### 2026-09-25 (later): Files in the details pane

The browser is also a "Files" sub-tab of pod details (dock and tab). Below 760 px wide (the
dock) it switches to a narrow layout, measured each frame: the panes stack, only name and size
columns, icon buttons, a smaller queue, no hints bar.

