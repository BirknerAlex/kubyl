# Phase 02: Resource engine, explorer, tables, favorites

**Status:** done (2026-09-24; UI verified by screenshots and live runs, dialogs/drag and drop not clicked through by hand, see Handoff log)
**Depends on:** 01
**Owns:** `crates/kubyl_resources`, `crates/kubyl_explorer`
**Mockups:** board 1 · Pods (plus the sidebar on every board)

## Goal

Browse **any** resource kind (built-in or CRD) in every connected cluster as a live, k9s-fast table,
with a details dock, the common actions, and cross-cluster namespace favorites. This is the
backbone the other feature phases plug into.

## Tasks

### Watch cache (`kubyl_resources`)
- [x] `ResourceStore` per (cluster, GVR, namespace-scope): kube `reflector` + `watcher` with bookmarks, relist on 410, backoff. It shares one watch across all views that need it (ref-counted) and stops the watch after a grace period once no view uses it
- [x] Batched diffs into GPUI models (coalesced ≤ 60 Hz), so thousands of pod updates never block the UI
- [x] Metadata-only watches (`PartialObjectMeta`) for big lists where full objects are not needed
- [x] Multi-namespace watches: one watch per selected namespace, or a cluster-wide watch plus a filter, depending on RBAC

### Columns and formatting
- [x] Hand-written column sets for core kinds, matching kubectl/k9s: Pods (name, ready, status, restarts, CPU, memory, node, age, plus IP in "wide"), Deployments, StatefulSets, DaemonSets, ReplicaSets, Jobs, CronJobs, Services, Ingresses, ConfigMaps, Secrets, PVC/PV, StorageClasses, Nodes, Namespaces, Events, ServiceAccounts, Roles/Bindings
- [x] Generic fallback: server-side Table (`Accept: application/json;as=Table;v=v1;g=meta.k8s.io`). This gives CRD `additionalPrinterColumns` for free
- [x] Pod status derivation identical to kubectl (CrashLoopBackOff, OOMKilled, Init:0/2, Terminating…) with status colors as in the mockup
- [x] CPU/memory columns get data from phase 07 once it lands (stubbed provider trait until then)
- [x] Sort by any column, column chooser, "wide" toggle, relative ages that tick live

### Explorer sidebar (`kubyl_explorer`)
- [x] Multi-root tree: Favorites section, then one root per cluster (color dot, PROD badge, auth-required key icon, offline state). Collapsed roots don't connect
- [x] Per-cluster tree: Overview, Events, Workloads, Network, Config & Secrets, Storage, Access Control, Cluster, Administration (Operators, OperatorHub, Cluster Updates), Custom Resources grouped by API group. Counts come from the watch caches
- [x] Only kinds the user can `list` (RBAC) are shown. Empty groups are hidden. The order is configurable
- [x] Filter kinds (search icon), keyboard navigation like Zed's project panel

### Favorites
- [x] Favorite = (context, source file, namespace, optional kind or label selector, optional alias). Stored in `state.json` and keyed by context name + server URL, so it survives kubeconfig reloads
- [x] Add from: namespace switcher (star), table context menu, sidebar, palette (phase 03 adds `*` mode)
- [x] Favorites section at the top of the sidebar: `namespace · cluster`, cluster color dot, tooltip with the source file, drag to reorder, rename alias. A favorite on a disconnected cluster connects on click
- [x] "Favorite workspace" view: open several favorites in one merged table (e.g. all `payments` pods on prod + staging) with a cluster column

### List view
- [x] Toolbar: breadcrumb, counts (total and failing), filter input (text, `label=value`, `label in (…)`, field selectors, `!` negate, regex), namespace chips (multi-select, "all namespaces"), columns, live indicator
- [x] Keyboard: j/k and arrows, Enter = details, `/` = filter, k9s single-key actions via `ActionRegistry`, multi-select with ⇧/⌘
- [x] Key-hints bar driven by the actions available for the selected kind

