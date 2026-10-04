# Phase 19: Split domain logic from the UI (GPUI-free `*_core` crates)

**Status:** in progress (code done; app check against the dev cluster pending)
**Depends on:** all feature phases (it moves their code); 18 done
**Owns:** new `crates/kubyl_base`, `crates/kubyl_*_core`; per step, the crate being split (one
crate at a time, see "Working rules")

## Goal

Every crate's domain logic (kubeconfig loading, auth, connections, watch caches, log streams,
exec, port-forwards, YAML apply, metrics, alerts, Argo CD, network flows, operators, updates…)
lives in a plain Rust crate that builds and tests without GPUI. The existing crate keeps its
name and becomes the GPUI adapter plus its views, so other crates' imports keep working.

The app behaves exactly as before after every step.

## Why

- **Tests:** domain logic is tested with plain `#[test]`/`#[tokio::test]` instead of
  `gpui::TestAppContext`, which forbids OS threads and real I/O in tests.
- **GPUI upgrades:** `gpui-pre` changes often. Only the adapters and views have to follow it.
- **Clear boundaries:** a view can't reach into connection internals, and the logic can't depend
  on theme colors or window state.

## Design

- **`kubyl_base`** (no GPUI): `ClusterId`, `ResourceRef`, `Gvk`/`Gvr`, `ViewKind`, caps, the error
  type, the shared Tokio runtime (`runtime()`, `handle()`), plain notifications, and the
  [`Host`](#the-host-trait) trait. `kubyl_core` re-exports all of it and keeps the GPUI parts:
  registries, actions, `spawn_kube`, `ActiveContext`, `NotificationCenter`.
- **`kubyl_<name>_core`** next to each crate with logic. It depends only on `kubyl_base` and
  other `*_core` crates. The GPUI crate re-exports the moved modules (`pub use
  kubyl_kube_core::auth;`), so `kubyl_kube::auth::store` and friends keep their paths.
- **Stateful services** (`ConnectionManager`, `ResourceStores`, `MetricsService`, the alerts,
  Argo CD, flows, operators, updates and Prometheus services, port-forwards, log sessions,
  settings) become a plain state struct in the core crate. The GPUI entity wraps it, derefs to it
  for reads, and passes a host to its mutating methods. Reads stay synchronous and return
  references as before; callbacks still run on the UI thread in the same order.
- **Colors, `SharedString`, `Hsla`** stay in the adapters. The core returns `String`s and semantic
  values (a color tag, a connection state), and the adapter maps them to the theme.

### The `Host` trait

A core service never spawns, emits or touches app globals itself. It asks its host:

- `spawn(future, then)`: runs `future` on the Tokio runtime and calls `then(&mut service,
  output, host)` back on the host's thread. It returns a `TaskHandle`; dropping it aborts the
  work, just like dropping a GPUI `Task`.
- `emit(event)` and `notify()`: what `cx.emit` and `cx.notify` did.
- App-level effects as typed calls on the service's own host trait (save a state section,
  update a settings section, push a notification), implemented by the adapter with GPUI.

`kubyl_core::host::GpuiHost` implements `Host` for any entity that holds a service. A test host
in `kubyl_base` runs the same core code on Tokio, without GPUI.

## Working rules

