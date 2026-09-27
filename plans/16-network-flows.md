# Phase 16: Network flows (Cilium/Hubble, NetObserv, Calico/Whisker)

**Status:** in progress (spike done, plan corrected)
**Depends on:** 02 (resource explorer, `ResourceStores`, details sections), 05 (port-forward manager, reused for gRPC transports), 07 (established provider/discovery pattern and demand-driven service cache, precedent this phase follows)
**Owns:** `crates/kubyl_netflow` (new), `script/netflow-dev.sh`
**Mockups:** board 18 · Network flows, to be added to `design/mockups/generate.py` before the UI work: the flow table (Wireshark-style), the topology graph, the "no flow visibility" empty state, the backend indicator, and the namespace/workload zoom levels of the graph.

## Goal

One cluster-agnostic **Network Flows** view: a live flow table (time, direction, source,
destination, protocol/port, verdict allowed/dropped, matched NetworkPolicy, bytes/packets),
filterable on every one of those fields — pod, IP, port, namespace, protocol, direction, verdict,
policy — not a preselected subset, plus a topology graph (nodes = namespaces or workloads, edges = aggregated flows, styled by verdict
and sized by volume, like OpenShift Console's topology). Both are driven by one internal `Flow`
model; which of three backends actually supplies it is detected per cluster and shown, never
assumed.

Kubernetes has no built-in flow API: every CNI/dataplane exposes (or doesn't) its own. This phase
covers the three backends that (a) are free to install and exercise on a local `kind` cluster and
in CI, with no cloud credentials or paid services, and (b) expose flows through a documented,
versioned API rather than needing Kubyl to parse node-local files or logs:

1. **Cilium + Hubble** — Hubble Relay's gRPC `Observer.GetFlows`. Richest source: verdict, the
   (Cilium)NetworkPolicy that allowed or denied a flow, L3–L7 identity already resolved to
   pod/namespace/labels.
2. **NetObserv** — its own eBPF DaemonSet agent, independent of the cluster's actual CNI. The
   fallback for clusters with no native flow visibility (OVN-Kubernetes/OpenShift, kindnet,
   flannel…). It has no pull API for single flows: its pipeline writes flow records to Loki and
   metrics to Prometheus, and Kubyl reads those.
3. **Calico + Whisker** — the Whisker backend's HTTP flow stream (Calico OSS ≥ 3.30, which
   introduced Whisker and Goldmane; tested 3.32.2). Newest and least stable API of the three;
   lowest priority.

Not in this phase: Antrea/IPFIX (push-based export doesn't fit "read via port-forward/service
proxy, expose nothing" — see Risks), any cloud-managed flow log service (VPC Flow Logs etc. —
cloud-only, can't be exercised without a paid account), live packet capture/pcap export.

## Decisions to make first

Each has a recommendation; record the outcome in the README's decision table.

1. **Crate.** Recommended: a new `kubyl_netflow` crate, owning both the data layer (the three
   backends behind one trait) and the view (table + graph), the same split `kubyl_overview` uses
   for metrics/events. `kubyl_charts` gains only what's generic (a graph layout primitive, if one
   is added there instead of in `kubyl_netflow` directly — decide in the spike).
2. **Unified model, not unified backend.** There is no cross-CNI wire format for flows (eBPF is a
   common substrate, not a common schema). Recommended: one internal `Flow` struct plus a
   `FlowProvider` trait (mirrors `MetricsProvider` from phase 07, `UpdateProvider` from phase 13),
   with `detect()` choosing a backend per cluster and each backend mapping its own schema onto
   `Flow`. A backend that doesn't have some field (e.g. Whisker's policy match may be looser than
   Hubble's) leaves it `None` rather than guessing.
3. **Transport.** Recommended: the same pattern as phase 07's Prometheus discovery — reach every
   backend through the API server's service proxy or a temporary loopback port-forward
   (`kubyl_portforward`), never a new exposed Service, never a NodePort. This is what rules out
   Antrea's IPFIX export in this phase (it's the node pushing UDP/TCP at a configured collector,
   the reverse direction of everything else Kubyl does). gRPC can't pass through the API server's
   service proxy (it needs HTTP/2 end to end), so gRPC backends (Hubble Relay) go through
   ephemeral forwards; HTTP backends (Whisker, Loki, Prometheus) use the service proxy. Kubyl
   never reads Secrets for a backend: TLS roots come from ConfigMaps, and a backend that needs a
   client certificate (mutual TLS) is reported, not worked around.
