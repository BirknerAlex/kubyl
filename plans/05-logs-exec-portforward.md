# Phase 05: Logs, exec terminal, port-forwarding

**Status:** in progress — solid, tested core in all three crates; several sub-items per bullet
below are still missing. See the Handoff log for the precise breakdown before starting the next
session.
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
forward with hard-coded default ports (8080 for Pod/workload, 80 for Service) and an
auto-assigned local port — **there's no target/port picker dialog yet**, which is the main
missing piece for real usability.

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
2. Add a target/port picker dialog for port-forwards instead of the hard-coded defaults.
3. Probe shells for real in `kubyl_terminal` (use `shell::probe_command` via a quick `exec`
   before the interactive one) and wire `exec::Mode::Attach`, ephemeral debug containers and node
   shell.
4. Decide where watch counts should live (`kubyl_core` vs. a small `kubyl_resources` addition)
   and surface them in `kubyl_logs::dock`.
5. Verify against `script/dev-cluster.sh`: 3-replica deployment log interleaving, pod-kill
   rejoin, `vim`/`htop` in an exec shell, and a Service forward surviving a pod restart — none of
   this was exercised live in this session.
