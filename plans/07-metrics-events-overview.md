# Phase 07: Overview, metrics (Prometheus), events

**Status:** done (2026-09-25, branch `phase/07-metrics-overview`)
**Depends on:** 02
**Owns:** `crates/kubyl_metrics`, `crates/kubyl_charts`, `crates/kubyl_overview`
**Mockups:** board 4 · Overview, Prometheus metrics, events (plus the usage columns and sparklines on board 1)

## Goal

A cluster overview dashboard with Prometheus-backed charts when available (metrics-server as
fallback), per-object usage everywhere, and a live events stream.

## Tasks

### Metrics sources (`kubyl_metrics`)
- [x] Prometheus discovery: services labelled or named like `prometheus`, `prometheus-operated`, kube-prometheus-stack, OpenShift `thanos-querier` (openshift-monitoring), VictoriaMetrics `vmselect`. User override per cluster (namespace/service/port/path, or an external URL plus auth header)
- [x] Transport: Kubernetes API service proxy (`/api/v1/namespaces/{ns}/services/{svc}:{port}/proxy/api/v1/query_range`), so no extra network access is needed. Direct URL optional
- [x] PromQL query library (versioned, overridable in settings): cluster CPU/memory used/requests/limits, per namespace, per workload, per pod/container, node usage, network, filesystem, restarts. Recording-rule-aware where possible
- [x] metrics-server fallback (`metrics.k8s.io`) for current values only (no history)
- [x] `MetricsProvider` trait feeding the phase 02 CPU/memory columns and pod details sparklines. Caching and query de-duplication across views

### Charts (`kubyl_charts`)
- [x] GPUI-native line, area, stacked area and sparkline charts; bar/progress meters. Hover crosshair with a tooltip, legend toggles, time-range selector (15m/1h/6h/24h/7d), auto-refresh
- [x] Colors from theme tokens. Series must also differ in lightness (color-blind safe)

### Overview (`kubyl_overview`)
- [x] Header: cluster name, PROD badge, distribution, version, node/namespace counts, metrics source chip
- [x] KPI tiles: CPU, memory (with requests/limits %), pods vs capacity, nodes ready
- [x] Charts: CPU by namespace, memory working set by namespace (top N plus "other")
- [x] Nodes table: status, CPU/memory bars, pods, instance type, kubelet version. Cordon/drain actions (from 02)
- [x] Namespace overview variant (when opening a favorite): workloads health, top pods by CPU/memory, recent warnings

