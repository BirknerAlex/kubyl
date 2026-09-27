use std::path::PathBuf;

use kube::config::Kubeconfig;
use secrecy::ExposeSecret as _;

use super::*;
use crate::providers::cloud::exec_info;
use crate::providers::cloud::mock::{self, fixture};
use crate::settings::ClusterUpdateSettings;

fn json<T: serde::de::DeserializeOwned>(name: &str) -> T {
    serde_json::from_str(&fixture("gke", name)).unwrap()
}

fn gke_target() -> GkeTarget {
    GkeTarget {
        project: "kubyl-example".into(),
        location: "europe-west1".into(),
        cluster: "prod".into(),
        adc: false,
        env: CliEnv::default(),
    }
}

fn snapshot(operations: Vec<Operation>) -> Snapshot {
    Snapshot {
        cluster: json("cluster.json"),
        server_config: Some(json("server-config.json")),
        operations,
        notes: Vec::new(),
    }
}

fn operations() -> Vec<Operation> {
    let value: Value = json("operations.json");
    serde_json::from_value(value["operations"].clone()).unwrap()
}

#[test]
fn reads_gke_context_names() {
    assert_eq!(
        parse_name("gke_kubyl-example_europe-west1_prod"),
        Some((
            "kubyl-example".to_string(),
            "europe-west1".to_string(),
            "prod".to_string()
        ))
    );
    // Domain-scoped projects keep their colon.
    assert_eq!(
        parse_name("gke_example.com:ops_us-central1-a_batch")
            .unwrap()
            .0,
        "example.com:ops"
    );
    assert_eq!(parse_name("gke_a_b"), None);
    assert_eq!(parse_name("gke_a_b_c_d"), None);
    assert_eq!(
        parse_name("arn:aws:eks:eu-west-1:111122223333:cluster/x"),
        None
    );
}

#[test]
fn resolves_from_names_settings_and_the_plugin() {
    // `gcloud container clusters get-credentials` output with the auth plugin.
    let config = Kubeconfig::from_yaml(
        r#"apiVersion: v1
kind: Config
clusters:
- name: gke_kubyl-example_europe-west1_prod
  cluster: {server: "https://34.76.0.10"}
contexts:
- name: gke_kubyl-example_europe-west1_prod
  context: {cluster: gke_kubyl-example_europe-west1_prod, user: gke_kubyl-example_europe-west1_prod}
users:
- name: gke_kubyl-example_europe-west1_prod
  user:
    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      command: gke-gcloud-auth-plugin
      installHint: Install gke-gcloud-auth-plugin for use with kubectl by following https://cloud.google.com/kubernetes-engine/docs/how-to/cluster-access-for-kubectl#install_plugin
      provideClusterInfo: true
      args: [--use_application_default_credentials]
      env:
      - {name: CLOUDSDK_CONFIG, value: /home/me/.config/gcloud-work}
      - {name: HTTPS_PROXY, value: "http://proxy.example.com:3128"}
"#,
    )
    .unwrap();
    let context = "gke_kubyl-example_europe-west1_prod";
    let exec = exec_info(&config, context, None).unwrap();
    let target = resolve(None, Some(&exec), context, context).unwrap();
    assert_eq!(
        (
            target.project.as_str(),
            target.location.as_str(),
            target.cluster.as_str()
        ),
        ("kubyl-example", "europe-west1", "prod")
    );
    assert!(target.adc);
    assert_eq!(target.env.names().collect::<Vec<_>>(), ["CLOUDSDK_CONFIG"]);
    assert_eq!(target.login(), "gcloud auth application-default login");
    assert_eq!(
        target.cluster_path(),
        "/v1/projects/kubyl-example/locations/europe-west1/clusters/prod"
    );
    assert_eq!(
        target.label(),
        "Google GKE (GKE API · project kubyl-example · europe-west1)"
    );
    assert!(!target.zonal());
    assert_eq!(
        target.gcloud_flags(),
        "--region europe-west1 --project kubyl-example"
    );

    // Settings win; a renamed context needs them.
    let settings = GkeSettings {
        project: Some("other".into()),
        location: Some("us-central1-a".into()),
        cluster: Some("batch".into()),
    };
    let target = resolve(Some(&settings), None, "my-context", "my-cluster").unwrap();
    assert_eq!(target.project, "other");
    assert!(target.zonal());
    assert!(!target.adc);
    let err = resolve(None, None, "my-context", "my-cluster").unwrap_err();
    assert!(
        matches!(&err, ProviderError::Other(m) if m.contains("\"my-context\"") && m.contains("\"gke\"")),
        "{err:?}"
    );
}

