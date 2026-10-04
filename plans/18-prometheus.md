# Phase 18: Prometheus web UI

**Status:** done
**Depends on:** 02 (sidebar view rows), 07 (`kubyl_metrics`: discovery, transport, PromQL client)
**Owns:** `crates/kubyl_prometheus` (new); `Flame` icon in `kubyl_ui`

## Goal

A **Prometheus** row under every cluster that has a Prometheus, Thanos Query or VictoriaMetrics,
opening a tab like the Prometheus web UI: Overview (build, runtime, TSDB status), Query (table
or graph, history, completion), Target Health, Rule Health and Service Discovery. A selector in
the header switches between several servers of one cluster.

## Tasks

- [x] Discovery of all servers (`service.rs`): every ranked Service is probed, Services in front of
      the same pods are one instance; the metrics service's client (overrides, Routes) comes first.
- [x] API reads and lenient parsing (`fetch.rs`, `model.rs`).
- [x] Tabs: overview, query (+ completion in `complete.rs`), targets, rules, service discovery.
- [x] Sidebar row (`chrome.rs`), palette actions (`actions.rs`), `prometheus` settings section.
- [x] Live check on `script/prometheus-dev.sh` (kube-prometheus-stack, with and without Thanos).
- [ ] Later, not needed for done: mockup board; keyboard navigation of the lists; persisted history.

## Handoff log

- 2026-09-30: The view reads the API itself, only for the tab on screen (no service cache).
  Thanos Query has no TSDB status or runtime info: the Overview shows each part's error alone.
  The query box is single-line; Enter runs, Cmd/Ctrl+Enter runs from anywhere in the view.
- 2026-10-04: Done. Checked by hand against live clusters; the view works as intended. The mockup
  board, keyboard navigation of the lists and persisted history are left for later.
