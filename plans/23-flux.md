# Phase 23: Flux CD

**Status:** done (branch `phase/23-flux`; see the handoff log)
**Depends on:** 02 (lists, details, actions), 04 (YAML editor), 05 (controller logs), 07 (events); 22 optional (link a HelmRelease to its Helm release), 21 optional ("Ask agent")
**Owns:** `crates/kubyl_flux` (new), `crates/kubyl_flux_core` (new), `script/flux-dev.sh`
**Mockups:** board 21 · Flux, to be added to `design/mockups/generate.py` before the UI work: the Flux dashboard (health, counts, needs attention, recent activity), the Kustomizations list, a Kustomization's details (conditions, source and revision, inventory tree, dependencies), a HelmRelease's details (chart, values sources, history, link to the Helm release), the Sources list, the "Managed by Flux" note in an object's details with the Flux column in workload lists, and the reconcile/suspend actions with their confirmations.

## Goal

See and drive Flux from Kubyl when a cluster runs it, like phase 10 does for Argo CD: what Flux
manages, whether it's healthy, what changed recently, and why something fails. Then act on it:
reconcile now, suspend, resume. The Flux UI appears only when Flux's CRDs are served.

Lens shows Flux read-only (paid). Kubyl adds the actions the `flux` CLI offers, without needing
the CLI: Flux's controllers take every action as an annotation or a `spec.suspend` patch.

## Decisions to make first

Each has a recommendation. Record the outcomes in the README's decision table.

1. **Access.** Recommended: Kubernetes only. Flux has no API server; everything is in its CRDs,
   their status, Events and the controllers' logs. Kubyl reads through `ResourceStores` and acts
   by patching, with the user's RBAC. No `flux` CLI needed.
2. **API versions.** Recommended: the preferred served version of each kind through discovery
   (Flux 2.x GA: `source.toolkit.fluxcd.io/v1`, `kustomize.toolkit.fluxcd.io/v1`,
   `helm.toolkit.fluxcd.io/v2`, `notification.toolkit.fluxcd.io/v1beta3`,
   `image.toolkit.fluxcd.io/v1beta2`), reading status fields that are the same across them;
   older beta versions show what they can. Each kind appears only when served.
3. **Actions** (what `flux` does):
   - Reconcile: annotate `reconcile.fluxcd.io/requestedAt=<now>`. "With source" annotates the
     source first, then the object.
   - HelmRelease extras: force (`reconcile.fluxcd.io/forceAt`) and reset failure counters
     (`reconcile.fluxcd.io/resetAt`), both set to the same value as `requestedAt`.
   - Suspend and resume: patch `spec.suspend`.
   - Delete: Kubernetes delete, with what `prune` will remove spelled out.
   All are hidden on read-only clusters, confirmed on PROD, and checked with `can_i patch`.
4. **Crates.** Recommended: new `kubyl_flux` / `kubyl_flux_core`, built like `kubyl_argocd`
   (phase 10) and on `Host` from the start (phase 20).
5. **Where it lives in the sidebar.** Recommended: a "Flux" group under Administration (next to
   Argo CD) with Overview, Kustomizations, HelmReleases, Sources, Image Automation and
   Notifications, each only when served; the kinds also stay under Custom Resources.

## Tasks

### Detection and model (`kubyl_flux_core`)
- [x] Detect Flux from discovery (`*.toolkit.fluxcd.io` groups), live: the UI appears and disappears with the CRDs, without a restart. Add `flux` to `ClusterCaps`
- [x] Find the controllers (`flux-system` by default, or by labels `app.kubernetes.io/part-of: flux`) and their version from the image tags
- [x] Model: Ready condition and its reason/message, Reconciling and Stalled, suspended, last applied and attempted revision, observed generation, interval; states Ready / Reconciling / Suspended / Failed / Stalled / Unknown
- [x] Kinds: Kustomization, HelmRelease, GitRepository, OCIRepository, HelmRepository, HelmChart, Bucket, ExternalArtifact, ImageRepository, ImagePolicy, ImageUpdateAutomation, Alert, Provider, Receiver
- [x] Inventory: a Kustomization's `status.inventory.entries` (`<ns>_<name>_<group>_<kind>`) and a HelmRelease's objects (its Helm release, phase 12/22); resolved against Kubyl's live caches with their health
- [x] Ownership: objects labelled `kustomize.toolkit.fluxcd.io/name|namespace` or `helm.toolkit.fluxcd.io/name|namespace` map to their Flux object
- [x] Dependencies: `spec.dependsOn` as a graph, with "waiting for X" when a dependency isn't ready

