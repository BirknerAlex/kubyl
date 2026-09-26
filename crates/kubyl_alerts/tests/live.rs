//! Alerts against a real cluster. Ignored by default; run against the kind dev cluster:
//!
//! ```sh
//! script/dev-cluster.sh && script/prometheus-dev.sh && script/alertmanager-dev.sh
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig \
//!   cargo test -p kubyl_alerts --test live -- --ignored --nocapture --test-threads=1
//! ```
//!
//! The auth-proxy test starts `kubectl port-forward` (kubectl must be on the PATH).

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams, LogParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};
use kubyl_alerts::client::{self, AmConn, Credentials, Probed};
use kubyl_alerts::discover::AmTarget;
use kubyl_alerts::matchers::{MatchOp, Matcher};
use kubyl_alerts::merge::{self, Heartbeat, MergeOptions};
use kubyl_alerts::model::{self, AlertState, ParseOptions};
use kubyl_alerts::settings::{AlertmanagerConfig, ClusterSettings};
use kubyl_metrics::discover::{self as prom_discover, Found};
use kubyl_metrics::prometheus::PromClient;

fn kubeconfig_path() -> String {
    std::env::var("KUBYL_TEST_KUBECONFIG").expect("set KUBYL_TEST_KUBECONFIG")
}

fn context() -> String {
    std::env::var("KUBYL_TEST_CONTEXT").unwrap_or("kind-kubyl-dev".into())
}

async fn client() -> Client {
    let kubeconfig = Kubeconfig::read_from(kubeconfig_path()).unwrap();
    let options = KubeConfigOptions {
        context: Some(context()),
        ..Default::default()
    };
    let config = Config::from_custom_kubeconfig(kubeconfig, &options)
        .await
        .unwrap();
    Client::try_from(config).unwrap()
}

async fn prometheus(client: &Client) -> PromClient {
    match prom_discover::discover(client).await {
        Found::Prometheus(prom) => prom,
        Found::Nothing { best } => panic!("no Prometheus found: {best:?}"),
    }
}

fn no_credentials() -> Credentials {
    Credentials {
        user_token: None,
        headers: Vec::new(),
    }
}

async fn main_alertmanager(client: &Client) -> AmConn {
    let target = AmTarget::service(
        "monitoring",
        "kube-prometheus-stack-alertmanager",
        "9093",
        "http",
        "",
    );
    match client::probe(client, &target, &no_credentials()).await {
        Probed::Connected(conn) => *conn,
        other => panic!("{other:?}"),
    }
}

/// Authorization headers the look-alike logged (it logs every request).
async fn look_alike_log(client: &Client) -> String {
    let pods: Api<Pod> = Api::namespaced(client.clone(), "monitoring-evil");
    let list = pods
        .list(&ListParams::default().labels("app=alertmanager-main"))
        .await
        .unwrap();
    let mut out = String::new();
    for pod in list {
        let name = pod.metadata.name.unwrap_or_default();
        out.push_str(
            &pods
                .logs(&name, &LogParams::default())
                .await
                .unwrap_or_default(),
        );
    }
    out
}

#[tokio::test]
#[ignore]
async fn discovery_finds_the_alertmanagers_and_the_look_alike_gets_no_token() {
    let client = client().await;
    let prom = prometheus(&client).await;
    let settings = ClusterSettings::default();
    // A token as an `oc login` user would have one: it must not reach the look-alike.
    let credentials = Credentials {
        user_token: Some(kubyl_kube::auth::BearerToken::Static(
            "kubyl-live-test-token".to_string().into(),
        )),
        headers: Vec::new(),
    };
    let found = client::discover(&client, &settings, true, Some(&prom), &credentials).await;
    for tried in &found.tried {
        println!("{} ({}): {:?}", tried.label, tried.found_by, tried.error);
    }
    let labels: Vec<String> = found.connected.iter().map(|c| c.label()).collect();
    println!("connected: {labels:?}");
    assert!(
        labels
            .iter()
            .any(|l| l == "monitoring/kube-prometheus-stack-alertmanager"),
        "{labels:?}"
    );
    let prefixed = found
        .connected
        .iter()
        .find(|c| c.label() == "team-am/prefixed-alertmanager")
        .expect("the routePrefix Alertmanager");
    assert!(matches!(&prefixed.target, AmTarget::Service { path, .. } if path == "/am"));
    assert!(
        found
            .tried
            .iter()
            .any(|t| t.label == "monitoring-evil/alertmanager-main" && t.error.is_some()),
        "the look-alike is tried and fails"
    );
    assert!(
        found.forwards.is_empty(),
        "no forward to untrusted Services"
    );
    let log = look_alike_log(&client).await;
    assert!(
        log.contains("/api/v2/status"),
        "the look-alike saw the probe:\n{log}"
    );
    assert!(
        !log.contains("Bearer"),
        "the look-alike got a token:\n{log}"
    );
    assert!(!log.contains("kubyl-live-test-token"));
}

