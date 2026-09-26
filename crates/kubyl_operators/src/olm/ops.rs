//! Writes: installing an operator (Namespace, OperatorGroup and Subscription as needed),
//! approving an InstallPlan, uninstalling. The dialogs show [`InstallSteps`] and the uninstall
//! steps before any of this runs; they run on Tokio.

use std::collections::BTreeSet;
use std::time::Duration;

use k8s_openapi::api::core::v1::Namespace;
use kube::api::{
    ApiResource, DeleteParams, DynamicObject, ListParams, Patch, PatchParams, PostParams,
};
use kube::{Api, Client};
use serde_json::json;

use super::model::{Approval, GROUP, InstallMode, OperatorGroup, Subscription};

/// The field manager of Kubyl's writes.
pub const FIELD_MANAGER: &str = "kubyl";

fn resource(version: &str, kind: &str, plural: &str) -> ApiResource {
    ApiResource {
        group: GROUP.into(),
        version: version.into(),
        api_version: format!("{GROUP}/{version}"),
        kind: kind.into(),
        plural: plural.into(),
    }
}

fn subscription_resource() -> ApiResource {
    resource("v1alpha1", "Subscription", "subscriptions")
}

fn operator_group_resource() -> ApiResource {
    resource("v1", "OperatorGroup", "operatorgroups")
}

fn install_plan_resource() -> ApiResource {
    resource("v1alpha1", "InstallPlan", "installplans")
}

fn csv_resource() -> ApiResource {
    resource(
        "v1alpha1",
        "ClusterServiceVersion",
        "clusterserviceversions",
    )
}

/// Where the operator goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Watches every namespace: installed next to the global OperatorGroup.
    AllNamespaces,
    /// Watches only the namespace it's installed in.
    Namespace(String),
}

/// What the user picked in the install dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallChoice {
    pub package: String,
    pub catalog: String,
    pub catalog_namespace: String,
    pub channel: String,
    /// A version other than the channel head (`startingCSV`).
    pub starting_csv: Option<String>,
    pub approval: Approval,
    pub target: Target,
}

/// The OperatorGroup an install uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroupStep {
    Existing(String),
    Create { name: String, global: bool },
}

/// What an install creates and uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallSteps {
    pub namespace: String,
    pub create_namespace: bool,
    pub group: GroupStep,
    pub subscription: String,
    pub mode: InstallMode,
}

/// Namespaces vanilla OLM and OpenShift put all-namespaces operators in.
const GLOBAL_NAMESPACES: [&str; 2] = ["openshift-operators", "operators"];

