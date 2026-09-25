//! Port-forwards running in this app, published by `kubyl_portforward` so other crates can show
//! them next to the ports they forward (the details pane) without depending on it. Start and stop
//! forwards with [`crate::actions::ForwardPort`] and [`crate::actions::StopForward`].

use gpui::{App, Global};

use crate::ResourceRef;

/// One forward, as other crates see it.
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveForward {
    /// Pass to [`crate::actions::StopForward`].
    pub id: u64,
    /// The pod, Service or workload that was forwarded.
    pub target: ResourceRef,
    /// The forwarded port as picked (`None` = the first port of the target).
    pub remote_port: Option<u16>,
    /// `localhost:18080` once the local port listens.
    pub local: Option<String>,
    /// `http://localhost:18080` for HTTP ports.
    pub url: Option<String>,
}

/// The forwards running now. Observe it with `cx.observe_global::<ActiveForwards>`.
#[derive(Clone, Debug, Default)]
pub struct ActiveForwards(Vec<ActiveForward>);

impl Global for ActiveForwards {}

impl ActiveForwards {
    pub(crate) fn init(cx: &mut App) {
        cx.set_global(Self::default());
    }

    pub fn all(cx: &App) -> &[ActiveForward] {
        cx.try_global::<Self>()
            .map(|f| f.0.as_slice())
            .unwrap_or_default()
    }

    /// The forward of `port` on `target`, if one runs.
    pub fn find<'a>(cx: &'a App, target: &ResourceRef, port: u16) -> Option<&'a ActiveForward> {
        Self::all(cx)
            .iter()
            .find(|f| f.remote_port == Some(port) && same_object(&f.target, target))
    }

    /// Replaces the list (the port-forward crate calls this whenever a forward changes).
    pub fn set(cx: &mut App, forwards: Vec<ActiveForward>) {
        if Self::all(cx) != forwards.as_slice() {
            cx.set_global(Self(forwards));
        }
    }
}

/// The same object: API versions may differ between the forward and the view.
fn same_object(a: &ResourceRef, b: &ResourceRef) -> bool {
    a.cluster == b.cluster
        && a.gvr.group == b.gvr.group
        && a.gvr.resource == b.gvr.resource
        && a.namespace == b.namespace
        && a.name == b.name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClusterId, Gvr};

    #[gpui::test]
    fn finds_forwards_by_object_and_port(cx: &mut gpui::TestAppContext) {
        let pod = |version: &str| {
            ResourceRef::object(
                ClusterId::new("c"),
                Gvr::new("", version, "pods"),
                Some("ns".into()),
                "web-0".into(),
            )
        };
        cx.update(|cx| {
            ActiveForwards::init(cx);
            ActiveForwards::set(
                cx,
                vec![ActiveForward {
                    id: 7,
                    target: pod("v1"),
                    remote_port: Some(8080),
                    local: Some("localhost:8080".into()),
                    url: None,
                }],
            );
            assert_eq!(ActiveForwards::find(cx, &pod("v1"), 8080).unwrap().id, 7);
            assert!(ActiveForwards::find(cx, &pod("v1"), 9090).is_none());
            assert!(ActiveForwards::find(cx, &pod("v2"), 8080).is_some());
        });
    }
}
