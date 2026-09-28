use std::path::PathBuf;

use kube::config::Kubeconfig;
use secrecy::SecretString;

use super::api::{AddonInfo, UpdateEnvelope};
use super::*;
use crate::providers::cloud::exec_info;
use crate::providers::cloud::mock::{self, fixture};
use crate::settings::ClusterUpdateSettings;

fn json<T: serde::de::DeserializeOwned>(name: &str) -> T {
    serde_json::from_str(&fixture("eks", name)).unwrap()
}

fn eks_target() -> EksTarget {
    EksTarget {
        cluster: "prod-eu-west-1".into(),
        region: "eu-west-1".into(),
        profile: Some("prod".into()),
        role_arn: None,
        domain: "amazonaws.com".into(),
        env: CliEnv::default(),
    }
}

fn addon_versions(name: &str) -> Vec<AddonVersion> {
    let value: Value = serde_json::from_str(&fixture(
        "eks",
        &format!("describe-addon-versions-{name}.json"),
    ))
    .unwrap();
    let infos: Vec<AddonInfo> = serde_json::from_value(value["addons"].clone()).unwrap();
    infos.into_iter().flat_map(|i| i.addon_versions).collect()
}

fn snapshot() -> Snapshot {
    let cluster: Value = json("describe-cluster.json");
    let nodegroup = |name: &str| -> Nodegroup {
        let value: Value = json(&format!("describe-nodegroup-{name}.json"));
        serde_json::from_value(value["nodegroup"].clone()).unwrap()
    };
    let addon = |name: &str| -> Addon {
        let value: Value = json(&format!("describe-addon-{name}.json"));
        serde_json::from_value(value["addon"].clone()).unwrap()
    };
    let versions: Value = json("describe-cluster-versions.json");
    let done: UpdateEnvelope = json("describe-update-done.json");
    let logging: UpdateEnvelope = json("describe-update-logging.json");
    let mut updates = vec![done.update, logging.update];
    updates.sort_by_key(|u| std::cmp::Reverse(u.created()));
    Snapshot {
        cluster: serde_json::from_value(cluster["cluster"].clone()).unwrap(),
        nodegroups: vec![nodegroup("general"), nodegroup("gpu")],
        addons: vec![addon("coredns"), addon("kube-proxy"), addon("vpc-cni")],
        addon_versions: ["coredns", "kube-proxy", "vpc-cni"]
            .into_iter()
            .map(|n| (n.to_string(), addon_versions(n)))
            .collect(),
        versions: Some(serde_json::from_value(versions["clusterVersions"].clone()).unwrap()),
        updates,
        nodegroup_updates: HashMap::new(),
        notes: Vec::new(),
    }
}

fn now() -> Timestamp {
    "2025-06-01T00:00:00Z".parse().unwrap()
}

fn kubeconfig_exec(yaml_user: &str) -> ExecInfo {
    let yaml = format!(
        "apiVersion: v1\nkind: Config\ncontexts:\n- name: ctx\n  context: {{cluster: c, user: u}}\n\
         clusters:\n- name: c\n  cluster: {{server: https://example.com}}\nusers:\n- name: u\n  user:\n{yaml_user}"
    );
    let config = Kubeconfig::from_yaml(&yaml).unwrap();
    exec_info(&config, "ctx", None).unwrap()
}

#[test]
fn reads_aws_eks_update_kubeconfig_output() {
    // `aws eks update-kubeconfig --name prod-eu-west-1 --region eu-west-1 --profile prod
    // --role-arn …` (AWS CLI v2): the profile goes into env, the role into args.
    let exec = kubeconfig_exec(
        r#"    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      command: aws
      args:
      - --region
      - eu-west-1
      - eks
      - get-token
      - --cluster-name
      - prod-eu-west-1
      - --output
      - json
      - --role-arn
      - arn:aws:iam::111122223333:role/EksAdmin
      env:
      - name: AWS_PROFILE
        value: prod
"#,
    );
    let hints = hints(&exec);
    assert_eq!(hints.cluster.as_deref(), Some("prod-eu-west-1"));
    assert_eq!(hints.region.as_deref(), Some("eu-west-1"));
    assert_eq!(hints.profile.as_deref(), Some("prod"));
    assert_eq!(
        hints.role_arn.as_deref(),
        Some("arn:aws:iam::111122223333:role/EksAdmin")
    );
    assert_eq!(hints.env.get("AWS_PROFILE"), Some("prod"));
}

