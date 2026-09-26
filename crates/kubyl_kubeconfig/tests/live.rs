//! Tests against the kind dev cluster. Ignored by default; run them with:
//!
//! ```sh
//! script/dev-cluster.sh
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig KUBYL_CREDENTIAL_STORE=memory \
//!   cargo test -p kubyl_kubeconfig --test live -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `KUBYL_TEST_CONTEXT` picks the context (default `kind-kubyl-dev`). The tests never touch
//! the real `~/.kube/config`: they copy what they need into temp folders. The kubectl checks
//! need `kubectl` in `PATH`. `kind_token_that_really_expired…` waits 11 minutes (a
//! TokenRequest lasts at least 10) and only runs with `KUBYL_TEST_SLOW=1`.

use std::path::{Path, PathBuf};
use std::process::Command;

use futures::channel::mpsc;
use kubyl_kubeconfig::conntest::{self, Fix, Input, Report, Status, StepKind};
use kubyl_kubeconfig::model::{self, Doc, Kind};
use kubyl_kubeconfig::{certs, files, tls, yaml};
use serde_json::{Map, Value, json};

fn kubeconfig_path() -> PathBuf {
    PathBuf::from(
        std::env::var("KUBYL_TEST_KUBECONFIG")
            .unwrap_or_else(|_| "/tmp/kubyl-dev/kubeconfig".into()),
    )
}

fn context_name() -> String {
    std::env::var("KUBYL_TEST_CONTEXT").unwrap_or_else(|_| "kind-kubyl-dev".into())
}

/// The dev kubeconfig and its context's server, CA, client certificate and key (PEM).
struct DevCluster {
    #[allow(dead_code)]
    doc: Doc,
    server: String,
    ca: String,
    cert: String,
    key: String,
}

fn kind() -> DevCluster {
    let text = std::fs::read_to_string(kubeconfig_path()).expect("the dev kubeconfig");
    let doc = Doc::parse(&text).unwrap();
    let (cluster, user) = doc.context_refs(&context_name());
    let cluster = doc.body(Kind::Cluster, &cluster.unwrap()).unwrap().clone();
    let user = doc.body(Kind::User, &user.unwrap()).unwrap().clone();
    let pem = |map: &Map<String, Value>, key: &str| {
        certs::pem_from_data(&model::get_str(map, &[key])).unwrap()
    };
    DevCluster {
        server: model::get_str(&cluster, &["server"]),
        ca: pem(&cluster, model::CA_DATA),
        cert: pem(&user, model::CERT_DATA),
        key: pem(&user, model::KEY_DATA),
        doc,
    }
}

async fn test(doc: Doc, context: &str, file: &Path, allow_exec: bool) -> Report {
    let (tx, _rx) = mpsc::unbounded();
    let report = conntest::run(
        Input {
            doc,
            context: context.into(),
            file: file.to_path_buf(),
            allow_exec,
        },
        tx,
    )
    .await;
    for step in &report.steps {
        println!(
            "{:?} {:<14} {:?} {:?} {:?}",
            step.status,
            step.kind.title(),
            step.lines,
            step.error,
            step.checks
                .iter()
                .map(|c| (&c.label, c.allowed))
                .collect::<Vec<_>>()
        );
    }
    report
}

/// A kubeconfig with one context `ctx` for the dev cluster's server.
fn single(server: &str, ca_data: Option<String>, user: Value) -> Doc {
    let mut cluster = json!({"server": server});
    if let Some(ca) = ca_data {
        cluster["certificate-authority-data"] = json!(ca);
    }
    Doc::parse(&yaml::render(&json!({
        "apiVersion": "v1",
        "kind": "Config",
        "clusters": [{"name": "kind", "cluster": cluster}],
        "contexts": [{"name": "ctx", "context": {"cluster": "kind", "user": "me"}}],
        "users": [{"name": "me", "user": user}],
        "current-context": "ctx",
    })))
    .unwrap()
}

