# Phase 24: Missing resource views: device resources, admission policies, Gateway API, VPA, EndpointSlices

**Status:** done
**Depends on:** 02 (catalog, columns, details sections), 04 (YAML editor)
**Owns:** none (small commits in `kubyl_explorer`, `kubyl_resources_core`, `kubyl_resources`), `script/views-dev.sh`
**Mockups:** board 22 · Resource views, to be added to `design/mockups/generate.py` before the UI work: the Device Resources group (Resource Claims, a Resource Slice's devices), a pod's "Resource Claims" section and a node's "Devices" section, the Admission Policies list and a policy's details (validations as CEL, bindings), a Gateway's details (listeners, attached routes), a Service's "Routes" section, and the VPA list with recommendations.

## Goal

Some built-in kinds only open through the palette or land under Custom Resources: Dynamic
Resource Allocation (GPUs and other devices), ValidatingAdmissionPolicy and
MutatingAdmissionPolicy, EndpointSlices, most of the Gateway API, and VerticalPodAutoscalers.
Others have a sidebar entry but only the server's generic columns. This phase gives each a
sidebar entry where it belongs, columns that say what matters, and details sections that
connect them to the objects they affect, like phase 15 did for ConfigMaps.

Lens has these views (DRA, Admission Policies, Gateway API, VPA, Leases, Priority/Runtime
Classes); this phase brings Kubyl level and keeps its own style: links between related objects
everywhere.

## Decisions to make first

Each has a recommendation. Record the outcomes in the README's decision table.

1. **Versions.** Recommended: discovery's preferred version for every kind, with known fields
   read the same across versions: DRA `resource.k8s.io/v1` (Kubernetes 1.34+), also `v1beta2`
   and `v1beta1` where served; admission policies `admissionregistration.k8s.io/v1`, falling back
   to `v1beta1`/`v1alpha1` (MutatingAdmissionPolicy is beta); Gateway API standard and
   experimental channels. An entry appears only when its kind is served.
2. **Where they go.** Recommended:
   - Workloads: VerticalPodAutoscalers (when the VPA CRD is served).
   - Network: EndpointSlices; Gateways, GatewayClasses, HTTPRoutes and, when served, GRPCRoutes,
     TCPRoutes, TLSRoutes, UDPRoutes, ListenerSets, BackendTLSPolicies, ReferenceGrants (moved
     out of Custom Resources).
   - Cluster: Validating/MutatingAdmissionPolicies and their Bindings, next to the webhook
     configurations.
   - A new "Device Resources" group: ResourceClaims, ResourceClaimTemplates, DeviceClasses,
     ResourceSlices, shown only when `resource.k8s.io` is served.
3. **CEL.** Admission policies, webhook `matchConditions` and DeviceClass selectors hold CEL.
   Recommended: show expressions in monospace blocks in the details, without a CEL grammar for
   now (gpui-component's tree-sitter set has none). Highlighting is "Later".

## Tasks

### Catalog and columns
- [x] Catalog entries per decision 2, each gated on discovery; kinds that now have a home leave Custom Resources (the catalog's curation does it; `BUILT_IN_GROUPS` itself is unchanged)
- [x] Columns (`kubyl_resources_core::columns`):
  - ResourceClaim: state (Pending / Allocated / Allocated, reserved by N), device classes, device health, age
  - ResourceClaimTemplate: device classes, age
  - DeviceClass: driver(s) from selectors, extended resource name, age
  - ResourceSlice: driver, pool, node, devices, allocated, age
  - ValidatingAdmissionPolicy: validations, failure policy, bindings, age; MutatingAdmissionPolicy: mutations, failure and reinvocation policy; bindings: policy (link), validation actions, param ref, age
  - EndpointSlice: service (link), address type, ready/total endpoints, ports, age
  - Gateway: class, addresses, programmed, listeners, attached routes; GatewayClass: controller, accepted; routes: parents, hostnames, backends, accepted
  - VPA: target (link), update mode, recommendation (CPU/memory target), age
  - Lease: holder, renewed, age; PriorityClass: value, global default, preemption; RuntimeClass: handler, overhead
- [x] Status tones for the new state columns (`status_tone`)

### Details sections
- [x] ResourceClaim: requests, allocation (devices, node), reserved-for pods (links); device conditions
- [x] ResourceSlice: devices with attributes, capacity and taints; DeviceClass: selectors (CEL) and config
- [x] Pod: a "Resource Claims" section (claims and templates the pod uses, their state and devices)
- [x] Node: a "Devices" section (the node's ResourceSlices: driver, pool, devices, which claim allocated each)
- [x] Admission policy: validations/mutations, match constraints, match conditions, variables, audit annotations (CEL in monospace), its bindings (links) and their param objects; binding: the policy and the namespaces/objects it matches
- [x] Webhook configurations: match conditions shown the same way
- [x] Gateway: listeners with their status and attached-route counts, addresses, the routes attached (links)
- [x] Route: parent refs with acceptance per parent, rules with matches and backends (links to Services)
- [x] Service: a "Routes" section listing every route that sends traffic to it, and its EndpointSlices
- [x] VPA: recommendations per container (lower bound, target, upper bound) next to the current requests

### Across Kubyl
- [x] Palette short names: `:rc` stays ReplicationController; DRA: `:claims`, `:deviceclasses`, `:slices`; `:vap`, `:vapb`, `:map`, `:mapb`; `:eps`, `:vpa`
- [x] Describe text (`kubyl_resources_core::describe`) for the new kinds
- [x] YAML templates for a ValidatingAdmissionPolicy with a binding and a ResourceClaimTemplate

### Dev setup and tests
- [x] `script/views-dev.sh`: on `kubyl-dev`, the DRA example driver (`dra-example-driver`) with a claim and a pod using it, a ValidatingAdmissionPolicy with a binding (and a MutatingAdmissionPolicy where served), Gateway API CRDs with a Gateway and an HTTPRoute to a sample Service, and the VPA CRDs with a VPA for a sample Deployment; `--delete` removes only what it installed
- [x] Column and section tests with fixtures for each kind and version
- [x] Live tests (ignored) against the dev cluster

## Acceptance criteria

- On a 1.34+ cluster with the DRA example driver: the Device Resources group lists the claim,
  its slice and device class; the pod and node show their claim and devices with links.
- The admission policy and its binding appear under Cluster with their CEL rules and links to
  each other.
- A Service with an HTTPRoute shows the route in its "Routes" section, and the Gateway lists
  the route as attached.
- Kinds the cluster doesn't serve add no sidebar entries.

## Risks

- **Fast-moving APIs** (DRA's beta-to-GA field changes, Gateway API experimental kinds): parse
  defensively and keep fixtures per version.
- **Cost of the Service "Routes" section**: it needs the route lists of the namespace (and
  cross-namespace via ReferenceGrants); use the shared stores and compute on demand.

## Later (not in this phase)

- CEL syntax highlighting and a CEL playground (evaluate a policy against an object).
- CSV export of any table.
- An "Applications" view grouping objects by `app.kubernetes.io/*` labels.

## Handoff log

### 2026-10-07 (phase 24 complete)

**Shipped**
- Catalog (`kubyl_explorer::catalog`): VerticalPodAutoscalers under Workloads; EndpointSlices,
  GRPC/TLS/TCP/UDPRoutes, ListenerSets (+ the pre-1.5 `XListenerSets`), BackendTLSPolicies and
  ReferenceGrants under Network (with the Gateways, GatewayClasses, HTTPRoutes that were there);
  Validating/MutatingAdmissionPolicies and their Bindings under Cluster; a new group
  **Device Resources** (`devices`, after Storage) with ResourceClaims, ResourceClaimTemplates,
  DeviceClasses, ResourceSlices and DeviceTaintRules. All gated on discovery (`group_kinds`);
  curated kinds leave Custom Resources. `BUILT_IN_GROUPS` is unchanged: the built-in groups'
  kinds that now have a home show there, the others stay hidden.
- Models in `kubyl_resources_core` (re-exported by `kubyl_resources`): `dra` (claims, requests
  with `exactly`/`firstAvailable`, allocation, reserved-for, device status, pod claims and the
  kubelet's `allocatedResourcesStatus`, slices, device classes and the drivers their CEL pins,
  `allocations(slice, claims)`), `admission` (policies, bindings, webhooks, label selectors),
  `gateway` (Gateways, listeners, GatewayClasses, routes, parents, backends, ReferenceGrants) and
  `vpa`. Each read the same across their versions; fixtures per version in the tests.
- Columns (`columns/views.rs`) for every kind of the plan, plus `CellLink` (a cell naming an
  object is drawn as a link: EndpointSlice → Service, binding → policy, VPA → target, Gateway →
  class, ResourceSlice → node; `CellButton::link` in `kubyl_core`, rendered in the list table) and
  `RELATED_COLUMNS` (a policy's bindings, a slice's allocated devices, a claim's device health
  come from a watch of the related kind the list starts per source; see "Code review fixes"
  for how they're indexed and scoped). Status tones for the new
  states in `status_tone` (`Allocated`, `Allocated, reserved by N`, `Accepted`, `Programmed`,
  `Not accepted…`, `Partly accepted`…); `is_failing` covers Gateways, GatewayClasses and routes.
- Details (`details/{views,devices,admission,gateway,vpa,parts}.rs`): header state and chips per
  kind; ResourceClaim (requests with CEL selectors, allocation with node link, devices with their
  slice and health, reserved-for pods with status, driver device status), ResourceClaimTemplate,
  DeviceClass (selectors, opaque config, the claims using it), ResourceSlice (devices with
  attributes, capacity, taints and the claim holding each), Pod "Resource claims", Node
  "Devices"; policies (match constraints, match conditions, variables, validations/mutations,
  audit annotations, type-checking warnings, bindings with their param objects as links),
  bindings (policy, param ref, the namespaces the selector matches as links), webhook
  configurations (webhooks with rules, selectors and match conditions); Gateway (listeners with
  state and attached-route counts, addresses, attached routes), GatewayClass (its Gateways),
  routes (parents with acceptance per parent, rules with matches, filters and backends as links),
  Service "Routes" (routes in its namespace, and in the namespaces a ReferenceGrant there
  lets refer to it) and "EndpointSlices", EndpointSlice
  (endpoints with pod links); VPA (target link, update mode, per container requests vs lower /
  target / upper). Restored detail tabs of these kinds get the right kind before discovery
  (`KNOWN_KINDS` in `kind_guess`).
- Describe text for all new kinds (`describe/views.rs`; `describe_as(group, …)` so Istio's
  `Gateway` isn't mistaken for the Gateway API's; `describe` takes the group from `apiVersion`).
  Webhook URLs lose their query (a token could sit there).
- Palette short names (`catalog::short_names`, used by the palette snapshot): `:claims`,
  `:claimtemplates`, `:deviceclass(es)`, `:slices`, `:vap`, `:vapb`, `:map`, `:mapb`, `:eps`,
  `:vpa`; `:rc` stays ReplicationController.
- YAML templates: ValidatingAdmissionPolicy with a binding for the editor's namespace (two
  documents) and ResourceClaimTemplate (`resource.k8s.io/v1`).
- Mockups: board 22 · Resource views (five screens appended to `SCREENS`).
- `script/views-dev.sh` (see its header): the real dra-example-driver 0.5.0 runs on kind 1.37
  (containerd 2 has CDI on), with `gpuDeviceStatus` so claims carry device data; `--fake-dra`
  makes a DeviceClass and hand-made ResourceSlices for a made-up driver instead (claims get
  allocated, the pods wait in ContainerCreating). Gateway API v1.6.3 experimental CRDs and VPA
  CRDs (1.8.0) with statuses written by the script (no controller or recommender runs).
  `--delete` removes only what carries `kubyl.dev/installed-by=views-dev.sh`, pods first while
  the driver still runs (forced after 90 s, for the fake driver), then waits for the namespace.

**Decisions** (README decision table): versions per kind with readers that accept all served
versions; placement as recommended plus DeviceTaintRules in Device Resources and XListenerSets;
CEL in monospace blocks without highlighting; cells that link and related columns.

**Verification**: `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`,
`cargo test --workspace` (1111 passed, 77 ignored), `cargo deny check`,
`script/check-core-crates.sh` pass. Live: `KUBYL_TEST_KUBECONFIG=… cargo test -p kubyl_resources
--test live_views -- --ignored` (3 tests) against kubyl-dev after `views-dev.sh` (also after a
`--delete` and a fresh install; `--fake-dra` checked by hand). GPUI tests: the sections of every
new kind (`resource_views_show_their_sections`, by `section-<title>` debug selectors that every
details section now has) and the link cells (`kubyl_resources::columns` tests). Screenshots of
the claims list, slices, admission policies, Gateways, VPAs, and the details of a pod, a node, a
Service, an HTTPRoute, a policy and a binding against kubyl-dev match the board.

**Deviations / deferred**
- Sidebar: DeviceTaintRules sit in Device Resources with server-side columns (added to the
  board); ListenerSets, BackendTLSPolicies and ReferenceGrants use their CRD printer columns.
- The Service "Routes" section counts a cross-namespace route only with a ReferenceGrant in the
  Service's namespace (what the Gateway API requires); it doesn't check a Gateway's
  `allowedRoutes`.
- A node's "Devices" lists the slices with `spec.nodeName` set to it (a field-selected watch).
  Slices published with `nodeSelector`, `allNodes` or `perDeviceNodeSelection` don't show under
  any node (their slice details show where the devices are); matching them needs every slice
  of the cluster and the node selector semantics: later.
- A Gateway's attached routes come from the route kinds its listeners admit
  (`allowedRoutes.kinds`, else by protocol), from its namespace only unless a listener allows
  others (then from every namespace, whatever a `Selector` picks): a route of a kind no
  listener admits isn't listed (its own details show it as not accepted). Routes attached
  through a ListenerSet aren't counted. Decided when the details open and redone when the
  Gateway's listeners change.
- Device health in the claim list: the driver's `Ready` condition when it reports one, else the
  kubelet's `allocatedResourcesStatus` of the pods it's reserved for (per container entry, not
  per device: "N healthy" counts the claim's devices when all entries are healthy).
- The ResourceClaimTemplate template names `resource.k8s.io/v1` (the `exactly` form); on a
  cluster that only serves `v1beta1` its apiVersion and request fields need editing.

**Gotchas**
- A `--delete` that uninstalls the DRA driver before the pods using its devices are gone leaves
  them Terminating (the kubelet can't unprepare): the script deletes the pods first.
- `kubectl apply` can't switch a claim between the example driver's class and the fake one:
  `--delete` first.
- Kubernetes 1.37 serves MutatingAdmissionPolicy as `v1`; the Gateway API install brings its
  own `safe-upgrades` VAP (removed by `--delete` only when the CRDs came from the script).

### 2026-10-07 (code review fixes)

Review findings, each checked against the code and the API semantics.

**Major**
- *Claim health matched pods of other namespaces and watched every pod* — fixed.
  `dra::PodClaims` indexes pods by `(namespace, name)`; `claim_health` looks up only the
  claim's namespace (`reservedFor` consumers live there). `columns::RelatedColumn` (a struct
  now) has `per_row_namespace`: with all namespaces, the claims list watches pods only in the
  namespaces that hold claims (one watch each, the whole cluster above 10), re-synced when rows
  appear in new namespaces. Tests: `pods_name_their_claims_and_health`, `device_resources`.
- *O(n·m) joins per render* — fixed. Lists: `columns::RelatedIndex` (bindings per policy, the
  claims holding each device, the claims of each pod) is built once per related-store
  generation in the list's store observer and looked up per cell; a change re-sorts only when
  sorted by that column. Details: `DetailsContent::derived` caches values derived from named
  stores until one of them changes (stamped by store and generation): a Gateway's attached
  routes, a Service's routes, a class's Gateways and claims, the slice of each device, the
  claims holding each device, a claim's pod index, a binding's namespaces, a policy's
  bindings; each route is parsed once per change. Narrowed watches: a Gateway watches only the
  route kinds its listeners admit, in its own namespace unless a listener takes routes from
  others (`gateway::attachable_routes`); a Service watches routes cluster-wide no more, only
  the `(kind, namespace)` its namespace's ReferenceGrants allow
  (`gateway::granted_route_sources`). Test: `derived_values_are_rebuilt_only_when_their_stores_change`.

**Minor**
1. *`status_of` fell back to another listener's status* — fixed: the fallback to any status
   of the same Gateway only applies to a parent ref without `sectionName` and `port`
   (`a_listener_ref_takes_only_its_own_status`).
2. *Allocations counted allocations, not devices* — fixed: `dra::DeviceHolders` maps each
   device to every claim holding it; `allocations` returns one entry per device with its
   claims, so the Allocated column, a slice's "N of M devices" and a node's total count
   devices; the details link every holder.
3. *Namespaced param kind without a namespace linked to nothing* — fixed: no link, label
   `per namespace: <name>` (`ParamRef::label_for`, `names_one`).
4. *Node details* — fixed: the claims watch starts only once the node's slices loaded with at
   least one; until the claims loaded, devices show no holder (not "free") and the title no
   allocated count; same for a slice's details. Selector-based slices: documented under
   Deviations.
5. *`--delete` removed an unmarked `kubyl-views`; install adopted existing namespaces* —
   fixed: `own_namespace` refuses a namespace it didn't create, `--delete` removes
   `kubyl-views` only when marked.
6. *Generic `$CONTEXT` without a dev-cluster check* — fixed: `KUBYL_VIEWS_CONTEXT` (default
   `kind-kubyl-dev`), a context not named `kind-*` needs `--force`. The other `*-dev.sh`
   scripts also honour `$CONTEXT` without a guard: follow-up outside this phase.
7. *Agent `list_resources` printed `-` for related columns* — fixed by dropping them there
   (`columns::is_related`, test `list_columns_skip_what_only_the_list_fills`): one line in
   `kubyl_agent_core`, which phase 21 owns.

**Nits**
- Related watches of hidden columns: fixed (only shown columns; re-synced on toggling a column
  or wide mode). No retry when discovery came late: fixed (`sync_sources` re-syncs the related
  watches when its sources didn't change).
- `dedup()` on drivers/pools: fixed (`distinct`).
- `drivers_in` missed `'x' == device.driver`: fixed, both sides of `==`.
- `grants_routes` didn't check `to.group == ""`: fixed.
- Store name `slices` for both kinds: the Service's EndpointSlices are `endpointslices` now.
- `KNOWN_KINDS`: added `networkpolicies`, `ingressclasses`, `endpoints` (test updated).
- Plan checkbox about `BUILT_IN_GROUPS`: reworded to what was done.
- AGENTS.md long line: reflowed.

**Verification**: `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`,
`cargo test --workspace` (1115 passed, 77 ignored), `cargo deny check`,
`script/check-core-crates.sh` pass. `views-dev.sh` re-run on kubyl-dev (its own marked
namespaces), the live tests (3) pass. Screenshots of the claims list (health per claim), the
slices list, a slice's and a node's devices (holders), a Service's routes, a Gateway's
attached routes and a binding's param link match the board.


### 2026-10-07 (second review)

A fresh review of the whole change set (core models against the DRA, admission, Gateway API,
VPA and EndpointSlice APIs; indexes and caches; watch scoping; catalog gating; the shared-crate
changes; the dev script; tests). Each finding checked against the code.

**Fixed**
1. *Details watches didn't follow edits of the object itself* (a Gateway's listeners, a
   binding's `policyName` or namespace selector, a VPA's `targetRef`, e.g. after saving in the
   YAML tab: the old scope stayed until the details were reopened). `views::views_inputs` is
   what `load_views_related` reads from the object; the source observer redoes the watches when
   it changes (new stores acquired before the old ones drop, cached derived values reset).
   Test: `watches_are_redone_when_their_inputs_change`.
2. *A policy's bindings were filtered on every render* (the handoff said cached): the
   `bindings` derived value is the policy's own bindings now.
3. *Flaky lease test*: `vpas_and_cluster_kinds` expected exactly "90s ago", which depends on
   the test's run time; it checks the tone and a fresh lease instead.
4. *Restored tabs of ListenerSets, XListenerSets, BackendTLSPolicies and DeviceTaintRules got
   a wrong kind before discovery* (`Devicetaintrule`): added to `KNOWN_KINDS`.
5. *No test for the per-row-namespace scoping of related list watches*: the namespace logic is
   `list::distinct_namespaces` now, tested with the cap
   (`related_watches_follow_the_namespaces_of_the_rows`).
6. *`section()` built its debug selector string eagerly on every render*: it clones the id.
7. *Plan text*: the Gateway deviation said routes from namespaces a `Selector` doesn't pick
   aren't listed; they are (the watch is cluster-wide once any listener allows others).
   Reworded, and routes attached through ListenerSets noted. Live test file: `shows` moved
   below the imports.

**Checked, no change**
- Version handling (`exactly` vs request fields, `basic`, capacity forms, `firstAvailable`,
  consumable-capacity sharing), Gateway defaults (parent group/kind/namespace, backend kind
  `Service` in group `""`, weight 1, `PathPrefix /`, ReferenceGrant `to.group == ""` and
  optional `to.name`), `status_of` per listener, admission defaults (failure policy `Fail`,
  webhook port 443, URL query dropped), kubelet health names (`claim:<pod claim>/<request>`):
  correct.
- Indexes: list `RelatedIndex` rebuilt only on a generation change of its stores; details
  `derived` stamped by store entity and generation, reset with the target; no derived value
  depends on the object itself. Watches: released with `Related`/`RelatedSource`; claims list
  watches pods per row namespace (whole cluster above 10).
- Route `kind` from list items: CRD lists carry it; `routes_in` fills it from the store name
  anyway. Binding `kind` (built-in lists drop it): `binding_link` falls back on
  `validationActions`.
- `CellButton::link` (kubyl_core) and `tool_columns` (kubyl_agent_core) are the only
  shared-crate changes besides `kubyl_palette` (short names), `kubyl_yaml_core` (templates),
  `kubyl_explorer_core` (doc comment) and the mockups; builds against the credentials API of
  `main` (6740fdb).
- `views-dev.sh`: guarded to `kind-*` contexts, refuses foreign namespaces, deletes only marked
  objects. No panics found on cluster data (string slicing only at ASCII quotes).

**Skipped**
- A Service's `routes-from:` watches stay until the details close when a ReferenceGrant is
  removed (the routes are filtered by the grants on every change, so nothing stale shows).
- Screenshots: nothing visual changed in this review.

**Verification**: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace` (1124 passed, 77 ignored), `cargo deny check`,
`script/check-core-crates.sh` pass. `views-dev.sh` re-run on kubyl-dev; the live tests (3)
pass.

### 2026-10-08 (CodeRabbit review of PR #45, rebased on Helm and Flux)

Rebased onto `main` with phases 22 (Helm) and 23 (Flux) merged (one conflict: the module list of
`kubyl_resources_core`, both `errors` and `dra` kept). Four review findings, each checked:

**Fixed**
1. *Admission details: the "Resources" and "Excluded" rule rows shared element ids* (one call
   site in `match_section`): the second list's indices are offset by the first one's length.
2. *`RelatedSpec` didn't hold the source's cluster and namespace*: `sync_related` kept the
   related watches when a source changed scope. Both are part of the spec now (test:
   `related_specs_differ_by_source_scope`).
3. *`describe` on an object without `apiVersion` (watch-cache rows) derived an empty group*:
   the agent attachment (`kubyl_agent::object_chip`) and the `describe` tool
   (`kubyl_agent_core::tools`) call `describe_as` with the target's group.
4. *`views-dev.sh` downloads with curl but only checked kubectl and helm*: curl is checked and
   listed under "Needs:".