#[test]
fn reads_eksctl_and_authenticator_shapes() {
    // eksctl: flags after the subcommand, the region from args, STS env.
    let eksctl = hints(&kubeconfig_exec(
        r#"    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      command: aws
      args: [eks, get-token, --output, json, --cluster-name, staging, --region, us-east-2]
      env:
      - {name: AWS_STS_REGIONAL_ENDPOINTS, value: regional}
      - {name: KUBECONFIG_UNRELATED, value: x}
"#,
    ));
    assert_eq!(eksctl.cluster.as_deref(), Some("staging"));
    assert_eq!(eksctl.region.as_deref(), Some("us-east-2"));
    assert_eq!(eksctl.profile, None);
    assert_eq!(
        eksctl.env.names().collect::<Vec<_>>(),
        ["AWS_STS_REGIONAL_ENDPOINTS"]
    );

    // aws-iam-authenticator: `-i` names the cluster, `-r` the role; region and profile from env.
    let authenticator = hints(&kubeconfig_exec(
        r#"    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      command: /usr/local/bin/aws-iam-authenticator
      args: [token, -i, legacy, -r, "arn:aws:iam::111122223333:role/EksAdmin"]
      env:
      - {name: AWS_PROFILE, value: ops}
      - {name: AWS_DEFAULT_REGION, value: eu-central-1}
"#,
    ));
    assert_eq!(authenticator.cluster.as_deref(), Some("legacy"));
    assert_eq!(
        authenticator.role_arn.as_deref(),
        Some("arn:aws:iam::111122223333:role/EksAdmin")
    );
    assert_eq!(authenticator.region.as_deref(), Some("eu-central-1"));
    assert_eq!(authenticator.profile.as_deref(), Some("ops"));

    // EKS on Outposts: `--cluster-id`.
    let outposts = hints(&kubeconfig_exec(
        r#"    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      command: aws
      args: [--region, us-west-2, eks, get-token, --cluster-id, 0a1b2c3d-0000-4000-8000-000000000001]
"#,
    ));
    assert_eq!(outposts.cluster, None);
    assert_eq!(
        outposts.cluster_id.as_deref(),
        Some("0a1b2c3d-0000-4000-8000-000000000001")
    );
}

#[test]
fn resolves_cluster_region_and_profile() {
    let server = "https://ABCDEF0123456789.gr7.eu-west-1.eks.amazonaws.com";
    let arn = "arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1";
    // Only the ARN and the server: enough.
    let target = resolve(None, &Hints::default(), "prod", arn, server).unwrap();
    assert_eq!(target.cluster, "prod-eu-west-1");
    assert_eq!(target.region, "eu-west-1");
    assert_eq!(target.profile, None);
    assert_eq!(target.endpoint(), "https://eks.eu-west-1.amazonaws.com");
    assert_eq!(target.label(), "Amazon EKS (AWS API · eu-west-1)");

    // Settings win over the exec plugin.
    let hints = Hints {
        cluster: Some("from-exec".into()),
        region: Some("us-east-1".into()),
        profile: Some("exec-profile".into()),
        ..Hints::default()
    };
    let settings = EksSettings {
        cluster: Some("from-settings".into()),
        region: None,
        profile: Some("admin".into()),
    };
    let target = resolve(Some(&settings), &hints, "ctx", "cluster", server).unwrap();
    assert_eq!(
        (target.cluster.as_str(), target.region.as_str()),
        ("from-settings", "us-east-1")
    );
    assert_eq!(target.profile.as_deref(), Some("admin"));
    assert_eq!(
        target.label(),
        "Amazon EKS (AWS API · profile admin · us-east-1)"
    );
    assert_eq!(target.cli_suffix(), " --region us-east-1 --profile admin");

    // The server's host alone gives the region; the name must come from somewhere.
    let err = resolve(None, &Hints::default(), "my-ctx", "my-cluster", server).unwrap_err();
    assert!(
        matches!(&err, ProviderError::Other(m) if m.contains("cluster name") && m.contains("\"my-ctx\"")),
        "{err:?}"
    );

    // China.
    let target = resolve(
        None,
        &Hints {
            cluster: Some("cn".into()),
            ..Hints::default()
        },
        "ctx",
        "c",
        "https://ABC.yl4.cn-north-1.eks.amazonaws.com.cn",
    )
    .unwrap();
    assert_eq!(target.endpoint(), "https://eks.cn-north-1.amazonaws.com.cn");
}

