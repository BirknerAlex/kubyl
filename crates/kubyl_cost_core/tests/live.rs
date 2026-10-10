//! OpenCost on the kind dev cluster, through the API server's service proxy. Ignored by default:
//!
//! ```sh
//! script/dev-cluster.sh
//! script/prometheus-dev.sh
//! script/opencost-dev.sh        # data needs a few minutes of Prometheus history
//! kind get kubeconfig --name kubyl-dev > /tmp/kubyl-dev.yaml
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev.yaml \
//!   cargo test -p kubyl_cost_core --test live -- --ignored --nocapture
//! ```
//!
//! Read-only. Checks the allocation API of the installed OpenCost against what the parser
//! expects (windows, `includeIdle`, `accumulate`, `step`), detection, and the messages of the
//! failures a user can cause (a wrong port, no such Service).

use k8s_openapi::api::core::v1::Service;
use kube::api::{Api, ListParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};
use kubyl_cost_core::detect::{self, ServiceInfo};
use kubyl_cost_core::fetch::{self, CostError};
use kubyl_cost_core::model::IDLE;
use kubyl_cost_core::settings::Endpoint;
use kubyl_cost_core::summary;
use kubyl_cost_core::window::{Query, Window};
use kubyl_metrics_core::transport::Transport;

async fn client() -> Client {
    let path = std::env::var("KUBYL_TEST_KUBECONFIG").expect("set KUBYL_TEST_KUBECONFIG");
    let options = KubeConfigOptions {
        context: Some(std::env::var("KUBYL_TEST_CONTEXT").unwrap_or("kind-kubyl-dev".into())),
        ..Default::default()
    };
    let config = Config::from_custom_kubeconfig(Kubeconfig::read_from(path).unwrap(), &options)
        .await
        .unwrap();
    Client::try_from(config).unwrap()
}

async fn endpoint(client: &Client) -> Endpoint {
    let api: Api<Service> = Api::all(client.clone());
    let list = api
        .list(&ListParams::default().labels("app.kubernetes.io/name=opencost"))
        .await
        .unwrap();
    let services: Vec<ServiceInfo> = list
        .items
        .into_iter()
        .map(|s| ServiceInfo {
            namespace: s.metadata.namespace.unwrap_or_default(),
            name: s.metadata.name.unwrap_or_default(),
            app_labels: vec!["opencost".into()],
            ports: s
                .spec
                .and_then(|s| s.ports)
                .unwrap_or_default()
                .into_iter()
                .map(|p| (p.name.unwrap_or_default(), p.port as u16))
                .collect(),
        })
        .collect();
    detect::find(&services).expect("OpenCost's Service: run script/opencost-dev.sh")
}

fn transport(client: &Client, endpoint: &Endpoint) -> Transport {
    Transport::service_proxy(
        client.clone(),
        &endpoint.namespace,
        &endpoint.service,
        &endpoint.port,
        "http",
        "",
    )
}

#[tokio::test]
#[ignore = "needs a cluster with script/opencost-dev.sh"]
async fn every_window_and_idle_choice_answers_in_the_shape_the_parser_reads() {
    let client = client().await;
    let endpoint = endpoint(&client).await;
    println!("OpenCost at {endpoint}");
    assert_eq!(endpoint.service, "opencost");
    let transport = transport(&client, &endpoint);
    for window in Window::ALL {
        for include_idle in [true, false] {
            let accumulated = Query {
                window,
                include_idle,
                accumulate: true,
            };
            let sets = fetch::allocations(&transport, &endpoint, &accumulated)
                .await
                .unwrap();
            assert_eq!(sets.len(), 1, "{window:?}: one set for the whole window");
            let totals = summary::totals(&sets[0]);
            let has_idle = sets[0].iter().any(|a| a.name == IDLE);
            println!(
                "{:>4} idle={include_idle:<5} {} namespaces, total ${:.5}, idle ${:.5}, cpu eff {:.2}",
                window.param(),
                sets[0].len(),
                totals.total,
                totals.idle,
                totals.cpu_efficiency
            );
            assert!(sets[0].iter().any(|a| a.name == "kube-system"));
            if !include_idle {
                assert!(
                    !has_idle || totals.idle == 0.0,
                    "idle left out of the totals"
                );
            }

            let steps = Query {
                accumulate: false,
                ..accumulated
            };
            let step_sets = fetch::allocations(&transport, &endpoint, &steps)
                .await
                .unwrap();
            let expected = (window.seconds() / window.step_seconds()) as usize;
            assert!(
                step_sets.len() == expected || step_sets.len() == expected + 1,
                "{window:?}: {} steps, expected about {expected}",
                step_sets.len()
            );
            let series = summary::series(&step_sets, window.step_seconds(), 6);
            assert_eq!(series.times.len(), step_sets.len());
            assert!(!series.is_empty(), "{window:?} has cost somewhere");
        }
    }
}

#[tokio::test]
#[ignore = "needs a cluster with script/opencost-dev.sh"]
async fn failures_say_what_to_check() {
    let client = client().await;
    let real = endpoint(&client).await;
    let query = Query {
        window: Window::Day,
        include_idle: true,
        accumulate: true,
    };

    // A port nothing listens on, and a Service that doesn't exist.
    for (name, port, expected) in [
        ("opencost", "9", "opencost/opencost"),
        ("no-such-service", "9003", "opencost/no-such-service"),
    ] {
        let endpoint = Endpoint {
            namespace: real.namespace.clone(),
            service: name.into(),
            port: port.into(),
        };
        let err = fetch::allocations(&transport(&client, &endpoint), &endpoint, &query)
            .await
            .unwrap_err();
        println!("{name}:{port}: {err}");
        assert!(matches!(err, CostError::Unreachable(_)));
        assert!(err.to_string().contains(expected), "{err}");
    }
}
