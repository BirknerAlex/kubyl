//! Applications: objects grouped by namespace and `app.kubernetes.io/instance`.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use jiff::Timestamp;
use kubyl_flux_core::ownership;
use kubyl_resources_core::format::{self, str_at};
use kubyl_resources_core::store::StoreStatus;
use serde_json::Value;

use crate::health::{self, Health};
use crate::kinds::Kind;
use crate::{COMPONENT, INSTANCE, MANAGED_BY, NAME, PART_OF, VERSION};

/// Who manages an application: the deployment tool that tracks its objects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Manager {
    ArgoCd {
        /// The Application's namespace, when it isn't Argo CD's own.
        namespace: Option<String>,
        app: String,
    },
    Flux {
        /// `Kustomization` or `HelmRelease`.
        kind: &'static str,
        namespace: String,
        name: String,
    },
    Helm {
        namespace: String,
        release: String,
    },
    /// Only the `app.kubernetes.io/managed-by` label (`kustomize`, an operator, Helm without
    /// release annotations).
    Label(String),
    None,
}

impl Manager {
    /// `Helm`, `Argo CD`, `Flux`, the label's own value or empty.
    pub fn tool(&self) -> &str {
        match self {
            Manager::ArgoCd { .. } => "Argo CD",
            Manager::Flux { .. } => "Flux",
            Manager::Helm { .. } => "Helm",
            Manager::Label(label) => label,
            Manager::None => "",
        }
    }

    /// What the table shows: `Helm · web`, `Argo CD · guestbook`, `Flux · apps/podinfo`.
    pub fn label(&self) -> String {
        match self {
            Manager::ArgoCd { app, .. } => format!("Argo CD · {app}"),
            Manager::Flux {
                kind,
                namespace,
                name,
            } => format!("Flux · {kind} {namespace}/{name}"),
            Manager::Helm { release, .. } => format!("Helm · {release}"),
            Manager::Label(label) => label.clone(),
            Manager::None => String::new(),
        }
    }
}

/// One object of an application.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub kind: Kind,
    pub namespace: String,
    pub name: String,
    pub created: Option<Timestamp>,
    /// Workloads only.
    pub health: Option<Health>,
    /// `3/3 ready` for a workload.
    pub detail: String,
    pub version: Option<String>,
    pub component: Option<String>,
}

/// An application: every object of a namespace that names the same instance.
#[derive(Clone, Debug, PartialEq)]
pub struct App {
    pub namespace: String,
    pub instance: String,
    /// The `app.kubernetes.io/name` values of its members (the chart or program name).
    pub names: BTreeSet<String>,
    pub part_of: Option<String>,
    pub manager: Manager,
    /// `app.kubernetes.io/version` of its workloads, else the tag of the first workload's image.
    pub version: String,
    pub created: Option<Timestamp>,
    pub health: Health,
    pub members: Vec<Member>,
}

impl App {
    /// `namespace/instance`.
    pub fn key(&self) -> String {
        format!("{}/{}", self.namespace, self.instance)
    }

    /// The label selector that finds the application's objects.
    pub fn selector(&self) -> String {
        format!("{INSTANCE}={}", self.instance)
    }

    /// The workloads whose logs can be streamed.
    pub fn log_sources(&self) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(|m| m.kind.has_logs())
    }

    pub fn workloads(&self) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(|m| m.kind.is_workload())
    }
}

/// What a watch of `kind` that isn't working means for the list: the verb and resource that
/// are missing, instead of an empty table.
pub fn problem(kind: Kind, status: &StoreStatus) -> Option<String> {
    let resource = if kind.group().is_empty() {
        kind.plural().to_string()
    } else {
        format!("{}.{}", kind.plural(), kind.group())
    };
    match status {
        StoreStatus::Forbidden => Some(format!(
            "Not allowed to list and watch {resource} in all namespaces (RBAC verbs list, watch)."
        )),
        StoreStatus::Error(err) => Some(format!("Watching {resource} failed: {err}")),
        _ => None,
    }
}

