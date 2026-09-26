use std::path::PathBuf;

use kube::config::Kubeconfig;
use secrecy::ExposeSecret as _;

use super::*;
use crate::providers::cloud::exec_info;
use crate::providers::cloud::mock::{self, fixture};
use crate::settings::ClusterUpdateSettings;

const PROD: &str = "00000000-0000-0000-0000-000000000002";

fn json<T: serde::de::DeserializeOwned>(name: &str) -> T {
    serde_json::from_str(&fixture("aks", name)).unwrap()
}

fn cref() -> ClusterRef {
    ClusterRef {
        subscription: PROD.into(),
        resource_group: "rg-prod".into(),
        name: "prod-westeurope".into(),
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        cluster: json("managed-cluster.json"),
        profile: Some(json("upgrade-profile.json")),
        subscription_name: Some("Production".into()),
        notes: Vec::new(),
    }
}

#[test]
fn resource_ids_settings_and_hosts() {
    let id = "/subscriptions/00000000-0000-0000-0000-000000000002/resourcegroups/rg-prod/providers/Microsoft.ContainerService/managedClusters/prod-westeurope";
    assert_eq!(parse_resource_id(id), Some(cref()));
    assert_eq!(
        cref().path(),
        "/subscriptions/00000000-0000-0000-0000-000000000002/resourceGroups/rg-prod/providers/Microsoft.ContainerService/managedClusters/prod-westeurope"
    );
    assert_eq!(parse_resource_id("/subscriptions/x"), None);

    assert_eq!(settings_ref(None), None);
    let partial = AksSettings {
        subscription: Some(PROD.into()),
        resource_group: None,
        name: Some("prod".into()),
    };
    assert_eq!(settings_ref(Some(&partial)), None);
    let full = AksSettings {
        resource_group: Some("rg-prod".into()),
        name: Some("prod-westeurope".into()),
        ..partial
    };
    assert_eq!(settings_ref(Some(&full)), Some(cref()));

    let clusters: Vec<ManagedCluster> = vec![
        serde_json::from_value(
            json::<Value>("managed-clusters-development.json")["value"][0].clone(),
        )
        .unwrap(),
        json("managed-cluster.json"),
    ];
    let host = "PROD-K8S-ABC123DE.hcp.westeurope.azmk8s.io.";
    assert_eq!(
        find_by_host(&clusters, host).unwrap().name,
        "prod-westeurope"
    );
    // The portal FQDN counts too; unrelated hosts don't match.
    assert!(
        find_by_host(
            &clusters,
            "prod-k8s-abc123de.portal.hcp.westeurope.azmk8s.io"
        )
        .is_some()
    );
    assert!(find_by_host(&clusters, "other.hcp.westeurope.azmk8s.io").is_none());
}

#[test]
fn reads_kubelogin_shapes() {
    let exec = |args: &str, env: &str| {
        let yaml = format!(
            "apiVersion: v1\nkind: Config\ncontexts:\n- name: prod\n  context: {{cluster: prod, user: u}}\n\
             clusters:\n- name: prod\n  cluster: {{server: \"https://prod-k8s-abc123de.hcp.westeurope.azmk8s.io:443\"}}\n\
             users:\n- name: u\n  user:\n    exec:\n      apiVersion: client.authentication.k8s.io/v1beta1\n\
             \x20     command: kubelogin\n      args: {args}\n      env: {env}\n"
        );
        exec_info(&Kubeconfig::from_yaml(&yaml).unwrap(), "prod", None).unwrap()
    };
    // `kubelogin convert-kubeconfig -l azurecli`.
    let azurecli = hints(&exec(
        "[get-token, --login, azurecli, --server-id, 6dae42f8-4368-4678-94ff-3960e28e3630]",
        "null",
    ));
    assert_eq!(azurecli, Hints::default());
    // `az aks get-credentials` with device code: the tenant is named.
    let devicecode = hints(&exec(
        "[get-token, --environment, AzurePublicCloud, --server-id, 6dae42f8-4368-4678-94ff-3960e28e3630, \
         --client-id, 80faf920-1908-4b52-b5ef-a8e7bedfc67a, --tenant-id, 11111111-1111-1111-1111-111111111111]",
        "[{name: AZURE_CONFIG_DIR, value: /home/me/.azure-work}, {name: AAD_SERVICE_PRINCIPAL_CLIENT_SECRET, value: s3cr3t}]",
    ));
    assert_eq!(
        devicecode.tenant.as_deref(),
        Some("11111111-1111-1111-1111-111111111111")
    );
    assert_eq!(
        devicecode.env.names().collect::<Vec<_>>(),
        ["AZURE_CONFIG_DIR"]
    );
    assert!(!format!("{devicecode:?}").contains("s3cr3t"));
}