### Views (`kubyl_flux`)
- [x] Overview: health banner, counts and ready totals per kind, "Needs attention" (failed, stalled, suspended — without a duration, which Flux doesn't record —, sources not fetched), recent activity from Flux objects' Events (filter by kind, namespace, warnings), controller versions
- [x] Lists per kind: name, namespace, ready state with message, source and revision (short SHA / tag / digest), suspended, interval, last reconcile, age; filters by state and namespace; counts in the toolbar
- [x] Kustomization details: conditions, source with revision (and a link to the commit for GitHub/GitLab/Bitbucket URLs), path, prune, target namespace, post-build substitutions (inline ones with their values, which are plain text in the spec; `substituteFrom` ConfigMaps/Secrets by name only, never their values; "Ask agent" gets names only), dependencies, inventory tree (object → its children from Kubyl's caches), Events, the kustomize-controller's logs filtered to this object (phase 05)
- [x] HelmRelease details: chart (from HelmRepository/OCIRepository/GitRepository), version, values sources (`valuesFrom` names only), install/upgrade remediation settings, history (`status.history`), failure counters, link to the Helm release tab, Events, helm-controller logs
- [x] Source details: URL (credentials stripped), ref, artifact revision and digest, last fetch, the objects that use the source
- [x] Image automation and notifications: ImagePolicy's latest image, ImageUpdateAutomation's last push and commit, Alerts with their Providers and event sources, Receivers (webhook path, never the token)

### Actions
- [x] Reconcile, reconcile with source, force and reset (HelmRelease), suspend, resume (decision 3), from lists, details and the palette; multi-select reconcile/suspend
- [x] Delete with a summary of what `prune` removes (the inventory) and a typed confirmation on PROD
- [x] Edit YAML (phase 04) with the CRD schema
- [x] Every action shows its result through the object's status (Reconciling → Ready/Failed), not just a toast

### Across Kubyl
- [x] Details of a Flux-managed object: "Managed by Flux Kustomization X" with a link, and a warning when editing that Flux will revert the change
- [x] A "Flux" column in workload lists (Deployments, StatefulSets, DaemonSets) linking to the managing object, with its state on the row's cluster (fixed in the code review: it used the first cluster's)
- [x] Palette: `:ks`, `:kustomizations`, `:hr`, `:helmreleases`, `:sources`, and `> Flux: Reconcile`, `Suspend`, `Resume`
- [x] "Ask agent" (phase 21) on a Flux object: state, conditions, revision, dependencies, failing inventory objects, recent warning Events; never Secret values (fixed in the code review: inventory and Events were only included when their tabs had been opened)

### Dev setup and tests
- [x] `script/flux-dev.sh`: Flux installed on `kubyl-dev` (its install manifests, no CLI needed) with a GitRepository and Kustomization for a sample app, a HelmRepository and HelmRelease (podinfo), a Kustomization that fails (bad path), a suspended one, and a dependency chain; `--delete` removes only what it installed
- [x] Core tests: status parsing for each kind and version, inventory ids, ownership, dependency graph, the patches each action sends
- [x] Live tests (ignored) against the dev cluster: detection, lists, reconcile and suspend/resume round trips
- [x] GPUI tests: the overview with a failing and a suspended object, actions hidden on read-only clusters

## Acceptance criteria

- Without Flux: nothing Flux-related anywhere. Running `script/flux-dev.sh` makes the Flux group
  appear without a restart.
- The overview shows the failing Kustomization under "Needs attention" with Flux's message; its
  details show the inventory, the source revision and the controller's log lines for it.
- Reconcile updates the object's status within seconds; suspend and resume flip it, and the
  workload lists show which objects Flux manages.
- Read-only clusters show no action; PROD asks for typed confirmation before delete.

## Risks

- **API drift.** Flux keeps moving fields between versions; parse defensively and test fixtures
  of each served version.
- **Large inventories** (thousands of objects): resolve the tree lazily and page the list.
- **Logs need access** to `flux-system` pods; without it, say which permission is missing.

## Later (not in this phase)

- A diff of what the next reconcile would change (`flux diff`), which needs building the
  kustomization locally.
- Bootstrapping or installing Flux from Kubyl.
- Image automation write actions and notification testing.

## Handoff log

### 2026-10-07 (branch `phase/23-flux`)

Everything in the task list is done. Tested on kind (`kubyl-dev`, Kubernetes v1.37) against
**Flux v2.9.6** installed by `script/flux-dev.sh` from the release's `install.yaml`.

**Decisions** (also in the README decision table and extension points).
1. **Access: Kubernetes only.** Reads through shared `ResourceStores` watches, acts with JSON
   merge patches under the user's RBAC; no `flux` CLI at runtime.
2. **API versions:** each kind at the version discovery prefers. `FluxObject` (core `model`)
   reads only fields shared by the versions and treats missing ones as absent; fixtures cover
   `v1`/`v1beta2` sources (`checksum`, `main/<sha>` revisions), `v2`/`v2beta1` HelmReleases
   (`history` vs `lastAppliedRevision`/`lastReleaseRevision`), `v1`/`v1beta2` ImagePolicies
   (`latestRef` vs `latestImage`), `v1beta3` Alerts/Providers (no status: shown as Ready,
   "static"). Flux 2.9 serves one version per kind (image `v1`, notification `v1beta3`/`v1`).
3. **Actions** exactly as decided (requestedAt, with source, forceAt/resetAt, spec.suspend,
   resume = suspend false + requestedAt, delete). "With source" on a HelmRelease with
   `spec.chart` annotates its HelmChart (`status.helmChart`), which pulls from the repository.
   Suspend, force and delete always confirm; everything confirms on PROD; delete needs the typed
   name on PROD. `can_i` (patch/delete) before every write, on the source too.
4. **Crates:** `kubyl_flux_core` (no GPUI; `FluxCore` on `Host` for detection, tested with
   `TestHost`) and `kubyl_flux`.
5. **Sidebar:** "Flux" under Administration with Overview, Kustomizations, HelmReleases,
   Sources, Image Automation, Notifications, each only while served; the group badge is the
   controllers' version. The kinds stay under Custom Resources (and open the Flux views).

**What shipped.**
- Core: `kinds` (14 kinds, groups, controllers, what each supports), `model` (conditions,
  state, revisions, short revisions, commit of a revision), `details` (per-kind spec/status,
  Secret-backed values by name only), `inventory` (ids incl. RBAC `__` names, health of applied
  objects from live caches, prune summary), `ownership` (kustomize/helm labels; HelmRelease
  wins), `deps` (graph, root-cause "waiting for"), `overview` (counts, needs attention,
  health, headline), `rows` (list rows, filters, sorting), `ops` (patches, `run`), `detect`
  (controllers by namespace or label, versions from image tags; Deployments only), `service`
  (`FluxCore`: detection follows caps and connection, retries while the install comes up),
  `links` (credentials stripped, commit links for GitHub/GitLab/Bitbucket), `agent` (Ask agent
  text without Secret/Helm values or last-applied).
- GPUI: overview (banner, tiles, needs attention, recent activity from the controllers' Events
  via `source=<controller>` watches with kind/namespace/warnings filters, controllers), one list
  view per category (multi-select with ⌘/ctrl- and shift-click, state chips, namespace and kind
  menus, counts), an object's tab (Summary with conditions/source/commit link/settings/
  substitutions/dependencies, HelmRelease chart/values/remediation/failures/releases with links
  to the Helm release tab, sources' artifact and "used by"; Inventory tree with ReplicaSets and
  Pods from Kubyl's caches, paged 200 at a time; History; Events; Controller logs), the details
  dock (state, reconcile/suspend/resume, waiting for), "Managed by Flux …" in other objects'
  details, the YAML editor notice ("kustomize-controller reverts changes made here at its next
  reconcile (every 10m)"; HelmReleases: next upgrade, or drift detection), generic-table columns
  for the 14 kinds, the "Flux" column in Deployments/StatefulSets/DaemonSets, palette actions
  (`> Flux: Reconcile`, `Reconcile With Source`, `Suspend…`, `Resume`, `Force Upgrade…`,
  `Reset Failures`, `Delete…`, `Edit YAML`, `Controller Logs`, `Open …`) and `:sources`
  (`:ks`, `:kustomizations`, `:hr`, `:helmreleases` come from the CRDs' names and open the Flux
  views through `register_list_view`). Keys in the lists and the tab: `r`, `⇧r`, `s`, `u`, `l`,
  `e`, `⌃d`, `/`.
- Shared-crate changes (each small, for their own commits): `kubyl_base`/`kubyl_core`
  (`FluxCaps` in `ClusterCaps`), `kubyl_kube_core` (`flux_caps` from discovery, tested),
  `kubyl_explorer` (`register_group_view` / `group_views`: view rows in contributed groups),
  `kubyl_palette` (`register_view`: views in `:` mode, tested), `kubyl` (dependency and init
  line), root `Cargo.toml` (the two crates), `AGENTS.md` (the dev script and live tests).
- Mockups: board 21 · Flux (9 screens) in `design/mockups/generate.py`. **The published mockup
  artifact still needs these boards.**

**Verification.**
- `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny
  check`, `./script/check-core-crates.sh` (`kubyl_flux_core` ok).
- Tests: `kubyl_flux_core` 27 unit tests (status per kind and version, inventory ids and
  health, ownership, dependency graph, overview, rows, patches per action, sources to reconcile,
  details without secrets, links, agent text, detection with `TestHost`); `kubyl_flux` 9 (GPUI:
  the overview with a failing, a waiting and a suspended object; actions hidden on a
  read-only cluster and shown where they apply; columns; the log filter against a real
  kustomize-controller line; delete confirmation with prune lines and the typed name on PROD);
  palette and kube caps tests in their crates.
- Live (ignored): `KUBYL_TEST_KUBECONFIG=… cargo test -p kubyl_flux_core --test live --
  --ignored --test-threads=1`: detection, lists/states, reconcile with source, suspend/resume,
  HelmRelease reset round trips: 5 passed (reconcile handled in ~0.4 s). `cargo test -p
  kubyl_flux --test live_ui -- --ignored`: the group, badge, controllers and an open
  Kustomizations tab follow `script/flux-dev.sh --delete` and a reinstall without a restart.
- Screenshots in the real app (`design/screenshots/phase-23-*.png`): overview, Kustomizations
  with the dock, a Kustomization (summary with commit link), its inventory tree, a HelmRelease,
  "Managed by" plus the Flux column, the suspend dialog, the list after resume + reconcile,
  filtered controller logs, `:sources`, the YAML editor notice on a managed Deployment. Suspend → Resume → Reconcile were run from the app
  against the cluster.

**Deviations from the board.** The version badge/chip is the kustomize-controller's
(`v1.9.6`), not the Flux release (`v2.9.6`): the release manifests carry no
`app.kubernetes.io/version`; when a distribution sets one it is shown. The "Flux" column shows
in tables of clusters that serve Flux and uses the row's cluster (since the code review:
`ColumnProvider::columns_in`/`cell_in`); it shows a dash for unmanaged rows.

**Deferred / gotchas.**
- `flux diff`, bootstrap/install from Kubyl, image automation writes and notification tests
  (plan's "Later").
- HelmReleases from before helm-controller kept `status.inventory` show a link to the Helm
  release instead of an inventory.
- The first `--delete` run of the dev script got stuck: a running notification-controller
  re-added its finalizer after the script stripped it. The script now deletes the samples
  through the controllers, stops the controllers, then strips leftovers.
- Detection retries (3 s doubling, 5×) while controllers are missing *or not ready*: a
  reinstall creates the Deployments one by one; "Flux: Look for Controllers Again" re-runs it.
- Views created before discovery arrives (restored tabs) re-sync their watches on
  `DiscoveryChanged`.
- Windows and Linux were built by CI only, not run here.

### 2026-10-07 (branch `phase/23-flux`): Code review fixes

Each finding was checked against the code and, where it's about Flux, against Flux's source
(kustomize-, helm- and source-controller `main`) and Flux v2.9.6 on `kubyl-dev`.

**Major**
1. *Flux column used the first cluster's state*: fixed. `ColumnProvider` (kubyl_core) got
   `columns_in(clusters, cx)` and `cell_in(cluster, object, column, cx)` with default impls
   (backward compatible, tested in kubyl_core); the explorer table passes its clusters and the
   row's cluster (`update_columns` takes `cx`). `FluxIndex` is per cluster; the column shows
   only in tables of clusters that serve Flux and re-evaluates on `DiscoveryChanged` (also
   fixes the Minor about visibility fixed at table build). GPUI test with two clusters.
2. *Background Flux tabs took over the selection*: fixed. Lists and object tabs publish only
   while focused (`on_focus_in`/`on_focus_out`, plus a direct check on row clicks, key
   navigation and clicks into the tab, since focus listeners only run on a draw of an active
   window). GPUI test.
3. *Delete dialog misstated pruning*: fixed. `ops::delete_effect`/`delete_note` follow
   `finalizerShouldDeleteResources` (suspended → nothing; `deletionPolicy` MirrorPrune/Delete/
   WaitForTermination/Orphan) and helm-controller's `reconcileDelete` (suspended or never
   installed → no uninstall; HelmRelease has no deletionPolicy). Objects marked `prune:
   disabled`, `reconcile: disabled` or `ssa: Ignore` (labels or annotations, what the
   finalizer excludes) are listed as kept when a running watch shows them, and the note says
   they stay. Tests: suspended, each policy, cluster-scoped entries, kept entries.
4. *"Suspended for more than a day"*: the duration was dropped. Suspending changes no
   condition and Kubyl's stores drop `managedFields` (the only timestamp); client-side
   tracking would only know the app's session. Suspended objects are listed as "Suspended"
   (lowest rank, Warning). The `seconds` doc comment went with `suspended_since`.
5. *Controller readiness was a snapshot*: fixed. The overview watches Deployments in the
   controllers' namespaces (shared stores) and computes readiness live
   (`Controller::with_live`); `is_ready` is now ready ≥ desired replicas (Nit).
6. *Ask agent missed inventory/Events*: fixed. It acquires both, waits until loaded (at most
   5 s), then builds the text; the button is disabled while waiting.
7. *Inventory watched full objects per (kind, namespace), Secrets included*: fixed. Only
   kinds whose health is computed (workloads, ReplicaSets, Jobs, Pods, PVCs, Services, Flux
   kinds) are watched whole; everything else (Secrets included) as metadata only. More than
   8 namespaces of one kind → one all-namespaces watch.
8. *OCI HelmRepositories*: confirmed (source-controller `migrationToStatic` empties the
   status; on kind the object has `status: {}` and ignores `requestedAt`). They are static:
   Ready, "Static object…", no reconcile (also not stuck as "Reconciling" after a request), not
   "Not fetched", outside the ready totals. `script/flux-dev.sh` adds `podinfo-oci`; fixture
   and live assertion.

**Minor**
- `Dependency::blocks`: fixed to Flux's check (exists, `generation == observedGeneration`,
  Ready=True); suspended/reconciling Ready dependencies don't block; `readyExpr` deps are
  never counted as blocking (CEL isn't evaluated). The dock (and the object tab) only show
  "Waiting for" on objects that aren't Ready or suspended (`Graph::waiting`).
- `Graph::cycles`: deleted (test-only, exponential).
- Log filter: the plain `"name","namespace"` alternative now needs `"controllerKind":"<Kind>"`;
  tested with GitRepository and HelmRepository `podinfo` lines.
- Silent skips: Delete on several objects refuses with a toast; objects whose kind isn't
  served anymore are reported in the result toasts.
- `helm_storage`: fixed for both versions: `spec.storageNamespace`, else
  `status.storageNamespace`, else the HelmRelease's namespace (`GetStorageNamespace` in v2 and
  v2beta1); never the target or history namespace.