/// Groups `objects` (a kind and the object) into applications, worst health first. Objects
/// without `app.kubernetes.io/instance` belong to none; Jobs a CronJob created are left to it.
///
/// `argo_apps`: names of the Argo CD Applications of the cluster. Argo CD's label tracking
/// uses the same `instance` label as everything else, so it only counts when an Application of
/// that name exists.
pub fn build(objects: &[(Kind, &Value)], argo_apps: &HashSet<String>) -> Vec<App> {
    struct Draft<'a> {
        members: Vec<(Member, &'a Value)>,
    }
    let mut drafts: BTreeMap<(String, String), Draft> = BTreeMap::new();
    for (kind, object) in objects {
        let labels = object.pointer("/metadata/labels");
        let label = |key: &str| {
            labels
                .and_then(|l| l.get(key))
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
        };
        let Some(instance) = label(INSTANCE) else {
            continue;
        };
        if *kind == Kind::Job && owned_by_cron_job(object) {
            continue;
        }
        let health = health::of(*kind, object);
        let member = Member {
            kind: *kind,
            namespace: format::namespace(object).unwrap_or_default().to_string(),
            name: format::name(object).to_string(),
            created: format::creation(object),
            health: health.as_ref().map(|(h, _)| *h),
            detail: health.map(|(_, d)| d).unwrap_or_default(),
            version: label(VERSION).map(str::to_string),
            component: label(COMPONENT).map(str::to_string),
        };
        drafts
            .entry((member.namespace.clone(), instance.to_string()))
            .or_insert_with(|| Draft {
                members: Vec::new(),
            })
            .members
            .push((member, object));
    }

    let mut apps: Vec<App> = drafts
        .into_iter()
        .map(|((namespace, instance), mut draft)| {
            draft
                .members
                .sort_by(|(a, _), (b, _)| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
            let label_of = |object: &Value, key: &str| {
                object
                    .pointer("/metadata/labels")
                    .and_then(|l| l.get(key))
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
            };
            let names = draft
                .members
                .iter()
                .filter_map(|(_, o)| label_of(o, NAME))
                .collect();
            let part_of = draft.members.iter().find_map(|(_, o)| label_of(o, PART_OF));
            let manager = manager_of(draft.members.iter().map(|(m, o)| (m.kind, *o)), argo_apps);
            let version = version_of(&draft.members);
            let created = draft.members.iter().filter_map(|(m, _)| m.created).min();
            let health = app_health(draft.members.iter().map(|(m, _)| m));
            App {
                namespace,
                instance,
                names,
                part_of,
                manager,
                version,
                created,
                health,
                members: draft.members.into_iter().map(|(m, _)| m).collect(),
            }
        })
        .collect();
    apps.sort_by(|a, b| {
        (a.health, &a.namespace, &a.instance).cmp(&(b.health, &b.namespace, &b.instance))
    });
    apps
}

fn owned_by_cron_job(object: &Value) -> bool {
    object
        .pointer("/metadata/ownerReferences")
        .and_then(Value::as_array)
        .is_some_and(|owners| owners.iter().any(|o| str_at(o, "/kind") == "CronJob"))
}

/// The worst health among the workloads; suspended only when every workload is.
fn app_health<'a>(members: impl Iterator<Item = &'a Member>) -> Health {
    let healths: Vec<Health> = members.filter_map(|m| m.health).collect();
    if healths.is_empty() {
        return Health::Unknown;
    }
    let live = healths.iter().copied().filter(|h| *h != Health::Suspended);
    live.min().unwrap_or(Health::Suspended)
}