4. **Polling model.** Recommended: demand-driven like `MetricsService` — a backend client streams
   only while a Network Flows view (table or graph) is open for that cluster, into a bounded,
   time-windowed in-memory ring buffer per cluster; nothing is persisted to `state.json` and flows
   are never written to disk or logs (they can contain pod/namespace names but also, depending on
   the backend's L7 visibility, request paths — treat any L7 field as sensitive until proven
   otherwise, same bar as Secret data).
5. **Filtering.** Recommended: every field on the `Flow` model is filterable, not just a
   preselected few — namespace, pod, workload, IP, port, protocol, direction, verdict and matched
   policy, on source and destination independently, combinable (AND) plus free text. Filters are
   pushed down into `stream_flows(filter)` and applied server-side wherever the active backend's
   API supports it (`capabilities()` says which fields); any field a backend can't filter
   server-side is still filterable client-side against the buffered window, so the filter bar
   behaves identically regardless of which backend is active — the abstraction covers filtering
   too, not only the columns shown.
6. **Graph layout.** Recommended: a small hand-rolled force-directed layout in `kubyl_netflow` (or
   `kubyl_charts` if it's judged reusable), rather than a third-party graph crate — evaluated
   against a maintained MIT/Apache crate in the spike if one exists and clears `cargo deny`.
   OpenShift's topology view is the closest reference for interaction (zoom, click node → filter,
   click edge → filter), not for layout implementation, which Kubyl builds itself like every other
   `kubyl_charts` visual.

## Backends

### Cilium + Hubble

- Detect: a `hubble-relay` Service (`kube-system`, `cilium`, or a Service labelled
  `k8s-app=hubble-relay` anywhere), probed with `Observer.ServerStatus` (flows buffered, flows
  per second, connected and unavailable nodes, version) over a temporary loopback forward.
- Transport: gRPC over `kubyl_portforward`'s ephemeral forward to the Service's port (80 → pod
  port 4245 in plain gRPC; 443 with `hubble.relay.tls.server.enabled`), a `tonic` client generated
  from Cilium's public `.proto` files (`cilium/cilium` v1.20.2: `api/v1/flow/flow.proto`,
  `api/v1/observer/observer.proto`, `api/v1/relay/relay.proto`; Apache-2.0, vendored unchanged).
  A Relay with server TLS presents `*.hubble-relay.cilium.io`, signed by the Cilium CA, which is
  public only when the cluster publishes it in the ConfigMap `kube-system/cilium-root-ca.crt`
  (Helm `tls.caBundle`); Kubyl verifies against that (or a CA file named in settings) and never
  reads the `cilium-ca` Secret. A Relay that requires client certificates
  (`hubble.relay.tls.server.mtls`) is reported and deferred.
- Mapping: `Flow.verdict` (`FORWARDED`/`DROPPED`/`ERROR`/`AUDIT`/`REDIRECTED`…) → the internal
  verdict; `Flow.l4`, `Flow.source`/`destination` (namespace, pod name, workloads, labels, or
  `reserved:world`/`host`/`remote-node` for non-pod endpoints); policies from
  `ingress_allowed_by`/`egress_allowed_by`/`ingress_denied_by`/`egress_denied_by` (name,
  namespace, kind: `CiliumNetworkPolicy`, `NetworkPolicy`…) and `drop_reason_desc`. There is no
  `policy_match_info`. Verified on Cilium 1.20.2: a plain NetworkPolicy that isolates a pod drops
  other callers with `POLICY_DENIED` and names nothing (nothing denied them, nothing allowed them);
  a CiliumNetworkPolicy `ingressDeny` rule drops with `POLICY_DENY`, and its policy-verdict event
  names the policy in `ingress_denied_by` (the matching drop notification doesn't, so Kubyl carries
  the name over from the policy-verdict event of the same connection); allowed flows name the
  allowing policy, NetworkPolicies included. L7 (HTTP through Envoy) carries full URLs, query
  strings and headers: sanitized before anything else sees them.