#[tokio::test]
#[ignore = "needs the kind dev cluster (script/dev-cluster.sh)"]
async fn kind_ca_is_fetched_after_confirmation_and_the_test_passes_with_cert_files() {
    let kind = kind();
    // The wizard: fetch the CA from the server (nothing but a TLS handshake and an anonymous
    // cluster-info read), show the fingerprint; the user compares it (here: with the CA the
    // dev kubeconfig holds) and confirms.
    let target = tls::Target::parse(&kind.server, None).unwrap();
    let fetched = tls::fetch_ca(&target).await.unwrap();
    let candidate = fetched
        .candidates
        .first()
        .expect("a CA that verifies the server");
    let known = certs::certificates(&kind.ca).unwrap().remove(0);
    println!(
        "CA {} from {} · SHA-256 {}",
        candidate.cert.subject,
        candidate.origin.label(),
        candidate.cert.fingerprint()
    );
    assert_eq!(
        candidate.cert.fingerprint(),
        known.fingerprint(),
        "fingerprint mismatch"
    );
    assert_eq!(candidate.origin, tls::CaOrigin::ClusterInfo);

    // Client certificate and key from files.
    let dir = tempfile::tempdir().unwrap();
    let cert_file = dir.path().join("admin.crt");
    let key_file = dir.path().join("admin.key");
    std::fs::write(&cert_file, &kind.cert).unwrap();
    files::write_atomic(&key_file, kind.key.as_bytes(), Some(0o600)).unwrap();
    let doc = single(
        &kind.server,
        Some(certs::data_from_pem(&candidate.cert.pem())),
        json!({"client-certificate": "admin.crt", "client-key": "admin.key"}),
    );
    let file = dir.path().join("kind.yaml");
    let report = test(doc, "ctx", &file, false).await;
    assert!(report.passed(), "{}", report.summary());
    assert_eq!(report.step(StepKind::Tls).status, Status::Ok);
    assert_eq!(report.user.as_deref(), Some("kubernetes-admin"));
    let permissions = report.step(StepKind::Permissions);
    assert!(
        permissions
            .checks
            .iter()
            .any(|c| c.label.starts_with("list pods") && c.allowed == Some(true)),
        "{permissions:?}"
    );
    assert!(
        report
            .namespaces
            .as_ref()
            .is_some_and(|n| n.iter().any(|n| n == "kube-system"))
    );
    println!("{}", report.summary());
}

#[tokio::test]
#[ignore = "needs the kind dev cluster (script/dev-cluster.sh)"]
async fn kind_failures_stop_at_the_right_step_with_a_clear_message() {
    let kind = kind();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("kind.yaml");
    let user = json!({"client-certificate-data": certs::data_from_pem(&kind.cert), "client-key-data": certs::data_from_pem(&kind.key)});

    // A wrong CA: TLS fails, with the fix, and no credentials go out.
    let other = include_str!("fixtures/other-ca.pem");
    let report = test(
        single(
            &kind.server,
            Some(certs::data_from_pem(other)),
            user.clone(),
        ),
        "ctx",
        &file,
        false,
    )
    .await;
    let tls = report.step(StepKind::Tls);
    assert_eq!(tls.status, Status::Fail);
    assert_eq!(tls.fix, Some(Fix::FetchCa));
    assert!(tls.error.as_deref().unwrap().contains("unknown authority"));
    assert_eq!(report.step(StepKind::Credentials).status, Status::Skipped);

    // An expired token: the API server answers 401, the authentication step explains it.
    // (A signed token can't be made to expire faster than a TokenRequest's 10 minutes; the
    // slow test below uses a real one. This one carries an `exp` in the past.)
    let now = jiff::Timestamp::now().as_second();
    let jwt = fake_jwt(now - 3600);
    let report = test(
        single(
            &kind.server,
            Some(certs::data_from_pem(&kind.ca)),
            json!({"token": jwt}),
        ),
        "ctx",
        &file,
        false,
    )
    .await;
    assert_eq!(report.step(StepKind::Credentials).status, Status::Warn);
    let auth = report.step(StepKind::Auth);
    assert_eq!(auth.status, Status::Fail, "{report:?}");
    assert!(
        auth.error
            .as_deref()
            .unwrap()
            .contains("the token expired on"),
        "{auth:?}"
    );
    assert_eq!(auth.fix, Some(Fix::ReplaceToken));

    // A missing exec plugin: the credentials step, with the install hint.
    let exec = json!({"exec": {
        "apiVersion": "client.authentication.k8s.io/v1beta1",
        "command": "kubyl-no-such-auth-plugin",
        "installHint": "Install kubyl-no-such-auth-plugin from https://example.com",
    }});
    let report = test(
        single(&kind.server, Some(certs::data_from_pem(&kind.ca)), exec),
        "ctx",
        &file,
        true,
    )
    .await;
    let credentials = report.step(StepKind::Credentials);
    assert_eq!(credentials.status, Status::Fail);
    assert!(
        credentials
            .error
            .as_deref()
            .unwrap()
            .contains("isn't installed"),
        "{credentials:?}"
    );
    assert!(matches!(&credentials.fix, Some(Fix::InstallHint(h)) if h.contains("example.com")));
    assert_eq!(report.step(StepKind::Tls).status, Status::Ok);
}

fn fake_jwt(exp: i64) -> String {
    use base64::Engine as _;
    let b64 = |v: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
    format!(
        "{}.{}.{}",
        b64(json!({"alg": "RS256", "kid": "kubyl-test"})),
        b64(
            json!({"iss": "https://kubernetes.default.svc.cluster.local", "sub": "system:serviceaccount:default:default", "exp": exp, "iat": exp - 3600})
        ),
        "c2lnbmF0dXJl"
    )
}