#[test]
fn reads_tokens_and_gcloud_errors() {
    let token = parse_token(b"ya29.a0AfB_example-token\n").unwrap();
    assert_eq!(token.0.expose_secret(), "ya29.a0AfB_example-token");
    assert!(!format!("{token:?}").contains("ya29"));
    assert!(parse_token(b"\n\n").is_err());
    assert_eq!(token_args(false), ["auth", "print-access-token"]);
    assert_eq!(
        token_args(true),
        ["auth", "application-default", "print-access-token"]
    );

    let failed = |stderr: &str| CliError::Failed {
        code: Some(1),
        stderr: stderr.into(),
    };
    assert!(matches!(
        classify(
            &failed(
                "ERROR: (gcloud.auth.print-access-token) You do not currently have an active \
                 account selected.\nPlease run:\n\n  $ gcloud auth login\n"
            ),
            false
        ),
        ProviderError::Credentials { command: Some(c), .. } if c == "gcloud auth login"
    ));
    assert!(matches!(
        classify(
            &failed(
                "ERROR: (gcloud.auth.print-access-token) There was a problem refreshing your \
                 current auth tokens: Reauthentication failed. cannot prompt during \
                 non-interactive execution."
            ),
            false
        ),
        ProviderError::Credentials { command: Some(c), .. } if c == "gcloud auth login"
    ));
    assert!(matches!(
        classify(
            &failed(
                "ERROR: (gcloud.auth.application-default.print-access-token) Your default \
                 credentials were not found."
            ),
            true
        ),
        ProviderError::Credentials { command: Some(c), .. } if c == "gcloud auth application-default login"
    ));
    assert!(matches!(
        classify(&CliError::NotFound, false),
        ProviderError::Unavailable(m) if m.contains("Google Cloud CLI") && m.contains("sdk/docs/install")
    ));
}

#[test]
fn offers_channel_versions() {
    let config: ServerConfig = json("server-config.json");
    let current = "1.30.5-gke.1014001";
    let regular = targets(current, Some("REGULAR"), &config, None);
    let kinds: Vec<_> = regular
        .iter()
        .map(|t| (t.version.as_str(), t.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("1.32.0-gke.1000000", TargetKind::Blocked),
            ("1.31.1-gke.1678000", TargetKind::Recommended),
            ("1.31.1-gke.1146000", TargetKind::Available),
            ("1.30.5-gke.1443001", TargetKind::Available),
        ]
    );
    assert_eq!(
        regular[0].blocked,
        ["GKE control planes update one minor at a time (1.30 → 1.31 first)"]
    );
    assert!(regular[1].minor && !regular[3].minor);
    assert_eq!(regular[1].channels, ["REGULAR"]);
    // STABLE has nothing newer.
    assert!(targets(current, Some("STABLE"), &config, None).is_empty());
    // No channel: the location's master versions; the default is recommended.
    let static_versions = targets(current, None, &config, None);
    assert_eq!(static_versions.len(), 3);
    assert_eq!(
        static_versions
            .iter()
            .find(|t| t.kind == TargetKind::Recommended)
            .unwrap()
            .version,
        "1.30.5-gke.1443001"
    );
    // Busy: everything blocked.
    assert!(
        targets(current, Some("REGULAR"), &config, Some("busy"))
            .iter()
            .all(|t| !t.startable())
    );
}

#[test]
fn status_of_recorded_responses() {
    let status = status(&gke_target(), &snapshot(Vec::new()));
    assert_eq!(
        status.provider,
        "Google GKE (GKE API · project kubyl-example · europe-west1)"
    );
    assert_eq!(status.current.version, "1.30.5-gke.1014001");
    assert_eq!(status.current.channel.as_deref(), Some("REGULAR"));
    assert_eq!(
        status.current.channels,
        ["RAPID", "REGULAR", "STABLE", "EXTENDED", "NONE"]
    );
    assert_eq!(status.suggested().unwrap().version, "1.31.1-gke.1678000");
    assert!(status.writes.control_plane && status.writes.pools && status.writes.channel);
    assert!(!status.writes.addons);
    assert!(status.progress.is_none());

    let ids: Vec<_> = status.pools.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        ["control-plane", "nodepool/default-pool", "nodepool/batch"]
    );
    let default_pool = &status.pools[1];
    assert_eq!(default_pool.nodes, None);
    assert_eq!(
        default_pool.surge.as_deref(),
        Some("surge 1 · maxUnavailable 0")
    );
    assert_eq!(
        default_pool.message.as_deref(),
        Some("e2-standard-4 · COS_CONTAINERD · autoscaling 1–3 per zone · auto-upgrade")
    );
    assert!(!default_pool.updatable);
    let batch = &status.pools[2];
    assert_eq!(batch.nodes, Some(6));
    assert_eq!(batch.surge.as_deref(), Some("blue-green"));
    assert!(batch.updatable);
}