- `has_status` per API version: `FluxKind::has_status_at`/`reconcilable_at`,
  `FluxObject::is_static`/`reconcilable`; v1beta1/v1beta2 Alerts and Providers have a status.
  Palette availability uses the ref's version; hints and buttons the object.
- UI-thread cost: the index re-parses only objects whose resourceVersion changed and
  `reindex` no longer notifies; the overview recomputes its summary only on Flux object
  changes (Events just repaint); generic Flux tables cache a row per (uid, resourceVersion).
- Flux column visibility: fixed with finding 1.
- `ManagedSection::sync`: clears the manager when the labels (or the object) go.
- "With source" didn't wait: implemented like the CLI: after annotating the source, waits
  (at most 2 min, polling) for `lastHandledReconcileAt` and stops with the source's message if
  it isn't ready (`ops::source_handled`, tested; live round trip passes).
- Scope (plans 22/24 and their README rows): skipped on purpose, identical in all three
  branches so they merge cleanly.

**Nits**
- `Controller::is_ready`: fixed (finding 5).
- Palette: two separate `register_view` calls, no dummy category or special cases.
- Argo CD duplication (`nav.rs`, `short_repo`/`commit_url`): follow-up. Sharing them means
  editing `kubyl_argocd`(`_core`), which this phase doesn't own.