#[test]
fn reads_az_tokens_and_errors() {
    let token = parse_token(fixture("aks", "az-get-access-token.json").as_bytes()).unwrap();
    assert_eq!(
        token.token.0.expose_secret(),
        "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9.fake-test-token.signature"
    );
    assert_eq!(token.expires, Some("2026-09-27T13:00:00Z".parse().unwrap()));
    assert_eq!(token.subscription.as_deref(), Some(PROD));
    assert!(!format!("{token:?}").contains("fake-test-token"));
    // Older CLIs: only the local `expiresOn`.
    let local = parse_token(
        br#"{"accessToken": "x", "expiresOn": "2026-09-27 13:00:00.000000", "tokenType": "Bearer"}"#,
    )
    .unwrap();
    assert!(local.expires.is_some());
    // Parse errors never echo the token.
    let err = parse_token(br#"{"accessToken": "secret-token", "#).unwrap_err();
    assert!(!err.contains("secret-token"));

    assert_eq!(
        token_args(Some("t")).join(" "),
        "account get-access-token --resource https://management.azure.com/ --output json --tenant t"
    );
    let failed = |stderr: &str| CliError::Failed {
        code: Some(1),
        stderr: stderr.into(),
    };
    assert!(matches!(
        classify(&failed("ERROR: Please run 'az login' to setup account."), None),
        ProviderError::Credentials { command: Some(c), .. } if c == "az login"
    ));
    assert!(matches!(
        classify(
            &failed(
                "ERROR: AADSTS700082: The refresh token has expired due to inactivity. \
                 Interactive authentication is needed. Run: az login --tenant 11111111"
            ),
            Some("11111111")
        ),
        ProviderError::Credentials { command: Some(c), .. } if c == "az login --tenant 11111111"
    ));
    assert!(matches!(
        classify(&CliError::NotFound, None),
        ProviderError::Unavailable(m) if m.contains("Azure CLI") && m.contains("install-azure-cli")
    ));
}

#[test]
fn offers_upgrade_profile_versions() {
    let profile: UpgradeProfile = json("upgrade-profile.json");
    let targets = targets("1.30.3", &profile, None);
    let kinds: Vec<_> = targets
        .iter()
        .map(|t| (t.version.as_str(), t.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("1.32.0", TargetKind::Blocked),
            ("1.31.3", TargetKind::Conditional),
            ("1.31.2", TargetKind::Recommended),
            ("1.31.1", TargetKind::Available),
            ("1.30.4", TargetKind::Available),
        ]
    );
    assert_eq!(
        targets[0].blocked,
        ["AKS updates one minor at a time (1.30 → 1.31 first)"]
    );
    assert_eq!(targets[1].risks[0].name, "Preview version");
    assert!(targets[2].minor && !targets[4].minor);
    assert!(
        super::targets("1.30.3", &profile, Some("busy"))
            .iter()
            .all(|t| !t.startable())
    );
}

#[test]
fn status_of_recorded_responses() {
    let status = status(&cref(), &snapshot());
    assert_eq!(
        status.provider,
        "Azure AKS (Azure API · subscription Production · westeurope)"
    );
    assert_eq!(status.current.version, "1.30.3");
    assert_eq!(status.current.channel.as_deref(), Some("patch"));
    assert!(status.current.channels.is_empty());
    assert!(status.current.support.is_none());
    assert_eq!(status.suggested().unwrap().version, "1.31.2");
    assert!(status.progress.is_none());
    assert!(status.writes.control_plane && status.writes.pools);
    assert!(!status.writes.channel && !status.writes.addons);

    let ids: Vec<_> = status.pools.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["control-plane", "agentpool/system", "agentpool/user"]);
    assert_eq!(
        status.pools[0].message.as_deref(),
        Some("Standard tier · auto-upgrade patch")
    );
    let system = &status.pools[1];
    assert_eq!(system.nodes, Some(3));
    assert_eq!(system.surge.as_deref(), Some("maxSurge 33%"));
    assert!(!system.updatable);
    let user = &status.pools[2];
    assert_eq!(user.version.as_deref(), Some("1.29.7"));
    assert_eq!(user.surge.as_deref(), Some("maxSurge 1 · maxUnavailable 0"));
    assert_eq!(
        user.message.as_deref(),
        Some(
            "User · Standard_E8s_v5 · autoscaling 2–8 · image AKSUbuntu-2204gen2containerd-202409.04.0"
        )
    );
    assert!(user.updatable);

    let mut lts = snapshot();
    lts.cluster.properties.support_plan = Some("AKSLongTermSupport".into());
    assert_eq!(
        super::status(&cref(), &lts).current.support.unwrap().text,
        "long-term support (LTS)"
    );
}