/// The tool that manages the objects: Argo CD, then Flux, then Helm, then the raw label.
fn manager_of<'a>(
    objects: impl Iterator<Item = (Kind, &'a Value)> + Clone,
    argo: &HashSet<String>,
) -> Manager {
    // The objects come from lists and metadata-only watches, which don't say their own kind.
    for (kind, object) in objects.clone() {
        if let Some(managed) =
            kubyl_argocd_core::tracking::managed_by_kind(object, Some(kind.kind()))
            && (!managed.via_label || argo.contains(&managed.app_name))
        {
            return Manager::ArgoCd {
                namespace: managed.app_namespace,
                app: managed.app_name,
            };
        }
    }
    for (_, object) in objects.clone() {
        if let Some(owner) = ownership::managed_by(object) {
            return Manager::Flux {
                kind: owner.kind.map(|k| k.kind()).unwrap_or("Flux object"),
                namespace: owner.namespace.to_string(),
                name: owner.name.to_string(),
            };
        }
    }
    for (_, object) in objects.clone() {
        if let Some((namespace, release)) = kubyl_helm_core::release::managed_by(object) {
            return Manager::Helm { namespace, release };
        }
    }
    for (_, object) in objects {
        if let Some(label) = object
            .pointer("/metadata/labels")
            .and_then(|l| l.get(MANAGED_BY))
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            return Manager::Label(label.to_string());
        }
    }
    Manager::None
}

/// The version of the workloads (their `app.kubernetes.io/version` labels), else the other
/// members', else the tag of the first workload's image.
fn version_of(members: &[(Member, &Value)]) -> String {
    let distinct = |workloads_only: bool| -> Vec<String> {
        let mut seen = Vec::new();
        for (member, _) in members {
            if workloads_only && !member.kind.is_workload() {
                continue;
            }
            if let Some(version) = &member.version
                && !seen.contains(version)
            {
                seen.push(version.clone());
            }
        }
        seen
    };
    for versions in [distinct(true), distinct(false)] {
        if !versions.is_empty() {
            return join_versions(versions);
        }
    }
    members
        .iter()
        .filter(|(m, _)| m.kind.is_workload())
        .find_map(|(m, o)| image_tag(m.kind, o))
        .unwrap_or_default()
}

/// `1.2.0`, `1.2.0, 1.3.0` or `1.2.0, 1.3.0 +2`.
fn join_versions(mut versions: Vec<String>) -> String {
    const SHOWN: usize = 2;
    let more = versions.len().saturating_sub(SHOWN);
    versions.truncate(SHOWN);
    let mut text = versions.join(", ");
    if more > 0 {
        text.push_str(&format!(" +{more}"));
    }
    text
}

