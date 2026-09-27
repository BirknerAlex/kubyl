# Phase 16: Network flows (Cilium/Hubble, NetObserv, Calico/Whisker)

**Status:** not started
**Depends on:** 02 (resource explorer, `ResourceStores`, details sections), 05 (port-forward manager, reused for gRPC transports), 07 (established provider/discovery pattern and demand-driven service cache, precedent this phase follows)
**Owns:** `crates/kubyl_netflow` (new), `script/netflow-dev.sh`
**Mockups:** board 18 · Network flows, to be added to `design/mockups/generate.py` before the UI work: the flow table (Wireshark-style), the topology graph, the "no flow visibility" empty state, the backend indicator, and the namespace/workload zoom levels of the graph.

## Goal

One cluster-agnostic **Network Flows** view: a live, filterable flow table (time, direction,
source, destination, protocol/port, verdict allowed/dropped, matched NetworkPolicy, bytes/packets)
plus a topology graph (nodes = namespaces or workloads, edges = aggregated flows, styled by verdict
and sized by volume, like OpenShift Console's topology). Both are driven by one internal `Flow`
model; which of three backends actually supplies it is detected per cluster and shown, never
assumed.

Kubernetes has no built-in flow API: every CNI/dataplane exposes (or doesn't) its own. This phase
covers the three backends that (a) are free to install and exercise on a local `kind` cluster and
in CI, with no cloud credentials or paid services, and (b) expose flows through a documented,
versioned API rather than needing Kubyl to parse node-local files or logs:

1. **Cilium + Hubble** — Hubble Relay's gRPC `Observer.GetFlows`. Richest source: verdict, the
   matching (Cilium)NetworkPolicy name, L3–L7 identity already resolved to pod/namespace/labels.
2. **NetObserv** — its own eBPF DaemonSet agent, independent of the cluster's actual CNI. The
   fallback for clusters with no native flow visibility (OVN-Kubernetes/OpenShift, kindnet,
   flannel…).
3. **Calico + Whisker/Goldmane** — gRPC to Goldmane (Calico OSS ≥ 3.29). Newest and least stable
   API of the three; lowest priority.

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
   the reverse direction of everything else Kubyl does).
4. **Polling model.** Recommended: demand-driven like `MetricsService` — a backend client streams
   only while a Network Flows view (table or graph) is open for that cluster, into a bounded,
   time-windowed in-memory ring buffer per cluster; nothing is persisted to `state.json` and flows
   are never written to disk or logs (they can contain pod/namespace names but also, depending on
   the backend's L7 visibility, request paths — treat any L7 field as sensitive until proven
   otherwise, same bar as Secret data).
5. **Graph layout.** Recommended: a small hand-rolled force-directed layout in `kubyl_netflow` (or
   `kubyl_charts` if it's judged reusable), rather than a third-party graph crate — evaluated
   against a maintained MIT/Apache crate in the spike if one exists and clears `cargo deny`.
   OpenShift's topology view is the closest reference for interaction (zoom, click node → filter,
   click edge → filter), not for layout implementation, which Kubyl builds itself like every other
   `kubyl_charts` visual.

## Backends

### Cilium + Hubble

- Detect: a `hubble-relay` Service (`cilium`, `kube-system`, or a labelled Service anywhere —
  `k8s-app=hubble-relay`), probed with a `GetFlows` call limited to 1 result over a temporary
  loopback forward.
- Transport: gRPC over `kubyl_portforward`'s ephemeral forward to the Service's gRPC port (4245 by
  default), `tonic` client generated from Cilium's public `.proto` files (`cilium/cilium`,
  `api/v1/observer`, `api/v1/flow`; Apache-2.0, vendor the exact files used and pin the Cilium
  version they came from in the handoff log).
- Mapping: `Flow.verdict` (`FORWARDED`/`DROPPED`/`ERROR`/`AUDIT`) → the internal verdict;
  `Flow.l4`, `Flow.source`/`destination` (namespace, pod name, labels, or `IP`/`reserved:world` for
  non-cluster endpoints); newer Cilium versions attach the matching `CiliumNetworkPolicy`/
  `NetworkPolicy` name in `Flow.policy_match_info` — surface it when present.
- Works unmodified on managed clusters that ship Cilium+Hubble already enabled (confirmed:
  DigitalOcean DOKS; also true of Cilium installed in full-replacement or chaining mode on EKS) —
  Kubyl needs no cloud-specific code for this, and this phase's own testing stays entirely on
  `kind` plus a plain `cilium install --set hubble.relay.enabled=true`.

### NetObserv

- Detect: the `netobserv` namespace's `flowlogs-pipeline` Service (direct gRPC/JSON export mode)
  or, when configured that way, its Loki Service (`loki`/`loki-gateway`), probed like Prometheus
  through the API server's service proxy.
