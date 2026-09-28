//! Network flow backends against the kind clusters of `script/netflow-dev.sh`. Ignored by
//! default; run one backend at a time against its cluster:
//!
//! ```sh
//! script/netflow-dev.sh --cilium        # or --cilium --relay-tls for Relay with server TLS
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/cilium-kubeconfig \
//!   cargo test -p kubyl_netflow --test live hubble -- --ignored --nocapture
//!
//! script/netflow-dev.sh --calico
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/calico-kubeconfig \
//!   cargo test -p kubyl_netflow --test live whisker -- --ignored --nocapture
//!
//! script/netflow-dev.sh --netobserv
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/netobserv-kubeconfig \
//!   cargo test -p kubyl_netflow --test live netobserv -- --ignored --nocapture
//! ```
//!
//! Each test detects its backend like the app does, reaches it the way the app does (a
//! loopback forward through `kubyl_portforward`'s listener for Hubble and Whisker, the API
//! server's service proxy for Loki and Prometheus), streams the fixture traffic for a while and checks the corrected
//! acceptance criteria: flows of both namespaces, the isolated `payments/ledger-api` and, where
//! the CNI has deny rules, the named `storefront/web-guard`; filtering down to one pod and to
//! the blocked flows. Flow contents are only asserted on, never printed.

use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use kube::Client;
use kubyl_netflow::detect::{self, Candidate, Inputs, RelayTls, Served};
use kubyl_netflow::filter::FlowFilter;
use kubyl_netflow::model::Flow;
use kubyl_netflow::provider::{FlowProvider, StreamEvent, StreamQuery};
use kubyl_netflow::settings::ClusterNetflowSettings;
use kubyl_portforward::listener::{self, ForwardEvent};
use kubyl_portforward::resolve::{ForwardKind, RemotePort};

async fn client() -> Client {
    let path = std::env::var("KUBYL_TEST_KUBECONFIG").expect("KUBYL_TEST_KUBECONFIG");
    let kubeconfig = kube::config::Kubeconfig::read_from(&path).expect("kubeconfig");
    let config = kube::Config::from_custom_kubeconfig(kubeconfig, &Default::default())
        .await
        .expect("config");
    Client::try_from(config).expect("client")
}

/// Served groups, like the app gets them from discovery.
async fn served(client: &Client) -> Served {
    let groups = client.list_api_groups().await.expect("api groups");
    let mut served = Served::default();
    for group in groups.groups {
        let name = group.name.as_str();
        match name {
            "flows.netobserv.io" => {
                served.flow_collectors = group.preferred_version.map(|v| v.version);
            }
            "cilium.io" => served.cilium = true,
            "crd.projectcalico.org" | "projectcalico.org" => served.calico = true,
            "k8s.ovn.org" => served.ovn = true,
            n if n.ends_with("antrea.io") => served.antrea = true,
            _ => {}
        }
    }
    served
}

async fn detection(client: &Client) -> detect::Detection {
    let version = client
        .apiserver_version()
        .await
        .expect("version")
        .git_version;
    detect::detect(Inputs {
        client: client.clone(),
        served: served(client).await,
        git_version: version,
        settings: ClusterNetflowSettings::default(),
    })
    .await
}

/// A loopback forward to a Service port, like the app's ephemeral forward.
async fn forward(client: &Client, namespace: &str, service: &str, port: u16) -> u16 {
    let (tx, mut rx) = mpsc::unbounded();
    let (client, namespace, service) = (client.clone(), namespace.to_string(), service.to_string());
    tokio::spawn(async move {
        listener::run(
            client,
            namespace,
            ForwardKind::Service { service },
            RemotePort::Service(Some(port)),
            "127.0.0.1".into(),
            0,
            false,
            tx,
        )
        .await
        .expect("forward");
    });
    loop {
        match rx.next().await.expect("forward events") {
            ForwardEvent::Listening { local_port } => return local_port,
            ForwardEvent::Error(err) => panic!("forward failed: {err}"),
            _ => {}
        }
    }
}

