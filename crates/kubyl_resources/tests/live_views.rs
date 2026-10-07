//! The resource views of phase 24 against the objects `script/views-dev.sh` creates on the kind
//! dev cluster (Kubernetes 1.34+). Ignored by default:
//!
//! ```sh
//! script/views-dev.sh
//! KUBYL_TEST_KUBECONFIG=<kubeconfig with kind-kubyl-dev> \
//!   cargo test -p kubyl_resources --test live_views -- --ignored --nocapture
//! ```
//!
//! Read-only: every kind is read in the version discovery prefers, like the app does.

use kube::api::{Api, DynamicObject, ListParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::discovery::{ApiResource, Discovery};
use kube::{Client, Config};
use kubyl_core::Tone;
use kubyl_resources::columns;
use kubyl_resources::{admission, describe, dra, gateway, vpa};
use serde_json::Value;

const NS: &str = "kubyl-views";

/// A cell as text (`Text("1")`), to compare without naming the cell type.
fn shows(cell: impl std::fmt::Debug) -> String {
    format!("{cell:?}")
}

async fn client() -> Client {
    let path = std::env::var("KUBYL_TEST_KUBECONFIG").expect("set KUBYL_TEST_KUBECONFIG");
    let kubeconfig = Kubeconfig::read_from(path).unwrap();
    let options = KubeConfigOptions {
        context: Some(std::env::var("KUBYL_TEST_CONTEXT").unwrap_or("kind-kubyl-dev".into())),
        ..Default::default()
    };
    let config = Config::from_custom_kubeconfig(kubeconfig, &options)
        .await
        .unwrap();
    Client::try_from(config).unwrap()
}

/// The objects of `group`/`kind` (preferred version) in `namespace` (or all), as JSON.
async fn list(
    client: &Client,
    discovery: &Discovery,
    group: &str,
    kind: &str,
    namespace: Option<&str>,
) -> Vec<Value> {
    let (resource, _) = discovery
        .groups()
        .find(|g| g.name() == group)
        .unwrap_or_else(|| panic!("{group} isn't served: run script/views-dev.sh"))
        .recommended_kind(kind)
        .unwrap_or_else(|| panic!("{kind} isn't served"));
    let api: Api<DynamicObject> = match namespace {
        Some(ns) => Api::namespaced_with(client.clone(), ns, &resource),
        None => Api::all_with(client.clone(), &resource),
    };
    let version = resource.version.clone();
    println!("{kind}: {group}/{version}");
    api.list(&ListParams::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|o| with_type(serde_json::to_value(o).unwrap(), &resource))
        .collect()
}

fn with_type(mut value: Value, resource: &ApiResource) -> Value {
    value["apiVersion"] = Value::String(resource.api_version.clone());
    value["kind"] = Value::String(resource.kind.clone());
    value
}

fn named<'a>(objects: &'a [Value], name: &str) -> &'a Value {
    objects
        .iter()
        .find(|o| o["metadata"]["name"] == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

fn kind(group: &str, name: &str) -> columns::Kind {
    columns::builtin()
        .into_iter()
        .find(|(g, k, _)| *g == group && *k == name)
        .map(|(_, _, kind)| kind)
        .unwrap()
}

#[tokio::test]
#[ignore]
async fn device_resources() {
    let client = client().await;
    let discovery = Discovery::new(client.clone()).run().await.unwrap();
    let claims = list(&client, &discovery, dra::GROUP, "ResourceClaim", Some(NS)).await;
    let all_claims = list(&client, &discovery, dra::GROUP, "ResourceClaim", None).await;
    let pods = list(&client, &discovery, "", "Pod", Some(NS)).await;
    let shared = named(&claims, "shared-gpu");
    let parsed = dra::Claim::parse(shared);
    assert_eq!(parsed.state(), "Allocated, reserved by 1", "{shared:#}");
    assert_eq!(parsed.allocated().len(), 2);
    assert!(parsed.allocation.as_ref().unwrap().node.is_some());
    assert_eq!(parsed.reserved_for[0].name, "gpu-shared");
    let class = parsed.device_classes()[0].clone();

    // The pod names the claim; the kubelet (or the driver) reports health.
    let pod = named(&pods, "gpu-shared");
    let pod_claims = dra::pod_claims(pod);
    assert_eq!(pod_claims[0].claim.as_deref(), Some("shared-gpu"));
    let health = dra::claim_health(shared, &dra::PodClaims::index(pods.iter()));
    println!("shared-gpu health: {health:?}");
    let claims_kind = kind(dra::GROUP, "ResourceClaim");
    let index = columns::RelatedIndex::build("ResourceClaim", "health", pods.iter());
    println!(
        "shared-gpu health cell: {}",
        shows(columns::related_cell(
            "ResourceClaim",
            "health",
            shared,
            &index
        ))
    );
    assert!(shows(claims_kind.cell(shared, "state")).contains("Allocated"));
    // The template's claim was generated with a name of its own.
    let job = dra::pod_claims(named(&pods, "gpu-job-0"));
    assert_eq!(job[0].template.as_deref(), Some("single-gpu"));
    assert!(
        job[0]
            .claim
            .as_deref()
            .unwrap()
            .starts_with("gpu-job-0-gpu")
    );

    // The class pins the driver; the slices hold the allocated devices.
    let classes = list(&client, &discovery, dra::GROUP, "DeviceClass", None).await;
    let class = dra::Class::parse(named(&classes, &class));
    let driver = class.drivers()[0].clone();
    let slices = list(&client, &discovery, dra::GROUP, "ResourceSlice", None).await;
    let mine: Vec<&Value> = slices
        .iter()
        .filter(|s| dra::Slice::parse(s).driver == driver)
        .collect();
    assert!(!mine.is_empty());
    let holders = dra::DeviceHolders::index(all_claims.iter());
    let held: usize = mine
        .iter()
        .map(|s| dra::allocations(&dra::Slice::parse(s), &holders).len())
        .sum();
    assert!(
        held >= 3,
        "the dev script's claims hold 3 devices, got {held}"
    );
    let slice = kind(dra::GROUP, "ResourceSlice");
    assert!(shows(slice.cell(mine[0], "devices")).starts_with("Text"));

    let templates = list(
        &client,
        &discovery,
        dra::GROUP,
        "ResourceClaimTemplate",
        Some(NS),
    )
    .await;
    let template = kind(dra::GROUP, "ResourceClaimTemplate");
    assert!(
        shows(template.cell(named(&templates, "single-gpu"), "requests"))
            .contains("gpu: exactly 1")
    );
    let text = describe::describe("ResourceClaim", shared, &[], jiff::Timestamp::now());
    assert!(text.contains("Reserved For:"), "{text}");
}

#[tokio::test]
#[ignore]
async fn admission_policies() {
    let client = client().await;
    let discovery = Discovery::new(client.clone()).run().await.unwrap();
    let policies = list(
        &client,
        &discovery,
        admission::GROUP,
        "ValidatingAdmissionPolicy",
        None,
    )
    .await;
    let bindings = list(
        &client,
        &discovery,
        admission::GROUP,
        "ValidatingAdmissionPolicyBinding",
        None,
    )
    .await;
    let policy = named(&policies, "kubyl-replica-limit");
    let parsed = admission::Policy::parse(policy);
    assert_eq!(parsed.validations.len(), 2);
    assert_eq!(parsed.variables.len(), 2);
    assert_eq!(parsed.param_label().as_deref(), Some("ConfigMap"));
    let mine = admission::bindings_of("kubyl-replica-limit", bindings.iter());
    assert_eq!(mine.len(), 1);
    let binding = admission::Binding::parse(mine[0]);
    assert_eq!(binding.validation_actions, ["Deny", "Audit"]);
    let namespaces = list(&client, &discovery, "", "Namespace", None).await;
    let selector = binding.resources.unwrap().namespace_selector.unwrap();
    let matching: Vec<&str> = namespaces
        .iter()
        .filter(|ns| admission::selector_matches(&selector, &ns["metadata"]["labels"]))
        .filter_map(|ns| ns["metadata"]["name"].as_str())
        .collect();
    assert_eq!(matching, [NS]);
    assert_eq!(
        shows(columns::related_cell(
            "ValidatingAdmissionPolicy",
            "bindings",
            policy,
            &columns::RelatedIndex::build("ValidatingAdmissionPolicy", "bindings", bindings.iter())
        )),
        r#"Text("1")"#
    );
    // Where served, the mutating policy too (v1 on 1.36+, beta before).
    if discovery.groups().any(|g| {
        g.name() == admission::GROUP && g.recommended_kind("MutatingAdmissionPolicy").is_some()
    }) {
        let policies = list(
            &client,
            &discovery,
            admission::GROUP,
            "MutatingAdmissionPolicy",
            None,
        )
        .await;
        let parsed = admission::Policy::parse(named(&policies, "kubyl-default-labels"));
        assert_eq!(parsed.mutations[0].patch_type, "ApplyConfiguration");
        assert_eq!(parsed.reinvocation_policy, "IfNeeded");
    }
}

#[tokio::test]
#[ignore]
async fn gateway_api_vpa_and_endpoint_slices() {
    let client = client().await;
    let discovery = Discovery::new(client.clone()).run().await.unwrap();
    let gateways = list(&client, &discovery, gateway::GROUP, "Gateway", Some(NS)).await;
    let gw = gateway::Gateway::parse(named(&gateways, "web-gateway"));
    assert_eq!(gw.state(), ("Programmed".into(), Tone::Good));
    assert_eq!(gw.attached_routes(), Some(3));
    let mut routes = list(&client, &discovery, gateway::GROUP, "HTTPRoute", None).await;
    routes.extend(list(&client, &discovery, gateway::GROUP, "GRPCRoute", None).await);
    let attached = gateway::routes_on_gateway(NS, "web-gateway", routes.iter());
    assert_eq!(attached.len(), 2);
    let grants = list(
        &client,
        &discovery,
        gateway::GROUP,
        "ReferenceGrant",
        Some(NS),
    )
    .await;
    let grants: Vec<&Value> = grants.iter().collect();
    let to_web = gateway::routes_to_service(NS, "web", routes.iter(), &grants);
    assert_eq!(to_web.len(), 1);
    assert_eq!(gateway::Route::parse(to_web[0]).state().0, "Accepted");
    let to_canary = gateway::routes_to_service(NS, "web-canary", routes.iter(), &grants);
    assert_eq!(to_canary.len(), 1);
    // The Gateway's listeners admit HTTP and GRPC routes, the https one from other namespaces.
    let (kinds, other_namespaces) = gateway::attachable_routes(named(&gateways, "web-gateway"));
    assert!(kinds.contains(&("HTTPRoute", "httproutes")));
    assert!(kinds.contains(&("GRPCRoute", "grpcroutes")));
    println!("web-gateway admits {kinds:?}, other namespaces: {other_namespaces}");
    println!(
        "grants allow routes from {:?}",
        gateway::granted_route_sources(grants.iter().copied(), "web")
    );

    let vpas = list(
        &client,
        &discovery,
        vpa::GROUP,
        "VerticalPodAutoscaler",
        Some(NS),
    )
    .await;
    let parsed = vpa::Vpa::parse(named(&vpas, "web"));
    assert_eq!(parsed.recommendation_label(), "nginx 80m / 96Mi");
    let deployments = list(&client, &discovery, "apps", "Deployment", Some(NS)).await;
    let requests = vpa::template_requests(named(&deployments, "web"));
    assert_eq!(requests[0].1.cpu, "50m");

    let slices = list(
        &client,
        &discovery,
        "discovery.k8s.io",
        "EndpointSlice",
        Some(NS),
    )
    .await;
    let web: Vec<&Value> = slices
        .iter()
        .filter(|s| s["metadata"]["labels"]["kubernetes.io/service-name"] == "web")
        .collect();
    assert_eq!(columns::endpoint_counts(web[0]), (2, 2));
}