#[test]
fn status_while_the_control_plane_updates() {
    let mut snap = snapshot();
    snap.cluster.properties.provisioning_state = Some("Upgrading".into());
    snap.cluster.properties.kubernetes_version = Some("1.31.2".into());
    let status = status(&cref(), &snap);
    let progress = status.progress.as_ref().unwrap();
    assert_eq!(progress.target, "1.31.2");
    assert_eq!(status.pools[0].state, PoolState::Updating);
    assert_eq!(status.pools[0].target_version.as_deref(), Some("1.31.2"));
    assert!(status.targets.iter().all(|t| !t.startable()));
    let err = plan(
        &cref(),
        &status,
        &Scope::Pool("agentpool/user".into()),
        "1.30.3",
        "prod",
        Some(&snap),
    )
    .unwrap_err();
    assert!(err.contains("one operation"), "{err}");
}

#[test]
fn put_bodies_change_only_the_version() {
    let fetched: Value = json("managed-cluster.json");
    let (body, etag) = control_plane_body(fetched.clone(), "1.31.2").unwrap();
    assert_eq!(
        etag.as_deref(),
        Some("c8a1d6f2-0000-4000-8000-000000000001")
    );
    assert_eq!(body["properties"]["kubernetesVersion"], "1.31.2");
    for gone in [
        "provisioningState",
        "powerState",
        "currentKubernetesVersion",
        "fqdn",
        "azurePortalFQDN",
        "maxAgentPools",
        "servicePrincipalProfile",
    ] {
        assert!(body["properties"].get(gone).is_none(), "{gone}");
    }
    for gone in ["id", "name", "type", "eTag"] {
        assert!(body.get(gone).is_none(), "{gone}");
    }
    // Control plane only: pools keep their versions.
    let pools = body["properties"]["agentPoolProfiles"].as_array().unwrap();
    assert_eq!(pools[1]["orchestratorVersion"], "1.29.7");
    assert!(pools[1].get("currentOrchestratorVersion").is_none());
    // Everything else goes back as it came.
    assert_eq!(body["location"], fetched["location"]);
    assert_eq!(
        body["properties"]["networkProfile"],
        fetched["properties"]["networkProfile"]
    );
    assert_eq!(
        body["properties"]["identityProfile"],
        fetched["properties"]["identityProfile"]
    );
    assert!(control_plane_body(json!([]), "1.31.2").is_err());

    let (pool, etag) = agent_pool_body(json("agent-pool-user.json"), "1.30.3").unwrap();
    assert_eq!(
        etag.as_deref(),
        Some("d9b2e7a3-0000-4000-8000-000000000002")
    );
    assert_eq!(pool["properties"]["orchestratorVersion"], "1.30.3");
    assert!(
        pool["properties"]
            .get("currentOrchestratorVersion")
            .is_none()
    );
    assert!(pool["properties"].get("eTag").is_none());
    assert_eq!(pool["properties"]["count"], 4);
    assert!(pool.get("id").is_none());
}