### Details dock
- [x] Summary for any object: status pills, owner chain (clickable), labels/annotations chips, conditions, related objects (owner refs, selectors → pods, Service → Endpoints, PVC → PV, Ingress → Service/Secret)
- [x] Kind-specific sections: Pod (containers, images, ports, probes, resources, usage sparklines via phase 07), Deployment (replicas, strategy, rollout history), Node (capacity, taints, conditions)
- [x] "Describe" tab: kubectl-describe-like rendering, including related events

### Actions (first batch)
- [x] Delete (with a typed confirmation on PROD clusters and a grace-period option), scale, rollout restart, rollout undo (to a chosen revision), pause/resume, cordon/uncordon, drain (with PDB-aware progress), trigger CronJob, suspend CronJob, delete pod (kill), copy name/YAML, open in a new tab/split
- [x] The read-only cluster flag disables every mutating action globally

## Acceptance criteria

- Browse pods in a 5,000-pod namespace smoothly. The status of a crashlooping pod updates within about 1s.
- A newly installed CRD shows up and lists with its printer columns and no code changes.
- Favorites from 3 clusters in 2 kubeconfig files work after an app restart and a kubeconfig edit.
- Actions respect RBAC (hidden or disabled) and the read-only/PROD safety flags.

## Handoff log

### 2026-09-25: details additions (branch `phase/05-finish-06-file-browser`)

- A "Files" sub-tab for pods (`ViewKind::Files` from `kubyl_files`, like Logs and Terminal).
  The sub-tab row wraps in the narrow dock and leaves room for the pin button.
- Ports: pods get a Ports section (per container) and Services list their ports in the Service
  section, each with a one-click "Forward" (`kubyl_core::actions::ForwardPort`). A running
  forward shows its local address (click opens HTTP ports in the browser, else copies) and a
  stop button, read from `kubyl_core::forwards::ActiveForwards`. UDP ports and read-only
  clusters get no button.
- Deployments, StatefulSets and ReplicaSets: − / + next to "ready" scale by one. Quick clicks
  are merged into one request (350 ms), shown as "scaling to N" until the object has it; 0
  asks first (typed on production). Hidden on read-only clusters and when RBAC denies
  `patch …/scale`.

### 2026-09-24: phase 02 implemented

**What exists.**
- `kubyl_resources` (engine, no UI): `store` (shared watch caches), `columns` (core kinds +
  kubectl-identical pod/node/job status), `table` (server-side `Table`), `filter`, `ops`
  (mutations), `selection` (`ResourceSelection`), `metrics` (`MetricsProvider` stub), `describe`,
  `format`. Unit tests for all of them; `tests/live.rs` runs every operation against kind
  (ignored by default, see its header).
- `kubyl_explorer` (UI): sidebar sections (Favorites, Clusters), `ResourceListView`
  (`ViewKind::Table` and the Favorites workspace `ViewKind::Custom("favorites")`), the Details
  dock panel and `ViewKind::Details` / `ViewKind::Custom("describe")` tabs, the namespace switcher
  (`SwitchNamespace`), actions and their dialogs, `explorer` settings section.
- `kubyl` shell: sample explorer/table removed, the Explorer header's search button dispatches
  `FilterSidebar`, kubeconfigs dropped anywhere on the window are added (phase 01 follow-up).
  `kubyl_core`: `ColumnDef::wide`, `CellValue::Tinted`, `actions::FilterSidebar`. `kubyl_ui`:
  the title-bar cluster icon uses `ClusterBadge.color` (phase 01 follow-up).
- `script/load-pods.sh`: 5,000 Pending pods (node selector matches nothing, so no containers
  run) plus a crashlooping pod in namespace `load`; `--churn` deletes 50 pods every 2 s.

**Verified against kind** (`script/dev-cluster.sh`, KUBECONFIG in a scratch file).
- Pods view: `design/screenshots/phase-02-pods.png` (macOS). Statuses, owner chain
  Deployment › ReplicaSet › Pod, containers, labels, conditions, counts in the tree.