#[tokio::test]
#[ignore]
async fn firing_and_pending_alerts_have_the_right_times() {
    let client = client().await;
    let prom = prometheus(&client).await;
    let am = main_alertmanager(&client).await;
    let body = am.alerts(&[]).await.unwrap();
    let severities = BTreeMap::new();
    let node_of = |_: &str| None;
    let parse = ParseOptions {
        severity_label: "severity",
        severities: &severities,
        node_of: &node_of,
    };
    let am_alerts = model::parse_am_alerts(&body, &am.label(), &parse);
    let prom_alerts =
        model::parse_prom_alerts(&prom.api("/api/v1/alerts", &[]).await.unwrap()).unwrap();
    let rules = model::parse_rules(
        &prom
            .api("/api/v1/rules", &[("type", "alert".to_string())])
            .await
            .unwrap(),
    )
    .unwrap();
    let heartbeat = vec!["Watchdog".to_string()];
    let hidden = vec!["InfoInhibitor".to_string()];
    let merged = merge::merge(
        Some(&am_alerts),
        Some(&prom_alerts),
        &rules,
        Some(1),
        &MergeOptions {
            heartbeat_alerts: &heartbeat,
            hidden_alerts: &hidden,
            parse: &parse,
        },
        jiff::Timestamp::now(),
    );
    let critical = merged
        .alerts
        .iter()
        .find(|a| a.name == "KubylDevCritical")
        .expect("KubylDevCritical fires (wait a minute after alertmanager-dev.sh)");
    let raw = am_alerts
        .iter()
        .find(|a| a.name == "KubylDevCritical")
        .unwrap();
    assert_eq!(critical.starts_at, raw.starts_at, "firing since = startsAt");
    assert!(!critical.starts_approx);
    assert_eq!(critical.state, AlertState::Firing);
    assert_eq!(
        critical.target.as_ref().map(|t| t.kind),
        Some(model::TargetKind::Pod)
    );
    assert!(critical.runbook_url().is_some());
    assert!(critical.receivers.iter().any(|r| r == "pagerduty-payments"));
    let pending = merged
        .alerts
        .iter()
        .find(|a| a.name == "KubylDevPending")
        .expect("KubylDevPending is pending");
    assert_eq!(pending.state, AlertState::Pending);
    assert!(pending.active_at.is_some());
    let inhibited = merged
        .alerts
        .iter()
        .find(|a| a.name == "KubylDevInhibited")
        .expect("KubylDevInhibited");
    assert_eq!(inhibited.state, AlertState::Inhibited);
    assert!(
        matches!(merged.heartbeat, Some(Heartbeat::Ok(_))),
        "{:?}",
        merged.heartbeat
    );
    assert!(!merged.alerts.iter().any(|a| a.name == "Watchdog"));
}

#[tokio::test]
#[ignore]
async fn rules_parse_with_health() {
    let client = client().await;
    let prom = prometheus(&client).await;
    let rules = model::parse_rules(
        &prom
            .api(
                "/api/v1/rules",
                &[
                    ("type", "alert".to_string()),
                    ("exclude_alerts", "true".to_string()),
                ],
            )
            .await
            .unwrap(),
    )
    .unwrap();
    let count: usize = rules.iter().map(|g| g.rules.len()).sum();
    println!("{} groups, {count} alerting rules", rules.len());
    assert!(count > 50, "kube-prometheus-stack's rules");
    let broken = rules
        .iter()
        .flat_map(|g| &g.rules)
        .find(|r| r.name == "KubylDevBrokenRule")
        .expect("the broken rule");
    assert!(broken.failing(), "{broken:?}");
    let pending = rules
        .iter()
        .flat_map(|g| &g.rules)
        .find(|r| r.name == "KubylDevPending")
        .unwrap();
    assert_eq!(pending.duration, 3600.0);
}