#[test]
fn plans_writes() {
    let snap = snapshot();
    let status = status(&cref(), &snap);

    let plan = plan(
        &cref(),
        &status,
        &Scope::ControlPlane,
        "1.31.2",
        "prod",
        Some(&snap),
    )
    .unwrap();
    assert_eq!(plan.title, "Update the control plane of prod to 1.31.2");
    assert_eq!(plan.kind_label, "minor update");
    assert!(plan.risks.is_empty());
    assert!(plan.changes[0].contains("--control-plane-only"));
    assert_eq!(
        plan.request,
        json!({
            "kind": "control-plane",
            "path": cref().path(),
            "version": "1.31.2",
            "permission": "Microsoft.ContainerService/managedClusters/write",
            "resource": "managed cluster prod-westeurope",
        })
    );
    // A preview carries its risk; a skipped minor is refused.
    let preview = super::plan(
        &cref(),
        &status,
        &Scope::ControlPlane,
        "1.31.3",
        "prod",
        Some(&snap),
    )
    .unwrap();
    assert_eq!(preview.risks.len(), 1);
    assert!(
        super::plan(
            &cref(),
            &status,
            &Scope::ControlPlane,
            "1.32.0",
            "prod",
            Some(&snap)
        )
        .unwrap_err()
        .contains("one minor at a time")
    );
    // Availability sets update node pools with the control plane.
    let mut vmas = snap.clone();
    vmas.cluster.properties.agent_pool_profiles[0].kind = Some("AvailabilitySet".into());
    assert!(
        super::plan(
            &cref(),
            &status,
            &Scope::ControlPlane,
            "1.31.2",
            "prod",
            Some(&vmas)
        )
        .unwrap_err()
        .contains("availability sets")
    );

    let pool = super::plan(
        &cref(),
        &status,
        &Scope::Pool("agentpool/user".into()),
        "1.30.3",
        "prod",
        Some(&snap),
    )
    .unwrap();
    assert_eq!(pool.request["path"], cref().agent_pool_path("user"));
    assert_eq!(pool.request["version"], "1.30.3");
    assert!(pool.notes[0].contains("maxSurge 1 · maxUnavailable 0"));
    assert!(
        super::plan(
            &cref(),
            &status,
            &Scope::Pool("agentpool/user".into()),
            "1.31.2",
            "prod",
            Some(&snap),
        )
        .unwrap_err()
        .contains("control plane to 1.31.2 first")
    );
    assert!(
        super::plan(
            &cref(),
            &status,
            &Scope::Pool("agentpool/system".into()),
            "1.30.3",
            "prod",
            Some(&snap),
        )
        .unwrap_err()
        .contains("already runs")
    );
    assert!(
        super::plan(
            &cref(),
            &status,
            &Scope::Channel("stable".into()),
            "",
            "prod",
            None
        )
        .is_err()
    );
}

fn test_provider(base: &Url, settings: ClusterUpdateSettings) -> Aks {
    let ctx = CloudContext {
        display_name: "prod".into(),
        kubeconfig: PathBuf::from("/nonexistent/kubeconfig"),
        context: "prod-westeurope".into(),
        user: None,
        cluster_entry: "prod-westeurope".into(),
        server: "https://prod-k8s-abc123de.hcp.westeurope.azmk8s.io:443".into(),
        settings,
    };
    let endpoint = base.as_str().to_string();
    build(
        ctx,
        move |var| (var == cloud::AKS_ENDPOINT).then(|| endpoint.clone()),
        Some(AzToken {
            token: BearerToken("fake-arm-token".to_string().into()),
            expires: None,
            subscription: Some(PROD.into()),
            tenant: Some("11111111-1111-1111-1111-111111111111".into()),
        }),
    )
}

fn routes(put_status: u16, put_body: String) -> Vec<mock::Route> {
    let path = cref().path();
    let clusters = |sub: &str| {
        format!("/subscriptions/{sub}/providers/Microsoft.ContainerService/managedClusters")
    };
    vec![
        (
            "GET",
            "/subscriptions".into(),
            200,
            fixture("aks", "subscriptions.json"),
        ),
        (
            "GET",
            clusters("00000000-0000-0000-0000-000000000001"),
            200,
            fixture("aks", "managed-clusters-development.json"),
        ),
        (
            "GET",
            clusters(PROD),
            200,
            format!(
                r#"{{"value": [{}]}}"#,
                fixture("aks", "managed-cluster.json")
            ),
        ),
        (
            "GET",
            path.clone(),
            200,
            fixture("aks", "managed-cluster.json"),
        ),
        (
            "GET",
            format!("{path}/upgradeProfiles/default"),
            200,
            fixture("aks", "upgrade-profile.json"),
        ),
        ("PUT", path, put_status, put_body),
    ]
}