- 5,000 pods: first load 62 ms per row refresh in a debug build; under `--churn` 117 refreshes
  averaging **3.3 ms** (max 20 ms, the cold first load) in a release build. Only visible rows
  are built (`uniform_list`). Scrolling wasn't profiled by hand.
- Crashloop latency: the table refreshed 30–35 ms after `kubectl get -w` saw each status change
  (Running → Error → CrashLoopBackOff), in the 5,000-pod namespace, debug build.
- CRD at runtime: a tab for `gadgets.test.kubyl.dev` opened before the CRD existed showed "not
  served", then listed the objects with their printer columns (Color, Size, Age; Owner is
  priority 1, so only in wide mode) ~100 ms after `kubectl apply`, and followed edits within
  ~1 s. No code knows the kind.
- Favorites: 3 clusters in 2 kubeconfig files (kind-kubyl-dev, kind-kubyl-oidc, and a second
  context for kind-kubyl-dev) resolve after a restart, after renaming a context in its file and
  after moving the other file (settings path updated).
- Operations (`crates/kubyl_resources/tests/live.rs`): scale, rollout restart, history, undo,
  pause/resume, trigger CronJob, suspend/resume, cordon/uncordon, kill (grace 0), get JSON,
  drain of the control-plane node (3 evicted, 6 static/DaemonSet pods skipped).

**Not verified / stubbed.**
- Dialogs (delete with typed PROD confirmation and grace period, scale, undo revision picker,
  drain progress, rename), drag-to-reorder favorites, context menus, the namespace dropdown and
  keyboard navigation compile and have unit-tested logic, but were **not clicked through by
  hand**: the screenshot harness can't drive input. Do a manual pass before release.
- CPU/memory columns and the dock's Usage section are empty until phase 07 installs a
  `kubyl_resources::metrics::MetricsProvider`. No sparklines yet (needs `kubyl_charts`).
- Rollout undo covers Deployments only (StatefulSet/DaemonSet undo via ControllerRevisions is
  not done). Drain behaves like `--ignore-daemonsets --delete-emptydir-data` and also evicts
  unmanaged pods; PDB blocking is retried every 5 s for 10 minutes but wasn't tested live.
- The Favorites workspace lists pods only (the kind is fixed per tab; phase 03 can open it for
  other kinds with `ViewRequest { kind: Custom("favorites"), target: list(_, gvr, _) }`).
  Favorites are included/excluded via their context menu, not toolbar chips.
- "Add favorite" from the palette is phase 03 (`add_favorite(cluster, namespace, cx)` in
  `kubyl_explorer::actions` does the work).
- Label-selector favorites prefill the list filter (client-side), they don't start a
  server-side label-selected watch in the normal list view (the Favorites workspace does).

