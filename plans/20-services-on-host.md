# Phase 20: The rest of the services on Host

**Status:** in progress (credential store seam)
**Depends on:** 19
**Branch:** `phase/21-core-seams`, one PR per step
**Owns:** the coordinating services of alerts, Prometheus, flows, Argo CD, OLM/Helm and updates,
and the core crates they move into

## Goal

Finish phase 19's "Later": run the coordinating services on `Host`, so their orchestration is a
plain struct that tests drive with `TestHost` instead of `gpui::TestAppContext`, and so the GPUI
crates only wrap it, like `ConnectionManager`.

Today the orchestration stays in the GPUI crates because those services start temporary
port-forwards through `PortForwardManager`, hold `ResourceStores` leases and read
`MetricsService`, all GPUI entities. Every seam below must work with any `Host`, not only
`GpuiHost`.

The app behaves exactly as before after every step.

## Tasks

- [x] Credential store: `kubyl_kube_core::auth::store::install` lets an app provide its own
      `SecretStore` (once, before first use); keychain, `file` and `memory` stay as they are
      when none is installed. (Exec plugins that return a client certificate were fixed on the
      way: they handed kube PEM instead of base64.)
- [ ] "Reach this service": a trait in `kubyl_portforward_core` that opens a temporary loopback
      forward or picks the service proxy and reports the local port, implemented on
      `PortForwardManager` and on `ForwardsCore`. Replaces the polling loops (`open_forward`,
      `start_forward`, `connect`) in alerts, Prometheus, flows and Argo CD.
- [ ] Store snapshots: services that read shared watch caches get them through a small trait
      (acquire a key, read objects, hear about changes), implemented by `ResourceStores` and by
      `StoreCore`.
- [ ] One service per PR on `Host`, their entities wrapping them: alerts, Prometheus, flows,
      Argo CD, OLM/Helm, updates.
- [ ] The existing GPUI tests and live tests pass unchanged after each PR.

## Working rules

Same as phase 19: move code first and change it second, no new behavior, bugs found on the way
get their own commit and PR.

## Handoff log

- 2026-10-04: Plan written. Credential store seam: `auth::store::install(Box<dyn SecretStore>)`,
  `AlreadyInUse` when the store was chosen already. The installed store wins over
  `KUBYL_CREDENTIAL_STORE`. The choice is process-wide, so its test is its own integration test
  (`crates/kubyl_kube_core/tests/custom_store.rs`).
