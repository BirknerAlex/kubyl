//! Account-wide discovery against fake `aws`, `az` and `gcloud` programs: shell scripts first on
//! the runner's search path that print the recorded-shape JSON of `tests/fixtures/cloud/` for
//! the exact commands Kubyl runs (the fixtures follow the CLIs' documented output; no cloud
//! account was available, so they aren't recordings of a live account) and fail the way the
//! real CLIs fail (expired SSO, missing permission, a disabled API, not logged in).
//!
//! Unix only: the fakes are `sh` scripts.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kubyl_kubeconfig_core::cloud::{
    Account, Cluster, Failure, Provider, SystemRunner, accounts, add_clusters, discover, scan,
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cloud")
}

/// Writes an executable `name` that runs `body` (a `case` over `"$*"`), with `$FIX` the fixtures.
fn fake(dir: &Path, name: &str, body: &str) {
    let script = format!("#!/bin/sh\nFIX='{}'\n{body}\n", fixtures().display());
    let path = dir.join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn runner(dir: &Path) -> SystemRunner {
    SystemRunner {
        timeout: Duration::from_secs(10),
        // `sh`, `cat` and `echo` come from the system; the fakes first.
        search_path: Some(format!("{}:/usr/bin:/bin", dir.display()).into()),
    }
}

const AWS: &str = r##"
case "$*" in
  "configure list-profiles") printf 'default\nprod\nbroken\n' ;;
  "configure get region --profile prod") echo eu-west-1 ;;
  "configure get region --profile broken") echo eu-west-1 ;;
  "configure get region --profile default") exit 1 ;;
  "sts get-caller-identity --profile prod --output json") cat "$FIX/aws-sts-get-caller-identity.json" ;;
  "sts get-caller-identity --profile default --output json") cat "$FIX/aws-sts-get-caller-identity.json" ;;
  "sts get-caller-identity --profile broken --output json")
    echo "The SSO session associated with this profile has expired or is otherwise invalid. To refresh this SSO session run aws sso login with the corresponding profile." >&2; exit 255 ;;
  "ec2 describe-regions --profile prod --region eu-west-1 --output json") cat "$FIX/aws-ec2-describe-regions.json" ;;
  "ec2 describe-regions --profile default --region us-east-1 --output json")
    echo "An error occurred (UnauthorizedOperation) when calling the DescribeRegions operation: You are not authorized to perform this operation." >&2; exit 254 ;;
  "eks list-clusters --region eu-west-1 --profile prod --output json") cat "$FIX/aws-eks-list-clusters-eu-west-1.json" ;;
  "eks list-clusters --region us-east-1 --profile prod --output json") cat "$FIX/aws-eks-list-clusters-empty.json" ;;
  "eks list-clusters --region eu-central-1 --profile prod --output json")
    echo "An error occurred (AccessDeniedException) when calling the ListClusters operation: User: arn:aws:sts::123456789012:assumed-role/Admin/alice is not authorized to perform: eks:ListClusters on resource: arn:aws:eks:eu-central-1:123456789012:cluster/*" >&2; exit 254 ;;
  "eks describe-cluster --name prod-eu --region eu-west-1 --profile prod --output json") cat "$FIX/aws-eks-describe-cluster-prod-eu.json" ;;
  "eks describe-cluster --name staging-eu --region eu-west-1 --profile prod --output json") cat "$FIX/aws-eks-describe-cluster-staging-eu.json" ;;
  eks\ update-kubeconfig*)
    file=""; prev=""
    for a in "$@"; do [ "$prev" = "--kubeconfig" ] && file="$a"; prev="$a"; done
    case "$*" in *staging-eu*) echo "An error occurred (AccessDeniedException): not authorized to perform: eks:DescribeCluster" >&2; exit 254 ;; esac
    echo "# aws $*" >> "$file" ;;
  *) echo "fake aws: unexpected arguments: $*" >&2; exit 2 ;;
esac
"##;