**Deviations from the mockup.**
- "Jobs & CronJobs" are two rows (Jobs, CronJobs). Events and Overview are top-level rows.
- The details dock has a Summary/Describe switch at the top and the pin star there (the
  dock's own header only has the close button).
- Usage shows a "no metrics source" note instead of sparklines (phase 07).
- Default pod column widths follow the mockup; names truncate at the default window size, as
  in the mockup.

**API for phases 03–08.**
- Selection: `kubyl_resources::ResourceSelection::global(cx)`: `primary()`, `items` (each has
  `target: ResourceRef`, `kind`, `object: Option<Arc<Value>>`, `store`), `caps`. Register your
  action with `ActionSpec::bind(keys, Some("ResourceList"))` (or `"ResourceList && kind == Pod"`)
  and handle it with a global `cx.on_action`; the list's key-hint bar and context menu pick it
  up automatically (hint order puts `l s d e ⇧f ⌃k ⌃d` first, like k9s).
- Live data: `ResourceStores::acquire(cx, StoreKey::new(cluster, gvr, ns).metadata()/.labels()/
  .fields())` → `StoreHandle`; observe `handle.entity()`. Don't build watchers yourself.
- Opening things: `OpenView(ViewRequest::for_resource(ViewKind::Table, ResourceRef::list(..)))`,
  `ViewKind::Details` / `Custom("describe")` with an object ref. Cluster-level views (Overview,
  Operators, OperatorHub, Updates) are opened from the tree with
  `ResourceRef::list(cluster, Gvr::new("", "", ""), active namespace)`.
- Metrics (phase 07): implement `MetricsProvider` and call `Metrics::set_provider`. List CPU and
  memory cells and the dock's Usage section read it; `pod_history` is for sparklines.
- Status helpers: `columns::{pod_status, node_status, job_status, event_time, is_failing}`,
  `describe::describe`, `format::{human_duration, parse_quantity, to_yaml}`.
- YAML (phase 04): Copy YAML uses `serde-saphyr` 1.3 (`format::to_yaml`); phase 04 may replace it.

**Gotchas.**
- Stores stop 30 s after their last `StoreHandle` drops and restart on reconnect (a failed
  health ping makes `client()` return `None`, so watches relist after it recovers). CRD
  changes retry "unsupported" stores.
- Kinds without a column provider watch metadata only and refetch the whole server-side table
  at most once per second while objects change: fine for CRDs, not for 10,000-object kinds.
- The list's key context is `ResourceList kind=<Kind>`; the filter input sits outside it so
  single-key bindings don't eat typed characters. Keep new inputs outside that context too.
- Default cluster colors come from the `ClusterId`, which contains the kubeconfig path: moving
  a file changes the color unless a color tag is set (and per-context settings keyed by id,
  like the color tag, don't follow a moved file; phase 01 design).
- Windows/Linux: nothing platform-specific in these crates; CI (fmt, clippy, tests) is green on
  all three OSes. `secondary-` bindings are ⌘ on macOS and Ctrl elsewhere; `ctrl-k`/`ctrl-d`
  (Kill/Delete) are Ctrl everywhere, like k9s. Not run interactively on Windows/Linux.

### 2026-09-24: follow-ups from phase 03

Clicked/typed through with the screenshot harness's `keys=` steps against kind (see
plans/03-command-palette.md):
- Fixed: delete, scale, undo and drain dialogs, the namespace switcher, "Favorites: Open
  Workspace" and the sidebar filter never opened when triggered by a key binding or the
  palette (window re-entry in `actions::with_window`; now deferred). Fixed: the undo dialog now shows the same PROD notice as delete/drain.
- Verified: delete dialog (grace period; on a cluster marked `production` the Delete button
  stays disabled until the name is typed), scale dialog (`shift-s`, prefilled replicas), undo
  revision picker (`u`, current revision marked, PROD confirmation), drain dialog (`shift-d` on
  a node, PROD confirmation), `j`/`k` navigation, adding a favorite from the palette (sidebar
  row appears), `/` filter.
- Not verified: the dialogs' confirm buttons (nothing was deleted/drained on purpose; the
  operations themselves are covered by `crates/kubyl_resources/tests/live.rs`), drag-to-reorder
  and rename of favorites, right-click context menus and the namespace dropdown by mouse (the
  harness types keys; mouse input isn't simulated).
- The Favorites workspace for other kinds: `*deploy` → "Favorites workspace: deployments".


### 2026-09-24: follow-ups from phase 04

- Rollout undo (`u`) now covers StatefulSets and DaemonSets (ControllerRevisions,
  `kubyl_resources::ops::workload_history/workload_undo`; live test
  `statefulset_history_and_undo`).
- Fixed: confirm buttons of the explorer dialogs closed the dialog instead of confirming (the view
  is now passed as dialog content). Verified by clicking Force apply/Apply in the YAML editor's
  dialogs, which use the same `dialogs::confirm`.

### 2026-09-25: inline YAML/Logs/Terminal sub-tabs and Secret value reveal (owner request, out of phase order)

Done on `phase/05-logs-exec-portforward` (PR #1) at the repo owner's explicit request, crossing
phase boundaries: `kubyl_explorer` (phase 02) now also reaches into the `ViewRegistry` entries
that phases 04/05 register (`kubyl_yaml`, `kubyl_logs`, `kubyl_terminal`), and this note is the
one-time "intentional exception" AGENTS.md/plans/README.md ask for when a session touches a
crate outside its own phase.

- `DetailsContent`'s `Mode` enum (`crates/kubyl_explorer/src/details.rs`) grew three variants —
  `Yaml`, `Logs`, `Terminal` — alongside the existing `Summary`/`Describe`. The Details pane
  (both the right-dock `DetailsPanel` and the `DetailsView` pane tab, since both wrap
  `DetailsContent`) now shows extra tab buttons for them next to Summary/Describe. Selecting one
  builds the corresponding view inline via `ViewRegistry::build(&ViewRequest::for_resource(kind,
  target), window, cx)` (kind `ViewKind::Yaml`/`Logs`/`Terminal`) — **not** `OpenView`, which
  still opens a separate top-level pane tab via the existing context-menu actions and is
  untouched. The built `Box<dyn TabHandle>` is cached per `Mode` in a new `extra:
  HashMap<Mode, ExtraTab>` field so switching tabs back and forth doesn't rebuild/reconnect the
  underlying kube watch/log-stream/exec session; the cache (and the current mode, if it's no
  longer applicable) is cleared when the target resource changes, and dropped along with
  `DetailsContent` when the pane closes, relying on each view's existing
  Drop/on-release cleanup (same as `kubyl_logs`/`kubyl_terminal`'s session teardown elsewhere).
- Tab visibility mirrors the exact predicates the equivalent context-menu `ActionSpec`s use
  (`kubyl_yaml::EditYaml`, `kubyl_logs::ShowLogs`, `kubyl_terminal::ShowShell`), reimplemented
  locally as `logs_applicable`/`terminal_applicable` in `details.rs` rather than adding
  `kubyl_explorer` as a dependency of those crates (or vice versa): YAML always applies (every
  Details target is an object), Logs applies to
  `pods`/`deployments`/`statefulsets`/`daemonsets`/`jobs`, Terminal applies to `pods` on a
  non-read-only cluster (`ClusterCaps::read_only`, read via `ConnectionManager`). Kinds a tab
  doesn't apply to simply don't get the button — no disabled state.
- Secret Summary: a new "Data" section (`DetailsContent::render_secret`) lists every
  `data`/`stringData` key, decoded to plain text (`data` is base64-decoded; a decode that isn't
  valid UTF-8, or isn't valid base64 at all, shows `<binary, N bytes>` / `<invalid base64>`
  instead of mojibake — never raw base64). Values are masked by default behind a local
  `SECRET_MASK` constant (`"••••••••"`, the same shape as `kubyl_yaml::render::MASK` but not
  imported — not worth a cross-crate dependency for one constant) with a per-key `IconButton`
  reveal toggle (new `IconName::EyeOff`, `crates/kubyl_ui/src/icon.rs` +
  `assets/icons/lucide/eye-off.svg`); revealed keys are tracked in a `revealed:
  HashSet<String>` on `DetailsContent`, reset when the target changes. A copy button
  (`IconName::Copy`) next to each key copies the decoded plain text (never the mask or raw
  base64) via `cx.write_to_clipboard`, with the same `NotificationCenter::push(Notification::info(..))`
  toast as `kubyl_explorer::actions::copy_name`/`copy_yaml`.
- `kubyl_explorer/Cargo.toml` gained `base64.workspace = true` (already a workspace dependency
  via `kubyl_yaml`, version not redeclared).
- Not done: no attempt to sync `DetailsView`'s own top-level tab title/kind with which extra
  sub-tab is active inside its `DetailsContent` — `DetailsView::mode` (used for the pane tab's
  title/`view_request`, i.e. `ViewKind::Details` vs. the `describe` custom kind) still only
  distinguishes Summary vs. Describe, same as before; the new sub-tabs are pane-internal state
  that isn't persisted/restored across restarts.