#[tokio::test]
#[ignore]
async fn a_silence_round_trip() {
    let client = client().await;
    let am = main_alertmanager(&client).await;
    let matchers = vec![
        Matcher::new("alertname", MatchOp::Equal, "KubylDevCritical"),
        Matcher::new("namespace", MatchOp::Equal, "payments"),
    ];
    let now = jiff::Timestamp::now();
    let body = client::silence_body(
        None,
        &matchers,
        now,
        now.checked_add(jiff::SignedDuration::from_mins(10))
            .unwrap(),
        "kubyl-live-test",
        "Kubyl live test",
    );
    let id = am.post_silence(&body).await.unwrap();
    println!("silence {id}");
    let silenced = |alerts: &serde_json::Value| {
        alerts.as_array().unwrap().iter().any(|a| {
            a["labels"]["alertname"] == "KubylDevCritical"
                && a["status"]["silencedBy"]
                    .as_array()
                    .is_some_and(|s| s.iter().any(|v| v == id.as_str()))
        })
    };
    let mut seen = false;
    for _ in 0..20 {
        if silenced(&am.alerts(&[]).await.unwrap()) {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(seen, "the alert is silenced within 10 s");
    let silences = model::parse_silences(&am.silences(&[]).await.unwrap(), &am.label());
    assert!(silences.iter().any(|s| s.id == id && s.state == "active"));
    am.expire_silence(&id).await.unwrap();
    let mut back = false;
    for _ in 0..20 {
        if !silenced(&am.alerts(&[]).await.unwrap()) {
            back = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(back, "expiring brings the alert back");
}

/// `kubectl port-forward` to a local port (what Kubyl's temporary forward does in the app).
struct PortForward(Child, u16);

impl Drop for PortForward {
    fn drop(&mut self) {
        self.0.kill().ok();
    }
}

fn port_forward(namespace: &str, target: &str, port: u16) -> PortForward {
    let mut child = Command::new("kubectl")
        .args([
            "--kubeconfig",
            &kubeconfig_path(),
            "--context",
            &context(),
            "-n",
            namespace,
            "port-forward",
            target,
            &format!(":{port}"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("kubectl");
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    // Keep reading: kubectl exits once its stdout is closed ("Handling connection for …").
    std::thread::spawn(move || {
        let mut sink = String::new();
        while reader.read_line(&mut sink).is_ok_and(|n| n > 0) {
            sink.clear();
        }
    });
    // "Forwarding from 127.0.0.1:54321 -> 9443"
    let local = line
        .split("127.0.0.1:")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("unexpected port-forward output: {line}"));
    PortForward(child, local)
}

#[tokio::test]
#[ignore]
async fn the_auth_proxy_alertmanager_works_through_a_forward_with_a_service_account() {
    let client = client().await;
    let config = AlertmanagerConfig {
        namespace: Some("team-secure".into()),
        service: Some("secured-alertmanager".into()),
        port: Some("https".into()),
        scheme: Some("https".into()),
        service_account: Some("team-secure/am-reader".into()),
        ..Default::default()
    };
    let target = kubyl_alerts::discover::from_settings(&[config])
        .remove(0)
        .target;
    // Through the API server the proxy strips credentials: 401, and no Route on kind.
    match client::probe(&client, &target, &no_credentials()).await {
        Probed::NeedsForward(_, why) => println!("needs a forward: {why}"),
        other => panic!("{other:?}"),
    }
    // Not named in settings: never a forward (and never a token).
    let untrusted = AmTarget::service("team-secure", "secured-alertmanager", "https", "https", "");
    assert!(matches!(
        client::probe(&client, &untrusted, &no_credentials()).await,
        Probed::Failed(..)
    ));
    assert!(matches!(
        client::probe_forward(&client, &untrusted, 1, &no_credentials()).await,
        Probed::Failed(..)
    ));
    let forward = port_forward("team-secure", "svc/secured-alertmanager", 9443);
    match client::probe_forward(&client, &target, forward.1, &no_credentials()).await {
        Probed::Connected(conn) => {
            println!("connected: {:?} as {:?}", conn.via, conn.service_account());
            assert_eq!(
                conn.service_account().as_deref(),
                Some("team-secure/am-reader")
            );
            let alerts = conn.alerts(&[]).await.unwrap();
            assert!(alerts.is_array());
        }
        other => panic!("{other:?}"),
    }
}