### Events
- [x] Live events watch (`events.k8s.io/v1`, falling back to `v1`), cluster-wide or per namespace. Warning/Normal filter chips with counts, pause, search, group repeated events (`×N`)
- [x] Events dock on the overview. A full Events view (table) in the sidebar
- [x] Per-object events in the details dock (involvedObject/regarding match; the explorer's details have watched them since phase 02)
- [x] Optional desktop notifications for new Warning events on favorited namespaces (throttled, opt-in)

## Acceptance criteria

- On kind with kube-prometheus-stack (`script/prometheus-dev.sh`): charts render, time ranges work, pods list shows CPU/memory.
- Without Prometheus: the overview degrades cleanly to metrics-server values and a "connect Prometheus" hint.
- An OOMKilled pod produces a visible Warning event within about 2s.

## Handoff log

### 2026-09-25 (branch `phase/07-metrics-overview`)

Everything in the task list is done and checked against the kind dev cluster, with and without
Prometheus. Where things live:

- `kubyl_metrics`: `MetricsService` (per-cluster, demand-driven cache: reads mark data as wanted,
  a 1 s loop refreshes what's stale; one fetch per cluster no matter how many views ask),
  `discover` (Service ranking + probes through the service proxy), `prometheus` (client for the
  proxy or an external URL), `queries` (versioned PromQL library, `LIBRARY_VERSION = 1`, prefers
  kube-prometheus recording rules), `metrics_server`, `settings` (`"metrics"` section), the
  `MetricsProvider` impl and a status bar item. Palette: "Metrics: Look for Prometheus Again",
  "Metrics: Set/Clear Prometheus Authorization Header" (masked prompt, stored in the keychain
  under `metrics-auth:<cluster id>`).
- `kubyl_charts`: `LineChart` (line/area/stacked, crosshair + tooltip, legend toggles, faint
  gridline labels, binary axes for bytes), `Sparkline`, `Meter`, `TimeRange`/`TimeRangePicker`,
  `data` (align, top-N + other), `palette`.
- `kubyl_overview`: `OverviewView` (cluster and namespace variants), the events feed/panel/view,
  the opt-in warning notifier, the `"overview"` settings section.
- Shared-crate commits: `kubyl_resources` (`Metrics::changed`/`revision`,
  `MetricsProvider::source_status`, `UsageHistory::window`) and `kubyl_explorer` (lists re-sort
  on metrics changes, usage sparklines in the pod details and usage in the node details, the
  Events sidebar row opens `ViewKind::Events`, the cluster Overview entry is always cluster-wide,
  favorites get "Open Namespace Overview", `dialogs::prompt_secret`).

**Decisions.**
- Prometheus is reached through the API server's service proxy with the cluster's own client,
  so it works wherever the cluster works (exec/OIDC auth, proxies) and needs only
  `get services/proxy`. Discovery lists Services cluster-wide (known namespaces when that's
  forbidden), ranks kube-prometheus-stack, kube-prometheus (`prometheus-k8s`),
  `prometheus-operated`, the community chart's `prometheus-server`, OpenShift's
  `thanos-querier`, Thanos query and VictoriaMetrics (`vmselect` with `/select/0/prometheus`,
  `vmsingle`), then probes up to five with `vector(1)`. It runs again after 3 consecutive target
  failures and every 5 min while a cluster has no Prometheus.
- Overrides in settings.json: `metrics.prometheus.<cluster id or context>` =
  `{namespace, service, port, scheme, path}` or `{url, insecure_skip_tls_verify}`, or
  `{disabled: true}`; `metrics.source` = `auto | prometheus | metrics_server | off`;
  `metrics.queries.<id>` replaces a query (`$sel` = extra matchers).
- Current usage (list columns, node bars, KPI values) comes from Prometheus instant queries when
  Prometheus is the source, else metrics-server; history (sparklines, charts) needs Prometheus.
  Without it, the pod details sparklines and the overview KPI sparklines use metrics-server
  samples collected while they're shown.
- Kubernetes records no Event for an OOM kill, only the `BackOff` after the restart. The feed
  derives an `OOMKilled` warning from `lastState/state.terminated.reason` of the pods in scope
  (marked "pod status", toggle: `overview.derived_events`). Live test: the row exists 1.8 s after
  the kill (finishedAt has 1 s resolution); the UI adds ≤ 0.2 s.
- Events: `events.k8s.io/v1`, falling back to core `v1` when that group is unsupported or
  forbidden; repeats about the same object+reason+type fold into `×N` (toggle in the view).
- Chart colors: the theme's accent/orange/purple/cyan hues re-stepped in lightness per theme and
  validated for CVD separation (worst adjacent ΔE 11.6 dark, 19.0 light) and 3:1 contrast on the
  card surface; a fifth series folds into a dashed "other"; colors stay with a namespace while it
  stays in the top N.

**Verification.** fmt, `clippy --workspace --all-targets -D warnings`, `cargo test --workspace`
and `cargo deny check` pass. Unit tests cover parsing (vector/matrix/errors), discovery ranking,
every library query rendering with and without rules, metrics-server parsing, chart math and
palette lightness steps, event rows from both APIs, grouping, OOM derivation, workload health.
Live tests (`KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig cargo test -p kubyl_metrics --test
live -- --ignored`, same for `kubyl_overview`): discovery finds
`monitoring/kube-prometheus-stack-prometheus` (or nothing with `KUBYL_TEST_PROMETHEUS=absent`
after `script/prometheus-dev.sh --metrics-server-only`), every library query runs raw and with
rules, range queries cover 15m/1h/7d windows, metrics-server answers, a missing Service reports
404, OOM kills show up in < 2 s, both Event APIs parse. Screenshots (`--features screenshot`):
the overview with Prometheus (dark and light, 15m/1h), hover tooltip, metrics-server only
(connect-Prometheus hint, sampled sparklines), the namespace variant, the Events view (selection
drives the details dock), pods list CPU/MEMORY columns, pod details sparklines, cordon/uncordon
from the nodes table.

**Deviations from board 4.**
- The Events stream is the right dock's "Events" tab (next to Details and Active Sessions), so
  `live`/pause sit in the chip row instead of the panel header. Opening the Overview activates
  it; restoring tabs at startup doesn't (2 s startup window).
- The Warning/Normal chips are filters (click to show one type); nothing is selected by default,
  the Warning chip keeps its yellow text.
- Charts show faint value labels on the gridlines; node table TYPE/KUBELET columns are 96/64 px.
- "Desktop notifications" for favorited namespaces are Kubyl toasts (NotificationCenter), not
  OS notifications.

**Not verified / limits.**
- OpenShift `thanos-querier` and VictoriaMetrics are discovered by name and probed, but weren't
  tried against real clusters. The API server strips the user's Authorization header on proxied
  requests, so an OpenShift querier that requires auth needs the URL override (route) plus the
  keychain header.
- Range queries use a fixed `[5m]` rate window; on the 7d range (84 min steps) peaks between
  steps are smoothed away.
- The Events view uses `kubyl_ui::DataTable`, which doesn't scroll horizontally: with the right
  dock open the Count/Namespace columns are clipped.
- Pods/Nodes KPI sparklines need kube-state-metrics (`kube_pod_status_phase`,
  `kube_node_status_condition`).

**Gotchas for later sessions.**
- `window.dispatch_action` routes from the focused element of the *last rendered* frame. A view
  that just got focus but hasn't rendered makes the action start at the window root, so
  workspace handlers (`ActivateDockPanel`, `OpenView`) never see it. Dispatch before the new
  view takes focus (see the Overview factory), or from an existing element.
- The screenshot harness stalls (timers throttled) when another app is in front of its window
  (App Nap). Run screenshots while the machine is otherwise idle.
- Views that show usage observe the `Metrics` global; don't cache usage in sort keys without
  clearing them on `Metrics::changed` (the list clears its sort cache when sorted by CPU/memory).

### 2026-09-25, later (same branch): network, disk and troubleshooting metrics

Asked for after the first round: network and I/O graphs in the details, more troubleshooting
metrics, and one color per namespace across all graphs.

- **Details "Metrics" section** (`kubyl_metrics::details`, via the new
  `DetailsSection` extension point in `kubyl_core`, rendered by the explorer after the
  kind-specific sections). Compact hover charts with the latest values in the header and a shared
  15m–7d range:
  - Pods: network bandwidth, packets, dropped packets, network errors, disk IOPS and throughput,
    CPU throttling (CFS), PSI pressure (cpu/memory/io), OOM kills and restarts. CPU/memory stay
    in the Usage section above.
  - Namespaces and workloads (Deployment, StatefulSet, DaemonSet, ReplicaSet, Job; pods matched
    by their generated names, no recording rule needed): CPU and memory plus all of the above
    (pressure = the highest pod).
  - Nodes (node-exporter, joined via `node_uname_info`): CPU, memory, network, packets, drops,
    errors, TCP retransmits, conntrack table use, disk IOPS/throughput/busy, pressure, fullest
    filesystem.
  - PVCs: volume used vs capacity, inodes (kubelet volume stats; not reported by kind's
    local-path storage, so unverified).
- **Overview**: network and CPU throttling by namespace, disk IOPS and dropped packets by node
  (node-exporter); the namespace variant adds network and throttling by pod.
- **Colors**: `kubyl_charts::ColorRegistry` gives a namespace (pod, node) one color on every chart
  of its cluster; the palette grew to eight validated slots and fills the safest first.
- **Queries**: library v2 (`LIBRARY_VERSION = 2`); availability from the metric-name index
  instead of the recording-rule probe. Pod network excludes host-network pods (their sandbox
  reports the node's interfaces); fs I/O and throttling use container-level series; PSI uses the
  pod cgroup (pressure isn't additive).
- Verified on kind with Prometheus: every panel query returns data for a pod, a deployment, a
  namespace and a node (live test `detail_panels_have_data`); screenshots of the overview (all
  charts), pod, node and deployment details. The harness has a new `scroll=x:y:dy` step.

Ideas not done yet: ingress-nginx request rate/latency/5xx on Ingress details (when its metrics
exist), per-container breakdowns in multi-container pods, Service details through their
endpoints' pods.