- Test gaps: added inventory names with `_`, malformed status, Ready=False with an older
  observed generation (stays Failed, like `flux get`), states of Bucket, HelmChart,
  ExternalArtifact, ImageUpdateAutomation, OCI HelmRepository and v1beta2 Alert, delete
  summaries (suspended, deletionPolicy, cluster-scoped, kept).

**Verification.** fmt, clippy `-D warnings`, `cargo deny check`,
`./script/check-core-crates.sh` clean; `cargo test --workspace` 1137 passed (kubyl_flux_core
37, kubyl_flux 11, kubyl_core +1). Live on kind with Flux v2.9.6: `kubyl_flux_core --test
live` 5/5, `kubyl_flux --test live_ui` 1/1 (uninstall + reinstall). Screenshots retaken:
`phase-23-overview.png` (suspended without duration, controllers live), new
`phase-23-delete-dialog.png` and `phase-23-static-source.png` (OCI HelmRepository without
Reconcile); the Flux column checked on Deployments.


### 2026-10-07 (branch `phase/23-flux`): Second review

An independent review of the whole change set (core, GPUI crate, shared-crate changes, dev
script, tests), checked against the code, Flux's sources (helm-controller `main`,
image-reflector-controller tags v0.33–v1.0) and Flux v2.9.6 on `kubyl-dev`. The branch sits on
`main` with the new credentials API (`a14a655`, `6740fdb`); Flux uses none of it.