#[test]
fn parses_arns_and_hosts() {
    assert_eq!(
        parse_arn("arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1"),
        Some((
            "aws".to_string(),
            "eu-west-1".to_string(),
            "prod-eu-west-1".to_string()
        ))
    );
    assert_eq!(parse_arn("arn:aws:iam::111122223333:role/x"), None);
    assert_eq!(parse_arn("gke_project_zone_name"), None);
    assert_eq!(
        server_region("https://ABCDEF.gr7.eu-west-1.eks.amazonaws.com:443"),
        Some(("eu-west-1".to_string(), "amazonaws.com".to_string()))
    );
    assert_eq!(server_region("https://api.example.com"), None);
}

#[test]
fn offers_the_next_minor_only() {
    let snap = snapshot();
    let offered = targets("1.30", snap.versions.as_deref(), None);
    let versions: Vec<_> = offered
        .iter()
        .map(|t| (t.version.as_str(), t.kind))
        .collect();
    assert_eq!(
        versions,
        [
            ("1.33", TargetKind::Blocked),
            ("1.32", TargetKind::Blocked),
            ("1.31", TargetKind::Recommended)
        ]
    );
    assert_eq!(
        offered[0].blocked,
        ["EKS updates one minor at a time (1.30 → 1.31 first)"]
    );
    // Without the version list: the next minor, not recommended.
    let derived = targets("1.30", None, None);
    assert_eq!(derived.len(), 1);
    assert_eq!(
        (derived[0].version.as_str(), derived[0].kind),
        ("1.31", TargetKind::Available)
    );
    // Busy: blocked with the reason.
    let busy = targets("1.30", snap.versions.as_deref(), Some("an update runs"));
    assert_eq!(busy[2].kind, TargetKind::Blocked);
    assert_eq!(busy[2].blocked, ["an update runs"]);
    // On the newest version: nothing.
    assert!(targets("1.33", snap.versions.as_deref(), None).is_empty());
}

#[test]
fn support_window() {
    let snap = snapshot();
    let info = snap
        .versions
        .as_ref()
        .unwrap()
        .iter()
        .find(|v| v.cluster_version == "1.30");
    let window = support(info, Some("EXTENDED"), now()).unwrap();
    assert_eq!(
        window.text,
        "standard support until 2025-07-23, extended until 2026-07-23"
    );
    // 52 days left.
    assert!(window.warning);
    let early = support(
        info,
        Some("STANDARD"),
        "2024-06-01T00:00:00Z".parse().unwrap(),
    )
    .unwrap();
    assert_eq!(
        early.text,
        "standard support until 2025-07-23, then EKS updates it to the next version"
    );
    assert!(!early.warning);
    let mut extended = info.unwrap().clone();
    extended.version_status = Some("EXTENDED_SUPPORT".into());
    let support = super::support(Some(&extended), None, now()).unwrap();
    assert_eq!(support.text, "extended support until 2026-07-23");
    assert!(support.warning);
}