- One step = one or more commits on `phase/19-core-split`. The app builds, passes
  `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and behaves
  the same after each step.
- Move code first, change it second: a commit that moves files and adds re-exports, then a commit
  that changes the API, so review stays readable.
- No new behavior in this phase. Bugs found on the way get their own commit.
- Core crates must not depend on `gpui`, `gpui-component` or `kubyl_ui`. CI enforces it
  (`script/check-core-crates.sh`: `cargo tree -p <core> -e normal` must not list them).

## Tasks

### Foundation
- [x] `kubyl_base`: types, error, runtime, plain notices, `SharedString`, `Host` + `TestHost`.
- [x] `kubyl_core` re-exports `kubyl_base`; `GpuiHost` (`kubyl_core::host`).
- [x] `kubyl_settings_core`: sections, parsing, merging, schema, `state.json`, atomic writes, the
      file watcher. `kubyl_settings` keeps the `Settings`/`State` globals and observers.
- [x] CI check for core crates (`script/check-core-crates.sh`, job `core-crates`).

### Kubernetes
- [x] `kubyl_kube_core`: kubeconfig, groups, auth, client, transport, discovery, access, openapi,
      watches, cluster info, settings; `ManagerCore` (the `ConnectionManager` state machine).
- [x] `kubyl_resources_core`: formatting, filters, describe, ops, routes, kubectl status, usage
      types; `StoreCore` (the watch caches). Column providers and the server-side table stay in
      `kubyl_resources` (they build UI cell values).

### Features: modules moved (logic without services)
- [x] `kubyl_logs_core`, `kubyl_terminal_core`, `kubyl_portforward_core` (with `ForwardsCore`).
- [x] `kubyl_yaml_core`, `kubyl_metrics_core` (with `MetricsCore`), `kubyl_charts_core`.
- [x] `kubyl_alerts_core`, `kubyl_argocd_core`, `kubyl_netflow_core`, `kubyl_operators_core`,
      `kubyl_updates_core`, `kubyl_prometheus_core`.
- [x] `kubyl_files_core`, `kubyl_kubeconfig_core`, `kubyl_webview_core`, `kubyl_palette_core`,
      `kubyl_explorer_core`, `kubyl_overview_core`, `kubyl_selfupdate_core`.

### Features: services
- [x] On `Host` (the whole state machine in the core): `ConnectionManager`, `ResourceStore`,
      `PortForwardManager`, `MetricsService`, `SelfUpdate`.
- [x] Domain parts in the core, orchestration in the GPUI crate: alerts (`cache`: counts,
      sources, fetch-and-merge), Prometheus (`servers`), updates (`service`: states, facts,
      building providers), flows (`state`), Argo CD (`settings`, `apps`, `run`), OLM
      (`olm::snapshot`), Helm (`helm::release`).
- [ ] Later (needs port-forward and store seams in the core): the orchestration of the services
      above. They start temporary port-forwards through `PortForwardManager`, hold
      `ResourceStores` leases and read `MetricsService`, all GPUI entities. Moving them on `Host`
      means a "reach this service" trait (forward or proxy) and store snapshots fed in by the
      adapter. Silences, the file transfer queue, log sessions and the remaining view state are
      UI.

### Wrap-up
- [x] `plans/README.md`: workspace layout, the decision; `AGENTS.md`: where logic goes.
- [ ] App checked against `script/dev-cluster.sh` after the feature steps (screenshots of the
      main views) and the live tests of metrics, alerts, operators, updates, flows. Done for kube,
      resources and port-forward; the rest is blocked on Docker (see the handoff log).

## Acceptance criteria

- No `*_core` crate has `gpui`, `gpui-component` or `kubyl_ui` in its dependency tree (CI).
- `cargo test --workspace`, clippy and the live tests pass; the app behaves the same.
- Each feature crate's non-view code is in its `*_core` crate.

## Risks

- **Behavior drift** while moving services behind `Host`: ordering of events and notifications,
  cancellation when tasks are dropped. Keep the existing GPUI tests passing unchanged.
- **Parallel phases** touching the same crates: land steps one crate at a time.

## Handoff log

- 2026-10-04: Plan written.
- 2026-10-04: Foundation, Kubernetes and every feature crate's movable modules are in 23
  `*_core` crates (all GPUI-free, CI-checked); `ConnectionManager`, `ResourceStore`,
  `PortForwardManager` and `MetricsService` run their state machines on `Host` with unchanged
  APIs. 979 tests pass (same as before the phase, minus moved duplicates, plus new core tests);
  the app was checked on the kind cluster (pods, deployments, metrics, alerts badges) after the
  kube and resources steps.
  - Moved modules keep their paths through `pub use <core>::{…}` in the GPUI crate. Theme colors
    of core enums are extension traits there (`ColorTagExt`, `ConnectionStateExt`).
  - `gpui::SharedString` is `gpui-pre-shared-string` (no UI); core crates use it via
    `kubyl_base::SharedString`, bumped together with gpui.
  - Test fixtures shared with GPUI crates sit behind `test-support` features
    (`kubyl_kube_core`, `kubyl_argocd_core`) or in the core crate's `tests/fixtures`.
  - Commits that add a core crate don't all build on their own: the root `Cargo.toml` entries
    landed with the last crate of each batch.
- 2026-10-04: Feature crates done: every crate with logic has a `*_core` (23), the self-contained
  services run on `Host`, the coordinating ones keep only their orchestration in the GPUI crate
  (see "Features: services"). 978 tests pass (two self-update GPUI tests became one core test);
  clippy with all features, `cargo deny` and the core-crates check are clean.
  - Not yet checked in the app: the feature steps after metrics. Docker's VM went read-only when
    the disk filled up (the build cache had grown to 157 GB; `target/debug/incremental` was
    deleted, and test runs now use `CARGO_INCREMENTAL=0`). Restart Docker Desktop, then
    `script/dev-cluster.sh`, the live tests (`--ignored`) and screenshots of alerts, Argo CD,
    flows, operators, updates and Prometheus.
  - The metrics live tests failed with "no Prometheus found" while the cluster was already
    unreachable; run them again after the restart before trusting either result.