**Fixed**
1. *Overview totals could underflow* (`usize` subtraction, a panic in debug builds): a suspended
   static object (a `v1beta3` Alert with `spec.suspend`) counted as static but not ready.
   Statics now count only while Ready. Test.
2. *ImagePolicies before Flux 2.7* (`image.toolkit.fluxcd.io/v1beta1`/`v1beta2` as the preferred
   version) have no `spec.suspend` and never handle reconcile requests (no
   `lastHandledReconcileAt`): suspend would be pruned silently and a request would show
   "Reconciling" forever. `FluxKind::suspendable_at`/`reconcilable_at` and
   `FluxObject::suspendable` follow the version. Tests.
3. *Helm release names over 53 characters*: helm-controller shortens them (`ShortenName`: 40
   characters, `-`, 12 hex digits of the SHA-256); the delete summary and the Helm release link
   used the long name. `details::shorten_release_name`, tested against the Go result.
4. *Selections across clusters* (Favorites lists can mark rows of several clusters): actions
   used the first object's cluster client, read-only and PROD flags for all of them. They now
   act on the primary's cluster only and say how many others were skipped.
5. *"With source" on a suspended or static source* waited 2 minutes for a request nobody
   handles. A suspended source stops it (like the CLI); a static one (OCI HelmRepository) is
   skipped and only the object is reconciled.