/// A valid namespace name (DNS-1123 label).
pub fn valid_namespace(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// Works out what installing `choice` needs, or why it can't go ahead. `modes`: what the
/// chosen version supports; `groups`, `namespaces`, `subscriptions`: the cluster's.
pub fn plan_install(
    choice: &InstallChoice,
    modes: &BTreeSet<InstallMode>,
    groups: &[OperatorGroup],
    namespaces: &[String],
    subscriptions: &[std::sync::Arc<Subscription>],
) -> Result<InstallSteps, String> {
    let (namespace, group, mode) = match &choice.target {
        Target::AllNamespaces => {
            if !modes.contains(&InstallMode::AllNamespaces) {
                return Err("This operator can't watch all namespaces: pick a namespace.".into());
            }
            let mut global: Vec<&OperatorGroup> = groups.iter().filter(|g| g.is_global()).collect();
            global.sort_by_key(|g| {
                GLOBAL_NAMESPACES
                    .iter()
                    .position(|n| *n == g.namespace)
                    .unwrap_or(GLOBAL_NAMESPACES.len())
            });
            match global.first() {
                Some(g) => {
                    let others = groups.iter().filter(|o| o.namespace == g.namespace).count();
                    if others > 1 {
                        return Err(format!(
                            "{} has {others} OperatorGroups; OLM installs nothing there until only one is left.",
                            g.namespace
                        ));
                    }
                    (
                        g.namespace.clone(),
                        GroupStep::Existing(g.name.clone()),
                        InstallMode::AllNamespaces,
                    )
                }
                None => {
                    // Vanilla OLM ships `operators/global-operators`; recreate that.
                    if groups.iter().any(|g| g.namespace == "operators") {
                        return Err("No OperatorGroup targets all namespaces, and the operators namespace has one that doesn't. Pick a namespace.".into());
                    }
                    (
                        "operators".to_string(),
                        GroupStep::Create {
                            name: "global-operators".into(),
                            global: true,
                        },
                        InstallMode::AllNamespaces,
                    )
                }
            }
        }
        Target::Namespace(ns) => {
            let ns = ns.trim().to_string();
            if !valid_namespace(&ns) {
                return Err(format!(
                    "\"{ns}\" isn't a valid namespace name (lowercase letters, digits and dashes)."
                ));
            }
            let here: Vec<&OperatorGroup> = groups.iter().filter(|g| g.namespace == ns).collect();
            match here.as_slice() {
                [] => {
                    if !modes.contains(&InstallMode::OwnNamespace) {
                        return Err(if modes.contains(&InstallMode::AllNamespaces) {
                            "This operator only installs for all namespaces.".into()
                        } else {
                            "This operator doesn't support watching its own namespace.".into()
                        });
                    }
                    (
                        ns.clone(),
                        GroupStep::Create {
                            name: ns.clone(),
                            global: false,
                        },
                        InstallMode::OwnNamespace,
                    )
                }
                [group] => {
                    let mode = group.mode().unwrap_or(InstallMode::MultiNamespace);
                    if !modes.contains(&mode) {
                        return Err(format!(
                            "The OperatorGroup {} in {ns} makes operators watch {}, which this operator doesn't support.",
                            group.name,
                            match mode {
                                InstallMode::AllNamespaces => "all namespaces".to_string(),
                                InstallMode::OwnNamespace => "their own namespace".to_string(),
                                _ => group.target_namespaces.join(", "),
                            }
                        ));
                    }
                    (ns.clone(), GroupStep::Existing(group.name.clone()), mode)
                }
                many => {
                    return Err(format!(
                        "{ns} has {} OperatorGroups; OLM installs nothing there until only one is left.",
                        many.len()
                    ));
                }
            }
        }
    };
    if let Some(existing) = subscriptions
        .iter()
        .find(|s| s.namespace == namespace && s.package == choice.package)
    {
        return Err(format!(
            "{} is already subscribed in {namespace} (Subscription {}).",
            choice.package, existing.name
        ));
    }
    Ok(InstallSteps {
        create_namespace: !namespaces.iter().any(|n| n == &namespace),
        namespace,
        group,
        subscription: choice.package.clone(),
        mode,
    })
}

/// The Subscription object an install creates.
pub fn subscription_object(choice: &InstallChoice, steps: &InstallSteps) -> serde_json::Value {
    let mut spec = json!({
        "name": choice.package,
        "channel": choice.channel,
        "source": choice.catalog,
        "sourceNamespace": choice.catalog_namespace,
        "installPlanApproval": choice.approval.label(),
    });
    if let Some(csv) = &choice.starting_csv {
        spec["startingCSV"] = json!(csv);
    }
    json!({
        "apiVersion": format!("{GROUP}/v1alpha1"),
        "kind": "Subscription",
        "metadata": {"name": steps.subscription, "namespace": steps.namespace},
        "spec": spec,
    })
}

fn created<T>(result: Result<T, kube::Error>, what: &str, ns: Option<&str>) -> Result<(), String> {
    match result {
        Ok(_) => Ok(()),
        // Created meanwhile (another click, another user): fine.
        Err(kube::Error::Api(status)) if status.code == 409 => Ok(()),
        Err(err) => Err(crate::errors::describe(&err, "create", what, ns)),
    }
}

/// Runs an install: Namespace and OperatorGroup when needed, then the Subscription.
pub async fn install(
    client: Client,
    choice: InstallChoice,
    steps: InstallSteps,
) -> Result<(), String> {
    let post = PostParams {
        field_manager: Some(FIELD_MANAGER.into()),
        ..Default::default()
    };
    if steps.create_namespace {
        let namespaces: Api<Namespace> = Api::all(client.clone());
        let object = Namespace {
            metadata: kube::api::ObjectMeta {
                name: Some(steps.namespace.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        created(namespaces.create(&post, &object).await, "namespaces", None)?;
    }
    if let GroupStep::Create { name, global } = &steps.group {
        let api: Api<DynamicObject> =
            Api::namespaced_with(client.clone(), &steps.namespace, &operator_group_resource());
        let spec = if *global {
            json!({})
        } else {
            json!({"targetNamespaces": [steps.namespace]})
        };
        let object: DynamicObject = serde_json::from_value(json!({
            "apiVersion": format!("{GROUP}/v1"),
            "kind": "OperatorGroup",
            "metadata": {"name": name, "namespace": steps.namespace},
            "spec": spec,
        }))
        .map_err(|e| e.to_string())?;
        created(
            api.create(&post, &object).await,
            "operatorgroups",
            Some(&steps.namespace),
        )?;
    }
    let api: Api<DynamicObject> =
        Api::namespaced_with(client, &steps.namespace, &subscription_resource());
    let object: DynamicObject =
        serde_json::from_value(subscription_object(&choice, &steps)).map_err(|e| e.to_string())?;
    api.create(&post, &object)
        .await
        .map(|_| ())
        .map_err(|e| crate::errors::describe(&e, "create", "subscriptions", Some(&steps.namespace)))
}

/// Approves an InstallPlan (`spec.approved: true`).
pub async fn approve(client: Client, namespace: String, name: String) -> Result<(), String> {
    let api: Api<DynamicObject> =
        Api::namespaced_with(client, &namespace, &install_plan_resource());
    let params = PatchParams {
        field_manager: Some(FIELD_MANAGER.into()),
        ..Default::default()
    };
    api.patch(
        &name,
        &params,
        &Patch::Merge(json!({"spec": {"approved": true}})),
    )
    .await
    .map(|_| ())
    .map_err(|e| crate::errors::describe(&e, "patch", "installplans", Some(&namespace)))
}

/// What an uninstall deletes.
#[derive(Clone, Debug, Default)]
pub struct UninstallSpec {
    /// `(namespace, name)`.
    pub subscription: Option<(String, String)>,
    pub csv: Option<(String, String)>,
    /// The operator's CRDs (`<plural>.<group>`) with the served resource of each, when their
    /// instances and definitions go too.
    pub crds: Vec<(String, ApiResource, bool)>,
}

/// How long an uninstall waits for instances to go (their finalizers need the operator).
pub const INSTANCE_WAIT: Duration = Duration::from_secs(120);

fn deleted<T>(result: Result<T, kube::Error>, what: &str, ns: Option<&str>) -> Result<(), String> {
    match result {
        Ok(_) => Ok(()),
        Err(err) if crate::errors::is_not_found(&err) => Ok(()),
        Err(err) => Err(crate::errors::describe(&err, "delete", what, ns)),
    }
}

/// Counts the instances of each CRD (all namespaces).
async fn instance_count(
    client: &Client,
    crds: &[(String, ApiResource, bool)],
) -> Result<usize, String> {
    let mut total = 0;
    for (name, resource, _) in crds {
        let api: Api<DynamicObject> = Api::all_with(client.clone(), resource);
        match api.list_metadata(&ListParams::default()).await {
            Ok(list) => total += list.items.len(),
            Err(err) if crate::errors::is_not_found(&err) => {}
            Err(err) => return Err(crate::errors::describe(&err, "list", name, None)),
        }
    }
    Ok(total)
}

/// Uninstalls. With CRDs: their instances first (while the operator still runs, so finalizers
/// complete), then the Subscription and CSV, then the CRDs. `progress` gets a line per step.
pub async fn uninstall(
    client: Client,
    spec: UninstallSpec,
    progress: impl Fn(String) + Send + 'static,
) -> Result<(), String> {
    if !spec.crds.is_empty() {
        let count = instance_count(&client, &spec.crds).await?;
        if count > 0 {
            progress(format!("Deleting {count} instances…"));
            for (name, resource, namespaced) in &spec.crds {
                let api: Api<DynamicObject> = Api::all_with(client.clone(), resource);
                let list = match api.list_metadata(&ListParams::default()).await {
                    Ok(list) => list,
                    Err(err) if crate::errors::is_not_found(&err) => continue,
                    Err(err) => return Err(crate::errors::describe(&err, "list", name, None)),
                };
                for item in list.items {
                    let item_name = item.metadata.name.clone().unwrap_or_default();
                    let ns = item.metadata.namespace.clone();
                    let api: Api<DynamicObject> = match (&ns, namespaced) {
                        (Some(ns), true) => Api::namespaced_with(client.clone(), ns, resource),
                        _ => Api::all_with(client.clone(), resource),
                    };
                    deleted(
                        api.delete(&item_name, &DeleteParams::default()).await,
                        name,
                        ns.as_deref(),
                    )?;
                }
            }
            let start = std::time::Instant::now();
            let mut announced = false;
            loop {
                let left = instance_count(&client, &spec.crds).await?;
                if left == 0 {
                    break;
                }
                if start.elapsed() > INSTANCE_WAIT {
                    return Err(format!(
                        "{left} instances are still there after {} s (finalizers). The operator stays installed; look at the instances and try again.",
                        INSTANCE_WAIT.as_secs()
                    ));
                }
                if !announced {
                    progress(format!("Waiting for {left} instances to go (finalizers)…"));
                    announced = true;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    if let Some((ns, name)) = &spec.subscription {
        progress(format!("Deleting Subscription {ns}/{name}…"));
        let api: Api<DynamicObject> =
            Api::namespaced_with(client.clone(), ns, &subscription_resource());
        deleted(
            api.delete(name, &DeleteParams::default()).await,
            "subscriptions",
            Some(ns),
        )?;
    }
    if let Some((ns, name)) = &spec.csv {
        progress(format!("Deleting ClusterServiceVersion {name}…"));
        let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), ns, &csv_resource());
        deleted(
            api.delete(name, &DeleteParams::default()).await,
            "clusterserviceversions",
            Some(ns),
        )?;
    }
    if !spec.crds.is_empty() {
        let api: Api<k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition> =
            Api::all(client.clone());
        for (name, _, _) in &spec.crds {
            progress(format!("Deleting CRD {name}…"));
            deleted(
                api.delete(name, &DeleteParams::default()).await,
                "customresourcedefinitions",
                None,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    fn group(ns: &str, name: &str, targets: &[&str]) -> OperatorGroup {
        OperatorGroup::parse(&json!({"metadata": {"name": name, "namespace": ns},
            "spec": if targets.is_empty() { json!({}) } else { json!({"targetNamespaces": targets}) }}))
        .unwrap()
    }

    fn choice(target: Target) -> InstallChoice {
        InstallChoice {
            package: "grafana-operator".into(),
            catalog: "operatorhubio-catalog".into(),
            catalog_namespace: "olm".into(),
            channel: "v5".into(),
            starting_csv: None,
            approval: Approval::Automatic,
            target,
        }
    }

    fn modes(list: &[InstallMode]) -> BTreeSet<InstallMode> {
        list.iter().copied().collect()
    }

    #[test]
    fn all_namespaces_installs_use_the_global_group() {
        let groups = [
            group("olm", "olm-operators", &["olm"]),
            group("operators", "global-operators", &[]),
        ];
        let steps = plan_install(
            &choice(Target::AllNamespaces),
            &modes(&[InstallMode::AllNamespaces, InstallMode::OwnNamespace]),
            &groups,
            &["olm".into(), "operators".into()],
            &[],
        )
        .unwrap();
        assert_eq!(steps.namespace, "operators");
        assert_eq!(steps.group, GroupStep::Existing("global-operators".into()));
        assert!(!steps.create_namespace);
        // OpenShift's comes first.
        let groups = [
            group("operators", "global-operators", &[]),
            group("openshift-operators", "global-operators", &[]),
        ];
        let steps = plan_install(
            &choice(Target::AllNamespaces),
            &modes(&[InstallMode::AllNamespaces]),
            &groups,
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(steps.namespace, "openshift-operators");
        // Not supported by the operator.
        assert!(
            plan_install(
                &choice(Target::AllNamespaces),
                &modes(&[InstallMode::OwnNamespace]),
                &groups,
                &[],
                &[]
            )
            .is_err()
        );
        // No global group: vanilla OLM's is recreated.
        let steps = plan_install(
            &choice(Target::AllNamespaces),
            &modes(&[InstallMode::AllNamespaces]),
            &[],
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(
            steps.group,
            GroupStep::Create {
                name: "global-operators".into(),
                global: true
            }
        );
        assert!(steps.create_namespace);
    }

    #[test]
    fn namespace_installs_create_or_check_the_group() {
        let own = modes(&[InstallMode::OwnNamespace]);
        // A new namespace: namespace and group are created.
        let steps = plan_install(
            &choice(Target::Namespace("grafana".into())),
            &own,
            &[],
            &[],
            &[],
        )
        .unwrap();
        assert!(steps.create_namespace);
        assert_eq!(
            steps.group,
            GroupStep::Create {
                name: "grafana".into(),
                global: false
            }
        );
        let object = subscription_object(&choice(Target::Namespace("grafana".into())), &steps);
        assert_eq!(object["spec"]["installPlanApproval"], "Automatic");
        assert_eq!(object["metadata"]["namespace"], "grafana");
        assert!(object["spec"].get("startingCSV").is_none());
        // A group targeting its own namespace is used.
        let groups = [group("grafana", "grafana-og", &["grafana"])];
        let steps = plan_install(
            &choice(Target::Namespace("grafana".into())),
            &own,
            &groups,
            &["grafana".into()],
            &[],
        )
        .unwrap();
        assert_eq!(steps.group, GroupStep::Existing("grafana-og".into()));
        // A group targeting other namespaces is refused, so are two groups.
        let groups = [group("grafana", "og", &["apps"])];
        assert!(
            plan_install(
                &choice(Target::Namespace("grafana".into())),
                &own,
                &groups,
                &[],
                &[]
            )
            .is_err()
        );
        let groups = [
            group("grafana", "a", &["grafana"]),
            group("grafana", "b", &["grafana"]),
        ];
        let err = plan_install(
            &choice(Target::Namespace("grafana".into())),
            &own,
            &groups,
            &[],
            &[],
        )
        .unwrap_err();
        assert!(err.contains("2 OperatorGroups"), "{err}");
        // All-namespaces-only operators can't go into a namespace of their own.
        let err = plan_install(
            &choice(Target::Namespace("grafana".into())),
            &modes(&[InstallMode::AllNamespaces]),
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert!(err.contains("only installs for all namespaces"));
        assert!(
            plan_install(
                &choice(Target::Namespace("Bad_Name".into())),
                &own,
                &[],
                &[],
                &[]
            )
            .is_err()
        );
        // Already subscribed.
        let sub = Arc::new(
            Subscription::parse(&json!({"metadata": {"name": "grafana-operator", "namespace": "grafana"}, "spec": {"name": "grafana-operator"}}))
                .unwrap(),
        );
        assert!(
            plan_install(
                &choice(Target::Namespace("grafana".into())),
                &own,
                &[],
                &[],
                &[sub]
            )
            .is_err()
        );
    }
}