- Works unmodified on managed clusters that ship Cilium+Hubble already enabled (confirmed:
  DigitalOcean DOKS; also true of Cilium installed in full-replacement or chaining mode on EKS) —
  Kubyl needs no cloud-specific code for this, and this phase's own testing stays entirely on
  `kind` plus a plain Cilium Helm install with `hubble.relay.enabled=true`.

### NetObserv

- NetObserv has no pull API for single flows: the eBPF agent pushes to flowlogs-pipeline, which
  writes flow records to Loki and metrics to Prometheus (its "direct export" targets are push
  exporters: Kafka, IPFIX, OpenTelemetry). Kubyl reads Loki for the table (LogQL through the API
  server's service proxy, polled) and the `netobserv_*` metrics for the graph
  (`netobserv_workload_ingress_bytes_total` by source and destination namespace and owner,
  `netobserv_namespace_drop_packets_total`), through `MetricsService` or the Prometheus the
  FlowCollector names. Without Loki the graph comes from metrics and the table explains why it's
  empty.
- Detect: the `FlowCollector` (`flows.netobserv.io`, `cluster`): its `spec.loki` (Monolithic URL,
  Microservices querier URL, LokiStack) and `spec.prometheus.querier` say where the data is; an
  in-cluster URL (`http://<svc>.<ns>.svc…:<port>`) becomes a service-proxy path. A LokiStack
  gateway authenticates the user's token, which the service proxy doesn't forward: the table is
  deferred there (metrics only) with that explanation.
- Mapping: namespace, owner, type and direction are Loki stream labels (`SrcK8S_Namespace`,
  `SrcK8S_OwnerName`, `SrcK8S_Type`, `DstK8S_*`, `FlowDirection`, `K8S_FlowLayer`); the JSON line
  has addresses, ports, `Proto`, `Bytes`, `Packets`, TCP `Flags`, pod and host names, times and,
  with the `PacketDrop` feature, `PktDropPackets`/`PktDropLatestDropCause` (kernel drop reasons).
  A drop is only attributed to a policy where the CNI reports it: OVN-Kubernetes with
  `k8s.ovn.org/acl-logging` and NetObserv's network events name the policy (not testable on kind:
  deferred). Verified on kindnet (kube-network-policies): an isolated pod's callers don't show as
  drops at all, only as TCP attempts that never got past `SYN`; Kubyl marks those "no reply"
  instead of guessing a verdict.
- Install: NetObserv 2.0's Helm chart needs cert-manager and trust-manager (its certificates) and
  can bring Loki (`install.loki`) and a Prometheus stack (`install.prom-stack`); `script/netflow-dev.sh`
  installs all of it, with `sampling: 1` and the `PacketDrop` and `DNSTracking` features.
- This is the only backend that works on clusters with **no** native flow visibility at all
  (OVN-Kubernetes/OpenShift's default CNI, kindnet, flannel): it doesn't read the CNI's dataplane,
  it sniffs at the node with its own eBPF program. That's why it's in scope despite being the
  heaviest of the three to install.

### Calico + Whisker

- Detect: the `whisker` Service in `calico-system` (Calico OSS ≥ 3.30 with the `Whisker` and
  `Goldmane` resources, which the default `custom-resources.yaml` enables).
- Transport: HTTP through the API server's service proxy to the Whisker Service (port 8081, which
  nginx splits into the UI and `/whisker-backend/`): `GET /whisker-backend/flows?watch=true&
  startTimeGte=-<seconds>&filters=<json>` is a server-sent event stream (`data: {flow}`) that
  replays Goldmane's buffer from that time and then follows; without `watch` it's a paged list;
  `/whisker-backend/flows-filter-hints?type=…` lists values seen. Goldmane's own gRPC API (port
  7443) needs mutual TLS with a client certificate the operator issues into a Secret, so Kubyl
  doesn't use it.