#[tokio::test]
#[ignore = "needs the kind dev cluster and 11 minutes (KUBYL_TEST_SLOW=1)"]
async fn kind_token_that_really_expired_fails_at_authentication() {
    if std::env::var("KUBYL_TEST_SLOW").is_err() {
        println!("set KUBYL_TEST_SLOW=1 to run this test");
        return;
    }
    let kind = kind();
    let token = kubectl(&[
        "create",
        "token",
        "default",
        "-n",
        "default",
        "--duration=10m",
    ]);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("kind.yaml");
    let doc = single(
        &kind.server,
        Some(certs::data_from_pem(&kind.ca)),
        json!({"token": token.trim()}),
    );
    let report = test(doc.clone(), "ctx", &file, false).await;
    assert_eq!(
        report.user.as_deref(),
        Some("system:serviceaccount:default:default")
    );
    println!("waiting for the token to expire…");
    tokio::time::sleep(std::time::Duration::from_secs(11 * 60)).await;
    let report = test(doc, "ctx", &file, false).await;
    let auth = report.step(StepKind::Auth);
    assert_eq!(auth.status, Status::Fail);
    assert!(
        auth.error
            .as_deref()
            .unwrap()
            .contains("the token expired on"),
        "{auth:?}"
    );
}

fn kubectl(args: &[&str]) -> String {
    let out = Command::new("kubectl")
        .args([
            "--kubeconfig",
            &kubeconfig_path().display().to_string(),
            "--context",
            &context_name(),
        ])
        .args(args)
        .output()
        .expect("kubectl");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// The opt-in acceptance test, without the UI: a throwaway HOME whose `.kube/config` is a
/// commented copy of the dev kubeconfig. Changing a context's namespace changes one line,
/// keeps every comment, leaves a backup, and kubectl still works with the file.
#[tokio::test]
#[ignore = "needs the kind dev cluster and kubectl"]
async fn kind_editing_an_opted_in_file_changes_one_line_and_kubectl_still_works() {
    let home = tempfile::tempdir().unwrap();
    let kube_dir = home.path().join(".kube");
    std::fs::create_dir_all(&kube_dir).unwrap();
    let config = kube_dir.join("config");
    let original = std::fs::read_to_string(kubeconfig_path()).unwrap();
    let commented = format!(
        "# My kubeconfig. Hand-maintained, keep the comments!\n{}\n# end of file\n",
        original
            .replace("clusters:\n", "clusters:\n# local clusters\n")
            .replace("users:\n", "users:\n# admin users, rotate yearly\n")
    );
    std::fs::write(&config, &commented).unwrap();

    let snapshot = files::read(&config).unwrap();
    let mut doc = Doc::parse(&snapshot.text).unwrap();
    let context = context_name();
    model::set_str(
        doc.body_mut(Kind::Context, &context).unwrap(),
        &["namespace"],
        "payments",
    );
    let written = yaml::write(&snapshot.text, &doc.0);
    assert!(written.in_place);
    assert!(written.lost_comments.is_empty());

    // The preview: exactly one added line.
    let diff = kubyl_yaml::diff::diff(&snapshot.text, &written.text, 3);
    assert_eq!(diff.change_count(), 1, "{:#?}", diff.hunks);
    let backups = home.path().join("kubyl-config/kubeconfig-backups");
    let saved = files::save(
        &config,
        &written.text,
        &files::SaveOptions {
            expected: snapshot.hash.clone(),
            backups: Some(files::Backups {
                dir: backups.clone(),
                keep: 10,
            }),
            private: doc.has_inline_credentials(),
        },
    )
    .unwrap();
    let backup = saved.backup.expect("a backup");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), commented);
    #[cfg(unix)]
    assert_eq!(saved.mode, Some(0o600));

    let out = Command::new("kubectl")
        .args([
            "--kubeconfig",
            &config.display().to_string(),
            "--context",
            &context,
        ])
        .args([
            "config",
            "view",
            "--minify",
            "-o",
            "jsonpath={.contexts[0].context.namespace}",
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "payments");
    let out = Command::new("kubectl")
        .args([
            "--kubeconfig",
            &config.display().to_string(),
            "--context",
            &context,
        ])
        .args(["get", "pods", "-o", "name"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = std::fs::read_to_string(&config).unwrap();
    for comment in [
        "# My kubeconfig",
        "# local clusters",
        "# admin users",
        "# end of file",
    ] {
        assert!(after.contains(comment), "{comment} is gone");
    }

    // Another tool writes the file meanwhile: a save with the old hash is refused.
    std::fs::write(&config, format!("{after}# touched by another tool\n")).unwrap();
    let refused = files::save(
        &config,
        &written.text,
        &files::SaveOptions {
            expected: Some(saved.hash.clone()),
            backups: None,
            private: true,
        },
    );
    assert!(matches!(refused, Err(files::SaveError::Changed { .. })));
    assert!(
        std::fs::read_to_string(&config)
            .unwrap()
            .ends_with("# touched by another tool\n")
    );
}