/// The tag (or short digest) of a workload's first container image.
fn image_tag(kind: Kind, object: &Value) -> Option<String> {
    let template = match kind {
        Kind::CronJob => "/spec/jobTemplate/spec/template/spec/containers/0/image",
        _ => "/spec/template/spec/containers/0/image",
    };
    let image = object.pointer(template)?.as_str()?;
    if let Some((_, digest)) = image.split_once('@') {
        let hex = digest.split(':').next_back().unwrap_or(digest);
        return Some(format!("@{}", hex.chars().take(7).collect::<String>()));
    }
    let name = image.rsplit('/').next()?;
    name.split_once(':').map(|(_, tag)| tag.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn labels(instance: &str, extra: Value) -> Value {
        let mut labels = json!({ INSTANCE: instance });
        for (k, v) in extra.as_object().into_iter().flatten() {
            labels[k] = v.clone();
        }
        labels
    }

    fn deployment(name: &str, ns: &str, instance: &str, extra: Value, ready: i64) -> Value {
        json!({"metadata": {"name": name, "namespace": ns, "creationTimestamp": "2026-10-01T10:00:00Z",
                "labels": labels(instance, extra)},
            "spec": {"replicas": 2, "template": {"spec": {"containers": [{"image": "registry.example.com/shop/web:1.4.2"}]}}},
            "status": {"replicas": 2, "readyReplicas": ready, "updatedReplicas": 2}})
    }

    fn service(name: &str, ns: &str, instance: &str) -> Value {
        json!({"metadata": {"name": name, "namespace": ns, "creationTimestamp": "2026-09-30T10:00:00Z",
            "labels": labels(instance, json!({}))}})
    }

    fn helm(extra: Value) -> Value {
        let mut base = json!({MANAGED_BY: "Helm"});
        for (k, v) in extra.as_object().into_iter().flatten() {
            base[k] = v.clone();
        }
        base
    }

    #[test]
    fn groups_by_namespace_and_instance() {
        let web = deployment("shop-web", "shop", "shop", json!({NAME: "web"}), 2);
        let db = deployment(
            "shop-db",
            "shop",
            "shop",
            json!({NAME: "db", COMPONENT: "database"}),
            2,
        );
        let svc = service("shop-web", "shop", "shop");
        let other = deployment("shop-web", "staging", "shop", json!({}), 1);
        let unlabelled =
            json!({"metadata": {"name": "tool", "namespace": "shop", "labels": {NAME: "tool"}}});
        let objects = [
            (Kind::Deployment, &web),
            (Kind::Deployment, &db),
            (Kind::Service, &svc),
            (Kind::Deployment, &other),
            (Kind::Deployment, &unlabelled),
        ];
        let apps = build(&objects, &HashSet::new());
        assert_eq!(apps.len(), 2, "{apps:#?}");
        // Staging runs 1 of 2 replicas: progressing sorts before the healthy one.
        assert_eq!(apps[0].key(), "staging/shop");
        assert_eq!(apps[0].health, Health::Progressing);
        let shop = &apps[1];
        assert_eq!(shop.key(), "shop/shop");
        assert_eq!(shop.health, Health::Healthy);
        assert_eq!(shop.names.iter().collect::<Vec<_>>(), ["db", "web"]);
        // Members by kind, then name; the Service is a member without health.
        let names: Vec<_> = shop
            .members
            .iter()
            .map(|m| (m.kind, m.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                (Kind::Deployment, "shop-db"),
                (Kind::Deployment, "shop-web"),
                (Kind::Service, "shop-web")
            ]
        );
        assert_eq!(shop.members[2].health, None);
        // The oldest member sets the age.
        assert_eq!(shop.created, "2026-09-30T10:00:00Z".parse().ok());
        assert_eq!(shop.selector(), "app.kubernetes.io/instance=shop");
        assert_eq!(shop.log_sources().count(), 2);
    }

    #[test]
    fn health_is_the_worst_workload_and_suspended_only_when_all_are() {
        let ok = deployment("a", "n", "x", json!({}), 2);
        let bad = json!({"metadata": {"name": "b", "namespace": "n", "labels": {INSTANCE: "x"}},
            "spec": {"replicas": 2}, "status": {"replicas": 2, "conditions": [{"type": "Available", "status": "False"}]}});
        let off = json!({"metadata": {"name": "c", "namespace": "n", "labels": {INSTANCE: "x"}}, "spec": {"replicas": 0}});
        let apps = build(
            &[
                (Kind::Deployment, &ok),
                (Kind::Deployment, &bad),
                (Kind::Deployment, &off),
            ],
            &HashSet::new(),
        );
        assert_eq!(apps[0].health, Health::Degraded);
        let apps = build(
            &[(Kind::Deployment, &ok), (Kind::Deployment, &off)],
            &HashSet::new(),
        );
        assert_eq!(apps[0].health, Health::Healthy);
        let apps = build(&[(Kind::Deployment, &off)], &HashSet::new());
        assert_eq!(apps[0].health, Health::Suspended);
        // Only Services and ConfigMaps: nothing reports health.
        let svc = service("s", "n", "x");
        let apps = build(&[(Kind::Service, &svc)], &HashSet::new());
        assert_eq!(apps[0].health, Health::Unknown);
    }

    #[test]
    fn jobs_of_a_cron_job_belong_to_it() {
        let cron = json!({"metadata": {"name": "report", "namespace": "n", "labels": {INSTANCE: "report"}}, "spec": {"schedule": "@daily"}});
        let job = json!({"metadata": {"name": "report-2891", "namespace": "n", "labels": {INSTANCE: "report"},
            "ownerReferences": [{"kind": "CronJob", "name": "report"}]}, "status": {"conditions": [{"type": "Failed", "status": "True"}]}});
        let apps = build(
            &[(Kind::CronJob, &cron), (Kind::Job, &job)],
            &HashSet::new(),
        );
        assert_eq!(apps[0].members.len(), 1);
        assert_eq!(apps[0].health, Health::Healthy);
    }

    #[test]
    fn helm_argo_and_flux_manage_by_their_own_markers() {
        let release = json!({"metadata": {"name": "web", "namespace": "shop",
            "labels": helm(json!({INSTANCE: "web"})),
            "annotations": {"meta.helm.sh/release-name": "web", "meta.helm.sh/release-namespace": "shop"}}});
        let app = &build(&[(Kind::Deployment, &release)], &HashSet::new())[0];
        assert_eq!(
            app.manager,
            Manager::Helm {
                namespace: "shop".into(),
                release: "web".into()
            }
        );
        assert_eq!(app.manager.label(), "Helm · web");
        // Helm without annotations (helm template | kubectl apply): the raw label.
        let templated = json!({"metadata": {"name": "web", "namespace": "shop", "labels": helm(json!({INSTANCE: "web"}))}});
        let app = &build(&[(Kind::Deployment, &templated)], &HashSet::new())[0];
        assert_eq!(app.manager, Manager::Label("Helm".into()));

        // Argo CD's annotation tracking names the app, also for objects that don't say their kind
        // (list items, metadata-only watches).
        let untyped = json!({"metadata": {"name": "ui", "namespace": "guestbook",
            "labels": {INSTANCE: "guestbook"},
            "annotations": {"argocd.argoproj.io/tracking-id": "guestbook:apps/Deployment:guestbook/ui"}}});
        let app = &build(&[(Kind::Deployment, &untyped)], &HashSet::new())[0];
        assert_eq!(
            app.manager,
            Manager::ArgoCd {
                namespace: None,
                app: "guestbook".into()
            }
        );
        let argo = json!({"kind": "Deployment", "metadata": {"name": "ui", "namespace": "guestbook",
            "labels": {INSTANCE: "guestbook"},
            "annotations": {"argocd.argoproj.io/tracking-id": "guestbook:apps/Deployment:guestbook/ui"}}});
        let app = &build(&[(Kind::Deployment, &argo)], &HashSet::new())[0];
        assert_eq!(
            app.manager,
            Manager::ArgoCd {
                namespace: None,
                app: "guestbook".into()
            }
        );
        assert_eq!(app.manager.label(), "Argo CD · guestbook");

        // Label tracking is the same label Helm uses: only an Application of that name counts.
        let by_label = json!({"kind": "Deployment", "metadata": {"name": "ui", "namespace": "guestbook",
            "labels": {INSTANCE: "guestbook"}}});
        let app = &build(&[(Kind::Deployment, &by_label)], &HashSet::new())[0];
        assert_eq!(app.manager, Manager::None);
        let known: HashSet<String> = ["guestbook".to_string()].into();
        let app = &build(&[(Kind::Deployment, &by_label)], &known)[0];
        assert_eq!(app.manager.tool(), "Argo CD");

        // Flux labels what its Kustomizations and HelmReleases apply; Argo CD, Flux, then Helm.
        let flux = json!({"metadata": {"name": "podinfo", "namespace": "flux-demo",
            "labels": {INSTANCE: "podinfo", "kustomize.toolkit.fluxcd.io/name": "podinfo", "kustomize.toolkit.fluxcd.io/namespace": "flux-system",
                MANAGED_BY: "Helm"},
            "annotations": {"meta.helm.sh/release-name": "podinfo"}}});
        let app = &build(&[(Kind::Deployment, &flux)], &HashSet::new())[0];
        assert_eq!(app.manager.tool(), "Flux");
        assert_eq!(
            app.manager.label(),
            "Flux · Kustomization flux-system/podinfo"
        );
    }

    #[test]
    fn version_comes_from_labels_then_the_image() {
        let labelled = deployment("a", "n", "x", json!({VERSION: "2.0.1"}), 2);
        let svc = {
            let mut s = service("a", "n", "x");
            s["metadata"]["labels"][VERSION] = json!("0.9");
            s
        };
        let app = &build(
            &[(Kind::Deployment, &labelled), (Kind::Service, &svc)],
            &HashSet::new(),
        )[0];
        // Workloads decide; the Service's label is only a fallback.
        assert_eq!(app.version, "2.0.1");
        let app = &build(&[(Kind::Service, &svc)], &HashSet::new())[0];
        assert_eq!(app.version, "0.9");
        // No label: the image's tag.
        let plain = deployment("a", "n", "x", json!({}), 2);
        let app = &build(&[(Kind::Deployment, &plain)], &HashSet::new())[0];
        assert_eq!(app.version, "1.4.2");
        // Components at different versions.
        let b = deployment("b", "n", "x", json!({VERSION: "3.0"}), 2);
        let app = &build(
            &[(Kind::Deployment, &labelled), (Kind::Deployment, &b)],
            &HashSet::new(),
        )[0];
        assert_eq!(app.version, "2.0.1, 3.0");
        let many: Vec<Value> = (0..4)
            .map(|i| {
                deployment(
                    &format!("d{i}"),
                    "n",
                    "x",
                    json!({VERSION: format!("{i}.0")}),
                    2,
                )
            })
            .collect();
        let objects: Vec<_> = many.iter().map(|d| (Kind::Deployment, d)).collect();
        assert_eq!(build(&objects, &HashSet::new())[0].version, "0.0, 1.0 +2");
    }

    #[test]
    fn a_refused_watch_names_the_verb_and_resource() {
        let text = problem(Kind::Deployment, &StoreStatus::Forbidden).unwrap();
        assert!(text.contains("list and watch deployments.apps"), "{text}");
        let text = problem(Kind::ConfigMap, &StoreStatus::Forbidden).unwrap();
        assert!(text.contains("configmaps in all namespaces"), "{text}");
        let text = problem(Kind::Job, &StoreStatus::Error("timeout".into())).unwrap();
        assert!(
            text.contains("jobs.batch") && text.contains("timeout"),
            "{text}"
        );
        assert_eq!(problem(Kind::Job, &StoreStatus::Ready), None);
    }

    #[test]
    fn image_tags_and_digests() {
        let image = |image: &str| json!({"spec": {"template": {"spec": {"containers": [{"image": image}]}}}});
        assert_eq!(
            image_tag(Kind::Deployment, &image("nginx:1.27")).as_deref(),
            Some("1.27")
        );
        assert_eq!(
            image_tag(Kind::Deployment, &image("localhost:5000/team/web")),
            None
        );
        assert_eq!(
            image_tag(Kind::Deployment, &image("localhost:5000/team/web:v2")).as_deref(),
            Some("v2")
        );
        assert_eq!(
            image_tag(Kind::Deployment, &image("nginx@sha256:0123456789abcdef")).as_deref(),
            Some("@0123456")
        );
        let cron = json!({"spec": {"jobTemplate": {"spec": {"template": {"spec": {"containers": [{"image": "b:3"}]}}}}}});
        assert_eq!(image_tag(Kind::CronJob, &cron).as_deref(), Some("3"));
    }
}
