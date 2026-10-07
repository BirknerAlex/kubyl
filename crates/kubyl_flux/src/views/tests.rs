//! GPUI tests of the Flux views: no cluster connects; stores are filled by hand and the
//! cluster's caps come from `TestCaps`.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{AppContext as _, Entity, TestAppContext};
use kubyl_core::{ClusterCaps, ClusterId, FluxCaps, Gvr, ResourceRef};
use kubyl_flux_core::fixtures;
use kubyl_flux_core::kinds::{Category, FluxKind};
use kubyl_flux_core::model::State;
use kubyl_flux_core::overview::{Attention, Health};
use kubyl_resources::{ResourceStore, ResourceStores, StoreHandle};
use serde_json::Value;

use super::list::ListView;
use super::overview::OverviewView;
use crate::state::{self, TestCaps};

/// What a test's app keeps until it's dropped.
struct KeepAlive {
    _stores: Vec<StoreHandle>,
    _dir: tempfile::TempDir,
}

impl gpui::Global for KeepAlive {}

fn caps(read_only: bool) -> ClusterCaps {
    ClusterCaps {
        read_only,
        flux: FluxCaps {
            kustomizations: true,
            helm_releases: true,
            sources: true,
            image_automation: false,
            notifications: true,
        },
        ..ClusterCaps::default()
    }
}

/// Installs what the views need and fills the all-namespaces stores of the kinds.
fn setup(
    cx: &mut TestAppContext,
    read_only: bool,
    objects: Vec<(FluxKind, Vec<Value>)>,
) -> (ClusterId, Vec<StoreHandle>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let cluster = ClusterId::new("kind-flux@/k");
    let handles = cx.update(|cx| {
        kubyl_core::init(cx);
        kubyl_settings::init_with_dir(cx, dir.path());
        kubyl_ui::init(cx);
        cx.set_global(TestCaps(caps(read_only)));
        crate::nav::init(cx);
        crate::actions::init(cx);
        super::init(cx);
        let mut handles = Vec::new();
        for (kind, values) in objects {
            let gvr = state::gvr(&cluster, kind, cx).unwrap();
            let key = state::all_key(&cluster, &gvr);
            let store = cx.new(|_| ResourceStore::from_objects(key.clone(), values));
            handles.push(ResourceStores::insert(cx, key, store));
        }
        handles
    });
    (cluster, handles, dir)
}

#[gpui::test]
fn the_overview_shows_a_failing_and_a_suspended_object(cx: &mut TestAppContext) {
    let (cluster, _stores, _dir) = setup(
        cx,
        false,
        vec![
            (
                FluxKind::Kustomization,
                vec![
                    fixtures::kustomization_ready(),
                    fixtures::kustomization_failed(),
                    fixtures::kustomization_suspended(),
                    fixtures::kustomization_waiting(),
                ],
            ),
            (FluxKind::HelmRelease, vec![fixtures::helm_release_v2()]),
            (FluxKind::GitRepository, vec![fixtures::git_repository()]),
        ],
    );
    let slot: Rc<RefCell<Option<Entity<OverviewView>>>> = Default::default();
    let (_root, cx) = cx.add_window_view({
        let slot = slot.clone();
        move |window, cx| {
            let view = cx.new(|cx| OverviewView::new(cluster, window, cx));
            *slot.borrow_mut() = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        }
    });
    let view = slot.borrow().clone().unwrap();
    cx.run_until_parked();
    view.update(cx, |view, _| {
        let attention = view.attention();
        let names: Vec<&str> = attention.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "Kustomization flux-demo/broken",
                "Kustomization flux-demo/apps-late",
                "Kustomization flux-demo/paused",
            ]
        );
        assert_eq!(attention[0].1, Attention::Failed);
        assert_eq!(
            attention[1].1,
            Attention::Waiting("flux-demo/broken".into())
        );
        assert_eq!(attention[2].1, Attention::Suspended);
        assert_eq!(view.health(), Health::Failing);
    });
}