#[test]
fn status_of_recorded_responses() {
    let status = status(&eks_target(), &snapshot(), now());
    assert_eq!(
        status.provider,
        "Amazon EKS (AWS API · profile prod · eu-west-1)"
    );
    assert_eq!(status.current.version, "1.30");
    assert_eq!(status.current.platform.as_deref(), Some("eks.12"));
    assert_eq!(
        status.current.cluster_id.as_deref(),
        Some("arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1")
    );
    assert_eq!(status.suggested().unwrap().version, "1.31");
    assert!(status.progress.is_none());
    assert!(status.writes.control_plane && status.writes.pools && status.writes.addons);
    assert!(!status.writes.channel);

    let ids: Vec<_> = status.pools.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["control-plane", "nodegroup/general", "nodegroup/gpu"]);
    let control_plane = &status.pools[0];
    assert_eq!(control_plane.message.as_deref(), Some("platform eks.12"));
    assert!(control_plane.updatable);
    let general = &status.pools[1];
    assert_eq!(general.nodes, Some(3));
    assert_eq!(general.surge.as_deref(), Some("maxUnavailable 1"));
    assert!(!general.updatable, "already at the control plane's version");
    assert_eq!(
        general.message.as_deref(),
        Some("AMI 1.30.4-20240917 · m6i.large · on-demand")
    );
    let gpu = &status.pools[2];
    assert_eq!(gpu.version.as_deref(), Some("1.29"));
    assert_eq!(gpu.surge.as_deref(), Some("maxUnavailable 33%"));
    assert!(gpu.updatable);

    let addon = |name: &str| status.addons.iter().find(|a| a.name == name).unwrap();
    assert_eq!(addon("coredns").compatible, Some(true));
    assert_eq!(
        addon("coredns").recommended.as_deref(),
        Some("v1.11.3-eksbuild.1")
    );
    assert_eq!(addon("vpc-cni").compatible, Some(false));
    assert_eq!(
        addon("vpc-cni").recommended.as_deref(),
        Some("v1.19.0-eksbuild.1")
    );
    assert!(addon("vpc-cni").updatable);
    assert_eq!(addon("kube-proxy").compatible, Some(false));

    assert_eq!(status.history.len(), 1);
    assert_eq!(status.history[0].version, "1.30");
    assert!(status.history[0].completed());
    assert!(status.current.support.as_ref().unwrap().warning);
}

#[test]
fn status_while_the_control_plane_updates() {
    let mut snap = snapshot();
    let running: UpdateEnvelope = json("describe-update-running.json");
    snap.updates.insert(0, running.update);
    snap.cluster.status = Some("UPDATING".into());
    let status = status(&eks_target(), &snap, now());
    let progress = status.progress.as_ref().unwrap();
    assert_eq!(progress.target, "1.31");
    assert!(
        progress.message.contains("c7a1f3e2"),
        "{}",
        progress.message
    );
    assert_eq!(status.pools[0].state, PoolState::Updating);
    assert_eq!(status.pools[0].target_version.as_deref(), Some("1.31"));
    assert!(status.targets.iter().all(|t| !t.startable()));
    assert!(status.history[0].version == "1.31" && !status.history[0].completed());
}

#[test]
fn addon_and_insight_checks() {
    let snap = snapshot();
    let addons: Vec<_> = snap
        .addons
        .iter()
        .map(|a| {
            (
                a.addon_name.clone(),
                a.addon_version.clone().unwrap(),
                snap.addon_versions.get(&a.addon_name).cloned(),
            )
        })
        .collect();
    let check = addon_check(&eks_target(), &addons, "1.31");
    assert_eq!(check.status, CheckStatus::Warn);
    assert_eq!(check.summary, "1 of 3 add-ons support 1.31");
    let vpc = check
        .details
        .iter()
        .find(|d| d.text.starts_with("vpc-cni"))
        .unwrap();
    assert_eq!(
        vpc.sub.as_deref(),
        Some("EKS recommends v1.19.0-eksbuild.1 for 1.31")
    );
    assert!(matches!(
        &vpc.fix,
        Some(Fix::Copy { text, .. }) if text == "aws eks update-addon --cluster-name prod-eu-west-1 \
            --addon-name vpc-cni --addon-version v1.19.0-eksbuild.1 --resolve-conflicts PRESERVE \
            --region eu-west-1 --profile prod"
    ));
    let mut unknown = addons.clone();
    unknown[0].2 = None;
    assert_eq!(
        addon_check(&eks_target(), &unknown[..1], "1.31").status,
        CheckStatus::Unknown
    );
    assert_eq!(
        addon_check(&eks_target(), &[], "1.31").status,
        CheckStatus::Info
    );

    let value: Value = json("list-insights.json");
    let insights: Vec<Insight> = serde_json::from_value(value["insights"].clone()).unwrap();
    let check = insights_check(Ok(insights), "1.31");
    // The 1.32 insight (an error) isn't about this target.
    assert_eq!(check.status, CheckStatus::Warn);
    assert_eq!(check.details.len(), 2);
    assert_eq!(
        check.summary,
        "EKS upgrade insights for 1.31: 1 warning, 1 passing"
    );
    let denied = insights_check(
        Err(ProviderError::Forbidden {
            verb: "eks:ListInsights".into(),
            resource: "cluster prod".into(),
        }),
        "1.31",
    );
    assert_eq!(denied.status, CheckStatus::Unknown);
    assert!(denied.summary.contains("eks:ListInsights"));
    assert_eq!(
        insights_check(Ok(Vec::new()), "1.31").status,
        CheckStatus::Info
    );
}

