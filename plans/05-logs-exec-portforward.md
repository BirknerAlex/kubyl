# Phase 05: Logs, exec terminal, port-forwarding

**Status:** done (2026-09-25; verified against the `kubyl-dev` kind cluster with live tests and
screenshots, see the Handoff log)
**Depends on:** 02
**Owns:** `crates/kubyl_logs`, `crates/kubyl_terminal`, `crates/kubyl_portforward`
**Mockups:** board 2 · Live logs, exec shell, port-forwards

## Goal

Live logs across pods and containers with fast search, a real terminal into containers, and a
port-forward manager. All of them appear in one "Active sessions" panel.

## Tasks

### Logs (`kubyl_logs`)
- [x] Sources: a single pod/container, all containers of a pod, and all pods of a workload (Deployment/StatefulSet/DaemonSet/Job, or any label selector). New pods join automatically and deleted pods are marked
- [x] Options: follow, since (duration or time), tail lines, timestamps, previous container, wrap, init/ephemeral containers
- [x] Virtualized log view in a ring buffer (default 100k lines, configurable), 60 fps at 5k lines/s. Per-pod color prefix
- [x] Level detection (JSON `level`/`severity`, logfmt, common text patterns). Level filter chips with counts
- [x] Search: text/regex, case toggle, match count, next/prev, highlight, "filter to matches" mode
- [x] JSON pretty/inline toggle with key highlighting. Click a JSON field to add it as a filter
- [x] Pause (the buffer keeps filling, a counter shows new lines), jump to bottom, copy selection, download (visible or full) to a file
- [x] Reconnect with backoff on stream errors, with a gap marker in the view

### Exec terminal (`kubyl_terminal`)
- [x] `alacritty_terminal` backend with a GPUI renderer (glyph atlas via GPUI text system, 256 colors and true color, selection, scrollback, links, bracketed paste, resize → `TerminalSize` over the exec channel)
- [x] Exec via kube `AttachedProcess` (websocket). Shell auto-detection (`/bin/bash` → `/bin/sh` → `sh`) with a manual override
- [x] Attach to a running process (`kubectl attach`) and ephemeral debug containers (`kubectl debug` with a chosen image, target container) for distroless pods
- [x] Node shell (privileged debug pod on a node, behind a confirmation, disabled on read-only clusters)
- [x] Terminal tabs in the bottom dock and as editor tabs. Split terminals

### Port-forwarding (`kubyl_portforward`)
- [x] Forward from a Pod, Service (resolve targetPort → pod, re-resolve when the pod dies) or Deployment. Pick local port or auto. Bind address 127.0.0.1 by default
- [x] Manager: list, connection count, bytes, reconnect state, stop. Persist favorite forwards, optionally auto-start them when the cluster connects
- [x] "Open in browser" for HTTP ports

### Active sessions panel
- [x] Right dock listing log streams, terminals, port-forwards and watches with status and a stop button (as in the mockup)
- [x] Status-bar counters (watches, forwards)

## Acceptance criteria

- Logs from a 3-replica deployment stream interleaved in order. Killing a pod shows its
  replacement joining. Searching "timeout" highlights and counts matches.
- `vim` and `htop` render correctly in an exec terminal. Resizing works.
- A port-forward to a Service survives a pod restart.

## Handoff log

### 2026-09-25 (completion, branch `phase/05-finish-06-file-browser`)

An audit of `main` found that none of the 18 tasks was complete (several features existed only
as library code, most log-view actions had no binding, `s` bypassed read-only clusters, 256
colors rendered wrong). This entry supersedes the gap lists in the older entries below. Every
task is now done and checked against the dev cluster.

**Verification.** `cargo test --workspace` (264 tests), clippy, fmt and `cargo deny` pass.
Live tests, ignored by default, run against kind with `KUBYL_TEST_KUBECONFIG` (see the header of
each file):
- `kubyl_logs/tests/live.rs`: a 3-replica deployment's backlog arrives interleaved in timestamp
  order with live lines from every pod; a killed pod leaves and its replacement joins.
- `kubyl_terminal/tests/live.rs`: exec with a TTY resize (`stty size` answers `30 100`), an
  ephemeral debug container sharing the target's processes, a node shell (privileged pod +
  `nsenter`) whose pod is deleted afterwards.