- Transport: prefer the eBPF agent's direct export (no Loki dependency) if the spike confirms it's
  documented and stable enough; otherwise fall back to querying Loki the same way phase 07 queries
  Prometheus (service proxy, PromQL-like LogQL client).
- Mapping: NetObserv's flow record already carries K8s enrichment (namespace/pod/workload for both
  ends, direction, bytes/packets); drop reasons map to the internal verdict; a "blocked by policy"
  verdict is only as precise as the CNI's own ACL logging (OVN-Kubernetes needs
  `k8s.ovn.org/acl-logging` on the NetworkPolicy to attribute a drop to a specific policy — without
  it NetObserv still shows the drop, just not which policy).
- This is the only backend that works on clusters with **no** native flow visibility at all
  (OVN-Kubernetes/OpenShift's default CNI, kindnet, flannel): it doesn't read the CNI's dataplane,
  it sniffs at the node with its own eBPF program. That's why it's in scope despite being the
  heaviest of the three to install.

### Calico + Whisker/Goldmane

- Detect: a `goldmane` Service in `calico-system` (Calico OSS ≥ 3.29 with Whisker enabled).
- Transport: gRPC to Goldmane, same forwarding pattern as Hubble.
- Mapping: Goldmane's flow log already has an allow/deny action and the matching policy name; map
  directly. Pin the exact Goldmane/Calico version tested, since this API is new and unstable
  relative to Hubble's.

## Tasks

### Spike first (decides transport, schema mapping and layout, ~3–4 days)
- [ ] `script/netflow-dev.sh`: three `kind` clusters (or three modes of one script, like
  `dev-cluster.sh --recreate`), each free and CI-able on GitHub Actions Linux runners the way
  `cilium/cilium`, `projectcalico/calico` and NetObserv's own upstream CI already do:
  - `kind` + `cilium install --set hubble.relay.enabled=true` (disable kindnet:
    `disableDefaultCNI: true` in the kind config)
  - `kind` + NetObserv's eBPF agent DaemonSet (default kindnet CNI, to prove the CNI-independent
    path), direct export mode
  - `kind` + Calico (`disableDefaultCNI: true`) with Whisker/Goldmane enabled
  - Reuse the `payments` namespace fixtures from `dev-cluster.sh`, plus a second namespace and one
    deliberately blocking `NetworkPolicy`, so cross-namespace and blocked flows both show up
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
- [ ] `Flow` model, `FlowProvider` trait, `detect()` (probe order: Hubble Relay, Goldmane,
  NetObserv's flowlogs-pipeline/Loki — settings override per cluster, same shape as
  `updates.clusters.<cluster>.provider`)
- [ ] `hubble`, `netobserv`, `calico_whisker` backend implementations
- [ ] No-backend state and its hint (mirrors phase 07's "connect Prometheus" hint)
- [ ] Demand-driven service: streams only while a view is open, bounded time-windowed ring buffer
  per cluster, namespace/verdict/protocol filters pushed down where the backend supports it
- [ ] Aggregation for the graph (namespace-level and workload-level), recomputed on the selected
  time window

### UI: flow table
- [ ] Columns: Time, Direction, Source, Destination, Protocol/Port, Verdict (colored), Policy,
  Bytes/Packets; filter bar (namespace, verdict, protocol, free text on pod/IP), pause/resume,
  search
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

- On `kind` + Cilium/Hubble: the table shows live flows for the sample workloads; the deliberately
  blocking `NetworkPolicy` produces red/Dropped rows naming that policy; the graph shows the same
  data as nodes/edges.
- On `kind` + NetObserv with the default CNI (no Cilium, no Calico): flows still show up, proving
  the CNI-independent fallback.
- On `kind` + Calico/Whisker: flows show up with allow/deny reflected.
- Switching to a cluster with a different backend switches the active adapter transparently; a
  cluster with none of the three shows the empty-state hint naming what to install.
- Nothing is exposed beyond loopback forwards and the API server's own service proxy; no flow data
  (which can include pod/namespace names and, on backends with L7 visibility, request paths) is
  logged or written to `state.json`.

## Risks

- Antrea/IPFIX is excluded from this phase because it's push-based (the node exports to a
  configured collector) and doesn't fit "read via proxy/port-forward, expose nothing" — revisit
  only if a pull-based or gRPC path appears.
- Goldmane/Whisker is new (2024) and may still change between Calico releases; keep its mapping
  isolated so a schema change doesn't ripple into the internal `Flow` model.
- NetObserv's direct-export (non-Loki) mode may be less documented/stable than the Loki path — the
  spike must confirm it works before it's picked as the default transport; Loki stays the
  documented fallback.
- Graph layout performance and legibility on clusters with many namespaces/workloads — needs
  aggregation/pagination limits found in the spike, not guessed upfront.
- Hubble/Goldmane `.proto` changes across CNI versions — vendor the exact files used and record the
  tested version in the handoff log, the way phase 08's certificate handling records wry's mangled
  delegate class name per version.

## Handoff log

(empty — not started)