/// Streams `query` for `seconds` (at least until the history arrived) and returns the flows.
async fn collect(provider: &dyn FlowProvider, query: StreamQuery, seconds: u64) -> Vec<Flow> {
    let (tx, mut rx) = mpsc::channel(64);
    let stream = provider.stream(query, tx);
    let task = tokio::spawn(stream);
    let mut flows = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        match tokio::time::timeout_at(deadline, rx.next()).await {
            Err(_) | Ok(None) => break,
            Ok(Some(StreamEvent::Flows(batch))) => flows.extend(batch),
            Ok(Some(_)) => {}
        }
    }
    task.abort();
    flows
}

fn since(seconds: i64) -> Option<jiff::Timestamp> {
    jiff::Timestamp::now()
        .checked_sub(jiff::SignedDuration::from_secs(seconds))
        .ok()
}

fn count(flows: &[Flow], filter: &str) -> usize {
    let filter = FlowFilter::parse(filter).expect("filter");
    flows.iter().filter(|f| filter.matches(f)).count()
}

#[tokio::test]
#[ignore = "needs the kind cluster of script/netflow-dev.sh --cilium"]
async fn hubble_streams_names_policies_and_filters() {
    let client = client().await;
    let detection = detection(&client).await;
    let Some(Candidate::Hubble {
        namespace,
        service,
        port,
        tls,
    }) = detection
        .find(kubyl_netflow::provider::BackendKind::Hubble)
        .cloned()
    else {
        panic!("no Hubble Relay found: {:?}", detection.checks);
    };
    let tls = match tls {
        RelayTls::Plain => None,
        RelayTls::Server {
            ca: Some(ca),
            server_name,
            ..
        } => Some((ca, server_name)),
        other => panic!("Relay TLS isn't usable: {other:?}"),
    };
    println!(
        "Relay {namespace}/{service}:{port}, {}",
        if tls.is_some() { "TLS" } else { "plain" }
    );
    let local_port = forward(&client, &namespace, &service, port).await;
    let hubble = kubyl_netflow::backends::hubble::Hubble {
        target: kubyl_netflow::backends::hubble::HubbleTarget {
            namespace,
            service,
            port,
            tls,
        },
        local_port,
        keep_query_values: false,
    };
    let status = hubble.probe().await.expect("probe");
    println!(
        "Relay {:?}, nodes {:?}, buffered {:?}",
        status.version, status.nodes, status.buffered
    );
    assert!(
        status
            .nodes
            .is_some_and(|(connected, total)| connected == total && total >= 1)
    );

    let flows = collect(
        &hubble,
        StreamQuery {
            filter: FlowFilter::default(),
            since: since(60),
        },
        12,
    )
    .await;
    println!("{} flows", flows.len());
    assert!(flows.len() > 100, "{}", flows.len());
    assert!(
        count(
            &flows,
            "src.ns=storefront dst.ns=payments verdict=forwarded"
        ) > 0
    );
    // Isolated: dropped with no policy named.
    let isolated = count(
        &flows,
        "src.workload=shopper dst.workload=ledger-api verdict=dropped policy=isolated",
    );
    assert!(isolated > 0, "no isolated drops");
    // The deny rule, named on every drop of the scraper (carried over to drop notifications).
    let denied = count(
        &flows,
        "src.workload=scraper dst.workload=web verdict=dropped",
    );
    let named = count(
        &flows,
        "src.workload=scraper dst.workload=web verdict=dropped policy=storefront/web-guard",
    );
    assert!(denied > 0 && named == denied, "{named} of {denied} named");
    // Allowed by the NetworkPolicy.
    assert!(
        count(
            &flows,
            "src.workload=checkout-client dst.workload=ledger-api policy=ledger-api-isolation"
        ) > 0
    );
    // L7 with the query values hidden.
    let http: Vec<&Flow> = flows
        .iter()
        .filter(|f| {
            FlowFilter::parse("proto=http http.path=/search")
                .unwrap()
                .matches(f)
        })
        .collect();
    assert!(!http.is_empty(), "no HTTP flows");
    for flow in &http {
        let summary = flow.protocol_label();
        assert!(!summary.contains("not-a-real-token"), "{summary}");
    }

    // Filtering server-side: one pod, and the blocked flows.
    let pushed =
        hubble.pushdown(&FlowFilter::parse("src.workload=scraper verdict=dropped").unwrap());
    assert_eq!(pushed.terms.len(), 2);
    let scraper = collect(
        &hubble,
        StreamQuery {
            filter: pushed,
            since: since(30),
        },
        6,
    )
    .await;
    assert!(!scraper.is_empty());
    assert_eq!(
        count(&scraper, "src.workload=scraper verdict=dropped"),
        scraper.len()
    );
}

