# Phase 25: Lens parity: CSV export, local terminal, Applications, Ask AI, Security Center, Cost, cloud discovery

**Status:** in progress (branch `phase/25-lens-parity`, one commit per feature; 6 of 7 done)
**Depends on:** 02 (tables, details), 05 (terminal), 07 (metrics, charts), 11 (kubeconfig, cloud import), 21 (agents), 22 (Helm), 24 (resource views)
**Owns:** `kubyl_security`, `kubyl_security_core`, `kubyl_cost`, `kubyl_cost_core` (new); `kubyl_apps`, `kubyl_apps_core` (new, if the Applications view doesn't fit an existing crate); shared commits listed under each feature. `script/trivy-dev.sh`, `script/opencost-dev.sh`.
**Mockups:** board 23 · Lens parity, one or more boards per feature in `design/mockups/generate.py` (`CsvExport.dc.html`, …).

## Goal

Lens has features Kubyl lacks. These seven are the ones worth bringing over, in this order, each
a commit of its own on one branch (the user asked for a single branch although
`plans/README.md` prefers small shared-crate PRs that land first; each shared-crate edit sits in
the commit of the feature that needs it, and the PR description lists them):

1. Table CSV export
2. Local terminal tab
3. Applications view
4. Ask AI on any resource
5. Security Center (Trivy Operator reports)
6. Cost monitoring via OpenCost
7. Cloud cluster discovery (EKS, AKS, GKE)

Rules that apply to all: no network or blocking work on the UI thread (`spawn_kube`), logic in
GPUI-free `kubyl_<name>_core` crates, no GPL dependencies, no tokens or Secret data in logs,
settings, state, `Debug` output or prompts.

## Shared-crate edits (for the PR description)

| Commit | Crate | Edit |
|---|---|---|
| 1 CSV | `kubyl_base` | `csv` module (RFC 4180 writer, formula guard, cell flattening) |
| 1 CSV | `kubyl_core` | `export::{save_text, file_name}`, `actions::ExportCsv`, `ActionSpec::in_context`, re-export of `csv`; deps `dirs`, `jiff` |
| 1 CSV | `kubyl_resources_core` | `columns::exportable` (Secret lists export key counts only) |
| 1 CSV | `kubyl_explorer` | the list's `csv_table`/`export_csv`, the table menu entry and the `List: Export CSV` palette action |
| 2 Local terminal | `kubyl_terminal_core`, `kubyl_terminal` | (owned by phase 05) `local` module, `SessionMode::Local`, `terminal.local_shell`, `Terminal: Open Local Shell for This Cluster` |
| 2 Local terminal | root `Cargo.toml` | `portable-pty` 0.9 (MIT) in `[workspace.dependencies]` |
| 3 Applications | `kubyl_argocd_core`, `kubyl_argocd` | (owned by phase 10) `tracking::{ManagedBy, managed_by}` moved from `kubyl_argocd::dock` into the core crate; `dock` re-exports them, so the old paths work |
| 6 Cost | root `Cargo.toml`, `crates/kubyl` | workspace entries for `kubyl_cost(_core)`, the `init` line in `main.rs` |
| 5 Security | `kubyl_base`, `kubyl_core` | `ClusterCaps::trivy` (`TrivyCaps`: which report CRDs are served; `any()`) |
| 5 Security | `kubyl_kube_core` | `cluster_info::trivy_caps` from discovery (listable kinds of `aquasecurity.github.io`), with a test |
| 5 Security | `kubyl_base`, `kubyl_explorer` | none beyond the above: the sidebar row uses `register_view_row` (after Alerts) with a `RowBadge` |
| 4 Ask AI | `kubyl_core` | `actions::AskAgentAbout { target, kind, prompt }` (handled by `kubyl_agent`) |
| 4 Ask AI | `kubyl_agent_core`, `kubyl_agent` | (owned by phase 21) `prompts` module; `ask` module, `AgentPanel::prefill`, one registry action per question |
| 3 Applications | `kubyl_explorer` | `catalog::workloads_views` (the Applications row inside the Workloads group) and `catalog::{register_view_count, view_count}` (a count at the end of a view entry's row) |
| 3 Applications | root `Cargo.toml`, `crates/kubyl` | workspace entries for `kubyl_apps(_core)`, the `init` line in `main.rs` |

## Tasks

### 1. Table CSV export
- [x] `kubyl_base::csv`: RFC 4180 writer (CRLF, quoting), formula guard (`= + - @`, tab, CR get a leading `'`), cell flattening; unit tests
- [x] `kubyl_core::export::save_text` (save dialog, write on the background executor, toast) and `ExportCsv` action
- [x] Resource lists export the visible columns in the current sort and filter, related columns included (`ResourceListView::csv_table`)
- [x] Secret lists export key counts only (`columns::exportable`, test on a Secret with values)
- [x] "Export CSV…" in the table's Columns menu and `List: Export CSV` in the palette
- [x] Live test against the kind cluster (`crates/kubyl_resources/tests/live_csv.rs`)
- [x] Mockup board `CsvExport.dc.html`
- [x] Reusable for Security and Cost (`kubyl_core::csv`, `export::save_text`, `ExportCsv`)

### 2. Local terminal tab
- [x] `kubyl_terminal_core::local`: one-context kubeconfig from the file that defines the context (relative CA, client cert/key, token file and exec command made absolute), private folder (0700/0600) removed when the tab closes and swept at startup after a crash; unit tests with fixtures
- [x] Kubyl-signed-in contexts (OIDC, OpenShift): the user becomes an exec plugin that prints `KUBYL_KUBE_TOKEN` from the shell's environment; the token is never in arguments, files, logs or `Debug` output (tests)
- [x] `local::run`: PTY (`portable-pty`) on Tokio with the channels of `exec::run`; killed when the tab closes; tests with real PTYs on Unix
- [x] `SessionMode::Local` in the terminal view, panel and (per `terminal.open_in`) editor tabs; never restored at startup; `+` opens another one
- [x] Read-only note and typed cluster name on PROD (`local_shell_confirmation`), a note in the shell's first line and a "local · not protected" chip
- [x] `Terminal: Open Local Shell for This Cluster` in the palette; `terminal.local_shell` setting (default `$SHELL`, `%COMSPEC%`, `/bin/sh`)
- [x] Live tests with the real `kubectl` on kind (`crates/kubyl_terminal/tests/live_local.rs`): only one context, and a token that reaches `kubectl` through the environment only
- [x] Mockup boards `LocalTerminal.dc.html`, `LocalTerminalProd.dc.html`

### 3. Applications view
- [x] `kubyl_apps_core`: objects with `app.kubernetes.io/instance` grouped by namespace and instance (Deployments, StatefulSets, DaemonSets, CronJobs, Jobs, Services, Ingresses, ConfigMaps, PVCs); Jobs of a CronJob belong to it; unit tests
- [x] Workload health like `kubectl rollout status` (Degraded, Progressing, Healthy, Suspended; "No workloads" for apps of Services and ConfigMaps only)
- [x] Managed by through the owners' own helpers: Argo CD (`kubyl_argocd_core::tracking`, label tracking only when an Application of that name exists), Flux (`kubyl_flux_core::ownership`), Helm (`kubyl_helm_core::release`), else the raw `managed-by` label
- [x] Version from the workloads' `app.kubernetes.io/version`, else the image tag; Age from the oldest object
- [x] The view per cluster: a view row of the Workloads group (`catalog::workloads_views`), `:apps`, `Applications: Open for This Cluster`; table on `DataTable`, filter, namespace menu, details with the objects as links
- [x] Logs: `l` and the pane's Logs button (a menu when the app has several workloads) open the workload's log view, which streams its pods by selector
- [x] Watches ask the server for `app.kubernetes.io/instance` objects only (label selector); everything but workloads is metadata-only; a refused list says which verb and resource is missing
- [x] CSV export through `kubyl_core::csv` (table menu, `Applications: Export CSV`)
- [x] `script/apps-dev.sh` (sample apps on kind), live test `crates/kubyl_apps_core/tests/live.rs`, GPUI tests with `debug_bounds`
- [x] Mockup boards `Applications.dc.html`, `ApplicationsEmpty.dc.html`

### 4. Ask AI on any resource
- [x] `kubyl_agent_core::prompts`: Summarize, Analyze events, Analyze logs, Analyze metrics, Analyze related resources; text names only cluster, kind, namespace and name and the read-only tools to use; references are reduced to name characters; tests (injection, no data, every named tool exists, which kinds a question applies to)
- [x] "Ask: …" entries on generic resource rows (the list's context menu, the palette's `>` mode: registry actions in the `ResourceList` context, available for objects the question fits) and an "Ask agent" details section with the buttons
- [x] Hidden while no agent is installed (`ask::agent_ready`; the registry predicates read a flag the agent service keeps current)
- [x] A click opens the Agent panel with the question in the composer; nothing is sent until Enter (`AgentPanel::prefill`)
- [x] No Secret data in the prompt (references only, tested with a Secret subject); the tools redact as before; the live test reads a Deployment and a Secret through the MCP tools the questions name
- [x] Doesn't depend on phase 21 part 2 (writes)
- [x] Mockup board `AskAgent.dc.html`

### 5. Security Center (Trivy Operator)
- [x] `kubyl_security_core`: report kinds, `Report` rows (labels from the metadata, counts from the printer columns), aggregation by image / resource / role, filters, totals, CSV records; unit tests and tests on recorded fixtures (`tests/fixtures/`, from `script/trivy-dev.sh` on kind)
- [x] Full reports parsed on demand (`details`): vulnerabilities worst first with `http(s)` links only, failed checks, exposed secrets; the panel lists at most 300 (the CSV button exports all); **exposed secrets never read the `match` field** (tested with a fake AWS key in the report, `Debug` output and CSV)
- [x] `ClusterCaps::trivy` from discovery (like `flux`), `kubyl_kube_core::cluster_info::trivy_caps`
- [x] The tab: sub-tabs Images · Resources · Roles (`1`/`2`/`3`), severity summary chips that filter, namespace menu, filter, table on `DataTable`, details panel, CSV export of the table and of a report's findings
- [x] Lists are metadata-only watches plus the API server's printer-column Table (refetched at most every 2 s); a report's findings are fetched by name when its row is selected; a refused list or get names the verb and resource
- [x] Where Trivy Operator isn't served: an empty state that installs `trivy-operator` into `trivy-system` through the Helm install flow (`ChartRef::Url`, so no repository has to be added; hidden on read-only clusters, where the command stays; PROD asks for the cluster's name in the Helm dialog) or copies the command
- [x] Sidebar: Security under Alerts, with a red badge of the critical findings of all three views together (a once-a-minute table read per cluster that serves Trivy; nothing for clusters without it); the sub-tabs show each view's criticals so the badge (their sum) adds up
- [x] A Security card on the cluster and namespace overviews: totals per severity and a line per view (`OverviewSection`), from the same read
- [x] Applications moved into the Workloads group, with its count like the other rows (`catalog::register_view_count`; `kubyl_apps::service::AppsService` counts once a minute with metadata-only lists)
- [x] `script/trivy-dev.sh` (operator + samples, `--fixtures`, `--delete`), live test `crates/kubyl_security_core/tests/live.rs`, GPUI tests with `debug_bounds`
- [x] Mockup boards `Security`, `SecurityResources`, `SecurityMissing`, `SecurityInstallProd`

### 6. Cost monitoring (OpenCost)
- [x] `kubyl_cost_core`: OpenCost's allocation API (`/allocation/compute`, `window`, `aggregate=namespace`, `includeIdle`, `accumulate`, `step`) as `Query`, lenient parsing, totals with cost-weighted CPU/memory efficiency, per-namespace rows (idle and unmounted volumes labelled), the stacked cost-over-time series (top 6 + other + idle, empty steps keep their time), CSV records, detection of OpenCost's Service, error messages that say what to check; unit tests
- [x] Verified against OpenCost 1.121.3 (chart 2.5.32) on kind: recorded fixtures (`tests/fixtures/`), a local HTTP server standing in for OpenCost checks the request line, and the live test runs every window x idle choice through the service proxy
- [x] `kubyl_cost::service::CostService`: demand-driven like `MetricsService` (a view says it wants a cluster's costs and keeps saying so; a loop refreshes once a minute, nothing runs for unwatched clusters); detection by label then by name, the address overridable (`cost.clusters.<cluster>.service` as `namespace/service:port`); reached through `Transport::service_proxy`
- [x] The tab: window 24h/7d/30d, include-idle toggle, Refresh, tiles (total, idle, CPU and memory efficiency), `kubyl_charts` stacked area with `ColorRegistry` colors (the same namespace keeps its color across charts), per-namespace table (CPU, memory, storage, network, total, efficiency), CSV export; window and idle remembered per cluster (state.json `cost`)
- [x] Empty states: looking, not installed (says OpenCost needs Prometheus, shows the install command and the settings override), refused list of Services, unreachable Service (names the verb/port); an error with numbers on screen keeps them with a banner
- [x] `script/opencost-dev.sh` (OpenCost on `prometheus-dev.sh`'s Prometheus, custom on-prem pricing so kind has prices; `--delete`), live test `crates/kubyl_cost_core/tests/live.rs`, GPUI tests with `debug_bounds`
- [x] Mockup boards `Cost`, `CostMissing`

### 7. Cloud cluster discovery
- [ ] to be filled in with the commit

## Testing

Unit tests in the core crates with recorded fixtures; live tests ignored by default against the
kind cluster (`script/dev-cluster.sh`, `script/trivy-dev.sh`, `script/opencost-dev.sh`); fake
`aws`/`az`/`gcloud` scripts on `PATH` for the cloud CLIs. Before each commit: `cargo fmt --all`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`,
`script/check-core-crates.sh`, `cargo deny check`.

## Later (not in this phase)

- Trivy: air-gapped database import/export, triggering scans.
- Cost: Kubecost, list-price estimates, an AI cost optimizer.
- Cloud discovery: in-app OAuth sign-in (the installed CLIs only).

## Handoff log

### 2026-10-09 (branch `phase/25-lens-parity`)

**1. Table CSV export.** `kubyl_base::csv` is the one writer (Security and Cost reuse it through
`kubyl_core::csv` and `kubyl_core::export::save_text`). A list exports the columns it shows, in
the order of its rows (sort and filter applied), with related columns; cells are the same text
the table shows (links and buttons by their labels). Decision: no BOM (Excel opens
ASCII fine; a BOM breaks tools that don't expect it). The formula guard prefixes `'` to
anything starting with `= + - @`, tab or CR, including negative numbers (`-5` becomes `'-5`).
Secret lists keep to name, namespace, type, key count and age (`columns::exportable`).
Checked by hand: not clicked through (no display session during the run); the live test
exports real Pods and Secrets from kind.

**2. Local terminal tab.** Reaching the context: `ConnectionManager::cli_target` (kubeconfig file,
context, Kubyl's sign-in) feeds `local::prepare`. kubectl has no environment variable for a
bearer token (helm does), so a context that signs in through Kubyl gets a user whose exec plugin
(`sh -c printf …` / PowerShell on Windows) prints `$KUBYL_KUBE_TOKEN` as an `ExecCredential`;
the token is the sign-in at the time the tab opened and expires like it (open a new tab).
Static credentials of other contexts (client certificate and key, static token) are copied from
the user's kubeconfig into the private one-context file, because kubectl can't read inline data
from another file; the file is 0600 in a 0700 folder under `<temp>/kubyl-local-shells-<uid>/`
and is deleted with the tab (`ShellKubeconfig`'s `Drop`, also when the future is dropped).
Unlike the agent's `context_kubeconfig`, which refuses inline credentials, a local shell is the
user's own, so it keeps them. The shell inherits Kubyl's environment except `KUBECONFIG`,
`PATH` (the login shell's), `TERM`, `COLORTERM`; it starts as an interactive non-login shell, so
a `KUBECONFIG` exported by the user's rc file wins over ours (documented, not fought).
The Windows ConPTY and PowerShell exec plugin are written but not run by hand (CI compiles
them). Not clicked through in the app (no display session during the run); the tests drive a
real PTY and the real `kubectl`.

**3. Applications view.** Decisions: an object is part of an application only when it carries
`app.kubernetes.io/instance` (the recommended label for "the instance of an application"; Helm
sets it from the release name); `name` alone doesn't group, so unrelated objects that share a
program name (`app.kubernetes.io/name: nginx`) aren't merged. Grouping key is (namespace,
instance). The watches filter on that label in the API server, so a cluster with thousands of
objects only sends the labelled ones, and ConfigMaps/Services/Ingresses/PVCs are metadata-only
(ConfigMap data never enters memory). Pods and ReplicaSets are not members (they repeat their
workload). "Logs" opens the workload's log view (which already streams every pod of its
selector); one view for *all* pods of an application (a label selector source) would need a
new entry point in `kubyl_logs` and is Later. Sorting is by health (worst first), then
namespace and instance; `DataTable` has no clickable headers. Checked against the kind cluster with `script/apps-dev.sh`: screenshot
`design/screenshots/phase-25-applications.png` (table, details, links rendered; clicking the links, the Logs menu and
the CSV save dialog were not clicked through by hand).

**4. Ask AI on any resource.** The canned question is a *prefill*, not an auto-send: a click
costs no tokens and the user sees (and can edit) exactly what will be sent. The existing
"Resource: Ask Agent" (`shift-a`) still attaches a masked description of the object as a
chip; the new questions don't attach anything: the agent calls `describe`, `events`, `logs`,
`top`, `query_prometheus` and `list_resources` itself, so the redaction of those tools applies
and output stays capped. The registry's availability predicates can't see the app, so
`ask::READY` mirrors "an agent is installed" and the details section checks it when it is
built (an agent found after the details opened shows on the next selection). The row's menu
lists the five questions flat (gpui-component's submenus need an entity per menu). Not
clicked through in the app: an installed agent is needed to show the entries; the GPUI tests
check the composer, the hiding and the section.

**5. Security Center.** Findings are never listed: a list is the reports' metadata (labels name
the workload and container) plus the server's printer-column Table (repository, tag, scanner and
the five severity counts), so 10,000 reports cost a Table, not their vulnerability arrays; the
selected row's report(s) are fetched by name. One scan per image is fetched for the Images
panel (the same findings), the config-audit report plus the exposed-secret reports for a
resource. The Images counts are the larger of an image's reports (the same image in several
workloads is one row listing them); the badge sums critical findings of each image once, plus
config-audit and RBAC criticals. `ConfigAuditReport`s are per ReplicaSet/Pod (what Trivy
Operator scans), so the Resources view lists ReplicaSets, like Lens. Trivy Operator only
stores failed checks. **Not done on purpose** (per the brief): air-gapped database import/export
and triggering scans. **What the live run showed:** the vulnerability database downloaded fine
here (mirror.gcr.io images, ghcr.io DB); the operator can't scan an image that was only loaded
into kind (no registry), so the exposed-secret sample is a hand-made fixture in Trivy's schema
(`exposedsecretreport-leaky.json`, applied by `script/trivy-dev.sh --fixtures`); the real
busybox/nginx ExposedSecretReports (empty) are recorded. The Docker disk filling up once stalled
the first install: `cargo clean` frees the build cache. Screenshot: not taken (the Mac's screen
was locked, which stalls the screenshot harness); the GPUI tests check the table, summary,
details, empty and install states.

**6. Cost monitoring.** The brief said to verify the allocation API against the installed
version; what 1.121.3 does: `GET /allocation/compute` answers `{"code":200,"data":[{name:
allocation}…]}` with one set per step (or one for the window with `accumulate=true`); with
`includeIdle=true` an `__idle__` allocation joins; steps without data are empty objects (so
the chart takes their time from the neighbours); costs of a young cluster can be tiny negative
numbers (clamped to zero) and rounded to five decimals; efficiency is usage over request and
can exceed 1 (kube-system's memory showed 258%): shown as it is. An invalid window is a plain
400 through the proxy. On kind OpenCost has no prices ("No pricing found", all costs 0) until
custom pricing is on, which the script does. The `/model/...` paths of older OpenCost
(`/allocation`) answer the same shape but aren't used. Out of scope per the brief: Kubecost,
list-price estimates, the AI cost optimizer. Not clicked through in the app (screen locked for
screenshots); the GPUI tests check tiles, chart, table, the remembered choices and the empty
and error states.
