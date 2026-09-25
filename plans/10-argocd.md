# Phase 10: Argo CD

**Status:** done (branch `phase/10-argocd`; see the handoff log)
**Depends on:** 02 (lists, details, actions), 04 (YAML/diff view), 05 (logs), optional 08 (open the Argo CD web UI)
**Owns:** `crates/kubyl_argocd`
**Mockups:** boards 12–15 · Argo CD applications, application (resource tree and summary), history and rollback, sync dialog

## Goal

See and manage Argo CD from Kubyl when a cluster runs it: applications with sync and health
status, a resource tree, history and **rollback**, sync/refresh actions, ApplicationSets and
Projects. The Argo CD UI in Kubyl appears **only if at least one Argo CD CRD exists** on the
cluster.

## Tasks

### Detection
- [x] Use discovery from phase 01, which updates live: Argo CD is present if any of `applications.argoproj.io`, `applicationsets.argoproj.io` or `appprojects.argoproj.io` is served. Add `argocd` to `ClusterCaps` (per CRD, since some installs only have some of them)
- [x] Find the Argo CD install namespace(s): the `argocd-cm` ConfigMap, the `argocd-server` and `argocd-application-controller` workloads, and version from their image tags. Support "apps in any namespace" (`application.namespaces` in `argocd-cmd-params-cm`)
- [x] When CRDs appear or disappear, the sidebar section and palette entries appear or disappear without a restart. Without CRDs, nothing Argo-related is shown anywhere