- Mapping: records are 15-second aggregates per source and destination (pod names aggregated to
  `<replicaset>-*`) with `action` (Allow/Deny/Pass), protocol, destination port, packets and bytes
  in both directions, the reporter (source or destination), and a policy trace (`enforced`,
  `pending`: kind, name, namespace, tier, action). Verified on 3.32.2: an explicit Calico `Deny`
  rule names its NetworkPolicy; an isolating Kubernetes NetworkPolicy shows as the tier's
  end-of-tier Deny with the NetworkPolicy that isolated the pod as its `trigger`. The API is the
  Whisker UI's own (unversioned), so the tested version is pinned.

## Tasks

### Spike first (decides transport, schema mapping and layout, ~3–4 days)
- [ ] `script/netflow-dev.sh`: three modes of one script, one `kind` cluster each (run one at a
  time), each free and CI-able on GitHub Actions Linux runners the way `cilium/cilium`,
  `projectcalico/calico` and NetObserv's own upstream CI already do:
  - `kind` + Cilium (Helm, `hubble.relay.enabled=true`; kindnet disabled with
    `disableDefaultCNI: true`), optionally with Relay server TLS and the CA bundle ConfigMap
  - `kind` + NetObserv (default kindnet CNI, to prove the CNI-independent path) with
    cert-manager, trust-manager, Loki and Prometheus
  - `kind` + Calico ≥ 3.30 (`disableDefaultCNI: true`) with Whisker and Goldmane
  - Reuse the `payments` namespace fixtures from `dev-cluster.sh`, plus a second namespace, one
    isolating `NetworkPolicy` and, where the CNI has them, one explicit deny rule (a
    CiliumNetworkPolicy `ingressDeny`, a Calico `Deny`), so cross-namespace, isolated and denied
    flows all show up; mark everything, `--delete` removes only what's marked
- [ ] Confirm each backend's exact `.proto`/API version, vendor what's needed, and confirm it's
  reachable only through a Service proxy or `kubyl_portforward` ephemeral forward — no NodePort,
  no `hostNetwork` listener Kubyl would need to expose
- [ ] Define the internal `Flow` struct and `FlowProvider` trait (`detect`, `stream_flows(filter)`,
  `capabilities()` — does this backend resolve policy names, support live-only or also historical
  queries, aggregate server-side)
- [ ] Prove the topology aggregation (group by src/dst namespace or workload + verdict, sum
  bytes/packets) and pick the layout approach (hand-rolled vs. a `cargo deny`-clean crate) against
  real flows from all three kind clusters, including the blocked-policy case

### Mockups
- [ ] Board 18 (`design/mockups/generate.py`): flow table, topology graph (namespace and workload
  zoom), backend indicator, "no flow visibility for this CNI" empty state with a link naming which
  of the three to install

### Data layer (`kubyl_netflow`)
- [ ] `Flow` model, `FlowProvider` trait, `detect()` (probe order: Hubble Relay, Calico Whisker,
  NetObserv's FlowCollector with Loki and Prometheus — the CNI's own flow API first, the
  CNI-independent fallback last; settings override per cluster, same shape as
  `updates.clusters.<cluster>.provider`)
