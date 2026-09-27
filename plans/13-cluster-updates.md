# Phase 13: Cluster updates

**Status:** done
**Depends on:** 02, 07 (metrics for deprecated-API usage), 12 (operator compatibility check)
**Owns:** `crates/kubyl_updates`
**Mockups:** board 8 · Cluster updates, OpenShift-style

## Goal

Show the current version, available updates and the update path. Run pre-flight checks. Start
and track control plane and node pool upgrades through a provider abstraction, because every
distribution upgrades differently.

## Tasks

### Provider abstraction
- [x] `UpdateProvider` trait: `detect(&ClusterCaps)`, `current()`, `available(channel)`, `graph()`, `preflight_extras()`, `start(target, plan)`, `progress() -> Stream`, `history()` (built as `read` (one snapshot of all of these) + `preflight_extras` + `plan` + `start`, detection in `detect::detect`, progress by reading again; see the handoff log)
- [x] **OpenShift**: `ClusterVersion` (`config.openshift.io/v1`): channel, `availableUpdates`, `conditionalUpdates` (with risks), update history, progress from conditions and ClusterOperators. Kubernetes API only, no extra credentials
- [x] **Amazon EKS**: `aws-sdk-eks` (cluster and nodegroup versions, `UpdateClusterVersion`, `UpdateNodegroupVersion`, add-on versions/compatibility). Credentials from the same AWS profile as the kubeconfig exec plugin (REST + SigV4 instead of the SDK; recorded responses only, no account)
- [x] **GKE**: Container API (`get_server_config` for channel versions, `update_master`, node pool upgrades). Credentials via Application Default Credentials / gcloud
- [x] **AKS**: `az`-compatible REST (`upgradeProfiles`, agent pool upgrades). Credentials via Azure CLI token
- [x] **k3s / RKE2**: system-upgrade-controller `Plan` CRs (create/track). **Cluster API**: `KubeadmControlPlane`/`MachineDeployment` version bumps
- [x] Fallback "unknown/self-managed": read-only version info, preflight checks, a link to docs
- [x] Each cloud provider is behind a cargo feature to keep the default binary small (`updates-eks`, `updates-gke`, `updates-aks`)

### Pre-flight checks (provider-independent)
- [x] Deprecated/removed APIs still in use: `apiserver_requested_deprecated_apis` metric (Prometheus from 07, or `/metrics` if allowed) plus a scan of stored objects and Helm manifests against the target version's removal list (bundled table, updated each release)
- [x] PodDisruptionBudgets that block drain (allowed disruptions = 0)
- [x] Node capacity headroom for surge upgrades
- [x] Installed operators compatible with the target version (OLM `maxKubeVersion`/`minKubeVersion`, from 12)
- [x] Add-on compatibility (provider-supplied: EKS add-ons, GKE/AKS components) (EKS add-ons and upgrade insights; GKE and AKS have no component check yet, see the handoff log)
- [x] Version skew: kubelet vs control plane, and the skip-version rules
- [x] Each check is pass/warn/fail with an explanation and a "fix" action (open the PDB, update the add-on…)

### UI
- [x] Current version card (version, platform version, support window, channel selector, last update)
- [x] Update path graph (current → recommended → further; blocked or not-recommended updates show the reason)
- [x] Pre-flight results list with re-run
- [x] Control plane and node pools table with rolling progress (nodes updated/total, currently draining node, surge settings)
- [x] Update confirmation: summary, checks, typed cluster name on PROD. Updates can't be undone, so say so