#[test]
fn plans_writes() {
    let snap = snapshot();
    let target = eks_target();
    let status = status(&target, &snap, now());

    let plan = plan(
        &target,
        &status,
        &Scope::ControlPlane,
        "1.31",
        "prod",
        Some(&snap),
    )
    .unwrap();
    assert_eq!(plan.title, "Update the control plane of prod to 1.31");
    assert_eq!((plan.from.as_str(), plan.to.as_str()), ("1.30", "1.31"));
    assert!(plan.irreversible);
    assert_eq!(
        plan.changes[0],
        "UpdateClusterVersion prod-eu-west-1 → 1.31 (like aws eks update-cluster-version --name \
         prod-eu-west-1 --kubernetes-version 1.31 --region eu-west-1 --profile prod)"
    );
    assert!(plan.notes[0].starts_with("Node groups keep 1.30"));
    assert!(
        plan.notes
            .iter()
            .any(|n| n.contains("can't downgrade a control plane"))
    );
    assert_eq!(
        plan.request,
        json!({
            "operation": "UpdateClusterVersion",
            "path": "/clusters/prod-eu-west-1/updates",
            "body": {"version": "1.31"},
            "permission": "eks:UpdateClusterVersion",
            "resource": "cluster prod-eu-west-1",
        })
    );
    assert_eq!(
        super::plan(
            &target,
            &status,
            &Scope::ControlPlane,
            "1.32",
            "prod",
            Some(&snap)
        )
        .unwrap_err(),
        "EKS updates one minor at a time (1.30 → 1.31 first)"
    );

    // Node groups: never newer than the control plane.
    let err = super::plan(
        &target,
        &status,
        &Scope::Pool("nodegroup/general".into()),
        "1.31",
        "prod",
        Some(&snap),
    )
    .unwrap_err();
    assert!(err.contains("control plane to 1.31 first"), "{err}");
    let gpu = super::plan(
        &target,
        &status,
        &Scope::Pool("nodegroup/gpu".into()),
        "1.30",
        "prod",
        Some(&snap),
    )
    .unwrap();
    assert_eq!(gpu.title, "Update node group gpu to 1.30");
    assert_eq!(
        gpu.request["path"],
        "/clusters/prod-eu-west-1/node-groups/gpu/update-version"
    );
    assert_eq!(gpu.request["body"], json!({"version": "1.30"}));
    assert!(gpu.notes[0].contains("maxUnavailable 33%"));
    assert!(
        gpu.notes
            .iter()
            .any(|n| n.contains("Launch template gpu-nodes (version 3)"))
    );
    // Same minor: a newer AMI.
    let ami = super::plan(
        &target,
        &status,
        &Scope::Pool("nodegroup/general".into()),
        "1.30",
        "prod",
        Some(&snap),
    )
    .unwrap();
    assert_eq!(
        ami.title,
        "Update node group general to the latest 1.30 AMI"
    );
    // A custom AMI can't be updated this way.
    let mut custom = snap.clone();
    custom.nodegroups[1].ami_type = Some("CUSTOM".into());
    assert!(
        super::plan(
            &target,
            &status,
            &Scope::Pool("nodegroup/gpu".into()),
            "1.30",
            "prod",
            Some(&custom),
        )
        .unwrap_err()
        .contains("launch template")
    );

    // Add-ons: the recommended version; one that doesn't list the current version is a risk.
    let vpc = super::plan(
        &target,
        &status,
        &Scope::AddOn("vpc-cni".into()),
        "1.31",
        "prod",
        Some(&snap),
    )
    .unwrap();
    assert_eq!(vpc.to, "v1.19.0-eksbuild.1");
    assert!(vpc.risks.is_empty());
    assert_eq!(
        vpc.request["body"],
        json!({"addonVersion": "v1.19.0-eksbuild.1", "resolveConflicts": "PRESERVE"})
    );
    assert_eq!(
        vpc.request["path"],
        "/clusters/prod-eu-west-1/addons/vpc-cni/update"
    );
    let proxy = super::plan(
        &target,
        &status,
        &Scope::AddOn("kube-proxy".into()),
        "1.31",
        "prod",
        Some(&snap),
    )
    .unwrap();
    assert_eq!(proxy.to, "v1.31.0-eksbuild.5");
    assert_eq!(proxy.risks.len(), 1);
    assert!(proxy.risks[0].message.contains("current version 1.30"));
    // An explicit add-on version.
    let pinned = super::plan(
        &target,
        &status,
        &Scope::AddOn("vpc-cni".into()),
        "v1.18.3-eksbuild.1",
        "prod",
        Some(&snap),
    )
    .unwrap();
    assert_eq!(pinned.to, "v1.18.3-eksbuild.1");

    assert!(
        super::plan(
            &target,
            &status,
            &Scope::Channel("stable".into()),
            "",
            "prod",
            None
        )
        .is_err()
    );
}