const AZ: &str = r##"
case "$*" in
  "account list --output json") cat "$FIX/az-account-list.json" ;;
  "aks list --subscription aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee --output json") cat "$FIX/az-aks-list-production.json" ;;
  "aks list --subscription ffffffff-0000-1111-2222-333333333333 --output json")
    echo "(AuthorizationFailed) The client 'alice@example.com' with object id '0000' does not have authorization to perform action 'Microsoft.ContainerService/managedClusters/read' over scope '/subscriptions/ffffffff-0000-1111-2222-333333333333' or the scope is invalid." >&2; exit 1 ;;
  aks\ get-credentials*)
    file=""; prev=""
    for a in "$@"; do [ "$prev" = "--file" ] && file="$a"; prev="$a"; done
    echo "# az $*" >> "$file" ;;
  *) echo "fake az: unexpected arguments: $*" >&2; exit 2 ;;
esac
"##;

const GCLOUD: &str = r##"
echo "prompts=$CLOUDSDK_CORE_DISABLE_PROMPTS" >> "${FAKE_LOG:-/dev/null}"
case "$*" in
  "projects list --format=json") cat "$FIX/gcloud-projects-list.json" ;;
  "container clusters list --project acme-analytics --format=json") cat "$FIX/gcloud-clusters-list-analytics.json" ;;
  "container clusters list --project acme-sandbox --format=json")
    echo "ERROR: (gcloud.container.clusters.list) ResponseError: code=403, message=Kubernetes Engine API has not been used in project acme-sandbox before or it is disabled. Enable it by visiting https://console.developers.google.com/apis/api/container.googleapis.com/overview?project=acme-sandbox then retry." >&2; exit 1 ;;
  container\ clusters\ get-credentials*) echo "# gcloud $* KUBECONFIG=$KUBECONFIG" >> "$KUBECONFIG" ;;
  *) echo "fake gcloud: unexpected arguments: $*" >&2; exit 2 ;;
esac
"##;

fn setup() -> (tempfile::TempDir, SystemRunner) {
    let dir = tempfile::tempdir().unwrap();
    fake(dir.path(), "aws", AWS);
    fake(dir.path(), "az", AZ);
    fake(dir.path(), "gcloud", GCLOUD);
    let runner = runner(dir.path());
    (dir, runner)
}

fn account(provider: Provider, id: &str) -> Account {
    Account {
        provider,
        id: id.into(),
        label: id.into(),
        detail: String::new(),
    }
}

#[tokio::test]
async fn aws_profiles_regions_and_clusters() {
    let (_dir, runner) = setup();
    let profiles = accounts(Provider::Aws, &runner).await.unwrap();
    let ids: Vec<&str> = profiles.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["default", "prod", "broken"]);

    let prod = scan(&runner, &profiles[1]).await;
    let found: Vec<(String, String, String, String)> = prod
        .clusters
        .iter()
        .map(|c| {
            (
                c.name.clone(),
                c.region.clone(),
                c.version.clone(),
                c.status.clone(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                "prod-eu".into(),
                "eu-west-1".into(),
                "1.31".into(),
                "ACTIVE".into()
            ),
            (
                "staging-eu".into(),
                "eu-west-1".into(),
                "1.30".into(),
                "UPDATING".into()
            ),
        ]
    );
    // The account id comes from sts; the region that wasn't opted into is skipped; one region
    // the profile may not list is a problem of that region, not of the scan.
    assert_eq!(prod.account.detail, "123456789012");
    assert_eq!(prod.problems.len(), 1, "{:?}", prod.problems);
    assert_eq!(prod.problems[0].scope, "eu-central-1");
    assert_eq!(
        prod.problems[0].failure,
        Failure::Permission {
            permission: "eks:ListClusters".into()
        }
    );
    assert!(
        prod.problems[0]
            .failure
            .to_string()
            .contains("Missing permission: eks:ListClusters")
    );
}

#[tokio::test]
async fn an_expired_sso_session_says_what_to_run() {
    let (_dir, runner) = setup();
    let broken = scan(&runner, &account(Provider::Aws, "broken")).await;
    assert!(broken.clusters.is_empty());
    let failure = &broken.problems[0].failure;
    assert_eq!(
        *failure,
        Failure::SignIn {
            command: "aws sso login --profile broken".into(),
            why: "The SSO session expired.".into()
        }
    );
    assert!(
        failure
            .to_string()
            .contains("Run `aws sso login --profile broken`"),
        "{failure}"
    );
}

