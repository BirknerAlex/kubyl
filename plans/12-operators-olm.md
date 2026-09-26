# Phase 12: Operators (OLM) and Helm releases

**Status:** done
**Depends on:** 02, 04 (YAML view/diff for InstallPlans and CR creation)
**Owns:** `crates/kubyl_operators`
**Mockups:** board 7 · Operators (OLM), OpenShift-style

## Goal

OpenShift-console-style operator management on any cluster with OLM: see installed operators,
browse OperatorHub, install, approve upgrades, and create instances of the APIs they provide.
Also list the Helm releases that manage cluster content.

## Tasks

### OLM v0 (operators.coreos.com)
- [x] Detect OLM (CRDs `clusterserviceversions`, `subscriptions`, `installplans`, `catalogsources`, `operatorgroups`, and the `packagemanifests` aggregated API). Show "OLM not installed" with an install hint (link to `script/olm-dev.sh` for dev)
- [x] Installed operators: join Subscription ↔ CSV ↔ InstallPlan. Columns: name, package, version, status (Succeeded / Installing / Failed / Upgrade available), channel, approval, provided APIs
- [x] Details dock: pending upgrade card (from → to, InstallPlan name), CRD changes (diff of CRD schemas old vs new), RBAC changes (permissions added/removed), compatibility notes. Approve / View YAML / Diff CRDs actions
- [x] Provided APIs: `owned` CRDs from the CSV with instance counts, "Create" (opens phase 04 with the CSV's `alm-examples` as template)
- [x] OperatorHub: browse PackageManifests from all CatalogSources, with search, categories, provider, capability level, channels. Install flow: pick channel, install mode (all namespaces / own namespace), approval strategy, target namespace/OperatorGroup (created if needed)
- [x] Uninstall: delete Subscription and CSV, optionally CRDs and instances (strongly guarded, blocked on PROD without typed confirmation)
- [x] Install plans tab (pending first) and Subscriptions tab

### OLM v1 (olm.operatorframework.io)
- [x] Detect `ClusterExtension` / `ClusterCatalog`. Show installed extensions, versions, conditions. Basic install/upgrade via YAML templates (full UI later, marked as follow-up)

### Helm releases
- [x] Read Helm v3 release Secrets (`owner=helm`, `sh.helm.release.v1.*`): base64 + gzip → release JSON. List releases (name, namespace, chart, app version, revision, status, updated)
- [x] Release details: values (YAML view), manifest, notes, history. Resources in the release, linked to their views
- [x] Rollback to a revision / uninstall: **out of scope for v1** (read-only). Show a "copy helm command" button instead

## Acceptance criteria

- On kind with OLM plus the operatorhub.io catalog: install cert-manager from OperatorHub, see it succeed, create a `Certificate` from `alm-examples`.
- A manual-approval subscription shows "Upgrade available". Approving it moves the install forward.
- Helm releases on the dev cluster list correctly, with values and manifest.

## Handoff log

### 2026-09-26 (branch `phase/12-operators-olm`, one PR)

**Shipped.** Mockups first (board 7 frames: OperatorHub, install dialog, upgrade review,
Create from `alm-examples`, Install plans, Subscriptions, Helm releases, a release, the OLM
states; the sidebar's "Helm Releases" row), then the decisions (README table: Operators OLM v0,
upgrade review, OperatorHub, install/uninstall, OLM v1, Helm releases, Operators views) and the
shared-crate commits: `kubyl_yaml::open_draft` (a new-resource editor with given text and a
note banner), the `anchor` icon, the explorer's "Helm Releases" row under Administration (not
gated on OLM) and `flate2` in the workspace. Then `kubyl_operators`:

- `olm::model`: Subscriptions, CSVs (copies filtered with `!olm.copiedFrom`), InstallPlans (steps
  and their bundle ConfigMaps), CatalogSources, OperatorGroups (global / own / single /
  multi), properties (`olm.maxOpenShiftVersion`, `olm.maxKubeVersion`), `alm-examples`,
  deprecation. `olm::join`: Subscription ↔ CSV ↔ InstallPlan with the statuses Failed,
  Approval required, Upgrade available, Installing, Deleting, Succeeded; a `ResolutionFailed`
  subscription with a running CSV keeps its status and says why (OLM reports it for a moment
  while it replaces a CSV).
- `olm::review` (the pending-upgrade card and dialog): the new bundle's CRDs and CSV from the
  InstallPlan's bundle ConfigMaps (`gzip+base64`), compared with the served CRDs (server
  defaults, `caBundle` and `conversion: None` ignored; a dropped version still in
  `storedVersions` is a blocker like OLM's), RBAC added/removed per rule (cluster and namespace
  scope), compatibility (`minKubeVersion`, max OpenShift/Kubernetes versions, install mode
  still supported).
- `olm::hub` + `hub.rs` (OperatorHub): PackageManifests fetched and parsed on Tokio (449
  packages, ~11 MB, cached per cluster, `⇧R` reloads), icons through the `icon` subresource
  (≤512 KB, lazily), category / capability / provider / catalog filters, search and sort,
  plain-text descriptions, details pane, `i` installs.