### OpenShift Routes (added on request)
- [x] Explorer: "Routes" in the Network section (after Ingresses), only on clusters that serve `route.openshift.io`; dropped from Custom Resources like Gateways; catalog unit tests
- [x] Columns (`kubyl_resources`): Name, Host (with `spec.path`), Services (`spec.to` + `spec.alternateBackends`, weights when there's more than one), Target port, TLS (edge/passthrough/reencrypt and the insecure policy None/Allow/Redirect), Admitted (per router, from `status.ingress[].conditions`), Age
- [x] Details: host(s) and URL, TLS termination and insecure policy, wildcard policy, router name and canonical hostname per `status.ingress` entry with Admitted conditions and reasons, related objects (backend Service(s) → Endpoints/EndpointSlices → Pods, the owner Ingress of generated Routes), "Open in browser" (https for edge/reencrypt/passthrough, http otherwise, plus the path; not for wildcard hosts)
- [x] Web view (`kubyl_webview`): a "Web view" section like Ingress's that opens the backend Service through a temporary forward; the picker lists Route backends too
- [x] Port-forward (`kubyl_portforward`): ⇧F and alt-⇧F on a Route forward its backend Service at the port the Route's target port resolves to (Ingresses too); running forwards show next to the Route
- [x] Target port resolution like OpenShift's router (`spec.port.targetPort` names the Service's target port: a port name or the targetPort number; no `spec.port` = the Service's first port), unit-tested with named and numeric ports, a missing port and several backends
- [x] Palette references: Route → Service(s), owner Ingress; Service → the Routes that point at it
- [x] YAML template: a Route (edge TLS, Redirect)
- [x] `spec.tls.key` (inline private key) masked like Secret data in details, the YAML view and diffs; never logged, copied without an explicit action, or shown in toasts or the palette
- [x] Tests: columns, URL building, target port resolution, masking (plus a GPUI test where a view changes behaviour)
- [x] Screenshots on the fake-OpenShift kind cluster: the Routes list with details, a Route web view (`design/screenshots/phase-13-routes*.png`)

## Acceptance criteria

- On OpenShift (CRC/OKD or a test cluster): channel and available updates match `oc adm upgrade`, and progress is tracked during an update.
- On EKS (test account): pre-flight flags a blocking PDB and a deprecated API. The control plane update starts and is tracked to completion.
- On kind: read-only info and preflight checks work, with no crashes.

## Risks

- Cloud SDKs add a lot of binary size and compile time. Keep them feature-gated, and consider a separate helper process later.
- Testing needs real cloud accounts. Keep provider logic thin and well unit-tested with recorded responses.

## Handoff log

### 2026-09-27 (branch `phase/13-cluster-updates`, one PR with the OpenShift Routes work)

**Shipped.** Mockups first (board 8: the OpenShift page with operators, pools and history;
version card and channel selector; update graph with recommended, conditional and blocked
updates; pre-flight; progress during an update; the confirmation; EKS; k3s Plans; the
self-managed and read-only fallback; credentials missing and provider not built. Board 1/10:
Routes under Network with details, and the Route web view), then the README decisions
(providers, writes, removed and deprecated APIs, cloud credentials, features, Routes) and
Extension points, then the shared-crate commits (the Routes work, one commit per crate; `hmac`
in the workspace; the `kubyl` crate forwards the features; CI builds and tests the features)
and `kubyl_updates`:

- `provider::UpdateProvider` (object-safe, `BoxFuture`s on Tokio): `kind`, `read` (one
  `model::Status`: current version, channel and channels, targets as recommended / available
  / conditional / blocked with risks and reasons, history, progress, components, pools,
  add-ons, notes, what it may write, docs), `preflight_extras(status, target)`, `plan(status,
  scope, target, cluster)` (the summary a write shows; `Scope::{ControlPlane, Pool, AllPools,
  AddOn, Channel}`) and `start(plan)`. The plan's list (`current`, `available`, `graph`,
  `history`, `progress() -> Stream`) is all in `read`: one request round per provider, and
  progress is reading again (`service::Updates` polls every 15 s while a tab shows the cluster,
  5 s during an update, right after a write and on Re-check; nothing without a view).
- `detect`: OpenShift (serves `clusterversions.config.openshift.io`), EKS / GKE / AKS (server
  host, version suffix, exec plugin), k3s / RKE2 (`+k3s`/`+rke2`), Cluster API management
  clusters (`clusters.cluster.x-k8s.io`), else self-managed. `updates.clusters.<cluster>.provider`
  overrides it.