#[tokio::test]
async fn a_profile_without_a_region_or_the_right_to_list_regions_says_how_to_set_one() {
    let (_dir, runner) = setup();
    let scan = scan(&runner, &account(Provider::Aws, "default")).await;
    let message = scan.problems[0].failure.to_string();
    assert!(
        message.contains("aws configure set region eu-west-1 --profile default"),
        "{message}"
    );
    assert!(message.contains("ec2:DescribeRegions"), "{message}");
}

#[tokio::test]
async fn azure_subscriptions_and_clusters() {
    let (_dir, runner) = setup();
    let subscriptions = accounts(Provider::Azure, &runner).await.unwrap();
    // The disabled one isn't offered.
    let names: Vec<&str> = subscriptions.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(names, ["Production", "Sandbox"]);
    assert_eq!(
        subscriptions[0].detail,
        "11111111-2222-3333-4444-555555555555"
    );

    let production = scan(&runner, &subscriptions[0]).await;
    let found: Vec<(&str, Option<&str>, &str, &str)> = production
        .clusters
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                c.group.as_deref(),
                c.region.as_str(),
                c.status.as_str(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            ("aks-prod", Some("rg-prod"), "westeurope", "Succeeded"),
            // A stopped cluster says so instead of "Succeeded".
            ("aks-batch", Some("rg-batch"), "northeurope", "Stopped"),
        ]
    );
    assert_eq!(production.clusters[0].version, "1.30.4");

    let sandbox = scan(&runner, &subscriptions[1]).await;
    assert_eq!(
        sandbox.problems[0].failure,
        Failure::Permission {
            permission: "Microsoft.ContainerService/managedClusters/read".into()
        }
    );
}

#[tokio::test]
async fn gcloud_projects_and_a_project_without_the_api() {
    let (_dir, runner) = setup();
    let projects = accounts(Provider::Google, &runner).await.unwrap();
    let ids: Vec<&str> = projects.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(
        ids,
        ["acme-analytics", "acme-sandbox"],
        "a project being deleted isn't offered"
    );

    let analytics = scan(&runner, &projects[0]).await;
    let found: Vec<(&str, &str, &str, &str)> = analytics
        .clusters
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                c.region.as_str(),
                c.version.as_str(),
                c.status.as_str(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            ("analytics", "europe-west4", "1.31.2-gke.1000", "RUNNING"),
            (
                "dev-zonal",
                "europe-west4-a",
                "1.30.5-gke.2000",
                "RECONCILING"
            ),
        ]
    );

    let sandbox = scan(&runner, &projects[1]).await;
    let failure = &sandbox.problems[0].failure;
    assert_eq!(
        *failure,
        Failure::ApiDisabled {
            project: "acme-sandbox".into()
        }
    );
    assert!(failure.is_benign());
    assert!(
        failure
            .to_string()
            .contains("gcloud services enable container.googleapis.com --project acme-sandbox")
    );
}

#[tokio::test]
async fn discovery_reports_each_account_as_it_finishes() {
    let (_dir, runner) = setup();
    let mut listed = Vec::new();
    let mut scans = Vec::new();
    discover(
        Provider::Google,
        &runner,
        |accounts| listed = accounts.iter().map(|a| a.id.clone()).collect(),
        |scan| scans.push(scan),
    )
    .await
    .unwrap();
    assert_eq!(listed, ["acme-analytics", "acme-sandbox"]);
    assert_eq!(scans.len(), 2);
    assert_eq!(scans.iter().map(|s| s.clusters.len()).sum::<usize>(), 2);
}

