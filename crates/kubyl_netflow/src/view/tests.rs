//! GPUI tests of the view against a service fed by hand (no cluster, no network, no timers).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use jiff::{SignedDuration, Timestamp};
use kubyl_core::ClusterId;

use super::{NetworkFlowsView, Tab};
use crate::aggregate::{Topology, Zoom};
use crate::detect::Detection;
use crate::filter::FlowFilter;
use crate::model::{Endpoint, EndpointKind, Flow, Verdict, Workload};
use crate::provider::{
    BackendKind, BackendStatus, Capabilities, FlowProvider, FlowSink, ProviderError,
    ProviderFuture, StreamQuery,
};
use crate::service::FlowService;

/// A backend that never sends anything: the tests push flows into the service themselves.
struct Quiet;

impl FlowProvider for Quiet {
    fn kind(&self) -> BackendKind {
        BackendKind::Hubble
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            names_denies: true,
            live: true,
            single_flows: true,
            ..Capabilities::default()
        }
    }

    fn pushdown(&self, _: &FlowFilter) -> FlowFilter {
        FlowFilter::default()
    }

    fn probe(&self) -> ProviderFuture<Result<BackendStatus, ProviderError>> {
        Box::pin(async { Ok(BackendStatus::default()) })
    }

    fn stream(&self, _: StreamQuery, _: FlowSink) -> ProviderFuture<Result<(), ProviderError>> {
        Box::pin(futures::future::pending())
    }

    fn graph(
        &self,
        _: Duration,
        _: Zoom,
        _: FlowFilter,
    ) -> ProviderFuture<Result<Option<Topology>, ProviderError>> {
        Box::pin(async { Ok(None) })
    }
}

fn pod(ns: &str, workload: &str, i: usize) -> Endpoint {
    Endpoint {
        kind: EndpointKind::Pod,
        namespace: Some(ns.into()),
        pod: Some(format!("{workload}-{i}").into()),
        workload: Some(Workload {
            kind: "Deployment".into(),
            name: workload.into(),
        }),
        port: Some(80),
        ..Endpoint::default()
    }
}

/// Flows `start..start + count`, oldest first, the last one a moment ago; every tenth is
/// dropped.
fn flows(start: usize, count: usize) -> Vec<Flow> {
    let now = Timestamp::now();
    (start..start + count)
        .map(|i| {
            let age = SignedDuration::from_millis(((start + count - i) * 10) as i64);
            let mut flow = Flow::new(now.checked_sub(age).unwrap());
            flow.source = pod("storefront", "shopper", i % 3);
            flow.destination = pod("payments", "ledger-api", i % 2);
            flow.verdict = if i % 10 == 0 {
                Verdict::Dropped
            } else {
                Verdict::Forwarded
            };
            flow
        })
        .collect()
}

struct Setup {
    service: Entity<FlowService>,
    view: Entity<NetworkFlowsView>,
    cluster: ClusterId,
    cx: VisualTestContext,
    _dir: tempfile::TempDir,
}

fn setup(cx: &mut TestAppContext) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let service = cx.update(|cx| {
        kubyl_core::init(cx);
        kubyl_settings::init_with_dir(cx, dir.path());
        kubyl_ui::init(cx);
        kubyl_settings::Settings::register::<crate::settings::NetflowSettings>(cx);
        FlowService::install(false, cx)
    });
    let cluster = ClusterId::new("shop-eu.example.com");
    service.update(cx, |s, cx| {
        let detection = Detection {
            candidates: Vec::new(),
            checks: Vec::new(),
            cni: None,
        };
        s.insert_for_test(&cluster, Arc::new(Quiet), detection, cx)
    });
    let slot: Rc<RefCell<Option<Entity<NetworkFlowsView>>>> = Rc::default();
    let (_root, cx) = cx.add_window_view({
        let slot = slot.clone();
        let cluster = cluster.clone();
        move |window, cx| {
            let view = cx.new(|cx| NetworkFlowsView::new(cluster, None, window, cx));
            *slot.borrow_mut() = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        }
    });
    cx.run_until_parked();
    let view = slot.borrow().clone().unwrap();
    Setup {
        service,
        view,
        cluster,
        cx: cx.clone(),
        _dir: dir,
    }
}

impl Setup {
    fn push(&mut self, flows: Vec<Flow>) {
        let cluster = self.cluster.clone();
        self.service.update(&mut self.cx, |s, cx| {
            s.push_for_test(&cluster, &FlowFilter::default(), flows, cx)
        });
        self.cx.run_until_parked();
    }

    fn fill(&mut self, batches: usize) {
        for batch in 0..batches {
            self.push(flows(batch * 500, 500));
        }
    }
}

#[gpui::test]
fn a_live_table_keeps_up_with_thousands_of_flows(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    t.fill(12);
    t.view.update(&mut t.cx, |view, _| {
        assert_eq!(view.rows.shown.len(), 6_000);
        assert!(view.rows.pending.is_empty());
        assert_eq!(view.rows.count(Verdict::Dropped), 600);
        assert_eq!(view.rows.count(Verdict::Forwarded), 5_400);
        // Newest first.
        let shown: Vec<u64> = view.rows.shown.iter().copied().collect();
        assert!(shown.windows(2).all(|w| w[0] > w[1]));
    });
}