- Providers: `openshift` (ClusterVersion, ClusterOperators, MachineConfigPools and the node the
  MCO works on; start = merge patch of `spec.desiredUpdate {version, image, force: false}` like
  `oc adm upgrade --to` / `--allow-not-recommended`, channel = `spec.channel`; minor updates
  blocked while `Upgradeable=False`, every target blocked while an update runs; extras: operators
  healthy, pools not paused/degraded, `Upgradeable` and the admin acks of the current minor with
  a copyable `oc patch`), `suc` (k3s/RKE2 system-upgrade-controller Plans, versions from the
  public channel server, progress from each Plan's `latestHash` node labels and `applying`;
  writes patch `spec.version`; "New plans…" opens server and agent Plan templates in the YAML
  editor), `capi` (KubeadmControlPlane / `Cluster.spec.topology.version` / MachineDeployment
  version bumps with the minor and control-plane-first rules; the version is typed),
  `fallback::ReadOnly` (version, nodes by role, docs; also a cloud without its feature or
  credentials, with a note), and the cloud providers in `providers::{eks, gke, aks}` behind
  `updates-eks`, `updates-gke`, `updates-aks` (REST, no SDKs: SigV4 in the crate checked against
  AWS's published vectors, credentials from `aws configure export-credentials` / `sts
  assume-role`, `gcloud auth print-access-token`, `az account get-access-token`, in memory only;
  `KUBYL_UPDATES_{EKS,GKE,AKS}_ENDPOINT` for mock servers).
- `preflight` (Tokio, provider-independent): PDBs with 0 allowed disruptions; deprecated APIs
  still requested (APIRequestCount on OpenShift, else Prometheus `apiserver_requested_deprecated_apis`
  + `apiserver_request_total` over 24 h, else `/metrics`; the run waits up to 15 s for phase 07's
  Prometheus discovery and the Helm list); removed APIs in Helm releases (latest revision decoded
  six at a time, only findings kept, `helm mapkubeapis` hint) and in objects' last-applied
  configuration (metadata lists of the affected kinds); node headroom to drain the busiest node
  per pool (managed clouds surge); version skew and minor-by-minor updates; operators (phase 12's
  `api::installed`, computed live so installs and removals show without a re-run). A check that
  can't read says which verb and resource is missing. `removed::REMOVED` is the bundled table
  (Kubernetes deprecation guide, through v1.32).
- The tab (`view`): header with provider and Re-check; notes (credentials, feature not built, a
  stale read, read-only); progress banner; version card (version, Kubernetes, platform, cluster
  ID, support window, last update, conditions, channel menu); update path graph (edges painted on
  a canvas, click or ↑↓/j k to select; the selection's release notes, channels, image, risks,
  blocked reasons); pre-flight (problems first, passed folded, details and fix buttons, Re-run;
  read-only providers pick the version to check against); cluster operators; pools (MCPs, node
  groups, Plans, workload clusters, nodes by role) with per-pool updates; add-ons; history; docs.
  During an update the operators and pools come first and pre-flight is hidden. Keys `r`
  (re-check), `⇧R` (re-run checks), `u` (update the selection), all in the palette too; write
  hints only where the provider can write and the cluster isn't read-only. Every write opens
  `view::confirm`: cluster, provider, from → to, what changes (the patch, the `oc` command it
  matches), the checks that failed or warn, one checkbox per accepted risk, "I've read the
  pre-flight results" when a check failed, "Updates can't be undone" for updates, the typed
  cluster name on PROD. Writes run once in `service::Updates` (closing the dialog doesn't cancel
  them), never retried; a failure is a toast with the server's message; read-only clusters
  refuse in the service too.
- `script/updates-dev.sh` (see its header and AGENTS.md): the PDB and removed-API Helm release
  on kind, the fake OpenShift cluster `kubyl-ocp` (openshift/api CRDs at release-4.17, a
  ClusterVersion with updates, conditional updates and an admin gate, 33 ClusterOperators, MCPs,
  APIRequestCounts, Routes with real backends; `--ocp-stage`, `--ocp-update`, `--fake-cvo`), and
  the k3d cluster `kubyl-k3s` with system-upgrade-controller; `--delete` removes only what it
  marked.

**OpenShift Routes** (the "added on request" block): `kubyl_resources::route` (model, columns,
target port resolution like the router, including numeric target ports through the endpoints,
URLs, key masking), Routes under Network (only where served, not under Custom Resources), Route
details (hosts and URL with "Open in browser", TLS with the key masked like Secret data and an
explicit reveal and copy, routers with Admitted conditions, backends with weights, resolved
ports and forwards, endpoints, pods; the owner Ingress in the owner chain), the "Web views"
section and `w`, ⇧F / alt-⇧F on Routes and Ingresses (the primary backend Service at the
resolved port; alternate backends forward from their rows), palette references both ways, the
Route template, and the key masked in the YAML editor and its diffs, Copy YAML, Describe, Helm
manifests and Argo CD diffs.

**Verified.**
- Tests: `kubyl_updates` 28 unit/GPUI tests without features, 91 with all (the workspace: 653 passed) (SigV4 vectors,
  parsers and planners for every provider on recorded responses, mock-server runs of read, plan
  and start); the Routes work added unit tests in each crate and a GPUI test for the masked key.
  fmt, clippy (`-D warnings`, also `--all-features`), `cargo test --workspace` and `cargo deny
  check` (also `--all-features`) pass.
- Live (`crates/kubyl_updates/tests/live.rs`, all passing): kind read-only view and pre-flight
  (the `kubyl-updates/ledger-writer-pdb` PDB fails, `legacy-app`'s removed APIs are found,
  skew passes); the fake OpenShift (targets, risks, the blocked minor, APIRequestCount, admin
  gates); an update started through `start` and tracked with `--fake-cvo` from 12 % to 61 % to
  89 % with the draining node, to completion; k3s Plans read, then a real update of the k3d
  cluster through the Plans (v1.36.3+k3s1 → v1.36.4+k3s1, server then agent) tracked to
  completion. Route backends: `crates/kubyl_portforward/tests/live.rs` forwards every Route's
  backend on the fake cluster.
- The user's OpenShift test cluster, read-only: version, channel, the six channels and the six
  updates match `oc adm upgrade`, no conditional updates, history, 34 operators and both pools;
  the pre-flight checks run there (APIRequestCount, PDBs, 3,300 objects scanned). The Routes
  list of a namespace matches `oc get routes` (paths, services, named and missing target ports,
  edge/Redirect, admitted), and forwards to every Route backend there answered HTTP (the user
  allowed forwards and web views; nothing was changed).
- In the app (screenshot harness, `design/screenshots/phase-13-*.png`, example.com names):
  the OpenShift page, channel menu, conditional update selected (pre-flight follows the
  selection), expanded pre-flight, progress, the PROD confirmation, EKS against a local mock of
  its API (control plane, node groups, add-ons, insights), credentials missing (a fake `aws`
  with an expired SSO session), provider not built (GKE in a build without `updates-gke`), k3s,
  kind (self-managed), read-only, a 403 from a view-only service account, Routes with details,
  the Route web views section, a Route backend web view.

**Deferred / notes.**
- OpenShift: no update was started on the user's cluster (the user said read-only); progress
  tracking is verified on the fake ClusterVersion with the fake CVO. To do when a z-stream update
  is due there: start it from Kubyl and watch the progress card.
- EKS, GKE, AKS: no test accounts. Recorded responses and a mock server only; the EKS
  acceptance (blocking PDB and deprecated API flagged, control plane update started and tracked
  on a real cluster) is deferred. Open API questions are in the provider modules' docs (EKS
  `DescribeClusterVersions`/insights paths, GKE operation metrics, AKS `2024-09-01` PUT/eTag).
- GKE and AKS component compatibility checks: none yet (EKS add-ons and insights exist).
- Cluster API: fixtures only (clusterctl isn't installed); the provider is untested live.
- Route TLS: certificate subject and expiry aren't shown (only "present"); that would need an
  X.509 parser in `kubyl_explorer`.
- The fake cluster's `/version` is kind's (v1.37), so its title bar says v1.37 next to
  OpenShift 4.17; real clusters report their own.
- `kubyl_kube`'s `groups_merge_member_settings_and_start_in_the_current_namespace` failed once
  in a full `cargo test --workspace` under load and passed on every rerun (not touched here).
- The published mockup artifact (claude.ai) must be republished from
  `design/mockups/generate.py`: boards 11–17, the phase 12 board 7 frames, the new board 8
  frames and the Route frames.