fn context(kubeconfig: PathBuf) -> CloudContext {
    CloudContext {
        display_name: "prod".into(),
        kubeconfig,
        context: "arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1".into(),
        user: None,
        cluster_entry: "arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1".into(),
        server: "https://ABCDEF0123456789.gr7.eu-west-1.eks.amazonaws.com".into(),
        settings: ClusterUpdateSettings::default(),
    }
}

fn fake_credentials() -> AwsCredentials {
    AwsCredentials {
        access_key_id: SecretString::from("ASIAEXAMPLEKEYID"),
        secret_access_key: SecretString::from("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY"),
        session_token: Some(SecretString::from("FwoGZXIvYXdzEXAMPLETOKEN")),
        expiration: None,
    }
}

fn test_provider(base: Url) -> Eks {
    Eks {
        inner: Arc::new(Inner::new(
            context(PathBuf::from("/nonexistent/kubeconfig")),
            Some(eks_target()),
            Ok(Some(base)),
            Some(fake_credentials()),
        )),
    }
}

/// `aws eks update-kubeconfig --name prod-eu-west-1 --region eu-west-1 --profile prod`.
const KUBECONFIG: &str = r#"apiVersion: v1
kind: Config
clusters:
- name: arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1
  cluster:
    server: https://ABCDEF0123456789.gr7.eu-west-1.eks.amazonaws.com
    certificate-authority-data: LS0tLS1CRUdJTiBDRVJUSUZJQ0FURS0tLS0tCkVYQU1QTEUKLS0tLS1FTkQgQ0VSVElGSUNBVEUtLS0tLQo=
contexts:
- name: arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1
  context:
    cluster: arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1
    user: arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1
current-context: arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1
users:
- name: arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1
  user:
    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      command: aws
      args: [--region, eu-west-1, eks, get-token, --cluster-name, prod-eu-west-1, --output, json]
      env:
      - name: AWS_PROFILE
        value: prod
"#;

/// The whole path through `KUBYL_UPDATES_EKS_ENDPOINT`: the kubeconfig file's exec plugin
/// names the cluster, region and profile; requests go to a server on 127.0.0.1 serving the
/// recorded responses. Only the credentials are preset (the CLI isn't run).
#[tokio::test]
async fn reads_and_plans_through_the_endpoint_override() {
    let (base, seen) = mock::serve(routes()).await;
    let dir = tempfile::tempdir().unwrap();
    let kubeconfig = dir.path().join("config");
    std::fs::write(&kubeconfig, KUBECONFIG).unwrap();
    let endpoint = base.as_str().trim_end_matches('/').to_string();
    let provider = build(
        context(kubeconfig.clone()),
        |var| (var == cloud::EKS_ENDPOINT).then(|| endpoint.clone()),
        Some(fake_credentials()),
    );

    let status = provider.read().await.unwrap();
    assert_eq!(
        status.provider,
        "Amazon EKS (AWS API · profile prod · eu-west-1)"
    );
    assert_eq!(status.current.version, "1.30");
    assert_eq!(status.suggested().unwrap().version, "1.31");
    assert_eq!(status.pools.len(), 3);
    let plan = provider
        .plan(&status, &Scope::ControlPlane, "1.31", "prod")
        .unwrap();
    assert_eq!(plan.request["path"], "/clusters/prod-eu-west-1/updates");
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .all(|s| s.header("host") == Some(base.authority()))
    );

    // Plain http elsewhere than loopback is refused before anything runs.
    let refused = build(
        context(kubeconfig),
        |_| Some("http://eks.example.com".into()),
        Some(fake_credentials()),
    );
    assert!(matches!(
        refused.read().await,
        Err(ProviderError::Other(m)) if m.starts_with("KUBYL_UPDATES_EKS_ENDPOINT is invalid")
    ));
}

