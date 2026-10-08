//! Hand-written column sets for the core kinds, matching `kubectl get` (and `-o wide`): the
//! columns and cells are `kubyl_resources_core::columns`; this registers them for the views.
//!
//! Kinds without a provider use the server-side `Table` ([`crate::table`]), which carries CRD
//! printer columns. CPU and memory cells are [`CellValue::Empty`] here: list views fill them from
//! [`crate::metrics`] (phase 07 provides the data).

use std::convert::Infallible;

use gpui::App;
use kubyl_core::actions::OpenView;
use kubyl_core::{
    CellButton, CellValue, ColumnDef, ColumnProvider, Gvr, ResourceColumns, ResourceRef, ViewKind,
    ViewRequest,
};
pub use kubyl_resources_core::columns::*;
use serde_json::Value;

/// A built-in kind's columns for the views.
struct Provider(Kind);

impl ColumnProvider for Provider {
    fn columns(&self) -> Vec<ColumnDef> {
        self.0.columns()
    }

    fn cell(&self, object: &Value, column: &str) -> CellValue {
        let cell = self
            .0
            .cell(object, column)
            .map_buttons(|button: Infallible| match button {});
        match self.0.link(object, column) {
            Some(link) => link_cell(cell, link),
            None => cell,
        }
    }
}

/// A cell naming another object, as a link that opens its details.
fn link_cell(cell: CellValue, link: CellLink) -> CellValue {
    let label = match &cell {
        CellValue::Text(label) | CellValue::Tinted { label, .. } => label.clone(),
        _ => return cell,
    };
    let tooltip = format!("Open {}", link.name);
    CellValue::Buttons(vec![
        CellButton::new(label, move |row: &ResourceRef| {
            let version = match &link.version {
                Some(version) => version.clone(),
                None if link.group == row.gvr.group => row.gvr.version.clone(),
                None => "v1".into(),
            };
            let target = ResourceRef::object(
                row.cluster.clone(),
                Gvr::new(link.group.clone(), version, link.resource.clone()),
                link.namespace.clone(),
                link.name.clone(),
            );
            Box::new(OpenView(ViewRequest::for_resource(
                ViewKind::Details,
                target,
            )))
        })
        .tooltip(tooltip)
        .link(),
    ])
}

/// Registers the built-in column sets.
pub fn register(cx: &mut App) {
    for (group, kind, columns) in builtin() {
        ResourceColumns::register(cx, group, kind, Provider(columns));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_core::{ClusterId, Gvk};
    use serde_json::json;

    #[gpui::test]
    fn cells_naming_objects_are_links_to_their_details(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            register(cx);
            let provider =
                ResourceColumns::get(cx, &Gvk::new("discovery.k8s.io", "v1", "EndpointSlice"))
                    .unwrap();
            let slice = json!({"metadata": {"name": "web-x", "namespace": "shop",
                "labels": {"kubernetes.io/service-name": "web"}}});
            let CellValue::Buttons(buttons) = provider.cell(&slice, "service") else {
                panic!("not a link");
            };
            assert!(buttons[0].link);
            assert_eq!(buttons[0].label, "web");
            let row = ResourceRef::object(
                ClusterId::new("c"),
                Gvr::new("discovery.k8s.io", "v1", "endpointslices"),
                Some("shop".into()),
                "web-x".into(),
            );
            let action = (buttons[0].action)(&row);
            let open = action.as_any().downcast_ref::<OpenView>().unwrap();
            assert_eq!(open.0.kind, ViewKind::Details);
            let target = open.0.target.as_ref().unwrap();
            assert_eq!(target.gvr, Gvr::new("", "v1", "services"));
            assert_eq!(target.namespace.as_deref(), Some("shop"));
            assert_eq!(target.name.as_deref(), Some("web"));

            // A binding's policy takes the binding's own version.
            let binding = ResourceColumns::get(
                cx,
                &Gvk::new(
                    "admissionregistration.k8s.io",
                    "v1beta1",
                    "ValidatingAdmissionPolicyBinding",
                ),
            )
            .unwrap();
            let object = json!({"spec": {"policyName": "p", "validationActions": ["Deny"]}});
            let CellValue::Buttons(buttons) = binding.cell(&object, "policy") else {
                panic!("not a link");
            };
            let row = ResourceRef::object(
                ClusterId::new("c"),
                Gvr::new(
                    "admissionregistration.k8s.io",
                    "v1beta1",
                    "validatingadmissionpolicybindings",
                ),
                None,
                "b".into(),
            );
            let action = (buttons[0].action)(&row);
            let open = action.as_any().downcast_ref::<OpenView>().unwrap();
            assert_eq!(
                open.0.target.as_ref().unwrap().gvr,
                Gvr::new(
                    "admissionregistration.k8s.io",
                    "v1beta1",
                    "validatingadmissionpolicies"
                )
            );
            // Plain cells stay plain.
            assert_eq!(
                binding.cell(&object, "actions"),
                CellValue::Tinted {
                    label: "Deny".into(),
                    tone: kubyl_core::Tone::Bad,
                }
            );
        });
    }
}