- `olm::ops`: install plans (`plan_install` picks the global OperatorGroup, creates one named
  after the namespace for single-namespace installs, refuses mismatched modes, several groups
  or a second subscription), Subscription creation, approve (merge patch), uninstall
  (instances first, waits up to 2 min, then Subscription, CSV, CRDs), all through
  `kubyl_core::spawn_kube` with field manager `kubyl`.
- `olm::v1`: ClusterExtensions (Installed, Blocked, Retrying, Installing, Failed) and
  ClusterCatalogs, the ClusterExtension template.
- `helm::decode` (Secret and ConfigMap drivers, plain JSON fallback; `Release`'s `Debug` has no
  values or manifest), `helm::present` (values masked until Reveal, Secret data masked in
  manifests, manifest objects, `helm` commands with quoted arguments), `helm::service`
  (metadata-only watches of Secrets and ConfigMaps labelled `owner=helm`, latest revision per
  release decoded on Tokio, six at a time; 403 falls back to the active namespace and says
  what's missing).
- Views: the Operators tab (Installed · Install plans · Subscriptions · Helm releases ·
  Extensions; details panes, keys `enter a d c y e ⌃d` / `v m h c` / `n`, `/` filter, bound on
  the lists only), the upgrade review dialog (CRD list changed-first, unified/side-by-side
  diff, RBAC, compatibility), install dialog (channel, version, install mode, approval, "What
  Kubyl creates"), uninstall dialog (with CRDs and instance counts), Create from
  `alm-examples` (kind picker when an operator has several), the release tab (Values ·
  Manifest · Notes · History · Resources, revision picker, keys `1–5 r [ ] c`), the states (no
  OLM with an install hint, not connected, loading, can't read OLM). Writes are hidden on
  read-only clusters; every write shows a summary first; PROD asks for the typed cluster name
  (install; Create applies through the YAML editor's own PROD check) or package name (approve); uninstall asks for the operator name on PROD
  and whenever CRDs and instances go too.
- `api::installed` for phase 13 (README, Extension points).
- `script/olm-dev.sh` (OLM v0.46.0 with the operatorhub.io catalog, a manual-approval
  cloudnative-pg subscription pinned with `startingCSV`; `--reset-manual`, `--v1` with
  operator-controller v1.12.0 and a grafana-operator ClusterExtension, `--delete`) and
  `crates/kubyl_operators/tests/live.rs`.

**Verified.** 25 unit/GPUI tests in `kubyl_operators` (parsers, joins and statuses incl. the
transient `ResolutionFailed`, CRD diff normalization and dropped stored versions, RBAC diff,
install planning, bundle and release decoding for both drivers, masking, no values in `Debug` or
errors, filters, 5,000 operators keeping the selection) plus `kubyl_yaml`'s draft test. Live on
kind (`--ignored --test-threads=1`, 7/7): join, 449 PackageManifests in ~0.2 s with icons, the
review of cloudnative-pg 1.30.0 → 1.30.1 (2 of 11 CRDs change, no RBAC changes), approve,
install cert-manager and create an Issuer and Certificate from `alm-examples`, uninstall
`ack-recyclebin-controller` with its CRD, Helm releases decoded with values and manifest. The
acceptance criteria in the app on kind (context marked PROD): cert-manager installed from
OperatorHub (typed name) → `cert-manager.v1.16.5` Succeeded; Issuer and Certificate created
from `alm-examples` → Ready; the manual subscription shows "Upgrade to 1.30.1", the review
dialog lists the CRD changes, Approve (typed package name) → 1.30.1 installed; the Helm tab
lists kube-prometheus-stack and metrics-server, values masked, manifest with 99 objects. A
view-only service account sees the 403 message instead of an empty list.

**Screenshots** (`design/screenshots/`, kind, example.com names): `phase-12-installed.png`,
`phase-12-upgrade-card.png`, `phase-12-upgrade-review.png` (PROD, typed package name),
`phase-12-operatorhub.png`, `phase-12-install-dialog.png`, `phase-12-create-from-examples.png`,
`phase-12-install-plans.png`, `phase-12-subscriptions.png`, `phase-12-uninstall-dialog.png`,
`phase-12-extensions-olm-v1.png`, `phase-12-helm-releases.png`,
`phase-12-helm-release-values.png`, `phase-12-helm-release-manifest.png`,
`phase-12-helm-forbidden.png`, `phase-12-olm-not-installed.png`.

**Deferred / notes.**
- OpenShift: the read-only check on the user's OpenShift test cluster didn't happen (its token
  had expired). To do after `oc login`: Installed Operators with copied CSVs hidden,
  OperatorHub with the Red Hat catalogs, `openshift-operators` as the global group,
  `olm.maxOpenShiftVersion` notes.
- OLM v1: lists and YAML templates only; a full UI (install from the catalog, upgrade review,
  uninstall) is a follow-up.
- Helm stays read-only (rollback, upgrade, uninstall are "Copy helm command"); the SQL driver
  isn't supported.
- CRD diffs with deep indentation are clipped in the unified view (`kubyl_yaml::diff_view` has
  no horizontal scroll); side by side shows more.
- The published mockup artifact (claude.ai) doesn't have boards 11–17 and the new board 7 frames
  yet: republish it from `design/mockups/generate.py`.