#[tokio::test]
#[ignore = "needs the kind cluster of script/netflow-dev.sh --netobserv"]
async fn netobserv_reads_loki_and_metrics() {
    use kubyl_netflow::aggregate::Zoom;
    use kubyl_netflow::backends::netobserv::NetObserv;
    use kubyl_netflow::detect::{LokiTarget, PromTarget};

    let client = client().await;
    let detection = detection(&client).await;
    // kindnet: no Hubble, no Whisker; the hint knows the CNI.
    assert!(
        detection
            .find(kubyl_netflow::provider::BackendKind::Hubble)
            .is_none()
    );
    assert_eq!(detection.cni, Some(detect::Cni::Kindnet));
    let Some(Candidate::NetObserv { loki, prometheus }) = detection
        .find(kubyl_netflow::provider::BackendKind::NetObserv)
        .cloned()
    else {
        panic!("no NetObserv found: {:?}", detection.checks);
    };
    let LokiTarget::Service(loki) = loki else {
        panic!("Loki isn't a Service: {loki:?}");
    };
    let PromTarget::Service(prom) = prometheus else {
        panic!("Prometheus isn't named by the FlowCollector: {prometheus:?}");
    };
    let netobserv = NetObserv {
        loki: NetObserv::loki_target(client.clone(), &loki),
        prometheus: Some(kubyl_metrics::prometheus::PromClient::new(
            client.clone(),
            kubyl_metrics::prometheus::Target::Service {
                namespace: prom.namespace.clone(),
                service: prom.service.clone(),
                port: prom.port.clone(),
                scheme: prom.scheme.clone(),
                path: prom.path.clone(),
            },
        )),
    };
    let status = netobserv.probe().await.expect("probe");
    println!(
        "{} · {} · notes {:?}",
        status.endpoint, status.via, status.notes
    );
    assert!(status.notes.is_empty(), "{:?}", status.notes);

    let flows = collect(
        &netobserv,
        StreamQuery {
            filter: FlowFilter::default(),
            since: since(120),
        },
        12,
    )
    .await;
    println!("{} records", flows.len());
    assert!(flows.len() > 50, "{}", flows.len());
    assert!(count(&flows, "src.ns=storefront dst.ns=payments") > 0);
    // kindnet reports no drop for the isolation: the calls never get past SYN.
    let isolated = count(
        &flows,
        "src.workload=shopper dst.ns=payments dst.service=ledger-api verdict=no-reply",
    );
    assert!(isolated > 0, "no unanswered calls to ledger-api");
    assert_eq!(
        count(
            &flows,
            "src.workload=checkout-client dst.service=ledger-api verdict=no-reply"
        ),
        0
    );

    // Server-side: one pod's flows only.
    let pushed =
        netobserv.pushdown(&FlowFilter::parse("src.workload=scraper verdict=forwarded").unwrap());
    assert_eq!(pushed.canonical(), "src.workload=scraper");
    let scraper = collect(
        &netobserv,
        StreamQuery {
            filter: pushed,
            since: since(60),
        },
        6,
    )
    .await;
    assert!(!scraper.is_empty());
    assert_eq!(count(&scraper, "src.workload=scraper"), scraper.len());

    // The topology from metrics alone.
    let graph = netobserv
        .graph(
            Duration::from_secs(600),
            Zoom::Namespaces,
            FlowFilter::default(),
        )
        .await
        .expect("graph")
        .expect("metrics");
    let storefront = graph
        .nodes
        .iter()
        .position(|n| n.id == "ns:storefront")
        .expect("storefront");
    let payments = graph
        .nodes
        .iter()
        .position(|n| n.id == "ns:payments")
        .expect("payments");
    assert!(
        graph
            .edges
            .iter()
            .any(|e| e.source == storefront && e.target == payments && e.bytes > 0)
    );
    let workloads = netobserv
        .graph(
            Duration::from_secs(600),
            Zoom::Workloads,
            FlowFilter::parse("ns=storefront").unwrap(),
        )
        .await
        .expect("graph")
        .expect("metrics");
    assert!(
        workloads
            .nodes
            .iter()
            .any(|n| n.id == "wl:storefront/shopper")
    );
    assert!(workloads.edges.iter().all(|e| {
        let (a, b) = (&workloads.nodes[e.source], &workloads.nodes[e.target]);
        a.namespace.as_deref() == Some("storefront") || b.namespace.as_deref() == Some("storefront")
    }));
}

