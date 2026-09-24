//! Tests against real clusters. Ignored by default; run them with the dev scripts:
//!
//! ```sh
//! script/dev-cluster.sh
//! KUBYL_TEST_KUBECONFIG=~/.kube/config KUBYL_TEST_CONTEXT=kind-kubyl-dev \
//!   cargo test -p kubyl_kube --test live -- --ignored kind --nocapture
//!
//! script/oidc-dev.sh
//! KUBYL_CREDENTIAL_STORE=memory KUBYL_TEST_OIDC_KUBECONFIG=~/.cache/kubyl-oidc-dev/kubeconfig.yaml \
//!   cargo test -p kubyl_kube --test live -- --ignored oidc --nocapture
//! ```
//!
//! The OIDC test prints the sign-in URL (and writes it to `$KUBYL_TEST_URL_FILE` if set); sign
//! in as admin@kubyl.dev / password. It then waits for the 2-minute ID token to expire and
//! checks that the refresh token keeps the client working.

use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt as _;
use kubyl_kube::auth::CredentialSource;
use kubyl_kube::auth::oidc::SignInMethod;
use kubyl_kube::kubeconfig::{self, ContextInfo, Loaded};
use kubyl_kube::settings::KubeSettings;
use kubyl_kube::{client, discovery};

fn load(path: &str) -> Loaded {
    let settings = KubeSettings {
        load_default_kubeconfig: false,
        load_kubeconfig_env: false,
        kubeconfigs: vec![path.to_string()],
        ..Default::default()
    };
    let specs = kubeconfig::source_specs(
        &settings,
        None,
        None,
        &std::env::temp_dir().join("kubyl-none"),
    );
    kubeconfig::load(&specs)
}

fn context<'a>(loaded: &'a Loaded, name: Option<&str>) -> &'a ContextInfo {
    loaded
        .contexts
        .iter()
        .find(|c| name.is_none_or(|n| c.context == n))
        .expect("context not found")
}

fn expand(path: String) -> String {
    kubyl_kube::settings::expand_home(&path)
        .display()
        .to_string()
}

#[tokio::test]
#[ignore = "needs a cluster (script/dev-cluster.sh)"]
async fn kind_connects_discovers_and_lists_namespaces() {
    let path = expand(std::env::var("KUBYL_TEST_KUBECONFIG").expect("KUBYL_TEST_KUBECONFIG"));
    let loaded = load(&path);
    let info = context(&loaded, std::env::var("KUBYL_TEST_CONTEXT").ok().as_deref());
    let built = client::build(info, loaded.configs[&info.file].clone())
        .await
        .unwrap();
    let probe = client::probe(&built.client).await.unwrap();
    println!(
        "{} {} {:?} user={:?}",
        info.name, probe.version.git_version, probe.latency, probe.user
    );

    let discovery = discovery::discover(&built.client).await.unwrap();
    assert!(discovery.aggregated);
    assert!(discovery.resolve("po").is_some());
    assert!(discovery.resolve("deploy").is_some());
    println!("{} kinds", discovery.kind_count());

    // OpenAPI v3: the second fetch is served from the ETag cache.
    let cache = tempfile::tempdir().unwrap();
    let key = kubyl_kube::openapi::OpenApiIndex::key("apps", "v1");
    let spec = kubyl_kube::openapi::spec(&built.client, cache.path(), &key)
        .await
        .unwrap();
    assert!(spec["components"]["schemas"]["io.k8s.api.apps.v1.Deployment"].is_object());
    let cached = kubyl_kube::openapi::spec(&built.client, cache.path(), &key)
        .await
        .unwrap();
    assert_eq!(spec, cached);
    assert!(std::fs::read_dir(cache.path()).unwrap().count() >= 2);

    // RBAC check.
    let allowed = kubyl_kube::access::check(
        built.client.clone(),
        kubyl_kube::access::AccessQuery::new(
            "list",
            &kubyl_core::Gvr::new("", "v1", "pods"),
            Some("payments"),
        ),
    )
    .await
    .unwrap();
    assert!(allowed);

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let watch = tokio::spawn(kubyl_kube::watches::watch_namespaces(
        built.client.clone(),
        tx,
    ));
    let first = tokio::time::timeout(Duration::from_secs(10), rx.next())
        .await
        .unwrap()
        .unwrap();
    println!("{first:?}");
    assert!(
        matches!(first, kubyl_kube::watches::NamespaceUpdate::Names(ref n) if n.contains(&"kube-system".to_string()))
    );
    watch.abort();
}

#[tokio::test]
#[ignore = "needs script/oidc-dev.sh and a browser sign-in"]
async fn oidc_signs_in_and_refreshes() {
    let path =
        expand(std::env::var("KUBYL_TEST_OIDC_KUBECONFIG").expect("KUBYL_TEST_OIDC_KUBECONFIG"));
    let loaded = load(&path);
    let info = context(&loaded, None);
    assert!(info.auth.is_oidc(), "{:?}", info.auth);
    let built = client::build(info, loaded.configs[&info.file].clone())
        .await
        .unwrap();
    let Some(CredentialSource::Oidc(auth)) = built.credentials.clone() else {
        panic!("expected OIDC credentials");
    };

    // Without a session the client reports that a sign-in is needed.
    assert_eq!(
        client::probe(&built.client).await.unwrap_err(),
        client::ConnectError::SignInRequired
    );

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let printer = tokio::spawn(async move {
        while let Some(event) = rx.next().await {
            println!("sign-in event: {event:?}");
            if let kubyl_kube::auth::SignInEvent::WaitingForBrowser { url, .. } = event
                && let Ok(file) = std::env::var("KUBYL_TEST_URL_FILE")
            {
                std::fs::write(PathBuf::from(file), url).unwrap();
            }
        }
    });
    auth.sign_in(SignInMethod::Browser, tx)
        .await
        .expect("sign-in");
    printer.await.ok();

    let probe = client::probe(&built.client).await.expect("authenticated");
    assert_eq!(probe.user.as_deref(), Some("oidc:admin@kubyl.dev"));
    let first_expiry = auth.expires_at().await.unwrap();
    println!(
        "signed in as {:?}, token expires at {first_expiry}",
        probe.user
    );

    // Let the 2-minute ID token expire; the next request must refresh it.
    let wait = (first_expiry.as_second() - jiff::Timestamp::now().as_second()).max(0) as u64 + 5;
    println!("waiting {wait}s for the ID token to expire");
    tokio::time::sleep(Duration::from_secs(wait)).await;
    let probe = client::probe(&built.client).await.expect("refreshed");
    let second_expiry = auth.expires_at().await.unwrap();
    assert!(second_expiry > first_expiry);
    println!("refreshed: {:?}, new expiry {second_expiry}", probe.user);
}