#[gpui::test]
fn an_open_flow_holds_new_rows_until_asked(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    t.fill(10);
    let seq = t.view.update(&mut t.cx, |view, cx| {
        let seq = view.rows.shown[2_500];
        view.select_seq(Some(seq), cx);
        view.set_details_open(true, cx);
        seq
    });
    t.push(flows(5_000, 100));
    t.view.update(&mut t.cx, |view, cx| {
        assert_eq!(view.rows.shown.len(), 5_000);
        assert_eq!(view.rows.pending.len(), 100);
        assert_eq!(view.rows.position(seq), Some(2_500));
        assert_eq!(view.selected, Some(seq));
        view.show_newest(cx);
        assert_eq!(view.rows.shown.len(), 5_100);
        assert!(view.rows.pending.is_empty());
        assert_eq!(view.selected, None);
    });
    // Following again.
    t.push(flows(5_100, 10));
    t.view.update(&mut t.cx, |view, _| {
        assert_eq!(view.rows.shown.len(), 5_110);
        assert!(view.rows.pending.is_empty());
    });
}

#[gpui::test]
fn pausing_holds_the_table_and_the_graph(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    t.fill(2);
    t.view.update(&mut t.cx, |view, cx| {
        view.toggle_pause(cx);
        view.tab = Tab::Topology;
        cx.notify();
    });
    t.cx.run_until_parked();
    t.push(flows(1_000, 50));
    t.view.update(&mut t.cx, |view, cx| {
        assert_eq!(view.rows.shown.len(), 1_000);
        assert_eq!(view.rows.pending.len(), 50);
        let graph = view.topology.graph.clone().expect("laid out");
        assert_eq!(graph.edges[0].flows, 1_000);
        view.toggle_pause(cx);
        assert_eq!(view.rows.shown.len(), 1_050);
    });
}

#[gpui::test]
fn verdict_chips_and_filters_narrow_the_rows(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    t.fill(4);
    t.view.update(&mut t.cx, |view, cx| {
        view.set_query("verdict=dropped".into(), cx)
    });
    t.cx.run_until_parked();
    t.view.update(&mut t.cx, |view, _| {
        assert_eq!(view.rows.shown.len(), 200);
        // The chips still count every verdict.
        assert_eq!(view.rows.count(Verdict::Forwarded), 1_800);
    });
    t.view.update(&mut t.cx, |view, cx| {
        view.set_query("src.pod=shopper-1".into(), cx)
    });
    t.cx.run_until_parked();
    t.view.update(&mut t.cx, |view, _| {
        assert!(view.parse_error.is_none());
        assert_eq!(view.rows.shown.len(), 667);
    });
    t.view.update(&mut t.cx, |view, cx| {
        view.set_query("dst.ns=storefront".into(), cx)
    });
    t.cx.run_until_parked();
    t.view
        .update(&mut t.cx, |view, _| assert!(view.rows.shown.is_empty()));
}

#[gpui::test]
fn the_term_being_typed_waits_for_its_value(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    t.fill(4);
    t.view.update(&mut t.cx, |view, cx| {
        view.query_edited("verdict=dropped src.pod=shop", true, cx);
        assert!(!view.suggestions.is_empty());
    });
    t.cx.run_until_parked();
    t.view.update(&mut t.cx, |view, cx| {
        // Only the finished term applies.
        assert_eq!(view.user_filter.canonical(), "verdict=dropped");
        assert_eq!(view.rows.shown.len(), 200);
        view.query_edited("verdict=dropped src.pod=shopper-1 ", true, cx);
    });
    t.cx.run_until_parked();
    t.view.update(&mut t.cx, |view, cx| {
        assert_eq!(view.rows.shown.len(), 67);
        // A value nothing extends applies at once (and finds nothing).
        view.query_edited("src.ns=nowhere", true, cx);
    });
    t.cx.run_until_parked();
    t.view
        .update(&mut t.cx, |view, _| assert!(view.rows.shown.is_empty()));
}

#[gpui::test]
fn a_longer_window_fetches_its_history_again(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    t.fill(2);
    t.view.update(&mut t.cx, |view, cx| {
        view.set_window(kubyl_charts::TimeRange::H1, cx)
    });
    t.cx.run_until_parked();
    let cluster = t.cluster.clone();
    t.service.read_with(&t.cx, |s, _| {
        let stream = s.stream(&cluster, &FlowFilter::default()).expect("stream");
        // Emptied for the fresh stream's history; the numbering goes on.
        assert!(stream.buffer.is_empty());
        assert_eq!(stream.buffer.range().start, 1_000);
        assert!(!stream.caught_up);
    });
    t.push(flows(0, 300));
    t.view
        .update(&mut t.cx, |view, _| assert_eq!(view.rows.shown.len(), 300));
}

#[gpui::test]
fn the_topology_aggregates_in_the_background(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    t.fill(2);
    t.view.update(&mut t.cx, |view, cx| {
        view.tab = Tab::Topology;
        cx.notify();
    });
    t.cx.run_until_parked();
    t.view.update(&mut t.cx, |view, cx| {
        let graph = view.topology.graph.clone().expect("laid out");
        let ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"ns:storefront") && ids.contains(&"ns:payments"));
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].dropped, 100);
        assert_eq!(view.topology.positions.len(), 2);
        view.step_graph(false, 1, cx);
        assert!(view.topology.selected.is_some());
    });
}

#[gpui::test]
fn closing_the_view_stops_its_stream(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    t.fill(1);
    let Setup {
        service,
        view,
        cluster,
        mut cx,
        _dir,
    } = t;
    assert!(!service.read_with(&cx, |s, _| s.idle_for_test(&cluster)));
    let weak = view.downgrade();
    drop(view);
    cx.update(|window, _| window.remove_window());
    cx.run_until_parked();
    assert!(weak.upgrade().is_none(), "the view outlived its window");
    let now = Instant::now();
    service.update(&mut cx, |s, cx| {
        // Noticed, then gone after the grace period.
        s.tick_at(now, cx);
        s.tick_at(now + Duration::from_secs(3), cx);
        s.tick_at(now + Duration::from_secs(6), cx);
    });
    assert!(service.read_with(&cx, |s, _| s.idle_for_test(&cluster)));
}
