# Phase 12: Argo CD

**Status:** not started
**Depends on:** 02 (lists, details, actions), 04 (YAML/diff view), 05 (logs), optional 11 (open the Argo CD web UI)
**Owns:** `crates/kubyl_argocd`
**Mockups:** none yet (add boards before building the UI: Applications list, Application details with resource tree and history)

## Goal

See and manage Argo CD from Kubyl when a cluster runs it: applications with sync and health
status, a resource tree, history and **rollback**, sync/refresh actions, ApplicationSets and
Projects. The Argo CD UI in Kubyl appears **only if at least one Argo CD CRD exists** on the
cluster.

## Tasks

### Detection
- [ ] Use discovery from phase 01, which updates live: Argo CD is present if any of `applications.argoproj.io`, `applicationsets.argoproj.io` or `appprojects.argoproj.io` is served. Add `argocd` to `ClusterCaps` (per CRD, since some installs only have some of them)
- [ ] Find the Argo CD install namespace(s): the `argocd-cm` ConfigMap, the `argocd-server` and `argocd-application-controller` workloads, and version from their image tags. Support "apps in any namespace" (`application.namespaces` in `argocd-cmd-params-cm`)
- [ ] When CRDs appear or disappear, the sidebar section and palette entries appear or disappear without a restart. Without CRDs, nothing Argo-related is shown anywhere

### Two access modes
- [ ] **Kubernetes mode** (default, needs only the user's kube access): reads and patches the CRDs directly. Status, health, history, resources (`status.resources`), conditions, and all actions below that can be done by patching the Application
- [ ] **API mode** (optional, richer): talks to `argocd-server` through the Kubernetes service proxy or a temporary forward (phase 11), and signs in with Argo CD SSO/local account (token kept in the keychain). Adds rendered manifests, live-vs-desired diff (`managed-resources`), full resource tree including child objects, and server-side rollback. The app shows which mode is active, and uses API mode for a feature only when it's connected

### Applications view
- [ ] Sidebar: an "Argo CD" group under Administration with Applications, ApplicationSets, Projects (with counts). Also listed under Custom Resources as usual
- [ ] Applications table: name, project, sync status (Synced / OutOfSync / Unknown), health (Healthy / Progressing / Degraded / Suspended / Missing / Unknown), auto-sync on/off, repo + path/chart, target revision, synced revision (short SHA), destination (cluster + namespace), last sync time and result. Filters by project, sync, health, destination; counts in the toolbar
- [ ] Health and sync pills with Argo CD's own colors mapped to theme tokens. The operation in progress shows as a progress chip ("Syncing 12/40")
- [ ] Destination links: if the destination cluster is one of your Kubyl contexts (matched by server URL), clicking it opens that cluster and namespace. In-cluster (`https://kubernetes.default.svc`) maps to the current cluster

### Application details
- [ ] Summary: source(s) (multi-source apps included), destination, sync policy (auto-sync, prune, self-heal), sync options, conditions (errors and warnings), current operation state with per-resource results
- [ ] **Resource tree**: App → managed resources (from `status.resources`) → their children from Kubyl's live caches (ownerRefs: Deployment → ReplicaSet → Pod). Per node: kind icon, health, sync status. Clicking a node opens it in Kubyl (details, YAML, logs). A flat list view as an alternative
- [ ] Diff: in API mode, desired vs live per resource (using phase 04's diff view). In Kubernetes mode, show "OutOfSync" resources and explain that the diff needs API mode
- [ ] **History**: `status.history` entries (revision/SHA, source, deployed at, initiated by), newest first, with the current one marked. Link to the commit if the repo URL is GitHub/GitLab/Bitbucket
- [ ] Events for the Application object, and the application controller's logs filtered to this app (phase 05)

### Actions
- [ ] **Refresh** / **Hard refresh**: set the `argocd.argoproj.io/refresh` annotation to `normal` / `hard`
- [ ] **Sync**: dialog with revision (default: target), prune, dry run, apply only, force, replace, server-side apply, selective resources. Kubernetes mode writes the `operation` field on the Application (what the CLI does); API mode calls the sync endpoint
- [ ] **Rollback** to a history entry: warn that auto-sync must be off (offer to turn it off first, as Argo CD does), confirm with the revision diff summary, then sync to that history entry's revision/source. Typed confirmation on PROD clusters
- [ ] **Terminate** the running operation
- [ ] Enable/disable auto-sync, prune and self-heal (patch `spec.syncPolicy`)
- [ ] **Delete** the Application with the choice of cascading (keeps the `resources-finalizer.argocd.argoproj.io` finalizer) or non-cascading (removes it). Strong confirmation, blocked on read-only clusters
- [ ] Edit the Application YAML (phase 04), with the Argo CD CRD schema
- [ ] All actions respect RBAC (`can_i patch applications`) and Kubyl's read-only flag. API mode also respects Argo CD's own RBAC (show 403s clearly)

### ApplicationSets and Projects
- [ ] ApplicationSets list: generators (list, cluster, git, matrix, merge, pull request…) summarised, sync policy, conditions, and the Applications they generated (via ownerRefs), with their health
- [ ] Projects: source repos, destinations, cluster/namespace resource allow/deny lists, roles, sync windows (and whether a window blocks sync right now)

### Integration
- [ ] Palette: `:apps` / `:app` / `:applications`, `:appsets`, `:appprojects`, and actions `> Argo CD: Sync…`, `Refresh`, `Rollback…`
- [ ] Objects managed by Argo CD (label `app.kubernetes.io/instance` or the tracking annotation `argocd.argoproj.io/tracking-id`) show "Managed by Argo CD app X" in their details dock, with a link. Editing them warns that Argo CD may revert the change (self-heal)
- [ ] "Open Argo CD UI" button that opens `argocd-server` in a web view (phase 11), when that phase exists

## Acceptance criteria

- On kind without Argo CD: no Argo CD UI anywhere. Installing Argo CD (`script/argocd-dev.sh`) makes the section appear without a restart, and uninstalling removes it.
- With a sample app (guestbook) from a Git repo: the list shows sync and health correctly. Refresh, sync, and rollback to the previous history entry work in Kubernetes mode, and the resource tree shows the pods.
- In API mode the per-resource diff works after an out-of-band `kubectl edit`.
- Delete asks cascade vs non-cascade and does what it says.

## Risks

- Writing `operation` directly is what the CLI and UI do under the hood, but its format differs a little between Argo CD versions. Test against the two latest minor releases.
- Rollback semantics (auto-sync must be off, multi-source history entries) need careful testing.

## Handoff log
