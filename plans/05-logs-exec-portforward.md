# Phase 05: Logs, exec terminal, port-forwarding

**Status:** not started
**Depends on:** 02
**Owns:** `crates/kubyl_logs`, `crates/kubyl_terminal`, `crates/kubyl_portforward`
**Mockups:** board 2 · Live logs, exec shell, port-forwards

## Goal

Live logs across pods and containers with fast search, a real terminal into containers, and a
port-forward manager. All of them appear in one "Active sessions" panel.

## Tasks

### Logs (`kubyl_logs`)
- [ ] Sources: a single pod/container, all containers of a pod, and all pods of a workload (Deployment/StatefulSet/DaemonSet/Job, or any label selector). New pods join automatically and deleted pods are marked
- [ ] Options: follow, since (duration or time), tail lines, timestamps, previous container, wrap, init/ephemeral containers
- [ ] Virtualized log view in a ring buffer (default 100k lines, configurable), 60 fps at 5k lines/s. Per-pod color prefix
- [ ] Level detection (JSON `level`/`severity`, logfmt, common text patterns). Level filter chips with counts
- [ ] Search: text/regex, case toggle, match count, next/prev, highlight, "filter to matches" mode
- [ ] JSON pretty/inline toggle with key highlighting. Click a JSON field to add it as a filter
- [ ] Pause (the buffer keeps filling, a counter shows new lines), jump to bottom, copy selection, download (visible or full) to a file
- [ ] Reconnect with backoff on stream errors, with a gap marker in the view

### Exec terminal (`kubyl_terminal`)
- [ ] `alacritty_terminal` backend with a GPUI renderer (glyph atlas via GPUI text system, 256 colors and true color, selection, scrollback, links, bracketed paste, resize → `TerminalSize` over the exec channel)
- [ ] Exec via kube `AttachedProcess` (websocket). Shell auto-detection (`/bin/bash` → `/bin/sh` → `sh`) with a manual override
- [ ] Attach to a running process (`kubectl attach`) and ephemeral debug containers (`kubectl debug` with a chosen image, target container) for distroless pods
- [ ] Node shell (privileged debug pod on a node, behind a confirmation, disabled on read-only clusters)
- [ ] Terminal tabs in the bottom dock and as editor tabs. Split terminals

### Port-forwarding (`kubyl_portforward`)
- [ ] Forward from a Pod, Service (resolve targetPort → pod, re-resolve when the pod dies) or Deployment. Pick local port or auto. Bind address 127.0.0.1 by default
- [ ] Manager: list, connection count, bytes, reconnect state, stop. Persist favorite forwards, optionally auto-start them when the cluster connects
- [ ] "Open in browser" for HTTP ports

### Active sessions panel
- [ ] Right dock listing log streams, terminals, port-forwards and watches with status and a stop button (as in the mockup)
- [ ] Status-bar counters (watches, forwards)

## Acceptance criteria

- Logs from a 3-replica deployment stream interleaved in order. Killing a pod shows its
  replacement joining. Searching "timeout" highlights and counts matches.
- `vim` and `htop` render correctly in an exec terminal. Resizing works.
- A port-forward to a Service survives a pod restart.

## Handoff log