### Two access modes
- [x] **Kubernetes mode** (default, needs only the user's kube access): reads and patches the CRDs directly. Status, health, history, resources (`status.resources`), conditions, and all actions below that can be done by patching the Application
- [x] **API mode** (optional, richer): talks to `argocd-server` through the Kubernetes service proxy or a temporary forward (phase 08), and signs in with Argo CD SSO/local account (token kept in the keychain). Adds rendered manifests, live-vs-desired diff (`managed-resources`), full resource tree including child objects, and server-side rollback. The app shows which mode is active, and uses API mode for a feature only when it's connected. *(SSO users sign in with a token; the browser SSO flow and a separate rendered-manifests view are deferred, see the handoff log.)*

### Applications view
- [x] Sidebar: an "Argo CD" group under Administration with Applications, ApplicationSets, Projects (with counts). Also listed under Custom Resources as usual
- [x] Applications table: name, project, sync status (Synced / OutOfSync / Unknown), health (Healthy / Progressing / Degraded / Suspended / Missing / Unknown), auto-sync on/off, repo + path/chart, target revision, synced revision (short SHA), destination (cluster + namespace), last sync time and result. Filters by project, sync, health, destination; counts in the toolbar
- [x] Health and sync pills with Argo CD's own colors mapped to theme tokens. The operation in progress shows as a progress chip ("Syncing 12/40")
- [x] Destination links: if the destination cluster is one of your Kubyl contexts (matched by server URL), clicking it opens that cluster and namespace. In-cluster (`https://kubernetes.default.svc`) maps to the current cluster

### Application details
- [x] Summary: source(s) (multi-source apps included), destination, sync policy (auto-sync, prune, self-heal), sync options, conditions (errors and warnings), current operation state with per-resource results
- [x] **Resource tree**: App → managed resources (from `status.resources`) → their children from Kubyl's live caches (ownerRefs: Deployment → ReplicaSet → Pod). Per node: kind icon, health, sync status. Clicking a node opens it in Kubyl (details, YAML, logs). A flat list view as an alternative
- [x] Diff: in API mode, desired vs live per resource (using phase 04's diff view). In Kubernetes mode, show "OutOfSync" resources and explain that the diff needs API mode
- [x] **History**: `status.history` entries (revision/SHA, source, deployed at, initiated by), newest first, with the current one marked. Link to the commit if the repo URL is GitHub/GitLab/Bitbucket
- [x] Events for the Application object, and the application controller's logs filtered to this app (phase 05)

### Actions
- [x] **Refresh** / **Hard refresh**: set the `argocd.argoproj.io/refresh` annotation to `normal` / `hard`
- [x] **Sync**: dialog with revision (default: target), prune, dry run, apply only, force, replace, server-side apply, selective resources. Kubernetes mode writes the `operation` field on the Application (what the CLI does); API mode calls the sync endpoint
- [x] **Rollback** to a history entry: warn that auto-sync must be off (offer to turn it off first, as Argo CD does), confirm with the revision diff summary, then sync to that history entry's revision/source. Typed confirmation on PROD clusters
- [x] **Terminate** the running operation
- [x] Enable/disable auto-sync, prune and self-heal (patch `spec.syncPolicy`)
- [x] **Delete** the Application with the choice of cascading (keeps the `resources-finalizer.argocd.argoproj.io` finalizer) or non-cascading (removes it). Strong confirmation, blocked on read-only clusters
- [x] Edit the Application YAML (phase 04), with the Argo CD CRD schema
- [x] All actions respect RBAC (`can_i patch applications`) and Kubyl's read-only flag. API mode also respects Argo CD's own RBAC (show 403s clearly)

### ApplicationSets and Projects
- [x] ApplicationSets list: generators (list, cluster, git, matrix, merge, pull request…) summarised, sync policy, conditions, and the Applications they generated (via ownerRefs), with their health
- [x] Projects: source repos, destinations, cluster/namespace resource allow/deny lists, roles, sync windows (and whether a window blocks sync right now)

### Integration
- [x] Palette: `:apps` / `:app` / `:applications`, `:appsets`, `:appprojects`, and actions `> Argo CD: Sync…`, `Refresh`, `Rollback…`
- [x] Objects managed by Argo CD (label `app.kubernetes.io/instance` or the tracking annotation `argocd.argoproj.io/tracking-id`) show "Managed by Argo CD app X" in their details dock, with a link. Editing them warns that Argo CD may revert the change (self-heal)
- [x] "Open Argo CD UI" button that opens `argocd-server` in a web view (phase 08), when that phase exists

## Acceptance criteria

- On kind without Argo CD: no Argo CD UI anywhere. Installing Argo CD (`script/argocd-dev.sh`) makes the section appear without a restart, and uninstalling removes it.
- With a sample app (guestbook) from a Git repo: the list shows sync and health correctly. Refresh, sync, and rollback to the previous history entry work in Kubernetes mode, and the resource tree shows the pods.
- In API mode the per-resource diff works after an out-of-band `kubectl edit`.
- Delete asks cascade vs non-cascade and does what it says.

## Risks

- Writing `operation` directly is what the CLI and UI do under the hood, but its format differs a little between Argo CD versions. Test against the two latest minor releases.
- Rollback semantics (auto-sync must be off, multi-source history entries) need careful testing.

## Handoff log

### 2026-09-25 (branch `phase/10-argocd`)

Everything in the task list is done; the deferrals are listed below. Tested live on kind against
the two latest minors, **Argo CD v3.5.3 and v3.4.9** (`script/argocd-dev.sh [VERSION]`): the
`operation`, `syncPolicy` and finalizer formats Kubyl writes are the same in both (compared
with the v3.4.9 and v3.5.3 sources), and all live tests pass on both. The stub-crate PR was
skipped on request: the crate, the web view crash fix from phase 08 (`49ab311`, cherry-picked
from `da519b4`) and the feature work are one PR.

**Access modes** (also in the README decision table).
- **Kubernetes mode** is the default and needs nothing but kube access. It reads the CRDs
  through shared `ResourceStores` watches and acts by patching the `Application` (`ops`):
  refresh annotation; `operation` written like `argocd app sync` (refuses when one is
  running, clears `status.operationState`, conditional on `resourceVersion` with a retry on
  409; sync revisions, sources, prune, dry run, apply only, force, sync options, selective
  resources, retry from the app's policy); rollback like the server's `Rollback` (history
  entry's source(s) and revision(s), `apply` strategy, refused while auto-sync is on);
  terminate (`status.operationState.phase = Terminating`); `spec.syncPolicy` changes (apps
  using `automated.enabled` get `enabled: false` instead of losing the block); delete with
  the finalizer that matches the choice (foreground `resources-finalizer.argocd.argoproj.io`,
  background `…/background`, non-cascading removes both). Every action checks `can_i` first
  and is blocked on read-only clusters; PROD rollbacks and every delete need the typed name.
- **API mode** (after signing in) calls `argocd-server`'s REST API (`api`): resource tree,
  managed resources (the per-resource diff), refresh, sync, rollback, terminate and delete run
  under Argo CD's RBAC ("Argo CD denied this: …" for 403s). Policy changes always patch the
  Application (Kubernetes RBAC). Everything else falls back to Kubernetes mode when API mode
  isn't connected. The mode chip in every Argo CD toolbar shows the mode and opens sign-in /
  sign-out.
- Argo CD 3.x no longer stores per-resource health in `status.resources`, so Kubernetes mode
  computes it from the live objects (`health`: a port of gitops-engine's checks for
  Deployment, StatefulSet, DaemonSet, ReplicaSet, Pod, Service, PVC, Ingress, Job, HPA,
  APIService; a live test compares it with Argo CD's own verdict). Kinds without a check show
  no health in Kubernetes mode; API mode shows Argo CD's (including Lua health checks).

**How the connection to argocd-server is trusted.** The security bar of phases 07/08: never
send credentials to a host derived from objects anyone could create.
- Detection (`detect`) finds installs from `argocd-cm` (cluster-wide list by field selector,
  else the namespaces in the Applications' `status.controllerNamespace` and the usual
  names), the `argocd-server` Service, the controller workload and its image tag (the
  version). It reads ConfigMaps, Services and workloads, **never Secrets** (the admin password
  in `argocd-initial-admin-secret` is the user's to fetch). Nothing is contacted during
  detection.
- Nothing signs in by itself the first time. The sign-in dialog shows the cluster, namespace,
  `service:port`, how it connects (service proxy or temporary forward), the `url` from
  `argocd-cm` (shown, never used) and the version; with several installs the user picks one.
  Signing in *confirms* that install: `state.json` (`argocd.trusted`) keeps it per context
  (context name + API server URL) with namespace, Service name and **Service UID**. A
  re-created Service or a vanished install shows a warning instead of connecting
  (`trust_problem`), and auto-connect (`argocd.auto_connect`, on by default) only reconnects
  to a confirmed install with a stored token.
- The token goes to the OS keychain (`kubyl_kube::auth::store`, key
  `argocd/<server>/<context>/<namespace>/<service>`), never to settings or state; it's only
  ever sent to that Service. Sign out deletes it; "Sign Out and Forget This Install" also
  drops the confirmation. The user's kube credentials never reach Argo CD: in proxy mode they
  authenticate the request to the API server, which strips `Authorization` before proxying
  (verified: Argo CD's `/api/v1/session/userinfo` answered `{}` for a bearer token sent that
  way), so the Argo CD token travels as the `argocd.token` cookie, which the proxy passes
  (verified: `loggedIn: true`). If the proxy doesn't work (NetworkPolicies, proxy disabled,
  `ApiTransport::Forward`), a temporary loopback forward (`ForwardSpec::ephemeral`, titled
  "Argo CD API · svc/argocd-server:443", stopped on sign-out or disconnect) carries a bearer
  header. Over the forward the server's (by default self-signed) certificate isn't verified:
  the tunnel is the authenticated API server connection and the client only talks to
  127.0.0.1.
- Sign-in methods: username and password (local accounts, `POST /api/v1/session`) or a pasted
  token. Browser SSO isn't implemented (deferred): SSO users paste a token
  (`argocd account generate-token`, or the UI's `argocd.token` cookie). The dialog says so
  when `argocd-cm` has Dex or OIDC configured.

**How it fits together** (`crates/kubyl_argocd`).
- Data, no UI: `model` (Application, ApplicationSet, AppProject, history, operation state,
  activity like "Syncing 12/40"), `health`, `tree` (Kubernetes-mode tree from
  `status.resources` + live caches by ownerRefs; API-mode tree from `parentRefs`; flat list,
  All / OutOfSync / Unhealthy filters), `diff` (normalized desired vs live, Secrets masked),
  `windows` (sync windows: cron + duration, `CanSync` semantics, evaluated locally for the
  Sync dialog warning and the Projects view), `links` (commit/compare/tree URLs for
  GitHub/GitLab/Bitbucket; destination → known context by server URL), `apps` (rows, filters,
  counts, sorting).
- `state::ArgoCd` global: per cluster the caps, detection (re-run when the CRDs change, and
  retried while an install is still coming up), and the API session. `run::run(target, op)`
  picks the mode and reports through toasts.
- Views: Applications (board 12), an application's tab (boards 13–14: Summary, Resources,
  Diff, History, Events, Logs), ApplicationSets and Projects lists with detail panes. Dialogs
  (boards 14–15): Sync, Rollback, Delete, sign-in. Keys: `s` sync, `r`/`shift-r` refresh,
  `b` rollback, `h` history, `e` edit YAML, `ctrl-d` delete, `/` filter; in the tree `l` logs,
  `e` YAML, `d` diff, `t` tree/list.
- `dock`: the details dock shows an Application's summary for Applications and "Managed by
  Argo CD app X" (with a link and the drift state) for tracked objects: the tracking
  annotation must name the object itself (`<app>:<group>/<kind>:<ns>/<name>`), and the
  `app.kubernetes.io/instance` label only counts when an Application of that name exists.
  The YAML editor shows a warning banner on such objects (self-heal may revert the edit).
- `columns`: printer-column providers for the three kinds in the generic tables.
- Controller logs: `kubyl_logs::open_filtered` with a regex matching this app in both log
  formats (JSON, the default of the 3.4 and 3.5 manifests: `"application":"guestbook"`; and
  text, `application=guestbook`).

**Shared-crate commits** (each its own commit): `kubyl_core` (`ClusterCaps::argocd`,
`ViewRegistry::register_list_view/register_object_view`, `ChromeRegistry::add_edit_notice`),
`kubyl_kube` (Argo CD caps from discovery; discovery re-runs while a new CRD isn't served
yet: the CRD watch is metadata-only, so a CRD becoming Established wasn't noticed and an
install showed only some kinds), `kubyl_explorer` (contributed tree groups with a badge; rows
and objects open their kind's registered view), `kubyl_palette` (same for `:kind`/objects),
`kubyl_yaml` (`diff_view` usable by other crates; edit notices as a banner), `kubyl_logs`
(`open_filtered` with a search, optionally regex), `kubyl_ui` (icons), `kubyl` (screenshot
harness: App Nap opt-out and thread-based waits, it stalled when another app was frontmost).

**Tests.**
- 49 unit tests (model, health, sync windows, links, tree, diff, rows and filters, operation
  and API bodies, detection, trust settings, managed-by, columns…).
- `tests/live.rs` (7, ignored): detection; refresh, sync and rollback in Kubernetes mode;
  auto-sync policy round trip; API mode through the proxy and through a forward (tree shows
  the pods); API-mode diff after an out-of-band edit; delete cascading vs non-cascading;
  computed health vs Argo CD's.
- `tests/live_ui.rs` (1, ignored, reinstalls Argo CD): one app and an open Applications tab
  live through `script/argocd-dev.sh --delete` and a reinstall; the sidebar group, its version
  badge, detection and the rows disappear and come back. It found the discovery race above.
- Run: `KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig KUBYL_CREDENTIAL_STORE=memory cargo
  test -p kubyl_argocd --test live --test live_ui -- --ignored --nocapture --test-threads=1`.
- Screenshots (`design/screenshots/phase-10-*.png`, screenshot harness on kind, v3.5.3):
  `applications` (Kubernetes mode, details dock), `app-resource-tree` (API mode), `app-diff`
  (after `kubectl scale`), `app-history`, `sync-dialog`, `rollback-dialog` (PROD, typed
  confirmation), `applicationsets`, `projects`.

**Mockups.** Boards 12–15 are in `design/mockups/generate.py`. **The published mockup artifact
still needs these new boards.**

**Deferred.**
- Browser SSO sign-in (Dex/OIDC through argocd-server, like `argocd login --sso`); SSO users
  sign in with a token.
- A separate rendered-manifests view of a whole app (`ArgoApi::manifests` exists); the Diff
  tab shows desired vs live per resource.
- Per-resource health in Kubernetes mode for kinds without a built-in check (CRDs with Lua
  health checks): API mode shows it.
- Windows and Linux were built by CI but not run interactively here.