#[tokio::test]
async fn finds_reads_and_updates_through_a_local_server() {
    let (base, seen) = mock::serve(routes(200, fixture("aks", "put-accepted.json"))).await;
    let provider = test_provider(&base, ClusterUpdateSettings::default());
    let status = provider.read().await.unwrap();
    assert_eq!(
        status.provider,
        "Azure AKS (Azure API · subscription Production · westeurope)"
    );
    assert_eq!(status.current.version, "1.30.3");
    {
        let seen = seen.lock().unwrap();
        assert!(
            seen.iter()
                .all(|s| s.header("authorization") == Some("Bearer fake-arm-token"))
        );
        assert!(
            seen.iter()
                .any(|s| s.target == "/subscriptions?api-version=2022-12-01")
        );
        assert!(
            seen.iter()
                .filter(|s| s.target.contains("managedClusters"))
                .all(|s| s.target.ends_with("api-version=2024-09-01"))
        );
    }
    // Found once: the next read goes straight to the cluster.
    let before = seen.lock().unwrap().len();
    provider.read().await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), before + 2);

    let plan = provider
        .plan(&status, &Scope::ControlPlane, "1.31.2", "prod")
        .unwrap();
    let message = provider.start(&plan).await.unwrap();
    assert!(
        message.contains("provisioning state Upgrading"),
        "{message}"
    );
    let put = seen
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.method == "PUT")
        .cloned()
        .unwrap();
    assert_eq!(
        put.header("if-match"),
        Some("c8a1d6f2-0000-4000-8000-000000000001")
    );
    let body: Value = serde_json::from_slice(&put.body).unwrap();
    assert_eq!(body["properties"]["kubernetesVersion"], "1.31.2");
    assert!(body["properties"].get("servicePrincipalProfile").is_none());
    assert_eq!(
        body["properties"]["agentPoolProfiles"][1]["orchestratorVersion"],
        "1.29.7"
    );
}

#[tokio::test]
async fn denied_writes_and_unknown_clusters() {
    let (base, _) = mock::serve(routes(
        403,
        fixture("aks", "error-authorization-failed.json"),
    ))
    .await;
    // Named in settings: no search.
    let settings = ClusterUpdateSettings {
        aks: Some(AksSettings {
            subscription: Some(PROD.into()),
            resource_group: Some("rg-prod".into()),
            name: Some("prod-westeurope".into()),
        }),
        ..ClusterUpdateSettings::default()
    };
    let provider = test_provider(&base, settings);
    let status = provider.read().await.unwrap();
    assert_eq!(
        status.provider,
        format!("Azure AKS (Azure API · subscription {PROD} · westeurope)")
    );
    let plan = provider
        .plan(&status, &Scope::ControlPlane, "1.31.2", "prod")
        .unwrap();
    assert_eq!(
        provider.start(&plan).await.unwrap_err(),
        ProviderError::Forbidden {
            verb: "Microsoft.ContainerService/managedClusters/write".into(),
            resource: "managed cluster prod-westeurope".into()
        }
    );

    // A server no subscription has.
    let (base, _) = mock::serve(vec![
        ("GET", "/subscriptions".into(), 200, fixture("aks", "subscriptions.json")),
        (
            "GET",
            "/subscriptions/00000000-0000-0000-0000-000000000001/providers/Microsoft.ContainerService/managedClusters".into(),
            200,
            fixture("aks", "managed-clusters-development.json"),
        ),
    ])
    .await;
    let provider = test_provider(&base, ClusterUpdateSettings::default());
    let err = provider.read().await.unwrap_err();
    assert!(
        matches!(&err, ProviderError::NotFound(m) if m.contains("2 Azure subscriptions") && m.contains("\"aks\"")),
        "{err:?}"
    );
}
