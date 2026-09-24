# Phase 06: Pod file browser, drag and drop transfers

**Status:** not started
**Depends on:** 05 (exec channel), 02
**Owns:** `crates/kubyl_files`
**Mockups:** board 9 · Pod file browser

## Goal

Browse a container's filesystem next to a local folder. Upload and download files and folders by
drag and drop, both inside the app and to and from Finder/Explorer/file managers, with a
resumable, verified transfer queue.

## Tasks

### Remote filesystem
- [ ] Listing via exec: prefer `ls -la --time-style=full-iso`, fall back to `find -printf` or a busybox `stat` loop. Parse into entries (name, type, size, mode, owner, mtime, symlink target)
- [ ] Capability probe per container: which of `tar`, `ls`, `find`, `stat`, `sha256sum`, `/bin/sh` exist. Shown as a chip in the toolbar
- [ ] Distroless fallback: offer an ephemeral debug container (`busybox`, sharing the target's process namespace) and access the target's files via `/proc/<pid>/root`
- [ ] Mark mount sources (from the pod spec): ConfigMap, Secret, PVC, emptyDir, and read-only mounts. Warn before writing to ConfigMap/Secret mounts (changes are lost)

### Transfers
- [ ] Download: `tar cf - <paths>` streamed over exec stdout, extracted locally on the fly. Single files can use `cat`. Large files are split into chunks with `dd`, so transfers can resume
- [ ] Upload: local tar stream into `tar xf - -C <dir>` via exec stdin. Keep mode bits where possible
- [ ] Verification: `sha256sum` remotely when available, compared locally. Mark "verified" / "unverified"
- [ ] Transfer queue (bottom panel): direction, name, progress, speed, ETA, cancel, retry, history tab. Limited concurrency per pod
- [ ] Conflicts: overwrite / keep both / skip, "apply to all". ⌥-drag = keep both (as in the mockup)

### UI
- [ ] Two-pane commander: local pane (any local folder, bookmarks) and pod pane (container picker, path breadcrumb, hidden files toggle). F5 copies to the other pane, swap panes
- [ ] Drag inside the app between panes, with a drop-zone highlight and a summary ("Drop to upload 3 items into /app/config")
- [ ] **Drop from the OS** onto the pod pane (GPUI `ExternalPaths` drop support)
- [ ] **Drag out to the OS** to download: needs per-platform native drag sources (macOS `NSFilePromiseProvider`, Windows `IDataObject` with `CFSTR_FILEDESCRIPTOR`/`FILECONTENTS`, Linux XDND/Wayland `text/uri-list` with a temp-file fallback). GPUI doesn't provide this. Implement it behind a `platform_drag_out` module and upstream it if possible
- [ ] Quick preview (text/images) and "edit in place": download to a temp file, open in the built-in editor, upload on save with a conflict check on mtime/hash
- [ ] Actions: new folder, rename, delete (with confirmation), chmod, copy path

## Acceptance criteria

- Drag a folder with 100 files from Finder onto `/app/config`: uploaded and verified.
- Drag a 400 MB heap dump from the pod pane onto the desktop: streams with progress and resumes after a network blip.
- Writing to a Secret mount is blocked with a clear error. A distroless pod works via the debug container.

## Risks

- Drag-out to the OS is the hardest part (native APIs on three platforms). If it slips, ship "Download to…" and an in-app drag first, and track drag-out as a follow-up.

## Handoff log