fn routes() -> Vec<mock::Route> {
    let c = "/clusters/prod-eu-west-1";
    let f = |name: &str| fixture("eks", name);
    let mut routes = vec![
        ("GET", c.to_string(), 200, f("describe-cluster.json")),
        (
            "GET",
            format!("{c}/node-groups"),
            200,
            f("list-nodegroups.json"),
        ),
        (
            "GET",
            format!("{c}/node-groups/general"),
            200,
            f("describe-nodegroup-general.json"),
        ),
        (
            "GET",
            format!("{c}/node-groups/gpu"),
            200,
            f("describe-nodegroup-gpu.json"),
        ),
        ("GET", format!("{c}/addons"), 200, f("list-addons.json")),
        (
            "GET",
            "/cluster-versions".into(),
            200,
            f("describe-cluster-versions.json"),
        ),
        ("GET", format!("{c}/updates"), 200, f("list-updates.json")),
        (
            "GET",
            format!("{c}/updates/b5f0ba18-0000-4000-8000-000000000001"),
            200,
            f("describe-update-done.json"),
        ),
        (
            "GET",
            format!("{c}/updates/b5f0ba18-0000-4000-8000-000000000002"),
            200,
            f("describe-update-logging.json"),
        ),
        (
            "POST",
            format!("{c}/insights"),
            200,
            f("list-insights.json"),
        ),
    ];
    for addon in ["coredns", "kube-proxy", "vpc-cni"] {
        routes.push((
            "GET",
            format!("{c}/addons/{addon}"),
            200,
            f(&format!("describe-addon-{addon}.json")),
        ));
        routes.push((
            "GET",
            format!("/addons/supported-versions?addonName={addon}"),
            200,
            f(&format!("describe-addon-versions-{addon}.json")),
        ));
    }
    routes
}

#[tokio::test]
async fn reads_plans_and_starts_through_a_local_server() {
    let mut routes = routes();
    routes.push((
        "POST",
        "/clusters/prod-eu-west-1/updates".into(),
        200,
        fixture("eks", "update-cluster-version.json"),
    ));
    let (base, seen) = mock::serve(routes).await;
    let provider = test_provider(base);

    let status = provider.read().await.unwrap();
    assert_eq!(status.current.version, "1.30");
    assert_eq!(status.pools.len(), 3);
    assert_eq!(status.addons.len(), 3);
    assert_eq!(status.history.len(), 1);
    assert!(status.notes.is_empty(), "{:?}", status.notes);

    // Every request is signed with the session's temporary credentials.
    for request in seen.lock().unwrap().iter() {
        let auth = request.header("authorization").unwrap();
        let signed = if request.body.is_empty() {
            "SignedHeaders=host;x-amz-date;x-amz-security-token"
        } else {
            "SignedHeaders=content-type;host;x-amz-date;x-amz-security-token"
        };
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256 Credential=ASIAEXAMPLEKEYID/")
                && auth.contains("/eu-west-1/eks/aws4_request")
                && auth.contains(signed),
            "{auth}"
        );
        assert_eq!(
            request.header("x-amz-security-token"),
            Some("FwoGZXIvYXdzEXAMPLETOKEN")
        );
    }
    // Finished updates are described once.
    let describes = |seen: &mock::Seen| seen.target.contains("/updates/b5f0ba18");
    let before = seen.lock().unwrap().iter().filter(|s| describes(s)).count();
    provider.read().await.unwrap();
    let after = seen.lock().unwrap().iter().filter(|s| describes(s)).count();
    assert_eq!(before, after);

    let checks = provider.preflight_extras(&status, "1.31").await;
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[1].status, CheckStatus::Warn);
    let insights = seen
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.target.ends_with("/insights"))
        .cloned()
        .unwrap();
    let body: Value = serde_json::from_slice(&insights.body).unwrap();
    assert_eq!(
        body,
        json!({"filter": {"categories": ["UPGRADE_READINESS"], "kubernetesVersions": ["1.31"]}})
    );

    let plan = provider
        .plan(&status, &Scope::ControlPlane, "1.31", "prod")
        .unwrap();
    let message = provider.start(&plan).await.unwrap();
    assert!(
        message.contains("c7a1f3e2-0000-4000-8000-000000000003"),
        "{message}"
    );
    let posts: Vec<_> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.method == "POST" && s.target == "/clusters/prod-eu-west-1/updates")
        .cloned()
        .collect();
    assert_eq!(posts.len(), 1, "writes are sent once");
    assert_eq!(posts[0].body, br#"{"version":"1.31"}"#);
    assert_eq!(posts[0].header("content-type"), Some("application/json"));
}