- [ ] `hubble`, `netobserv`, `calico_whisker` backend implementations
- [ ] No-backend state and its hint (mirrors phase 07's "connect Prometheus" hint)
- [ ] Demand-driven service: streams only while a view is open, bounded time-windowed ring buffer
  per cluster
- [ ] `FlowFilter`: every `Flow` field (namespace, pod, workload, IP, port, protocol, direction,
  verdict, policy — source and destination independently), pushed down into `stream_flows` per
  backend's `capabilities()`; a client-side fallback filter over the ring buffer for any field a
  backend can't filter server-side, so the table's filter bar works identically on all three
- [ ] Aggregation for the graph (namespace-level and workload-level), recomputed on the selected
  time window

### UI: flow table
- [ ] Columns: Time, Direction, Source, Destination, Protocol/Port, Verdict (colored), Policy,
  Bytes/Packets
- [ ] Filter bar: one facet per filterable field — namespace, pod, workload, IP, port, protocol,
  direction, verdict, policy — each offered for source and destination separately, with
  autocomplete from what's actually been seen in the buffered window (like k9s's `/` filter and
  phase 05's log search, not a fixed dropdown of guessed values). Facets combine with AND; a
  free-text/expression box covers ad-hoc queries across all fields at once (e.g.
  `ns=payments verdict=dropped port=443`), Wireshark-display-filter style rather than a second,
  separate search mechanism. Chips show which backend actually applied a facet server-side vs.
  client-side (only relevant when the active backend's `capabilities()` don't cover that field)
- [ ] Pause/resume streaming; clearing all facets returns to the unfiltered live view
- [ ] Row → detail panel with the raw backend fields (debugging/trust)
- [ ] Header shows which backend is active; the empty state when none is found

### UI: topology graph
- [ ] Namespace-level and workload-level zoom, nodes sized/colored by volume or health, edges
  styled by verdict (allowed vs. dropped) and thickness by volume
- [ ] Click a node/edge filters the table to that scope; shared time range with the table
  (`kubyl_charts::TimeRangePicker`)
- [ ] Stays responsive at a few hundred nodes/edges (aggregate further or paginate beyond that —
  exact threshold decided in the spike against real cluster sizes)

### Placement
- [ ] Sidebar row "Network Flows" (cluster-wide) and a namespace-scoped variant, following the
  `register_view_row`/overview cluster-vs-namespace split from phases 07/14
- [ ] Palette action `> Network Flows`

## Acceptance criteria

- On `kind` + Cilium/Hubble: the table shows live flows for the sample workloads; blocked flows
  are red/Dropped rows. A drop names the policy where the backend attributes it (Hubble with a
  CiliumNetworkPolicy `ingressDeny` rule, Calico's policy trace, OVN ACL logging), else shows
  "denied: isolated, no policy allows it" (a plain Kubernetes NetworkPolicy has no deny rules: a
  drop comes from isolation with nothing allowing it, and Hubble can't name "the" policy). The
  Hubble fixture has both: `payments/ledger-api-isolation` isolates, `storefront/web-guard`
  (CiliumNetworkPolicy) denies by name. The graph shows the same data as nodes/edges.
- On `kind` + NetObserv with the default CNI (no Cilium, no Calico): flows still show up, proving
  the CNI-independent fallback. kindnet reports no drops for its NetworkPolicies, so the isolated
  calls show as "no reply" (TCP attempts that never got past `SYN`), not as drops. Without Loki
  the graph still shows the traffic from NetObserv's metrics and the table says why it's empty.
- On `kind` + Calico/Whisker: flows show up with allow/deny reflected; the `web-guard` deny names
  its policy, the isolated calls name the NetworkPolicy that isolated the pod (the end-of-tier
  trigger).
- Switching to a cluster with a different backend switches the active adapter transparently; a
  cluster with none of the three shows the empty-state hint naming what to install.
- Filtering by any single field (pod, IP, port, namespace, protocol, direction, verdict, policy),
  alone or combined, narrows both the table and the graph identically on all three backends —
  server-side where the active backend's API supports it, client-side over the buffered window
  otherwise; the acceptance test on each `kind` cluster includes filtering the sample traffic down
  to one pod and to the deliberately blocked flows by verdict (by "no reply" on kindnet).
- Nothing is exposed beyond loopback forwards and the API server's own service proxy; no Secret is
  read; no flow data (which can include pod/namespace names and, on backends with L7 visibility,
  request paths, query strings and headers) is logged, written to `settings.json` or
  `state.json`, or shown in toasts, `Debug` output or the palette. A 403 names the missing verb and
  resource (`create pods/portforward in kube-system`, `get services/proxy in calico-system`)
  instead of an empty state.

## Risks

- Antrea/IPFIX is excluded from this phase because it's push-based (the node exports to a
  configured collector) and doesn't fit "read via proxy/port-forward, expose nothing" — revisit
  only if a pull-based or gRPC path appears.
- Whisker (Calico 3.30, 2025) is new and its backend API is the Whisker UI's own, unversioned; it
  may change between Calico releases. Keep its mapping isolated so a schema change doesn't ripple
  into the internal `Flow` model, and pin the tested version (3.32.2).
- NetObserv has no pull API for flows (spike: its exporters push to Kafka, IPFIX or
  OpenTelemetry); Kubyl depends on Loki for single flows and on its metrics for the graph. LokiStack
  (OpenShift's usual setup) needs the user's token at its gateway, which the service proxy strips.
- Graph layout performance and legibility on clusters with many namespaces/workloads — needs
  aggregation/pagination limits found in the spike, not guessed upfront.
- Hubble `.proto` changes across Cilium versions — vendor the exact files used and record the
  tested version in the handoff log, the way phase 08's certificate handling records wry's mangled
  delegate class name per version.

## Handoff log

### 2026-09-27 (spike, branch `phase/16-network-flows`)

Tested versions: kind v0.33.0 (Kubernetes v1.37.0, arm64), Cilium 1.20.2 (Helm), Calico v3.32.2
(tigera-operator manifests), NetObserv 2.0.0 (Helm chart 2.0.0 with its loki-stack subchart,
Loki 2.6.1, and the kube-prometheus-stack subchart), cert-manager v1.21.2, trust-manager v0.25.0.
`script/netflow-dev.sh` builds each (one at a time; about 3 GB of Docker memory each, NetObserv
about 4 GB).

The plan's backend sections, acceptance criteria and risks were corrected after the spike:
- Calico: Whisker and Goldmane arrived in 3.30, not 3.29. Goldmane's gRPC needs mutual TLS with a
  client certificate from a Secret (whisker-backend mounts `whisker-backend-key-pair`); the
  Whisker backend's HTTP API gives the same flows (server-sent events with `watch=true`, a paged
  list without) and works through the API server's service proxy. Kubyl uses that.
- Hubble: the policy fields are `ingress_allowed_by`, `egress_allowed_by`, `ingress_denied_by`,
  `egress_denied_by` and `drop_reason_desc`; there's no `policy_match_info`. A plain NetworkPolicy
  can't deny, it isolates: those drops are `POLICY_DENIED` with no name (the criterion now asks for
  "denied: isolated, no policy allows it" there), a CiliumNetworkPolicy `ingressDeny` rule is
  `POLICY_DENY` and named (the fixture has both). Every attempt shows up twice (a policy-verdict
  event, then a drop notification without the name).
- Hubble Relay serves plain gRPC by default (`disable-server-tls: true`, Service port 80). With
  server TLS (port 443, certificate `*.hubble-relay.cilium.io`) its CA is only in Secrets unless
  the cluster enables `tls.caBundle` (ConfigMap `kube-system/cilium-root-ca.crt`); the script's
  `--relay-tls` sets that up. `grpcurl` through `kubectl port-forward` hung on every call while
  plain HTTP/2 through the same forward answered (Relay replied with a gRPC error to curl); the
  `hubble` CLI inside the cluster worked. Kubyl's own forward is tested separately.
- NetObserv: no pull API for single flows (the plan's "direct export" pushes to Kafka, IPFIX or
  OpenTelemetry). Loki through the service proxy works (`/loki/api/v1/query_range`, stream labels
  for namespaces, owners, types and direction; the JSON line for the rest); the default metrics
  include `netobserv_workload_ingress_bytes_total` with source and destination namespace and owner.
  The Helm chart needs cert-manager and trust-manager. On kindnet the isolated calls never show as
  drops (the `PacketDrop` feature only reports kernel drops such as TCP teardown); they are TCP
  flows with only `SYN` and no reply.
- The user's OpenShift test cluster (read-only check): no NetObserv (no `flowcollectors` CRD) and
  OVN-Kubernetes, so there's no Loki or NetObserv to open forwards or queries to; Kubyl should
  show the empty state there (checked read-only later).
