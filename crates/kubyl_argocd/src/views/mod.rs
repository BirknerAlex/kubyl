//! The Argo CD tabs: Applications (board 12), an application (boards 13 and 14),
//! ApplicationSets and Projects.

pub mod app;
pub mod apps;
pub mod appsets;
pub mod projects;

use gpui::{
    App, AppContext as _, IntoElement, KeyBinding, SharedString, Window, actions, div, prelude::*,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{ActiveContext, ClusterId, Gvr, ResourceRef, ViewKind, ViewRegistry, ViewRequest};
use kubyl_kube::ConnectionManager;

use crate::dialogs;
use crate::state::{ApiState, ArgoCd};
use crate::widgets;

actions!(
    argocd_views,
    [
        /// Focuses the filter field of an Argo CD view.
        FocusFilter,
    ]
);

pub(crate) fn init(cx: &mut App) {
    for context in ["ArgoApps", "ArgoApp", "ArgoSets", "ArgoProjects"] {
        cx.bind_keys([KeyBinding::new("/", FocusFilter, Some(context))]);
    }
    ViewRegistry::register(
        cx,
        ViewKind::Custom(apps::VIEW_KIND.into()),
        |request, window, cx| {
            let target = request.target.clone()?;
            Some(Box::new(
                cx.new(|cx| apps::AppsView::new(target, window, cx)),
            ))
        },
    );
    ViewRegistry::register(
        cx,
        ViewKind::Custom(app::VIEW_KIND.into()),
        |request, window, cx| {
            let target = request.target.clone().filter(ResourceRef::is_object)?;
            Some(Box::new(cx.new(|cx| app::AppView::new(target, window, cx))))
        },
    );
    ViewRegistry::register(
        cx,
        ViewKind::Custom(appsets::VIEW_KIND.into()),
        |request, window, cx| {
            let target = request.target.clone()?;
            Some(Box::new(
                cx.new(|cx| appsets::AppSetsView::new(target, window, cx)),
            ))
        },
    );
    ViewRegistry::register(
        cx,
        ViewKind::Custom(projects::VIEW_KIND.into()),
        |request, window, cx| {
            let target = request.target.clone()?;
            Some(Box::new(
                cx.new(|cx| projects::ProjectsView::new(target, window, cx)),
            ))
        },
    );
    let group = crate::model::GROUP;
    ViewRegistry::register_list_view(
        cx,
        group,
        "applications",
        ViewKind::Custom(apps::VIEW_KIND.into()),
    );
    ViewRegistry::register_object_view(
        cx,
        group,
        "applications",
        ViewKind::Custom(app::VIEW_KIND.into()),
    );
    ViewRegistry::register_list_view(
        cx,
        group,
        "applicationsets",
        ViewKind::Custom(appsets::VIEW_KIND.into()),
    );
    ViewRegistry::register_list_view(
        cx,
        group,
        "appprojects",
        ViewKind::Custom(projects::VIEW_KIND.into()),
    );
}

/// The mode chip: "Kubernetes mode" opens the sign-in dialog; "API · user" opens a menu.
pub fn mode_button(
    id: &'static str,
    cluster: &ClusterId,
    state: &ApiState,
    cx: &App,
) -> gpui::AnyElement {
    let chip = widgets::mode_chip(state);
    let tooltip: SharedString = match state {
        ApiState::Off => "Reads and patches the Application objects with your Kubernetes access. Sign in to Argo CD for desired-vs-live diffs and the full resource tree.".into(),
        ApiState::Connecting => "Connecting to argocd-server…".into(),
        ApiState::SignInRequired(reason) => reason
            .clone()
            .unwrap_or_else(|| "Sign in to Argo CD for API mode.".into())
            .into(),
        ApiState::Failed(err) => format!("API mode failed: {err}").into(),
        ApiState::Connected { user, version, via } => {
            format!("Signed in to Argo CD {version} as {user}, through {via}.").into()
        }
    };
    let _ = cx;
    let cluster = cluster.clone();
    match state {
        ApiState::Connected { .. } => {
            let sign_out = cluster.clone();
            let forget = cluster.clone();
            let redetect = cluster.clone();
            MenuButton::new(id)
                .ghost()
                .compact()
                .p_0()
                .child(chip)
                .dropdown_menu(move |menu, _, _| {
                    let sign_out = sign_out.clone();
                    let forget = forget.clone();
                    let redetect = redetect.clone();
                    menu.label(tooltip.clone())
                        .separator()
                        .item(PopupMenuItem::new("Sign Out").on_click(move |_, _, cx| {
                            if let Some(argo) = ArgoCd::try_global(cx) {
                                argo.update(cx, |argo, cx| argo.sign_out(&sign_out, false, cx));
                            }
                        }))
                        .item(
                            PopupMenuItem::new("Sign Out and Forget This Install").on_click(
                                move |_, _, cx| {
                                    if let Some(argo) = ArgoCd::try_global(cx) {
                                        argo.update(cx, |argo, cx| {
                                            argo.sign_out(&forget, true, cx)
                                        });
                                    }
                                },
                            ),
                        )
                        .separator()
                        .item(PopupMenuItem::new("Look for Argo CD Again").on_click(
                            move |_, _, cx| {
                                if let Some(argo) = ArgoCd::try_global(cx) {
                                    argo.update(cx, |argo, cx| argo.redetect(&redetect, cx));
                                }
                            },
                        ))
                })
                .into_any_element()
        }
        _ => div()
            .id(id)
            .cursor_pointer()
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
            })
            .on_click(move |_, window, cx| dialogs::open_sign_in(cluster.clone(), window, cx))
            .child(chip)
            .into_any_element(),
    }
}

/// Opens a destination: its cluster (made active) and namespace, on the namespace overview
/// (the Pods list without phase 07).
pub fn open_destination(
    cluster: &ClusterId,
    namespace: Option<String>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(manager) = ConnectionManager::try_global(cx) {
        manager.update(cx, |m, cx| m.activate(cluster, cx));
    }
    let active = ActiveContext::global(cx).clone();
    ActiveContext::set(
        cx,
        ActiveContext {
            namespace: namespace.clone().map(SharedString::from),
            ..active
        },
    );
    let request = if ViewRegistry::is_registered(cx, &ViewKind::Overview) {
        ViewRequest::for_resource(
            ViewKind::Overview,
            ResourceRef::list(cluster.clone(), Gvr::new("", "", ""), namespace),
        )
    } else {
        ViewRequest::for_resource(
            ViewKind::Table,
            ResourceRef::list(cluster.clone(), Gvr::new("", "v1", "pods"), namespace),
        )
    };
    window.dispatch_action(Box::new(OpenView(request)), cx);
}