#[tokio::test]
async fn a_denied_write_names_the_permission() {
    let mut routes = routes();
    routes.push((
        "POST",
        "/clusters/prod-eu-west-1/updates".into(),
        403,
        fixture("eks", "error-access-denied.json"),
    ));
    let (base, seen) = mock::serve(routes).await;
    let provider = test_provider(base);
    let status = provider.read().await.unwrap();
    let plan = provider
        .plan(&status, &Scope::ControlPlane, "1.31", "prod")
        .unwrap();
    assert_eq!(
        provider.start(&plan).await.unwrap_err(),
        ProviderError::Forbidden {
            verb: "eks:UpdateClusterVersion".into(),
            resource: "cluster prod-eu-west-1".into()
        }
    );
    let posts = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.method == "POST" && s.target.ends_with("/updates"))
        .count();
    assert_eq!(posts, 1, "never retried");
}

#[tokio::test]
async fn a_missing_cluster_fails_the_read() {
    let (base, _) = mock::serve(Vec::new()).await;
    let provider = test_provider(base);
    assert!(matches!(
        provider.read().await,
        Err(ProviderError::NotFound(m)) if m.contains("cluster prod-eu-west-1")
    ));
}

/// Every add-on's versions denied: one note, not one per add-on.
#[tokio::test]
async fn denied_add_on_versions_make_one_note() {
    let mut routes: Vec<mock::Route> = routes()
        .into_iter()
        .filter(|r| !r.1.starts_with("/addons/supported-versions"))
        .collect();
    for addon in ["coredns", "kube-proxy", "vpc-cni"] {
        routes.push((
            "GET",
            format!("/addons/supported-versions?addonName={addon}"),
            403,
            fixture("eks", "error-access-denied.json"),
        ));
    }
    let (base, _) = mock::serve(routes).await;
    let status = test_provider(base).read().await.unwrap();
    let notes = status
        .notes
        .iter()
        .filter(|n| format!("{} {}", n.title, n.text).contains("add-on versions"))
        .count();
    assert_eq!(notes, 1, "{:?}", status.notes);
}

#[test]
fn a_region_that_could_redirect_the_endpoint_is_refused() {
    for ok in ["eu-west-1", "us-gov-east-1", "cn-north-1", "ap-southeast-2"] {
        assert!(valid_region(ok), "{ok}");
    }
    for bad in [
        "",
        "evil.com/x#",
        "eu-west-1.evil.com",
        "eu-west-1@evil.com",
        "eu-west",
        "EU-WEST-1",
        "eu-west-12",
        "e-west-1",
        "eu--west-1",
    ] {
        assert!(!valid_region(bad), "{bad}");
    }
    let hints = Hints {
        region: Some("attacker.example/".into()),
        cluster: Some("prod".into()),
        ..Hints::default()
    };
    let err = resolve(None, &hints, "ctx", "cluster", "https://x.example").unwrap_err();
    assert!(
        matches!(&err, ProviderError::Other(m) if m.contains("valid AWS region")),
        "{err:?}"
    );
}
