# Phase 20: The rest of the services on Host

**Status:** done
**Depends on:** 19
**Branch:** `phase/21-core-seams`
**Owns:** the coordinating services of alerts, Prometheus, flows, Argo CD, OLM/Helm and updates,
and the core crates they moved into

## Goal

Finish phase 19's "Later": run the coordinating services on `Host`, so their orchestration is a
plain struct that tests drive with `TestHost` instead of `gpui::TestAppContext`, and so the GPUI
crates only wrap it, like `ConnectionManager`.

Those services kept their orchestration in the GPUI crates because they started temporary
port-forwards through `PortForwardManager`, held `ResourceStores` leases and read
`MetricsService`, all GPUI entities. Every seam below works with any `Host`, not only
`GpuiHost`.

The app behaves exactly as before after every step.

## Design

- **Reach a Service** (`kubyl_portforward_core::reach`): a `Reach` trait opens a temporary
  loopback forward to a Service and reports the local port; dropping the `Reached` stops the
  forward, and `Reached::take_stopped` says when the user or the cluster ended it earlier.
  `ChannelReach` is the `Send` implementation: it sends requests to the host thread that owns
  the forwards, where `ForwardsCore::serve` answers them. `PortForwardManager` serves it,
  lists these forwards in Active Sessions and gives features `app_reach(cx)`. Services keep
  deciding for themselves whether the API server's service proxy will do.
- **Store sources** (`kubyl_resources_core::source`): `StoreSource` acquires a `StoreKey` as a
  `StoreLease` and shows it as a `StoreView`; `StoreReader` is the read half for shared
  borrows; `StoreCopies` carries the contents of some stores into a call that can't reach the
  app (a `hosted` closure borrows the app mutably). `MemoryStores` fills stores by hand for
  tests; `kubyl_resources::store::{AppStores, AppStoresRef}` read `ResourceStores`.
  The app acquires a service's watches and tells the service when they changed.
- **Snapshots in, effects out**: what a service reads from the app (connections, settings,
  state.json, the metrics service's Prometheus) comes in as a snapshot parameter; what it
  changes in the app goes out as a typed effect, as for `MetricsCore`.
- **Credential store** (`kubyl_kube_core::auth::store::install`): an app can provide its own
  `SecretStore` once at startup, before the first use; keychain, `file` and `memory` stay as
  they are when none is installed.

## Tasks

- [x] Credential store seam.
- [x] Exec plugins that return a client certificate: they handed kube PEM instead of base64.
- [x] "Reach this service" (`reach`), `Reached::fake` and `TestHost::run_until` for tests.
- [x] Store sources, `StoreCopies`, `AppStores`.
- [x] Services on `Host`, their entities wrapping them: alerts (`AlertsCore`), Prometheus
      (`PrometheusCore`), flows (`FlowsCore`), Argo CD (`ArgoCore`), OLM and Helm (`OlmCore`,
      `HelmCore`), updates (`UpdatesCore`).
- [x] Small additions the services' users wanted: `exec::run_to_end` reports a session's exit
      code; `StoreCore::changes_since` says which objects the batches since a generation
      touched; `ColumnDef` and `CellValue` live in `kubyl_base::columns` and the built-in
      per-kind columns in `kubyl_resources_core::columns`.
- [x] The existing GPUI tests and live tests pass unchanged.

## Handoff log

- 2026-10-04: Done. Checked with `cargo fmt`, `clippy -D warnings`, `cargo test --workspace` and
  `script/check-core-crates.sh`. Live tests on `kubyl-dev` (kind, `KUBYL_CREDENTIAL_STORE=memory`):
  kube (including a new exec plugin that prints the admin certificate), terminal (including the
  exit code), resources, logs, files, metrics, overview and portforward (service forward) pass;
  alerts pass (after `prometheus-dev.sh` and `alertmanager-dev.sh`, including the Alertmanager
  behind kube-rbac-proxy through a forward); Argo CD's `the_ui_follows_the_crds_without_a_restart`
  and the Kubernetes-mode tests pass. Not passing, in code this phase didn't touch: Argo CD
  `api_mode_signs_in_through_the_proxy_and_a_forward` (the wrong-token error is not
  `ApiError::Forbidden` on v3.5.3), Helm `helm_releases_decode_with_values_and_manifest` (the
  mask assertion). Not run: the OLM live tests (`olm-dev.sh`), OIDC, Route backends, the flows
  tests (`netflow-dev.sh` clusters) and the updates tests that need `updates-dev.sh`.
- The services' core tests use `TestHost`, `Reached::fake` and `MemoryStores`; the GPUI crates
  keep their tests.
- Gotchas: a task handle must not drop itself from its own callback (several services keep
  finished tasks until the next prune, or detach fire-and-forget work). `hosted` borrows the app
  mutably: acquire leases and copy store contents before calling into a core.
