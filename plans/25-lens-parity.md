# Phase 25: Lens parity: CSV export, local terminal, Applications, Ask AI, Security Center, Cost, cloud discovery

**Status:** in progress (branch `phase/25-lens-parity`, one commit per feature; 3 of 7 done)
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
| 4 Ask AI | `kubyl_core` | `actions::AskAgentAbout { target, kind, prompt }` (handled by `kubyl_agent`) |
| 4 Ask AI | `kubyl_agent_core`, `kubyl_agent` | (owned by phase 21) `prompts` module; `ask` module, `AgentPanel::prefill`, one registry action per question |
| 3 Applications | `kubyl_explorer` | `catalog::workloads_views`: the Applications row inside the Workloads group |
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
- [ ] to be filled in with the commit

### 5. Security Center (Trivy Operator)
- [ ] to be filled in with the commit

### 6. Cost monitoring (OpenCost)
- [ ] to be filled in with the commit

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
