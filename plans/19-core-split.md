# Phase 19: Split domain logic from the UI (GPUI-free `*_core` crates)

**Status:** in progress
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
- [ ] `kubyl_base`: types, error, runtime, plain notifications, `Host` + test host.
- [ ] `kubyl_core` re-exports `kubyl_base`; `GpuiHost`.
- [ ] `kubyl_settings_core`: sections, parsing, merging, schema, `state.json`, atomic writes, the
      file watcher. `kubyl_settings` keeps the `Settings`/`State` globals and observers.
- [ ] CI check for core crates.

### Kubernetes
- [ ] `kubyl_kube_core`: kubeconfig, groups, auth, client, transport, discovery, access, openapi,
      watches, cluster info, settings; the `ConnectionManager` state machine.
- [ ] `kubyl_resources_core`: formatting, filters, describe, ops, routes, table rows, columns;
      the `ResourceStores` watch caches.

### Features (in dependency order)
- [ ] `kubyl_logs_core`, `kubyl_terminal_core` (exec sessions), `kubyl_portforward_core`.
- [ ] `kubyl_yaml_core`, `kubyl_metrics_core`.
- [ ] `kubyl_alerts_core`, `kubyl_argocd_core`, `kubyl_netflow_core`, `kubyl_operators_core`,
      `kubyl_updates_core`, `kubyl_prometheus_core`.
- [ ] `kubyl_files_core`, `kubyl_kubeconfig_core`, `kubyl_webview_core`, `kubyl_palette_core`,
      `kubyl_explorer_core`, `kubyl_overview_core`, `kubyl_selfupdate_core`, `kubyl_charts_core`.

### Wrap-up
- [ ] `plans/README.md`: workspace layout, crate ownership, the decision.
- [ ] App checked against `script/dev-cluster.sh` (screenshots of the main views) and the live
      tests.

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
