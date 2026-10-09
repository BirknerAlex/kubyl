# Phase 25: Lens parity: CSV export, local terminal, Applications, Ask AI, Security Center, Cost, cloud discovery

**Status:** in progress (branch `phase/25-lens-parity`, one commit per feature; 1 of 7 done)
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
- [ ] to be filled in with the commit

### 3. Applications view
- [ ] to be filled in with the commit

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