- `kubyl_portforward/tests/live.rs`: a Service forward keeps answering on the same local port
  after its only pod was deleted and replaced.

Screenshots (`--features screenshot`, isolated `HOME` with the kind kubeconfig) confirmed the log
view (interleaved pods, level chips with counts, ERROR rows, search "timeout" highlighted with
"144 of 147", inline and pretty JSON, clicking a JSON key adds a `level=info` filter, pause
counter), `vi` and busybox `top` in an exec tab (full-height grid, inverse header), `s` opening
the Terminal panel in the bottom dock with a working shell, the Port-Forward and Debug Container
dialogs and the Active Sessions panel with a forward and seven watches. `script/dev-cluster.sh`
now deploys `checkout-events` (3 replicas, JSON and text logs at every level) for this. htop
isn't in any dev-cluster image; `top` exercises the same inverse/cursor paths.

**Shared-crate changes** (own commits): `kubyl_core::actions::ActivateDockPanel(id)` (the
workspace shows the dock holding that panel and focuses it); `ResourceStores::watches`,
`ResourceStore::pause/resume` and `StoreStatus::Paused` in `kubyl_resources` (the explorer list
shows "paused"); the Details pane's logs sub-tab follows `kubyl_logs::logs_applicable`
(ReplicaSets and Services too).

#### Logs
- Selector sources (workloads, ReplicaSets, Services, or a selector typed via the container menu's
  "Label selector…") watch their pods with `kube::runtime::watcher`: pods join when their
  containers start, deleted pods leave (chip dimmed and struck through, marker row).
- Timestamps are always requested and split off the text: level detection and JSON work with
  timestamps shown, reconnects resume at `sinceTime` and skip duplicates. The initial backlog of
  all containers is fetched non-following and merged by timestamp before live lines start.
  Containers of finished pods end ("exited with code 0") instead of reconnecting forever.
- Options: Follow (API follow + tail-follow scrolling), Timestamps (default on), Wrap, Previous,
  JSON (raw / inline / pretty), since menu (last N lines, last minutes/hours, a local time,
  everything), container menu (one container, init and ephemeral containers).
- The list is a `gpui::list` with `FollowMode::Tail`, spliced incrementally, so wrapped lines and
  pretty JSON get variable heights. Ingest benchmark: 5,040 lines in 60 frame batches cost
  0.06 ms per frame (release), 0.4 ms (debug); rendering only touches visible rows.
- Search highlights matches (current one stronger), counts "N of M", Enter/Shift-Enter and n/N
  scroll to matches, `.*` and `Aa` toggles, filter to matches. Row click/shift-click selects,
  ⌘C copies; download visible lines, or the complete log fetched from the API, to a file.
