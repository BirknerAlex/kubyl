# Phase 08: Operators (OLM) and Helm releases

**Status:** not started
**Depends on:** 02, 04 (YAML view/diff for InstallPlans and CR creation)
**Owns:** `crates/kubyl_operators`
**Mockups:** board 7 · Operators (OLM), OpenShift-style

## Goal

OpenShift-console-style operator management on any cluster with OLM: see installed operators,
browse OperatorHub, install, approve upgrades, and create instances of the APIs they provide.
Also list the Helm releases that manage cluster content.

## Tasks

### OLM v0 (operators.coreos.com)
- [ ] Detect OLM (CRDs `clusterserviceversions`, `subscriptions`, `installplans`, `catalogsources`, `operatorgroups`, and the `packagemanifests` aggregated API). Show "OLM not installed" with an install hint (link to `script/olm-dev.sh` for dev)
- [ ] Installed operators: join Subscription ↔ CSV ↔ InstallPlan. Columns: name, package, version, status (Succeeded / Installing / Failed / Upgrade available), channel, approval, provided APIs
- [ ] Details dock: pending upgrade card (from → to, InstallPlan name), CRD changes (diff of CRD schemas old vs new), RBAC changes (permissions added/removed), compatibility notes. Approve / View YAML / Diff CRDs actions
- [ ] Provided APIs: `owned` CRDs from the CSV with instance counts, "Create" (opens phase 04 with the CSV's `alm-examples` as template)
- [ ] OperatorHub: browse PackageManifests from all CatalogSources, with search, categories, provider, capability level, channels. Install flow: pick channel, install mode (all namespaces / own namespace), approval strategy, target namespace/OperatorGroup (created if needed)
- [ ] Uninstall: delete Subscription and CSV, optionally CRDs and instances (strongly guarded, blocked on PROD without typed confirmation)
- [ ] Install plans tab (pending first) and Subscriptions tab

### OLM v1 (olm.operatorframework.io)
- [ ] Detect `ClusterExtension` / `ClusterCatalog`. Show installed extensions, versions, conditions. Basic install/upgrade via YAML templates (full UI later, marked as follow-up)

### Helm releases
- [ ] Read Helm v3 release Secrets (`owner=helm`, `sh.helm.release.v1.*`): base64 + gzip → release JSON. List releases (name, namespace, chart, app version, revision, status, updated)
- [ ] Release details: values (YAML view), manifest, notes, history. Resources in the release, linked to their views
- [ ] Rollback to a revision / uninstall: **out of scope for v1** (read-only). Show a "copy helm command" button instead

## Acceptance criteria

- On kind with OLM plus the operatorhub.io catalog: install cert-manager from OperatorHub, see it succeed, create a `Certificate` from `alm-examples`.
- A manual-approval subscription shows "Upgrade available". Approving it moves the install forward.
- Helm releases on the dev cluster list correctly, with values and manifest.

## Handoff log
