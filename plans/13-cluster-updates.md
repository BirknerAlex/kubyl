# Phase 13: Cluster updates

**Status:** in progress
**Depends on:** 02, 07 (metrics for deprecated-API usage), 12 (operator compatibility check)
**Owns:** `crates/kubyl_updates`
**Mockups:** board 8 · Cluster updates, OpenShift-style

## Goal

Show the current version, available updates and the update path. Run pre-flight checks. Start
and track control plane and node pool upgrades through a provider abstraction, because every
distribution upgrades differently.

## Tasks

### Provider abstraction
- [ ] `UpdateProvider` trait: `detect(&ClusterCaps)`, `current()`, `available(channel)`, `graph()`, `preflight_extras()`, `start(target, plan)`, `progress() -> Stream`, `history()`
- [ ] **OpenShift**: `ClusterVersion` (`config.openshift.io/v1`): channel, `availableUpdates`, `conditionalUpdates` (with risks), update history, progress from conditions and ClusterOperators. Kubernetes API only, no extra credentials
- [ ] **Amazon EKS**: `aws-sdk-eks` (cluster and nodegroup versions, `UpdateClusterVersion`, `UpdateNodegroupVersion`, add-on versions/compatibility). Credentials from the same AWS profile as the kubeconfig exec plugin
- [ ] **GKE**: Container API (`get_server_config` for channel versions, `update_master`, node pool upgrades). Credentials via Application Default Credentials / gcloud
- [ ] **AKS**: `az`-compatible REST (`upgradeProfiles`, agent pool upgrades). Credentials via Azure CLI token
- [ ] **k3s / RKE2**: system-upgrade-controller `Plan` CRs (create/track). **Cluster API**: `KubeadmControlPlane`/`MachineDeployment` version bumps
- [ ] Fallback "unknown/self-managed": read-only version info, preflight checks, a link to docs
- [ ] Each cloud provider is behind a cargo feature to keep the default binary small (`updates-eks`, `updates-gke`, `updates-aks`)

### Pre-flight checks (provider-independent)
- [ ] Deprecated/removed APIs still in use: `apiserver_requested_deprecated_apis` metric (Prometheus from 07, or `/metrics` if allowed) plus a scan of stored objects and Helm manifests against the target version's removal list (bundled table, updated each release)
- [ ] PodDisruptionBudgets that block drain (allowed disruptions = 0)
- [ ] Node capacity headroom for surge upgrades
- [ ] Installed operators compatible with the target version (OLM `maxKubeVersion`/`minKubeVersion`, from 12)
- [ ] Add-on compatibility (provider-supplied: EKS add-ons, GKE/AKS components)
- [ ] Version skew: kubelet vs control plane, and the skip-version rules
- [ ] Each check is pass/warn/fail with an explanation and a "fix" action (open the PDB, update the add-on…)

### UI
- [ ] Current version card (version, platform version, support window, channel selector, last update)
- [ ] Update path graph (current → recommended → further; blocked or not-recommended updates show the reason)
- [ ] Pre-flight results list with re-run
- [ ] Control plane and node pools table with rolling progress (nodes updated/total, currently draining node, surge settings)
- [ ] Update confirmation: summary, checks, typed cluster name on PROD. Updates can't be undone, so say so

### OpenShift Routes (added on request)
- [ ] Explorer: "Routes" in the Network section (after Ingresses), only on clusters that serve `route.openshift.io`; dropped from Custom Resources like Gateways; catalog unit tests
- [ ] Columns (`kubyl_resources`): Name, Host (with `spec.path`), Services (`spec.to` + `spec.alternateBackends`, weights when there's more than one), Target port, TLS (edge/passthrough/reencrypt and the insecure policy None/Allow/Redirect), Admitted (per router, from `status.ingress[].conditions`), Age
- [ ] Details: host(s) and URL, TLS termination and insecure policy, wildcard policy, router name and canonical hostname per `status.ingress` entry with Admitted conditions and reasons, related objects (backend Service(s) → Endpoints/EndpointSlices → Pods, the owner Ingress of generated Routes), "Open in browser" (https for edge/reencrypt/passthrough, http otherwise, plus the path; not for wildcard hosts)
- [ ] Web view (`kubyl_webview`): a "Web view" section like Ingress's that opens the backend Service through a temporary forward; the picker lists Route backends too
- [ ] Port-forward (`kubyl_portforward`): ⇧F and alt-⇧F on a Route forward its backend Service at the port the Route's target port resolves to (Ingresses too); running forwards show next to the Route
- [ ] Target port resolution like OpenShift's router (`spec.port.targetPort` names the Service's target port: a port name or the targetPort number; no `spec.port` = the Service's first port), unit-tested with named and numeric ports, a missing port and several backends
- [ ] Palette references: Route → Service(s), owner Ingress; Service → the Routes that point at it
- [ ] YAML template: a Route (edge TLS, Redirect)
- [ ] `spec.tls.key` (inline private key) masked like Secret data in details, the YAML view and diffs; never logged, copied without an explicit action, or shown in toasts or the palette
- [ ] Tests: columns, URL building, target port resolution, masking (plus a GPUI test where a view changes behaviour)
- [ ] Screenshots on the fake-OpenShift kind cluster: the Routes list with details, a Route web view (`design/screenshots/phase-13-routes*.png`)

## Acceptance criteria

- On OpenShift (CRC/OKD or a test cluster): channel and available updates match `oc adm upgrade`, and progress is tracked during an update.
- On EKS (test account): pre-flight flags a blocking PDB and a deprecated API. The control plane update starts and is tracked to completion.
- On kind: read-only info and preflight checks work, with no crashes.

## Risks

- Cloud SDKs add a lot of binary size and compile time. Keep them feature-gated, and consider a separate helper process later.
- Testing needs real cloud accounts. Keep provider logic thin and well unit-tested with recorded responses.

## Handoff log