#[tokio::test]
#[ignore = "needs the kind cluster of script/netflow-dev.sh --calico"]
async fn whisker_streams_policy_traces_and_filters() {
    use kubyl_netflow::backends::whisker::Whisker;

    let client = client().await;
    let detection = detection(&client).await;
    let Some(Candidate::Whisker(target)) = detection
        .find(kubyl_netflow::provider::BackendKind::Whisker)
        .cloned()
    else {
        panic!("no Calico Whisker found: {:?}", detection.checks);
    };
    // Through a forward, like the app: Calico's policy keeps the service proxy out.
    let local_port = forward(
        &client,
        &target.namespace,
        &target.service,
        target.port.parse().expect("port"),
    )
    .await;
    let whisker = Whisker {
        client: client.clone(),
        target,
        local_port,
    };
    let status = whisker.probe().await.expect("probe");
    println!(
        "{} · {} · {:?}",
        status.endpoint, status.via, status.version
    );

    // Records are aggregated over 15 s: history first, then a live interval.
    let flows = collect(
        &whisker,
        StreamQuery {
            filter: FlowFilter::default(),
            since: since(300),
        },
        20,
    )
    .await;
    println!("{} records", flows.len());
    assert!(flows.len() > 10, "{}", flows.len());
    assert!(
        count(
            &flows,
            "src.ns=storefront dst.ns=payments verdict=forwarded"
        ) > 0
    );
    // Isolated by the NetworkPolicy: Calico's end-of-tier trigger names it.
    let isolated = count(
        &flows,
        "src.workload=shopper dst.workload=ledger-api verdict=dropped policy=isolated",
    );
    let named = count(
        &flows,
        "src.workload=shopper dst.workload=ledger-api verdict=dropped policy=ledger-api-isolation",
    );
    assert!(
        isolated > 0 && named == isolated,
        "{named} of {isolated} named"
    );
    // The Deny rule.
    let denied = count(
        &flows,
        "src.workload=scraper dst.workload=web verdict=dropped",
    );
    let by_guard = count(
        &flows,
        "src.workload=scraper dst.workload=web verdict=dropped policy=web-guard",
    );
    assert!(
        denied > 0 && by_guard == denied,
        "{by_guard} of {denied} named"
    );
    // Allowed by the NetworkPolicy.
    assert!(
        count(
            &flows,
            "src.workload=checkout-client dst.workload=ledger-api verdict=forwarded policy=ledger-api-isolation"
        ) > 0
    );

    // Filtering server-side: one workload's blocked flows.
    let pushed = whisker.pushdown(
        &FlowFilter::parse("src.ns=storefront src.workload=scraper verdict=dropped").unwrap(),
    );
    assert_eq!(pushed.terms.len(), 3);
    let scraper = collect(
        &whisker,
        StreamQuery {
            filter: pushed,
            since: since(120),
        },
        8,
    )
    .await;
    assert!(!scraper.is_empty());
    assert_eq!(
        count(&scraper, "src.workload=scraper verdict=dropped"),
        scraper.len()
    );
}