#[tokio::test]
async fn not_logged_in_to_azure_says_to_run_az_login() {
    let dir = tempfile::tempdir().unwrap();
    fake(
        dir.path(),
        "az",
        "echo \"ERROR: Please run 'az login' to setup account.\" >&2; exit 1",
    );
    let err = accounts(Provider::Azure, &runner(dir.path()))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        Failure::SignIn {
            command: "az login".into(),
            why: "Azure needs a sign-in (none yet, or it expired).".into()
        }
    );
    let dir = tempfile::tempdir().unwrap();
    fake(
        dir.path(),
        "gcloud",
        "echo 'ERROR: (gcloud.projects.list) You do not currently have an active account selected. Please run: $ gcloud auth login' >&2; exit 1",
    );
    let err = accounts(Provider::Google, &runner(dir.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Failure::SignIn { command, .. } if command == "gcloud auth login"),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_missing_cli_names_where_to_get_it() {
    let dir = tempfile::tempdir().unwrap();
    // Only the empty folder on the path: CI images ship `az` and `gcloud` in /usr/bin.
    let runner = SystemRunner {
        timeout: Duration::from_secs(10),
        search_path: Some(dir.path().as_os_str().to_owned()),
    };
    for provider in Provider::ALL {
        let err = accounts(provider, &runner).await.unwrap_err();
        assert_eq!(err, Failure::NotInstalled(provider));
        let message = err.to_string();
        assert!(
            message.contains(provider.cli()) && message.contains("login shell's PATH"),
            "{message}"
        );
        assert!(message.contains(provider.install_url()), "{message}");
    }
}

#[tokio::test]
async fn a_command_that_hangs_times_out() {
    let dir = tempfile::tempdir().unwrap();
    fake(dir.path(), "az", "sleep 5");
    let runner = SystemRunner {
        timeout: Duration::from_millis(300),
        search_path: Some(format!("{}:/usr/bin:/bin", dir.path().display()).into()),
    };
    let started = std::time::Instant::now();
    assert_eq!(
        accounts(Provider::Azure, &runner).await.unwrap_err(),
        Failure::Timeout
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn commands_never_wait_for_input_and_run_with_prompts_off() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log");
    // `cat` returns at once only when stdin is closed (the test harness's isn't).
    fake(
        dir.path(),
        "gcloud",
        &format!(
            "cat > /dev/null; echo \"prompts=$CLOUDSDK_CORE_DISABLE_PROMPTS pager=[$AWS_PAGER]\" > '{}'; echo '[]'",
            log.display()
        ),
    );
    let runner = SystemRunner {
        timeout: Duration::from_secs(5),
        search_path: Some(format!("{}:/usr/bin:/bin", dir.path().display()).into()),
    };
    assert!(
        accounts(Provider::Google, &runner)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        std::fs::read_to_string(log).unwrap().trim(),
        "prompts=1 pager=[]"
    );
}

fn cluster(
    provider: Provider,
    account: &str,
    name: &str,
    region: &str,
    group: Option<&str>,
) -> Cluster {
    Cluster {
        provider,
        account: account.into(),
        name: name.into(),
        region: region.into(),
        group: group.map(str::to_string),
        version: String::new(),
        status: String::new(),
    }
}

#[tokio::test]
async fn chosen_clusters_are_added_with_the_get_credentials_commands() {
    let (dir, runner) = setup();
    let file = dir.path().join("config");
    let chosen = [
        cluster(Provider::Aws, "prod", "prod-eu", "eu-west-1", None),
        cluster(
            Provider::Google,
            "acme-analytics",
            "analytics",
            "europe-west4",
            None,
        ),
        cluster(
            Provider::Azure,
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "aks-prod",
            "westeurope",
            Some("rg-prod"),
        ),
        // The one the profile may not describe.
        cluster(Provider::Aws, "prod", "staging-eu", "eu-west-1", None),
    ];
    let added = add_clusters(&runner, &chosen, &file).await;
    assert_eq!(added.added.len(), 3);
    assert_eq!(added.failed.len(), 1);
    assert_eq!(added.failed[0].0.name, "staging-eu");
    assert_eq!(
        added.failed[0].1,
        Failure::Permission {
            permission: "eks:DescribeCluster".into()
        }
    );
    let text = added.kubeconfig.unwrap();
    // Each CLI wrote into the same file, with the account of the cluster.
    let f = file.display();
    assert!(text.contains(&format!("# aws eks update-kubeconfig --name prod-eu --region eu-west-1 --kubeconfig {f} --profile prod")), "{text}");
    assert!(text.contains(&format!("# gcloud container clusters get-credentials analytics --location europe-west4 --project acme-analytics KUBECONFIG={f}")), "{text}");
    assert!(text.contains(&format!("# az aks get-credentials --name aks-prod --resource-group rg-prod --file {f} --subscription aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")), "{text}");
    assert!(!text.contains("staging-eu"));

    // Nothing added: no kubeconfig.
    let none = add_clusters(&runner, &chosen[3..], &dir.path().join("other")).await;
    assert!(none.kubeconfig.is_none() && none.added.is_empty());
}