#[test]
fn status_while_a_node_pool_updates() {
    let status = status(&gke_target(), &snapshot(operations()));
    let progress = status.progress.as_ref().unwrap();
    assert_eq!(progress.percent, Some(50.0));
    assert_eq!(progress.target, "1.30.5-gke.1014001");
    assert!(progress.message.contains("node pool batch"));
    assert_eq!(status.pools[2].state, PoolState::Updating);
    assert_eq!(
        status.pools[0].state,
        PoolState::Idle,
        "staging's operation isn't ours"
    );
    assert!(status.targets.iter().all(|t| !t.startable()));
    let err = plan(
        &gke_target(),
        &status,
        &Scope::Pool("nodepool/default-pool".into()),
        "1.30.5-gke.1014001",
        "prod",
    )
    .unwrap_err();
    assert!(err.contains("one operation"), "{err}");
}

#[test]
fn autopilot_clusters() {
    let mut snap = snapshot(Vec::new());
    snap.cluster.autopilot = Some(api::Autopilot { enabled: true });
    let status = status(&gke_target(), &snap);
    assert!(!status.writes.pools);
    assert!(!status.current.channels.contains(&NO_CHANNEL.to_string()));
    assert!(status.notes.iter().any(|n| n.title == "Autopilot"));
    assert!(
        plan(
            &gke_target(),
            &status,
            &Scope::Pool("nodepool/batch".into()),
            "1.30.5-gke.1014001",
            "prod"
        )
        .is_err()
    );
}

#[test]
fn plans_writes() {
    let target = gke_target();
    let status = status(&target, &snapshot(Vec::new()));

    let master = plan(
        &target,
        &status,
        &Scope::ControlPlane,
        "1.31.1-gke.1678000",
        "prod",
    )
    .unwrap();
    assert_eq!(
        master.title,
        "Update the control plane of prod to 1.31.1-gke.1678000"
    );
    assert_eq!(master.kind_label, "minor update");
    assert!(master.irreversible);
    assert_eq!(
        master.request,
        json!({
            "operation": "clusters.update",
            "method": "PUT",
            "path": "/v1/projects/kubyl-example/locations/europe-west1/clusters/prod",
            "body": {"update": {"desiredMasterVersion": "1.31.1-gke.1678000"}},
            "permission": "container.clusters.update",
            "resource": "cluster prod",
        })
    );
    assert!(master.changes[0].contains(
        "gcloud container clusters upgrade prod --master --cluster-version 1.31.1-gke.1678000 \
         --region europe-west1 --project kubyl-example"
    ));
    let patch = plan(
        &target,
        &status,
        &Scope::ControlPlane,
        "1.30.5-gke.1443001",
        "prod",
    )
    .unwrap();
    assert_eq!(patch.kind_label, "patch update");
    assert!(
        plan(
            &target,
            &status,
            &Scope::ControlPlane,
            "1.32.0-gke.1000000",
            "prod"
        )
        .unwrap_err()
        .contains("one minor at a time")
    );
    assert!(plan(&target, &status, &Scope::ControlPlane, "9.9.9", "prod").is_err());

    let batch = plan(
        &target,
        &status,
        &Scope::Pool("nodepool/batch".into()),
        "1.30.5-gke.1014001",
        "prod",
    )
    .unwrap();
    assert_eq!(
        batch.request["path"],
        "/v1/projects/kubyl-example/locations/europe-west1/clusters/prod/nodePools/batch"
    );
    assert_eq!(
        batch.request["body"],
        json!({"nodeVersion": "1.30.5-gke.1014001"})
    );
    assert_eq!(batch.request["keepImageType"], true);
    assert!(batch.notes[0].contains("blue-green"));
    assert!(
        plan(
            &target,
            &status,
            &Scope::Pool("nodepool/batch".into()),
            "1.31.1-gke.1678000",
            "prod",
        )
        .unwrap_err()
        .contains("control plane to 1.31.1-gke.1678000 first")
    );

    let stable = plan(
        &target,
        &status,
        &Scope::Channel("STABLE".into()),
        "",
        "prod",
    )
    .unwrap();
    assert!(!stable.irreversible);
    assert_eq!(
        (stable.from.as_str(), stable.to.as_str()),
        ("REGULAR", "STABLE")
    );
    assert_eq!(
        stable.request["body"],
        json!({"update": {"desiredReleaseChannel": {"channel": "STABLE"}}})
    );
    let none = plan(&target, &status, &Scope::Channel("NONE".into()), "", "prod").unwrap();
    assert_eq!(
        none.request["body"],
        json!({"update": {"desiredReleaseChannel": {"channel": "UNSPECIFIED"}}})
    );
    assert!(none.changes[0].contains("--release-channel None"));
    assert!(
        plan(
            &target,
            &status,
            &Scope::Channel("regular".into()),
            "",
            "prod"
        )
        .is_err()
    );
    assert!(
        plan(
            &target,
            &status,
            &Scope::Channel("nightly".into()),
            "",
            "prod"
        )
        .is_err()
    );
    assert!(plan(&target, &status, &Scope::AddOn("x".into()), "", "prod").is_err());
}