6. *The "Flux" column asked `ConnectionManager::caps` for every cell* (a settings merge per
   call): `FluxIndex::serves(cluster)` is a set lookup, kept in sync with discovery.
   The explorer table also re-evaluates provider columns on `DiscoveryChanged` now (it only did
   when its sources changed, so the column didn't appear when Flux was installed with a table
   open, contrary to the README). Favorites' cluster list no longer relies on adjacent
   duplicates (`dedup` on an unsorted list).
7. *UI-thread cost*: the inventory tree scanned every ReplicaSet/Job/Pod store for each entry
   and child on every render; it now indexes them by owner uid once per render (and caches GVR
   lookups). The object tab, the dock and the overview re-parsed all objects of related kinds on
   every render: `state::ParsedObjects` re-parses a store only when its generation or size
   changes, `find` reads one object by key, the inventory is parsed when the object changes. The
   lists re-parse on store changes only, not on each filter keystroke.
8. *The dock's "Waiting for"* didn't follow its dependencies (no observer on the kind's store):
   it observes the all-namespaces watch now.
9. *Observer leaks*: "Managed by" and the overview's controller watches kept the observers of
   replaced stores; they are replaced together with their stores now.
10. *Stale selection*: when a filter hid every selected row, the list kept the old global
    selection (dock and keys acted on a hidden object); a focused list now publishes the empty
    selection. GPUI test.
11. *A restored single-kind list* (GitRepositories) came back as the whole category: the kind
    is saved in the tab's ref.
12. *Dead code*: `PendingTab` and the `tab` parameter of `actions::open_object`, the tree
    bindings (`CollapseNode`/`ExpandNode`, `FluxTree`), the overview's "enter Open" hint without
    a handler, `state::loaded`, a stale comment.
13. *URL credentials in SCP-like text* (`user:token@host:path`, which `Url` reads as a scheme)
    came back unchanged: the user info is dropped unless it's `git`. Test.
14. *`script/flux-dev.sh`*: an existing Flux was recognised only by the Kustomization CRD
    (helm-only installs or a Flux Operator namespace would have been overwritten and, with
    `--delete`, removed cluster-wide); now any Flux CRD or an unmarked `flux-system` counts.
    `flux-system` is marked before the apply (a failed apply stays removable); `--delete`
    requires the mark `installed` and refuses when Flux objects exist outside `flux-demo`
    (`FORCE=1`); existing namespaces are never taken over (only created ones get the mark;
    `flux-chain`, which the sample `infra` applies as its own Namespace, is recognised by its
    Flux labels); the served API versions are checked before the samples (Flux 2.7+), so they're
    never half applied; `apps` sets `suspend: false`.
15. *Live tests*: the round trips share a mutex (they no longer need `--test-threads=1`),
    resume `apps` if an earlier run stopped halfway, check that the source handled the same
    request, and force is tested too (`lastHandledForceAt`); objects are found by name.
    `live_ui` checks that the script installed Flux before uninstalling it and kills the
    script when it gives up.
16. *Tests*: detection retries use a configurable delay (`FluxCore::with_retry`), so the
    service tests run in milliseconds (they took 3 s each of TestHost's 10 s stall budget), and
    cover the retry cap, errors, a forbidden listing and "look again". `v1beta2` Kustomization
    and `v2beta2` HelmRelease shapes are tested; the GPUI tests' temp dir is removed with the
    app; fixture comments say which fields were added by hand. Root `Cargo.toml` order.
17. *Plan text*: inline post-build substitutions are shown with their values (plain text in
    the spec); only `substituteFrom` is names-only, and "Ask agent" gets names only.

**Skipped**
- *Each object tab also watches its own object by name* although the all-namespaces watch of
  its kind runs too: kept on purpose, it's what works for users who may only read one
  namespace.
- *`live_ui` advances the virtual clock 5× faster than real time*: detection's retries run out
  after ~20 s real time, but the test counts the controllers found (not ready ones), which
  come with the CRDs in the same apply. Passed again here.
- *ArtifactGenerator* (`source.extensions.fluxcd.io`, Flux 2.7+) and `source-watcher`: not in
  this phase's kind list (the controller shows under the controllers).
- *Checksum of `install.yaml`*: a dev script against a local kind cluster, downloaded from the
  GitHub release over HTTPS.

**Verification.** fmt, clippy `-D warnings`, `cargo deny check`, `./script/check-core-crates.sh`
clean; `cargo test --workspace`: 1146 passed (kubyl_flux_core 39, kubyl_flux 11). Live on kind
with Flux v2.9.6: `script/flux-dev.sh` re-run (idempotent), `kubyl_flux_core --test live` 5/5
(in parallel), `kubyl_flux --test live_ui` 1/1 (uninstall + reinstall, with the reworked script). `phase-23-overview.png` retaken (no key-hint bar).
