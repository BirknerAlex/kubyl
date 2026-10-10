//! The cost code against responses recorded from OpenCost 1.121.3 (chart 2.5.32) on the kind dev
//! cluster, with `script/opencost-dev.sh` (custom on-prem rates): `allocation-24h-accumulated.json`
//! and `allocation-24h-steps.json` (24 hourly sets, most of them empty: the cluster was young),
//! trimmed to the fields Kubyl reads, and `service-opencost.json` (the chart's Service). A tiny
//! local HTTP server plays OpenCost for the request side (path and parameters).

use kubyl_cost_core::detect::{self, ServiceInfo};
use kubyl_cost_core::fetch::{self, CostError};
use kubyl_cost_core::model::{self, IDLE};
use kubyl_cost_core::settings::Endpoint;
use kubyl_cost_core::summary;
use kubyl_cost_core::window::{Query, Window};
use kubyl_metrics_core::transport::Transport;
use serde_json::Value;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap()
}

#[test]
fn the_accumulated_window_gives_totals_and_rows() {
    let sets = model::parse(&fixture("allocation-24h-accumulated.json")).unwrap();
    assert_eq!(sets.len(), 1);
    let set = &sets[0];
    assert!(set.iter().any(|a| a.name == "kube-system"));
    // Idle comes last; the namespaces are by cost.
    assert!(set.last().unwrap().is_idle());
    let costs: Vec<f64> = set
        .iter()
        .filter(|a| !a.is_idle())
        .map(|a| a.total_cost)
        .collect();
    assert!(costs.windows(2).all(|w| w[0] >= w[1]), "{costs:?}");
    let totals = summary::totals(set);
    assert!(totals.total > 0.0 && totals.cpu > 0.0 && totals.ram > 0.0);
    assert!(
        totals.cpu + totals.ram + totals.storage + totals.network + totals.other
            <= totals.total + 1e-4,
        "OpenCost rounds to five decimals"
    );
    // Usage over request can exceed 1 (pods that request little use more: kube-system's memory
    // was at 258% of what it requested).
    assert!(totals.ram_efficiency > 0.0 && totals.cpu_efficiency >= 0.0);
    let rows = summary::rows(set);
    assert_eq!(rows.len(), set.len());
    assert_eq!(rows.last().unwrap().label(), "idle");
    assert!(rows.last().unwrap().efficiency.is_none());
    let csv = kubyl_base::csv::write(
        &kubyl_cost_core::table::header(),
        &kubyl_cost_core::table::records(&rows),
    );
    assert_eq!(csv.lines().count(), rows.len() + 1);
    assert!(csv.contains("kube-system,"), "{csv}");
}

#[test]
fn the_steps_make_a_chart_with_gaps_for_empty_hours() {
    let sets = model::parse(&fixture("allocation-24h-steps.json")).unwrap();
    assert_eq!(sets.len(), 24);
    let series = summary::series(&sets, Window::Day.step_seconds(), 6);
    assert_eq!(series.times.len(), 24);
    // Hourly, even over the empty steps.
    assert!(
        series
            .times
            .windows(2)
            .all(|w| (w[1] - w[0] - 3600.0).abs() < 1e-6),
        "{:?}",
        series.times
    );
    assert!(!series.is_empty());
    assert!(series.stacks.iter().all(|(_, v)| v.len() == 24));
    let names: Vec<&str> = series.stacks.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"kube-system"), "{names:?}");
    if names.contains(&IDLE) {
        assert_eq!(*names.last().unwrap(), IDLE, "idle is on top");
    }
}

#[test]
fn the_charts_service_is_found_by_its_label_and_the_api_port() {
    let service = fixture("service-opencost.json");
    let info = ServiceInfo {
        namespace: service["namespace"].as_str().unwrap().into(),
        name: service["name"].as_str().unwrap().into(),
        app_labels: vec![
            service["labels"]["app.kubernetes.io/name"]
                .as_str()
                .unwrap()
                .into(),
        ],
        ports: service["ports"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p[0].as_str().unwrap().to_string(),
                    p[1].as_u64().unwrap() as u16,
                )
            })
            .collect(),
    };
    let found = detect::find(&[info]).unwrap();
    assert_eq!(found.to_string(), "opencost/opencost:9003");
}

/// Serves `body` once and returns the request line it saw.
async fn serve_once(body: String) -> (u16, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        request.lines().next().unwrap_or_default().to_string()
    });
    (port, task)
}

#[tokio::test]
async fn the_request_asks_for_namespaces_with_the_chosen_window_and_idle() {
    let body = fixture("allocation-24h-steps.json").to_string();
    let (port, seen) = serve_once(body).await;
    let transport = Transport::loopback(port, "").unwrap();
    let endpoint: Endpoint = "opencost/opencost:9003".parse().unwrap();
    let query = Query {
        window: Window::Day,
        include_idle: true,
        accumulate: false,
    };
    let sets = fetch::allocations(&transport, &endpoint, &query)
        .await
        .unwrap();
    assert_eq!(sets.len(), 24);
    let line = seen.await.unwrap();
    assert!(line.starts_with("GET /allocation/compute?"), "{line}");
    for expected in [
        "window=24h",
        "aggregate=namespace",
        "includeIdle=true",
        "accumulate=false",
        "step=1h",
    ] {
        assert!(line.contains(expected), "{expected} missing from {line}");
    }
}

#[tokio::test]
async fn an_error_response_names_prometheus() {
    let body =
        r#"{"code": 500, "message": "error querying Prometheus: connection refused"}"#.to_string();
    let (port, _seen) = serve_once(body).await;
    let transport = Transport::loopback(port, "").unwrap();
    let endpoint: Endpoint = "opencost/opencost:9003".parse().unwrap();
    let query = Query {
        window: Window::Week,
        include_idle: false,
        accumulate: true,
    };
    let err = fetch::allocations(&transport, &endpoint, &query)
        .await
        .unwrap_err();
    assert!(matches!(err, CostError::OpenCost(_)));
    assert!(
        err.to_string().contains("OpenCost needs the Prometheus"),
        "{err}"
    );
}