- All view actions are in the palette ("Logs: …") and bound: single letters in the `LogList`
  context (so typing in the search box doesn't trigger them), ⌘F/⌘S/⌘⇧S/⌥⌘X/⌥⌘C in `LogsView`.

#### Terminal
- The renderer paints a cell grid on a canvas: cell width is the advance of `m` from GPUI's text
  system and glyph runs are shaped with that width forced, so columns line up at any zoom.
  Full xterm palette (plus OSC 4 changes), inverse/dim/hidden/italic/underline/strikeout, wide
  characters, cursor shapes and visibility, scrollback indicator.
- Mouse selection (double/triple click for words/lines) and copy, wheel scrollback (alternate
  scroll on the alternate screen, reports in mouse mode), ⌘/Ctrl-click links (URLs and OSC 8),
  SGR/X10 mouse reporting, bracketed paste, xterm key encoding.
- **Key policy:** gpui-component's `Root` binds `ctrl-c` (copy) and `tab`, and Linux/Windows map
  `secondary-*` to Ctrl, so control keys, tab, escape and a few alt keys are bound to
  `terminal::SendKeystroke` in the `TerminalView` context. Copy/paste are ⌘C/⌘V on macOS and
  Ctrl-Shift-C/V elsewhere. Later phases that embed terminals get this for free.
- Sessions: exec (container and shell pickers in the header, reconnect), attach (`a`, follows
  the container's tty/stdin), ephemeral debug containers (`shift-d`, dialog: image, process
  sharing target, command), node shells (`s` on Nodes: typed confirmation on PROD, privileged
  pod in `terminal.node_shell_namespace`, deleted when the session ends and after 12 h at the
  latest). All handlers re-check read-only; `s` only applies to Pods.
- Terminals open in the bottom-dock Terminal panel (tabs, `+`, split side by side);
  `terminal.open_in = "tab"` or `alt-s` opens exec shells as editor tabs (restored on start).
  `` ctrl-` `` shows the panel.

#### Port-forwarding and sessions
- `shift-f` opens a dialog: the target's TCP ports (container or Service ports with their target,
  HTTP ones marked), a custom port for pods that declare none, local port (auto or fixed), bind
  address, open in browser, save, start on connect. `alt-shift-f` forwards the first port
  directly.
- Forwards are listening, reconnecting (target doesn't resolve; the port stays open and a probe
  re-resolves every 4 s) or failed (bind error, with a toast). Terminating pods are never picked.
- Saved forwards (`state.json` `port_forwards`, keyed by context + server + file like explorer
  favorites) start when their cluster connects; "Port Forward: Saved Forwards…" lists them.
- Active Sessions: header count, status-colored icons, per-row buttons (open in browser for HTTP
  ports, copy address, save/forget), details on their own line, resource watches with
  pause/resume. The status bar counts logs, shells, forwards and watches and opens the panel.

#### Deviations and limits
- Board 2 shows logs and the terminal in one layout; here the log view is an editor tab and the
  Terminal panel sits in the bottom dock, which gives the same arrangement. The log toolbars wrap
  onto a second line at narrow widths instead of clipping.
- Processes started in an exec session keep running after the websocket closes (a Kubernetes
  behavior `kubectl exec` shares). Ephemeral containers stay in the pod until it is deleted.
- Paused watches show their last objects and "paused" in the resource list until resumed.

### 2026-09-25 (CodeRabbit review fixes, PR #1)

Fixed all 20 CodeRabbit findings from the PR #1 review (`kubyl_logs`, `kubyl_portforward`,
`kubyl_terminal`); none were stale. Full workspace `cargo fmt --all`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace` and `cargo deny check` all pass after the
combined change. Not run against a live cluster in this environment.

**`kubyl_logs`**
- `view.rs`: `LogsView::start` now holds real stop state and registers `cx.on_release` so closing
  the tab (not just clicking "stop" in Active Sessions) stops the stream task and removes the
  `SessionRegistry` row.
- `json.rs`: `FieldFilter::matches` only falls back to substring matching for lines that are *not*
  valid JSON; a valid-JSON line simply missing the filtered field no longer matches by
  coincidence. Added a unit test.
- `stream.rs`: `workload_loop` no longer returns immediately (dropping `known` and aborting
  container tasks) when `follow=false`; non-follow workload views now actually collect and
  deliver lines before returning.
- `stream.rs`: the reconnect loop now tracks the last-seen timestamp and passes it as
  `since_time`/`since_seconds` on retry, and only resets backoff after data is actually received
  (not merely on connection open) — fixes full-log replay loops against terminated containers.
- `view.rs`: `apply_events` no longer unconditionally stomps a "reconnecting"/error status set
  earlier in the same batch with "streaming", and only calls `SessionRegistry::set_status` when
  the status actually changed.
- `view.rs`/`ring.rs`/`search.rs`: replaced full-ring rescans on every batch/frame with
  incremental rebuild caches (`rebuild_matches_and_rendered`, `rebuild_rendered_cache`,
  `rebuild_visible_cache`) so search/level counts/rendered lines aren't recomputed from scratch
  ~60x/sec; a proportionate fix, not a full incremental-index rewrite.
- `view.rs`: `recompute_search` only resets `MatchCursor` to 0 when the query/filter actually
  changes, not on every data batch, so Next/Prev navigation survives streaming updates.
- `view.rs`: `resolve_source` (new `label_selector_to_string` helper) now builds the selector from
  both `matchLabels` and `matchExpressions` (In/NotIn/Exists/DoesNotExist), so workloads using
  only `matchExpressions` no longer stream every pod in the namespace. Added unit tests.
- `view.rs`: `uniform_list` still assumes fixed row height (true variable-height rows need
  `gpui::list`/`ListState`, out of scope for this pass); as a proportionate fix, `pretty_json`
  rendering now uses the flat `key=value` form instead of multi-line pretty JSON so rows never
  span multiple lines and clip/overlap. `wrap` mode is unchanged and still a known gap.

**`kubyl_portforward`**
- `lib.rs`/`resolve.rs`: `RemotePort::Container`/`Service` are now `Option<u16>`; `None` means
  "first port", resolved from the pod's first container port or `match_service_port(..., None)`
  for Services, instead of a hardcoded 8080/80 default.
- `listener.rs`: persistent `accept()` errors (e.g. EMFILE) now back off briefly between retries
  instead of spinning in a tight loop and flooding the event channel.
- `listener.rs`: accepted connections are tracked in a `tokio::task::JoinSet` owned by the
  listener loop, so stopping a forward aborts in-flight `copy_bidirectional` connections instead
  of leaving them detached and running.
- `manager.rs`: the spawned listener task's `Result` is now awaited/handled, so a `bind()` failure
  moves the session to an error status instead of leaving it stuck in "starting" forever.
- `manager.rs`: the event loop now drains all currently-ready events (non-blocking) after each
  wait before processing, instead of sleeping 100ms per single event, fixing the ~10 events/sec
  throughput cap under bursts.
- `resolve.rs`: Service and workload selector resolution now build from both `matchLabels`/
  `spec.selector` and `matchExpressions`, and return an explicit error instead of an empty-string
  selector (which `ListParams::labels("")` treats as "match everything") when no selector
  information is present at all — no more forwarding to a random unrelated pod. Added unit tests
  for the `matchExpressions` and empty-selector cases.

**`kubyl_terminal`**
- `grid.rs`: `NullListener` (renamed `PtyWriteListener`) now buffers `Event::PtyWrite` bytes
  (cursor position DSR, device attribute/color query replies) instead of dropping them;
  `TerminalGrid::take_pty_writes()` drains the buffer and `view.rs`'s output loop forwards it
  back over `input_tx`/stdin, so vim/fish no longer stall on unanswered terminal queries.
- `shell.rs`: added `shell::detect` (async, off the UI thread) which execs `probe_command()` per
  candidate via a real `Api::exec` + `take_status()` check before falling back to `pick`; `sh`
  is no longer the default for every session.
- `exec.rs`: added `resolve_container` (fetches the Pod, checks the
  `kubectl.kubernetes.io/default-container` annotation, falls back to `spec.containers[0]`) called
  from `view.rs`'s `start()` before building `ExecTarget` — multi-container pods no longer hit "a
  container name must be specified".
- `exec.rs`/`view.rs`: `exec::run` now takes a `connected: oneshot::Sender<Result<(), String>>`,
  signaled right after the exec/attach call resolves. `view.rs` only marks the session/status
  "connected" once that fires `Ok(())`, and surfaces the real error message (RBAC, missing pod,
  …) instead of a bare "disconnected" when it fires `Err`.
- `view.rs`: the measurement `canvas()` now has `.absolute().top_0().left_0().size_full()` (same
  pattern as `kubyl_yaml/src/ui.rs`'s overlay canvas) so `maybe_resize` sees real bounds instead
  of ~0 height collapsing the terminal to 2 rows.

### 2026-09-25

Implemented all three crates end to end with real (not stubbed) kube integration, wired into
their own `init(cx)` per the registries in `kubyl_core`. No live cluster was available in this
environment, so **none of the three acceptance criteria were run against a real cluster** — they
are unverified. Everything below is unit-tested where the logic doesn't need a cluster
(`cargo test --workspace`: 210 tests, 0 failures; `cargo clippy --workspace --all-targets -- -D
warnings` and `cargo deny check` both clean).

**Cross-crate decision:** `kubyl_terminal` and `kubyl_portforward` both depend on `kubyl_logs`
for a new `kubyl_logs::sessions::SessionRegistry` global (add/update-status/stop), which backs
the "Active Sessions" right-dock panel and the status-bar counters in `kubyl_logs::dock`. This
was necessary because the phase asks for *one* combined panel/counters but the phase's hard rule
only lets this session touch these three crates — `kubyl_logs` initializes first in
`crates/kubyl/src/main.rs`'s list, so it's a safe crate to own the shared registry. Not
architecturally ideal (a `kubyl_sessions` crate would be cleaner) but avoids touching
`kubyl_core`/`main.rs`.

**Watches are not in the Active Sessions panel or its status-bar counters.** `kubyl_resources`
owns the watch caches (`ResourceStores`) and this phase isn't allowed to touch that crate, and it
can't depend on `kubyl_logs` (phase 02 can't depend on phase 05). Surfacing watch counts needs
either a small `kubyl_resources` change (a phase-02-owned counter global) or moving
`SessionRegistry` into `kubyl_core`, in a follow-up PR that lands first.

#### `kubyl_logs`
Done: ring buffer (`ring.rs`, default 100k lines, configurable via the new `"logs"` settings
section), level detection (JSON/logfmt/bracketed-text, `level.rs`), text/regex search with a
match cursor (`search.rs`), JSON parse/pretty/inline/field-filter helpers (`json.rs`), per-pod
color hashing (`line.rs`). Streaming (`stream.rs`) supports single-pod (all containers) and
workload (label-selector) sources with per-container reconnect+backoff and a gap-marker event;
**workload pod membership is polled every 4s, not watched** (kube's `watcher::Event` enum shape
is version-sensitive and polling avoided that risk pool — the acceptance criterion "killing a pod
shows its replacement joining" should still pass, just with a few seconds of lag). The single
*container* source (`LogSource::Container`) exists in `stream.rs` but has no UI entry point — only
"Pod" (all containers) and workload sources are reachable from the `l` action.

Missing from the view (`view.rs`): since/tail-lines/previous-container/init-container UI controls
(the settings/stream plumbing exists — `LogOptions` has the fields — but nothing sets `since`,
`previous` or exposes per-view tail-line/timestamps overrides); search match highlighting inside
the line text (navigation and the count work, the matched substring isn't visually marked);
inline JSON mode and click-a-field-to-filter (the `json` module has both, the view only wires the
pretty/raw toggle); jump-to-bottom (action declared, not wired — no scroll handle yet); download
to file (not implemented, `CopyVisible` copies all currently-filtered lines to the clipboard
instead). Virtualization uses `gpui::uniform_list`; 60fps-at-5k-lines/s is architecturally
plausible (batched at ~60Hz, only visible rows render) but was never benchmarked.

#### `kubyl_terminal`
Done: a real `alacritty_terminal::Term` driving a pure, well-tested grid snapshot (`grid.rs`:
ANSI/SGR parsing, cursor movement, 16/256/true-color resolution, bold); keystroke -> PTY-byte
encoding (`input.rs`: printables, arrows, ctrl-combos, IME text); the kube exec/attach bridge
(`exec.rs`, real websocket I/O via `AttachedProcess`, off the UI thread); resize sends
`TerminalSize` over the exec channel via `AttachedProcess::terminal_size()`.

Missing/deferred (this is the biggest gap in the phase): the renderer draws grid rows as GPUI
text spans (one span per contiguous same-style run), which is *not* a glyph-atlas renderer — no
mouse text selection, no scrollback UI (alacritty's `Term` has scrollback; nothing exposes it),
no hyperlink detection, and bracketed-paste is implemented (`input::bracketed_paste`) but never
called from an actual paste event handler. Shell auto-detection (`shell.rs`) now execs
`probe_command()` for each candidate off the UI thread (`shell::detect`, wired from `view.rs`'s
`start()` via `spawn_kube`) before falling back to `pick`'s pure decision — bash-first fallback
works unless `terminal.shell_override` is set. `exec::Mode::Attach` exists and is exercised nowhere
(only `Mode::Exec` is wired, behind the `s` action). Ephemeral debug containers and node shell are
not implemented at all — no debug-pod spec builder, no confirmation dialog, no read-only-cluster
gate. Terminals open via the generic `ViewKind::Terminal` factory; there's no bottom-dock-specific
routing or split-terminal support. Resize-to-container-size uses a fixed approximate
7.8x18px-per-cell heuristic (a `canvas()` prepaint callback measures the tab's bounds), not real
glyph metrics from GPUI's text system — `vim`/`htop` alignment at unusual font sizes may drift.

#### `kubyl_portforward`
Done: `resolve.rs` (unit-tested pure matching/port-resolution logic) resolves Pod, Service
(`spec.ports[].targetPort`, numeric or named, against the chosen pod's container ports) and
Deployment/StatefulSet/DaemonSet (via their pod-template selector) targets to a live pod, and is
re-run for *every new local TCP connection* — so a Service forward transparently lands on a new
pod after the old one dies, without any separate "reconnect" machinery. The local listener
(`listener.rs`) binds `127.0.0.1` (or the given bind address) on a fixed or OS-picked (`0`) port
and proxies bytes bidirectionally per connection via `tokio::io::copy_bidirectional`.
`manager.rs` tracks connection count and bytes sent/received per forward and reports them as the
session's status string in the shared Active Sessions panel (so "list, connection count, bytes,
stop" all work there); `shift-f` on a Pod/Service/Deployment/StatefulSet/DaemonSet starts a
forward using the first container port (Pod/workload) or first Service port (Service, no
explicit port requested) and an auto-assigned local port — **there's no target/port picker
dialog yet**, which is the main missing piece for real usability.

Missing: `favorites.rs` (the persisted-favorites data model, `state.json` key
`"port_forwards"`) is fully unit-tested but **not wired into `init(cx)` at all** — nothing saves
a forward as a favorite, nothing loads or auto-starts them on cluster connect. "Reconnect state"
isn't tracked (a broken forward's connections just error out; the listener itself never retries
with backoff the way `kubyl_kube::ConnectionManager` does for cluster connections). "Open in
browser" is a single global action (`Port Forward: Open Last in Browser`, `secondary-alt-o`) that
opens the most recently started forward's URL — not a per-row button, and it doesn't special-case
HTTP vs. non-HTTP ports.

#### Mockup deviations
Board 2 wasn't reproduced pixel-for-pixel: the log view's toolbar/level-chip row is a plain
`h_flex` row rather than the mockup's styled chip group, and the terminal view has no chrome
(tab bar aside) beyond a bare status label in the corner — screenshotting wasn't attempted (no
running cluster to point it at, and the harness needs `cargo run --features screenshot`, which
this session didn't invoke; see `AGENTS.md` for the exact invocation the next session should
try).

#### For the next session
1. Wire `favorites.rs` into an actual "save as favorite" / auto-start-on-connect flow.
2. Add a target/port picker dialog for port-forwards instead of always using the first port.
3. Wire `exec::Mode::Attach`, ephemeral debug containers and node shell in `kubyl_terminal`.
4. Decide where watch counts should live (`kubyl_core` vs. a small `kubyl_resources` addition)
   and surface them in `kubyl_logs::dock`.
5. Verify against `script/dev-cluster.sh`: 3-replica deployment log interleaving, pod-kill
   rejoin, `vim`/`htop` in an exec shell, and a Service forward surviving a pod restart — none of
   this was exercised live in this session.

#### 2026-09-25: second CodeRabbit pass (3 more findings)
- `manager.rs`: `Forward` gained a `listening` flag, set only on `ForwardEvent::Listening`;
  `last_url` now filters to listening forwards so a bind failure never surfaces
  `http://127.0.0.1:0` to "open last forward in browser". `ForwardEvent::Error`'s session status
  now includes the error message instead of the bare string `"error"`.
- `resolve.rs`: the Service path no longer falls back to the Service's own numeric port when a
  named `targetPort` isn't present on the chosen pod's containers — that could forward to an
  unrelated process listening on the same port number by coincidence. It now returns an error
  naming the service, requested target port and chosen pod instead.
- Removed a stale "probe shells for real" TODO left over from the first CodeRabbit pass — that
  was already fixed then (`shell::detect`).

#### 2026-09-25: Details pane grew inline Logs/Terminal sub-tabs (owner request, out of phase order)

At the repo owner's explicit request, `kubyl_explorer`'s Details pane
(`crates/kubyl_explorer/src/details.rs`) now offers Logs/Terminal (and Yaml) as sub-tabs next to
Summary/Describe, building `kubyl_logs`'s/`kubyl_terminal`'s `ViewKind::Logs`/`ViewKind::Terminal`
views inline via `ViewRegistry::build` rather than `OpenView`. This crate's own `ViewKind`
registrations, actions and the log/terminal views themselves are unchanged — see
plans/02-resource-explorer.md's 2026-09-25 entry for the full description (that's the phase
file whose crate, `kubyl_explorer`, actually changed).
