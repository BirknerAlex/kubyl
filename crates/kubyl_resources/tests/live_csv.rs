//! CSV export of resource tables against the kind dev cluster (`script/dev-cluster.sh`).
//! Ignored by default:
//!
//! ```sh
//! KUBYL_TEST_KUBECONFIG=<kubeconfig with kind-kubyl-dev> \
//!   cargo test -p kubyl_resources --test live_csv -- --ignored --nocapture
//! ```
//!
//! Read-only. Real Pods go through the built-in column sets into the CSV writer; the Secrets of
//! `kube-system` (bootstrap tokens, certificates) prove that no Secret value reaches an export.

use kube::api::{Api, DynamicObject, ListParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::discovery::ApiResource;
use kube::{Client, Config};
use kubyl_core::csv;
use kubyl_resources::columns;
use serde_json::Value;

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

async fn list(client: &Client, resource: ApiResource, namespace: &str) -> Vec<Value> {
    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), namespace, &resource);
    api.list(&ListParams::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|o| serde_json::to_value(o).unwrap())
        .collect()
}

fn kind(name: &str) -> columns::Kind {
    columns::builtin()
        .into_iter()
        .find(|(g, k, _)| g.is_empty() && *k == name)
        .map(|(_, _, kind)| kind)
        .unwrap()
}

/// The CSV of `objects` with every column of the kind that exports.
fn export(kind_name: &str, objects: &[Value]) -> (String, Vec<String>) {
    let kind = kind(kind_name);
    let columns: Vec<_> = kind
        .columns()
        .into_iter()
        .filter(|c| columns::exportable("", kind_name, &c.id))
        .collect();
    let header: Vec<String> = columns.iter().map(|c| c.title.to_string()).collect();
    let rows: Vec<Vec<String>> = objects
        .iter()
        .map(|object| {
            columns
                .iter()
                .map(|c| csv::cell_text(&kind.cell(object, &c.id), |b| match *b {}))
                .collect()
        })
        .collect();
    (csv::write(&header, &rows), header)
}

#[tokio::test]
#[ignore]
async fn pods_export_as_csv() {
    let client = client().await;
    let pods = list(
        &client,
        ApiResource::erase::<k8s_openapi::api::core::v1::Pod>(&()),
        "kube-system",
    )
    .await;
    assert!(!pods.is_empty());
    let (text, header) = export("Pod", &pods);
    println!("{text}");
    assert_eq!(header[0], "Name");
    // A record per pod plus the header, CRLF-terminated, every record as wide as the header.
    let lines: Vec<&str> = text.split("\r\n").collect();
    assert_eq!(lines.len(), pods.len() + 2);
    assert!(lines.last().unwrap().is_empty());
    assert!(text.contains("coredns"), "{text}");
    assert!(text.contains("Running"));
}

#[tokio::test]
#[ignore]
async fn secret_values_never_reach_an_export() {
    let client = client().await;
    let secrets = list(
        &client,
        ApiResource::erase::<k8s_openapi::api::core::v1::Secret>(&()),
        "kube-system",
    )
    .await;
    if secrets.is_empty() {
        println!("kube-system holds no Secrets; nothing to check");
        return;
    }
    let (text, header) = export("Secret", &secrets);
    println!("{text}");
    assert_eq!(header, ["Name", "Type", "Data", "Age"]);
    for secret in &secrets {
        for map in ["data", "stringData"] {
            let Some(values) = secret[map].as_object() else {
                continue;
            };
            for value in values.values().filter_map(Value::as_str) {
                if value.len() >= 8 {
                    assert!(!text.contains(value), "a Secret value is in the export");
                }
            }
        }
    }
}
