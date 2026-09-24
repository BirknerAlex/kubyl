# Phase 07: Overview, metrics (Prometheus), events

**Status:** not started
**Depends on:** 02
**Owns:** `crates/kubyl_metrics`, `crates/kubyl_charts`, `crates/kubyl_overview`
**Mockups:** board 4 · Overview, Prometheus metrics, events (plus the usage columns and sparklines on board 1)

## Goal

A cluster overview dashboard with Prometheus-backed charts when available (metrics-server as
fallback), per-object usage everywhere, and a live events stream.

## Tasks

### Metrics sources (`kubyl_metrics`)
- [ ] Prometheus discovery: services labelled or named like `prometheus`, `prometheus-operated`, kube-prometheus-stack, OpenShift `thanos-querier` (openshift-monitoring), VictoriaMetrics `vmselect`. User override per cluster (namespace/service/port/path, or an external URL plus auth header)
- [ ] Transport: Kubernetes API service proxy (`/api/v1/namespaces/{ns}/services/{svc}:{port}/proxy/api/v1/query_range`), so no extra network access is needed. Direct URL optional
- [ ] PromQL query library (versioned, overridable in settings): cluster CPU/memory used/requests/limits, per namespace, per workload, per pod/container, node usage, network, filesystem, restarts. Recording-rule-aware where possible
- [ ] metrics-server fallback (`metrics.k8s.io`) for current values only (no history)
- [ ] `MetricsProvider` trait feeding the phase 02 CPU/memory columns and pod details sparklines. Caching and query de-duplication across views

### Charts (`kubyl_charts`)
- [ ] GPUI-native line, area, stacked area and sparkline charts; bar/progress meters. Hover crosshair with a tooltip, legend toggles, time-range selector (15m/1h/6h/24h/7d), auto-refresh
- [ ] Colors from theme tokens. Series must also differ in lightness (color-blind safe)

### Overview (`kubyl_overview`)
- [ ] Header: cluster name, PROD badge, distribution, version, node/namespace counts, metrics source chip
- [ ] KPI tiles: CPU, memory (with requests/limits %), pods vs capacity, nodes ready
- [ ] Charts: CPU by namespace, memory working set by namespace (top N plus "other")
- [ ] Nodes table: status, CPU/memory bars, pods, instance type, kubelet version. Cordon/drain actions (from 02)
- [ ] Namespace overview variant (when opening a favorite): workloads health, top pods by CPU/memory, recent warnings

### Events
- [ ] Live events watch (`events.k8s.io/v1`, falling back to `v1`), cluster-wide or per namespace. Warning/Normal filter chips with counts, pause, search, group repeated events (`×N`)
- [ ] Events dock on the overview. A full Events view (table) in the sidebar
- [ ] Per-object events in the details dock (involvedObject/regarding match)
- [ ] Optional desktop notifications for new Warning events on favorited namespaces (throttled, opt-in)

## Acceptance criteria

- On kind with kube-prometheus-stack (`script/prometheus-dev.sh`): charts render, time ranges work, pods list shows CPU/memory.
- Without Prometheus: the overview degrades cleanly to metrics-server values and a "connect Prometheus" hint.
- An OOMKilled pod produces a visible Warning event within about 2s.

## Handoff log
