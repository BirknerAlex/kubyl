//! Flux CD (phase 23, board 21): see and drive Flux when a cluster runs it: an overview with
//! what needs attention, lists of Kustomizations, HelmReleases, sources, image automation and
//! notifications, an object's tab with its inventory, dependencies, history, Events and
//! controller logs, and the actions of the `flux` CLI (reconcile, with source, force, reset,
//! suspend, resume, delete) as patches with the user's own access. Nothing shows unless the
//! cluster serves Flux's CRDs; the UI follows them appearing and disappearing.
//!
//! - [`state`]: the Flux entity (served kinds, controllers, the index for the workload column).
//! - [`views`]: overview, lists, an object's tab. [`actions`]: palette, keys and the actions.
//! - [`dock`]: details-dock sections and the YAML editor notice. [`columns`]: the explorer's
//!   tables.

pub mod actions;
pub mod columns;
pub mod dock;
pub mod nav;
pub mod state;
pub mod views;
pub mod widgets;

pub use kubyl_flux_core::{
    agent, deps, details, detect, inventory, kinds, links, model, ops, overview, ownership, rows,
    service,
};

use std::sync::Arc;

use gpui::{App, SharedString};
use kubyl_core::{ChromeRegistry, ClusterId, FluxCaps, ViewKind};
use kubyl_explorer::catalog::{self, GroupView, TreeGroup, ViewEntry};
use kubyl_flux_core::kinds::{Category, FluxKind};
use kubyl_palette::PaletteView;
use kubyl_ui::IconName;

use state::Flux;

/// A view row of the Flux group: id, label, icon, view, above the kinds, served when.
type GroupViewSpec = (
    &'static str,
    &'static str,
    IconName,
    ViewKind,
    bool,
    fn(FluxCaps) -> bool,
);

/// Registers this crate's views, actions and chrome contributions.
pub fn init(cx: &mut App) {
    let flux = Flux::install(cx);
    let index = flux.read(cx).index();
    nav::init(cx);
    views::init(cx);
    actions::init(cx);
    columns::init(index, cx);
    ChromeRegistry::add_details_section(cx, dock::FluxDetails);
    ChromeRegistry::add_edit_notice(cx, dock::FluxNotice);
    catalog::register_tree_group(
        cx,
        TreeGroup {
            id: "flux",
            parent: "administration",
            label: "Flux",
            kinds: vec![
                catalog::k(
                    FluxKind::Kustomization.group(),
                    FluxKind::Kustomization.plural(),
                    "Kustomizations",
                    widgets::kind_icon(FluxKind::Kustomization),
                ),
                catalog::k(
                    FluxKind::HelmRelease.group(),
                    FluxKind::HelmRelease.plural(),
                    "HelmReleases",
                    widgets::kind_icon(FluxKind::HelmRelease),
                ),
            ],
            // The cluster's Flux version.
            badge: Some(Arc::new(|cluster, cx| {
                Flux::try_global(cx)?
                    .read(cx)
                    .version(cluster)
                    .map(SharedString::from)
            })),
        },
    );
    let views: [GroupViewSpec; 4] = [
        (
            "flux-overview",
            "Overview",
            IconName::Gauge,
            ViewKind::Custom(views::overview::VIEW_KIND.into()),
            true,
            |caps| caps.any(),
        ),
        (
            "flux-sources",
            "Sources",
            widgets::category_icon(Category::Sources),
            ViewKind::Custom(Category::Sources.view_id().into()),
            false,
            |caps| caps.sources,
        ),
        (
            "flux-images",
            "Image Automation",
            widgets::category_icon(Category::ImageAutomation),
            ViewKind::Custom(Category::ImageAutomation.view_id().into()),
            false,
            |caps| caps.image_automation,
        ),
        (
            "flux-notifications",
            "Notifications",
            widgets::category_icon(Category::Notifications),
            ViewKind::Custom(Category::Notifications.view_id().into()),
            false,
            |caps| caps.notifications,
        ),
    ];
    for (id, label, icon, kind, before_kinds, served) in views {
        catalog::register_group_view(
            cx,
            GroupView {
                group: "flux",
                entry: ViewEntry {
                    id,
                    label,
                    icon,
                    kind,
                    needs_olm: false,
                },
                before_kinds,
                visible: Arc::new(move |cluster: &ClusterId, cx: &App| {
                    served(Flux::caps(cluster, cx))
                }),
            },
        );
    }
    // `:sources` and `:flux` (the kinds answer to `:ks`, `:kustomizations`, `:hr`,
    // `:helmreleases` through their CRDs).
    kubyl_palette::register_view(
        cx,
        PaletteView {
            name: "sources",
            aliases: vec!["flux-sources"],
            detail: "Flux sources",
            icon: widgets::category_icon(Category::Sources),
            kind: ViewKind::Custom(Category::Sources.view_id().into()),
            visible: Arc::new(|cluster, cx| Flux::caps(cluster, cx).sources),
        },
    );
    kubyl_palette::register_view(
        cx,
        PaletteView {
            name: "fluxoverview",
            aliases: vec!["flux"],
            detail: "Flux overview",
            icon: IconName::Gauge,
            kind: ViewKind::Custom(views::overview::VIEW_KIND.into()),
            visible: Arc::new(|cluster, cx| Flux::caps(cluster, cx).any()),
        },
    );
}