fn list(
    cx: &mut TestAppContext,
    read_only: bool,
) -> (Entity<ListView>, &mut gpui::VisualTestContext) {
    let (cluster, stores, dir) = setup(
        cx,
        read_only,
        vec![(
            FluxKind::Kustomization,
            vec![
                fixtures::kustomization_ready(),
                fixtures::kustomization_failed(),
                fixtures::kustomization_suspended(),
            ],
        )],
    );
    // The stores and the settings dir live as long as the app (removed with it).
    cx.update(|cx| {
        cx.set_global(KeepAlive {
            _stores: stores,
            _dir: dir,
        })
    });
    let slot: Rc<RefCell<Option<Entity<ListView>>>> = Default::default();
    let (_root, cx) = cx.add_window_view({
        let slot = slot.clone();
        move |window, cx| {
            let target = ResourceRef::list(cluster, Gvr::new("", "", ""), None);
            let view = cx.new(|cx| ListView::new(Category::Kustomizations, target, window, cx));
            *slot.borrow_mut() = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        }
    });
    let view = slot.borrow().clone().unwrap();
    cx.run_until_parked();
    (view, cx)
}

fn labels(hints: Vec<(gpui::SharedString, gpui::SharedString)>) -> Vec<String> {
    hints.into_iter().map(|(_, l)| l.to_string()).collect()
}

#[gpui::test]
fn actions_are_hidden_on_read_only_clusters(cx: &mut TestAppContext) {
    let (view, cx) = list(cx, true);
    view.update(cx, |view, cx| {
        assert_eq!(
            view.row_names(),
            ["flux-demo/broken", "flux-demo/paused", "flux-demo/podinfo"]
        );
        // Nothing selected, and with a selection: no writing action.
        for select in [None, Some("flux-demo/podinfo")] {
            if let Some(key) = select {
                view.select_name(key, cx);
            }
            let hints = labels(view.hints(cx));
            for writing in ["Reconcile", "With source", "Suspend", "Resume", "Delete…"] {
                assert!(
                    !hints.iter().any(|h| h == writing),
                    "{writing} in {hints:?}"
                );
            }
            assert!(hints.iter().any(|h| h == "Edit YAML"));
        }
    });
}

#[gpui::test]
fn actions_show_where_they_apply(cx: &mut TestAppContext) {
    let (view, cx) = list(cx, false);
    view.update(cx, |view, cx| {
        view.select_name("flux-demo/podinfo", cx);
        let hints = labels(view.hints(cx));
        for action in ["Reconcile", "With source", "Suspend", "Delete…", "Logs"] {
            assert!(
                hints.iter().any(|h| h == action),
                "{action} missing in {hints:?}"
            );
        }
        // What doesn't apply to the object isn't offered.
        assert!(!hints.iter().any(|h| h == "Resume"), "{hints:?}");
        view.select_name("flux-demo/paused", cx);
        let hints = labels(view.hints(cx));
        assert!(hints.iter().any(|h| h == "Resume"), "{hints:?}");
        assert!(!hints.iter().any(|h| h == "Reconcile"), "{hints:?}");
        // The state chips filter the rows.
        view.set_states([State::Failed], cx);
        assert_eq!(view.row_names(), ["flux-demo/broken"]);
    });
}

/// A list in a background tab doesn't take the global selection over; once focused it does.
#[gpui::test]
fn only_a_focused_list_publishes_its_selection(cx: &mut TestAppContext) {
    let (view, cx) = list(cx, false);
    let selected = |cx: &mut gpui::VisualTestContext| -> Vec<String> {
        cx.update(|_, cx| {
            cx.try_global::<kubyl_resources::ResourceSelection>()
                .map(|s| s.targets().filter_map(|t| t.name.clone()).collect())
                .unwrap_or_default()
        })
    };
    view.update(cx, |view, cx| view.select_name("flux-demo/podinfo", cx));
    cx.run_until_parked();
    assert!(selected(cx).is_empty());
    cx.update(|window, cx| {
        let focus = gpui::Focusable::focus_handle(view.read(cx), cx);
        window.activate_window();
        focus.focus(window, cx);
        // Focus listeners run when the window draws.
        window.refresh();
    });
    cx.run_until_parked();
    assert_eq!(selected(cx), ["podinfo"]);
    // Filtered out: the selection goes with the row (keys must not act on a hidden object).
    view.update(cx, |view, cx| view.set_states([State::Failed], cx));
    cx.run_until_parked();
    assert!(selected(cx).is_empty());
}