fn test_provider(base: Url) -> Gke {
    let ctx = CloudContext {
        display_name: "prod".into(),
        kubeconfig: PathBuf::from("/nonexistent/kubeconfig"),
        context: "gke_kubyl-example_europe-west1_prod".into(),
        user: None,
        cluster_entry: "gke_kubyl-example_europe-west1_prod".into(),
        server: "https://34.76.0.10".into(),
        settings: ClusterUpdateSettings::default(),
    };
    let endpoint = base.as_str().to_string();
    build(
        ctx,
        move |var| (var == cloud::GKE_ENDPOINT).then(|| endpoint.clone()),
        Some(BearerToken("ya29.fake-test-token".to_string().into())),
    )
}

fn routes(write_status: u16, write_body: String) -> Vec<mock::Route> {
    let location = "/v1/projects/kubyl-example/locations/europe-west1";
    let cluster = format!("{location}/clusters/prod");
    vec![
        ("GET", cluster.clone(), 200, fixture("gke", "cluster.json")),
        (
            "GET",
            format!("{location}/serverConfig"),
            200,
            fixture("gke", "server-config.json"),
        ),
        (
            "GET",
            format!("{location}/operations"),
            200,
            r#"{"operations": []}"#.into(),
        ),
        (
            "GET",
            format!("{cluster}/nodePools/batch"),
            200,
            fixture("gke", "node-pool-batch.json"),
        ),
        (
            "PUT",
            format!("{cluster}/nodePools/batch"),
            write_status,
            write_body.clone(),
        ),
        ("PUT", cluster, write_status, write_body),
    ]
}

#[tokio::test]
async fn reads_and_updates_a_node_pool_through_a_local_server() {
    let (base, seen) = mock::serve(routes(200, fixture("gke", "operation-update.json"))).await;
    let provider = test_provider(base);
    let status = provider.read().await.unwrap();
    assert_eq!(status.current.version, "1.30.5-gke.1014001");
    assert!(status.notes.is_empty(), "{:?}", status.notes);
    for request in seen.lock().unwrap().iter() {
        assert_eq!(
            request.header("authorization"),
            Some("Bearer ya29.fake-test-token")
        );
    }

    let plan = provider
        .plan(
            &status,
            &Scope::Pool("nodepool/batch".into()),
            "1.30.5-gke.1014001",
            "prod",
        )
        .unwrap();
    let message = provider.start(&plan).await.unwrap();
    assert!(
        message.contains("operation-1727200000000-3d4e5f6a"),
        "{message}"
    );
    let puts: Vec<_> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.method == "PUT")
        .cloned()
        .collect();
    assert_eq!(puts.len(), 1);
    let body: Value = serde_json::from_slice(&puts[0].body).unwrap();
    // The pool's image type, read right before.
    assert_eq!(
        body,
        json!({"nodeVersion": "1.30.5-gke.1014001", "imageType": "UBUNTU_CONTAINERD"})
    );
}

#[tokio::test]
async fn denied_and_expired_writes() {
    let (base, _) = mock::serve(routes(403, fixture("gke", "error-permission-denied.json"))).await;
    let provider = test_provider(base);
    let status = provider.read().await.unwrap();
    let plan = provider
        .plan(&status, &Scope::ControlPlane, "1.31.1-gke.1678000", "prod")
        .unwrap();
    assert_eq!(
        provider.start(&plan).await.unwrap_err(),
        ProviderError::Forbidden {
            verb: "container.clusters.update".into(),
            resource: "cluster prod".into()
        }
    );

    let (base, _) = mock::serve(routes(401, fixture("gke", "error-unauthenticated.json"))).await;
    let provider = test_provider(base);
    let status = provider.read().await.unwrap();
    let plan = provider
        .plan(&status, &Scope::Channel("STABLE".into()), "", "prod")
        .unwrap();
    assert!(matches!(
        provider.start(&plan).await.unwrap_err(),
        ProviderError::Credentials { command: Some(c), .. } if c == "gcloud auth login"
    ));
}
