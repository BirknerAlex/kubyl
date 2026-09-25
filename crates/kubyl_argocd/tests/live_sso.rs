//! SSO sign-in against Argo CD's Dex on kind. Ignored by default; switches the dev install to
//! SSO for the test (`script/argocd-dev.sh --sso`: Dex with a mock connector, Argo CD at
//! http://localhost:8080 through a port-forward) and back afterwards (`--no-sso`):
//!
//! ```sh
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig KUBYL_CREDENTIAL_STORE=memory \
//!   cargo test -p kubyl_argocd --test live_sso -- --ignored --nocapture
//! ```
//!
//! The browser is an HTTP client following the redirects: Dex's mock connector signs in
//! without a page, then sends the browser to Kubyl's loopback callback.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use kubyl_argocd::api::{ArgoApi, Transport};
use kubyl_argocd::sso::{self, SsoConfig};
use kubyl_argocd::web;
use openidconnect::reqwest;
use secrecy::ExposeSecret as _;

fn script(arg: &str) {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../script/argocd-dev.sh");
    let status = Command::new(path)
        .arg(arg)
        .env(
            "KUBECONFIG",
            std::env::var("KUBYL_TEST_KUBECONFIG").unwrap(),
        )
        .env(
            "CONTEXT",
            std::env::var("KUBYL_TEST_CONTEXT").unwrap_or("kind-kubyl-dev".into()),
        )
        .status()
        .expect("script/argocd-dev.sh");
    assert!(status.success(), "argocd-dev.sh {arg} failed");
}

/// SSO on for the test, off again afterwards (also when it fails).
struct SsoMode;

impl SsoMode {
    /// The guard exists before the script runs: a half-applied `--sso` is undone too.
    fn on() -> Self {
        let guard = Self;
        script("--sso");
        guard
    }
}

impl Drop for SsoMode {
    fn drop(&mut self) {
        // No panic while unwinding from a failed test (that would abort).
        if std::panic::catch_unwind(|| script("--no-sso")).is_err() {
            eprintln!("argocd-dev.sh --no-sso failed: run it by hand to restore the dev install");
        }
    }
}

/// `kubectl port-forward svc/argocd-server 8080:80`: Argo CD's `url` for the browser and Dex.
struct PortForward(Child);

impl PortForward {
    fn start() -> Self {
        let kubeconfig = std::env::var("KUBYL_TEST_KUBECONFIG").unwrap();
        let child = Command::new("kubectl")
            .args([
                "--kubeconfig",
                &kubeconfig,
                "-n",
                "argocd",
                "port-forward",
                "svc/argocd-server",
                "8080:80",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("kubectl");
        Self(child)
    }
}

impl Drop for PortForward {
    fn drop(&mut self) {
        self.0.kill().ok();
    }
}

#[tokio::test]
#[ignore = "needs Argo CD on kind (script/argocd-dev.sh); switches it to SSO and back"]
async fn sso_sign_in_and_renewal_through_dex() {
    let _sso = SsoMode::on();
    let _forward = PortForward::start();
    let api = ArgoApi::new(Transport::forward(8080, false, "").unwrap(), None);
    let start = Instant::now();
    while api.version().await.is_err() {
        assert!(start.elapsed() < Duration::from_secs(60), "argocd-server");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Where to sign in comes from Argo CD's settings, like `argocd login --sso`.
    let settings = api.settings().await.unwrap();
    let config = SsoConfig::from_settings(&settings).unwrap();
    println!("sso: {config:?}");
    assert_eq!(config.issuer, "http://localhost:8080/api/dex");
    assert_eq!(config.client_id, "argo-cd-cli");

    let tokens = sso::sign_in(&config, |url| {
        assert!(url.starts_with("http://localhost:8080/api/dex/auth?"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("offline_access"));
        // Browsers may try either loopback address for `localhost`: both listen.
        for address in ["127.0.0.1", "::1"] {
            assert!(
                std::net::TcpStream::connect((address, sso::CALLBACK_PORT)).is_ok(),
                "nothing listens on {address}"
            );
        }
        tokio::spawn(async move {
            // The browser: Dex's redirects end at Kubyl's loopback callback.
            let response = reqwest::Client::new().get(url).send().await;
            println!("browser: {:?}", response.map(|r| r.status()));
        });
    })
    .await
    .unwrap();
    let me = api
        .with_token(tokens.id_token.clone())
        .user_info()
        .await
        .unwrap();
    println!(
        "signed in as {} ({:?}), issuer {}",
        me.username, me.groups, me.iss
    );
    assert!(me.logged_in);
    assert!(!me.username.is_empty());
    // The web UI's cookie format works with the SSO token too.
    assert_eq!(web::session_cookies(&tokens.id_token).len(), 1);

    // Renewal with the refresh token (Dex rotates it).
    let refresh = tokens
        .refresh_token
        .expect("Dex issues refresh tokens for offline_access");
    let renewed = sso::refresh(&config, &refresh).await.unwrap();
    assert_ne!(
        renewed.id_token.expose_secret(),
        tokens.id_token.expose_secret()
    );
    let again = api.with_token(renewed.id_token).user_info().await.unwrap();
    assert_eq!(again.username, me.username);
    println!("renewed: ok");
}
