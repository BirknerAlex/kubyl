//! "Test connection": checks a context step by step, from the document being edited (nothing
//! has to be saved), with phase 01's client builder.
//!
//! Steps: DNS and TCP, TLS (Kubyl's own handshake, so a failure shows the server's
//! certificate), credentials (exec plugin, OIDC, token, client certificate), `/version`,
//! authentication (`SelfSubjectReview`), permissions (`SelfSubjectAccessReview`s) and latency.
//! A failed step skips the rest; after a TLS failure no credentials were sent anywhere. The
//! caller asks for consent before running an exec plugin from unsaved edits and says so with
//! [`Input::allow_exec`]; without it the credentials step refuses to run the plugin.
//!
//! Runs on the Tokio runtime. Every change of the report is sent to the caller.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::channel::mpsc;
use k8s_openapi::api::authentication::v1::SelfSubjectReview;
use k8s_openapi::api::authorization::v1::{
    ResourceAttributes, SelfSubjectAccessReview, SelfSubjectAccessReviewSpec,
};
use k8s_openapi::api::core::v1::Namespace;
use kube::api::{ListParams, PostParams};
use kube::config::{ExecAuthCluster, Kubeconfig};
use kube::{Api, Client};
use kubyl_kube::auth::{AuthError, AuthMethod, CredentialSource, ExecAuth, OidcAuth, exec};
use kubyl_kube::client::{self, ConnectError};
use secrecy::ExposeSecret as _;

use crate::certs;
use crate::model::Doc;
use crate::tls::{self, Target, TlsError, Trust};

/// Each request of the test gives up after this.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StepKind {
    Network,
    Tls,
    Credentials,
    Api,
    Auth,
    Permissions,
    Latency,
}

impl StepKind {
    pub const ALL: [StepKind; 7] = [
        StepKind::Network,
        StepKind::Tls,
        StepKind::Credentials,
        StepKind::Api,
        StepKind::Auth,
        StepKind::Permissions,
        StepKind::Latency,
    ];

    pub fn title(self) -> &'static str {
        match self {
            StepKind::Network => "DNS and TCP",
            StepKind::Tls => "TLS",
            StepKind::Credentials => "Credentials",
            StepKind::Api => "API server",
            StepKind::Auth => "Authentication",
            StepKind::Permissions => "Permissions",
            StepKind::Latency => "Latency",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    Running,
    Ok,
    Warn,
    Fail,
    Skipped,
}

/// What the UI can offer to fix a failed step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fix {
    /// The server's CA isn't trusted: fetch it (trust on first use).
    FetchCa,
    /// An exec plugin isn't installed: its install hint.
    InstallHint(String),
    /// OIDC needs a sign-in.
    SignIn,
    /// The token was rejected or expired.
    ReplaceToken,
}

/// A permission check: `Some(true)` allowed, `Some(false)` denied, `None` unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub allowed: Option<bool>,
    pub label: String,
    /// Denied isn't a problem (cluster-admin).
    pub optional: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub kind: StepKind,
    pub status: Status,
    /// Details: what was checked and found.
    pub lines: Vec<String>,
    /// Plain-language error.
    pub error: Option<String>,
    /// Exec plugin stderr.
    pub stderr: Option<String>,
    pub fix: Option<Fix>,
    pub checks: Vec<Check>,
    pub millis: Option<u64>,
}

impl Step {
    fn new(kind: StepKind) -> Self {
        Self {
            kind,
            status: Status::Pending,
            lines: Vec::new(),
            error: None,
            stderr: None,
            fix: None,
            checks: Vec::new(),
            millis: None,
        }
    }
}

/// The state of a test run. Contains no secrets.
#[derive(Clone)]
pub struct Report {
    pub context: String,
    pub steps: Vec<Step>,
    /// From the permissions step, when listing namespaces is allowed.
    pub namespaces: Option<Vec<String>>,
    pub user: Option<String>,
    pub groups: Vec<String>,
    pub version: Option<String>,
    /// Median round trip of `GET /version`.
    pub latency: Option<Duration>,
    pub done: bool,
    pub started: jiff::Timestamp,
    pub millis: u64,
    /// The OIDC session to sign in with when [`Fix::SignIn`] is offered.
    pub oidc: Option<Arc<OidcAuth>>,
}

impl std::fmt::Debug for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Report")
            .field("context", &self.context)
            .field("steps", &self.steps)
            .field("user", &self.user)
            .field("done", &self.done)
            .finish()
    }
}

impl Report {
    pub fn new(context: &str) -> Self {
        Self {
            context: context.to_string(),
            steps: StepKind::ALL.iter().map(|k| Step::new(*k)).collect(),
            namespaces: None,
            user: None,
            groups: Vec::new(),
            version: None,
            latency: None,
            done: false,
            started: jiff::Timestamp::now(),
            millis: 0,
            oidc: None,
        }
    }

    pub fn step(&self, kind: StepKind) -> &Step {
        self.steps
            .iter()
            .find(|s| s.kind == kind)
            .expect("every step exists")
    }

    fn step_mut(&mut self, kind: StepKind) -> &mut Step {
        self.steps
            .iter_mut()
            .find(|s| s.kind == kind)
            .expect("every step exists")
    }

    /// The first failed step.
    pub fn failure(&self) -> Option<&Step> {
        self.steps.iter().find(|s| s.status == Status::Fail)
    }

    /// Finished without a failed step.
    pub fn passed(&self) -> bool {
        self.done && self.failure().is_none()
    }

    /// `Connected · 38 ms · v1.33.1 · as kubernetes-admin`, or the failure.
    pub fn summary(&self) -> String {
        if !self.done {
            return "Testing…".into();
        }
        if let Some(step) = self.failure() {
            return format!(
                "Failed at {}: {}",
                step.kind.title(),
                step.error.clone().unwrap_or_default()
            );
        }
        let mut parts = vec!["Connected".to_string()];
        if let Some(latency) = self.latency {
            parts.push(format_latency(latency));
        }
        if let Some(version) = &self.version {
            parts.push(version.clone());
        }
        if let Some(user) = &self.user {
            parts.push(format!("as {user}"));
        }
        parts.join(" · ")
    }
}

/// What to test.
#[derive(Clone)]
pub struct Input {
    pub doc: Doc,
    pub context: String,
    /// The kubeconfig's path (relative paths resolve against its folder; it names the client).
    pub file: PathBuf,
    /// The user agreed to run the context's exec plugin (or it's saved and already in use).
    pub allow_exec: bool,
}

struct Run {
    report: Report,
    tx: mpsc::UnboundedSender<Report>,
    started: Instant,
}

impl Run {
    fn send(&mut self) {
        self.report.millis = self.started.elapsed().as_millis() as u64;
        self.tx.unbounded_send(self.report.clone()).ok();
    }

    fn start(&mut self, kind: StepKind) -> Instant {
        self.report.step_mut(kind).status = Status::Running;
        self.send();
        Instant::now()
    }

    fn finish(
        &mut self,
        kind: StepKind,
        status: Status,
        started: Instant,
        f: impl FnOnce(&mut Step),
    ) {
        let step = self.report.step_mut(kind);
        step.status = status;
        step.millis = Some(started.elapsed().as_millis() as u64);
        f(step);
        self.send();
    }

    /// Fails `kind` and skips everything after it.
    fn fail(&mut self, kind: StepKind, started: Instant, error: String, f: impl FnOnce(&mut Step)) {
        self.fail_with(kind, started, error, "Skipped", f);
    }

    /// [`Self::fail`] with why the rest is skipped.
    fn fail_with(
        &mut self,
        kind: StepKind,
        started: Instant,
        error: String,
        note: &str,
        f: impl FnOnce(&mut Step),
    ) {
        self.skip_rest(kind, note);
        self.finish(kind, Status::Fail, started, |step| {
            step.error = Some(error);
            f(step);
        });
    }

    fn skip_rest(&mut self, after: StepKind, note: &str) {
        let mut past = false;
        for step in &mut self.report.steps {
            if past && matches!(step.status, Status::Pending | Status::Running) {
                step.status = Status::Skipped;
                step.lines = vec![note.to_string()];
            }
            if step.kind == after {
                past = true;
            }
        }
    }

    fn done(mut self) -> Report {
        self.report.done = true;
        self.send();
        self.report
    }
}

/// Runs the test. Progress (the whole report) goes to `tx`; the final report is returned.
pub async fn run(input: Input, tx: mpsc::UnboundedSender<Report>) -> Report {
    let mut run = Run {
        report: Report::new(&input.context),
        tx,
        started: Instant::now(),
    };
    run.send();
    let mut doc = input.doc.clone();
    if let Some(dir) = input.file.parent() {
        doc.absolutize_paths(dir);
    }
    let prepared = doc.to_kube().and_then(|kube| {
        let info = kubyl_kube::kubeconfig::context_info(&kube, &input.context, &input.file)
            .ok_or_else(|| format!("no context named \"{}\"", input.context))?;
        match &info.error {
            Some(err) => Err(err.clone()),
            None => Ok((kube, info)),
        }
    });
    let (kube, info) = match prepared {
        Ok(found) => found,
        Err(err) => {
            let t = run.start(StepKind::Network);
            run.fail(StepKind::Network, t, err, |_| {});
            return run.done();
        }
    };
    let Some(server) = info.server.clone().filter(|s| !s.is_empty()) else {
        let t = run.start(StepKind::Network);
        run.fail(
            StepKind::Network,
            t,
            "the cluster has no server URL".into(),
            |_| {},
        );
        return run.done();
    };
    let target = match Target::parse(&server, info.tls_server_name.as_deref()) {
        Ok(target) => target,
        Err(err) => {
            let t = run.start(StepKind::Network);
            run.fail(StepKind::Network, t, err, |_| {});
            return run.done();
        }
    };
    let proxy = info.proxy_url.clone().or_else(|| {
        server
            .parse::<http::Uri>()
            .ok()
            .and_then(|uri| client::env_proxy(&uri))
            .map(|u| u.to_string())
    });

    // DNS and TCP.
    if let Some(proxy) = &proxy {
        let proxy = client::redact_userinfo(proxy);
        let t = run.start(StepKind::Network);
        run.finish(StepKind::Network, Status::Skipped, t, |s| {
            s.lines = vec![format!(
                "through the proxy {proxy}: checked by the API request"
            )];
        });
        let t = run.start(StepKind::Tls);
        run.finish(StepKind::Tls, Status::Skipped, t, |s| {
            s.lines = vec!["through the proxy: the API request verifies the certificate".into()];
        });
    } else {
        if !network(&mut run, &target).await {
            return run.done();
        }
        if !tls_step(&mut run, &target, &kube, &info).await {
            return run.done();
        }
    }

    // Credentials and the client.
    let Some(built) = credentials(&mut run, &input, &kube, &info).await else {
        return run.done();
    };
    let client = built.client.clone();

    // /version.
    let t = run.start(StepKind::Api);
    let unauthorized = match timeout(client.apiserver_version()).await {
        Ok(version) => {
            run.report.version = Some(version.git_version.clone());
            run.finish(StepKind::Api, Status::Ok, t, |s| {
                s.lines = vec![format!(
                    "GET /version · {} · {}",
                    version.git_version, version.platform
                )];
            });
            false
        }
        Err(Failure::Kube(kube::Error::Api(status))) if status.code == 401 => {
            run.finish(StepKind::Api, Status::Ok, t, |s| {
                s.lines = vec!["the API server answered GET /version with 401 Unauthorized".into()];
            });
            true
        }
        Err(err) => {
            let (message, stderr) = err.describe();
            run.fail(StepKind::Api, t, message, |s| s.stderr = stderr);
            return run.done();
        }
    };

    // Authentication.
    let t = run.start(StepKind::Auth);
    let reviews: Api<SelfSubjectReview> = Api::all(client.clone());
    let review = if unauthorized {
        Err(Failure::Unauthorized)
    } else {
        timeout(reviews.create(&PostParams::default(), &SelfSubjectReview::default())).await
    };
    match review {
        Ok(review) => {
            let info_user = review.status.and_then(|s| s.user_info).unwrap_or_default();
            let user = info_user
                .username
                .unwrap_or_else(|| "(unknown user)".into());
            let groups = info_user.groups.unwrap_or_default();
            run.report.user = Some(user.clone());
            run.report.groups = groups.clone();
            run.finish(StepKind::Auth, Status::Ok, t, |s| {
                s.lines = vec![user];
                if !groups.is_empty() {
                    s.lines.push(format!("groups: {}", groups.join(", ")));
                }
            });
        }
        Err(Failure::Kube(kube::Error::Api(status))) if status.code == 404 => {
            run.finish(StepKind::Auth, Status::Warn, t, |s| {
                s.lines = vec!["the server doesn't serve SelfSubjectReview (Kubernetes before 1.28): it answered, but the user name is unknown".into()];
            });
        }
        Err(Failure::Kube(kube::Error::Api(status))) if status.code == 403 => {
            run.finish(StepKind::Auth, Status::Warn, t, |s| {
                s.lines = vec!["authenticated, but not allowed to ask who you are".into()];
            });
        }
        Err(Failure::Unauthorized) | Err(Failure::Kube(kube::Error::Api(_))) => {
            let (message, fix) = unauthorized_message(&input.doc, &info, &kube);
            run.fail(StepKind::Auth, t, message, |s| s.fix = fix);
            return run.done();
        }
        Err(err) => {
            let (message, stderr) = err.describe();
            let sign_in = matches!(&err, Failure::Kube(e) if matches!(ConnectError::from_kube(e), ConnectError::SignInRequired));
            run.fail(StepKind::Auth, t, message, |s| {
                s.stderr = stderr;
                if sign_in {
                    s.fix = Some(Fix::SignIn);
                }
            });
            return run.done();
        }
    }

    // Permissions.
    let namespace = info
        .namespace
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "default".into());
    let t = run.start(StepKind::Permissions);
    let list_ns = can(&client, "list", "", "namespaces", None).await;
    let list_pods = can(&client, "list", "", "pods", Some(&namespace)).await;
    let admin = can(&client, "*", "*", "*", None).await;
    let mut namespaces = None;
    if list_ns == Some(true) {
        let api: Api<Namespace> = Api::all(client.clone());
        if let Ok(list) = timeout(api.list_metadata(&ListParams::default().limit(500))).await {
            let mut names: Vec<String> = list
                .items
                .into_iter()
                .filter_map(|n| n.metadata.name)
                .collect();
            names.sort();
            namespaces = Some(names);
        }
    }
    let count = namespaces.as_ref().map(Vec::len);
    run.report.namespaces = namespaces;
    let status = if list_pods == Some(true) || list_ns == Some(true) {
        Status::Ok
    } else {
        Status::Warn
    };
    run.finish(StepKind::Permissions, status, t, |s| {
        s.checks = vec![
            Check {
                allowed: list_ns,
                label: match count {
                    Some(n) => format!("list namespaces ({n})"),
                    None => "list namespaces".into(),
                },
                optional: false,
            },
            Check {
                allowed: list_pods,
                label: format!("list pods in {namespace}"),
                optional: false,
            },
            Check {
                allowed: admin,
                label: if admin == Some(true) {
                    "cluster-admin".into()
                } else {
                    "cluster-admin (not required)".into()
                },
                optional: true,
            },
        ];
        if status == Status::Warn {
            s.lines = vec![format!(
                "this user can't list namespaces or pods in {namespace}"
            )];
        }
    });

    // Latency.
    let t = run.start(StepKind::Latency);
    let mut samples = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        if timeout(client.apiserver_version()).await.is_ok() {
            samples.push(started.elapsed());
        }
    }
    samples.sort_unstable();
    match samples.get(samples.len() / 2).copied() {
        Some(median) => {
            run.report.latency = Some(median);
            let count = samples.len();
            run.finish(StepKind::Latency, Status::Ok, t, |s| {
                s.lines = vec![format!("{} (median of {count})", format_latency(median))];
            });
        }
        None => run.finish(StepKind::Latency, Status::Warn, t, |s| {
            s.lines = vec!["the latency requests failed".into()];
        }),
    }
    run.done()
}

async fn network(run: &mut Run, target: &Target) -> bool {
    let t = run.start(StepKind::Network);
    let lookup = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio::net::lookup_host((target.host.as_str(), target.port)),
    )
    .await;
    let addrs: Vec<std::net::SocketAddr> = match lookup {
        Err(_) => {
            run.fail(
                StepKind::Network,
                t,
                format!("looking up {} timed out", target.host),
                |_| {},
            );
            return false;
        }
        Ok(Err(err)) => {
            run.fail(
                StepKind::Network,
                t,
                format!("couldn't resolve {}: {err}", target.host),
                |_| {},
            );
            return false;
        }
        Ok(Ok(addrs)) => addrs.collect(),
    };
    let mut last_err = String::from("no addresses");
    for addr in &addrs {
        match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(addr)).await {
            Ok(Ok(_)) => {
                let via = if target.host.parse::<std::net::IpAddr>().is_ok() {
                    target.authority()
                } else {
                    format!("{} → {addr}", target.host)
                };
                run.finish(StepKind::Network, Status::Ok, t, |s| s.lines = vec![via]);
                return true;
            }
            Ok(Err(err)) => {
                last_err = match err.kind() {
                    std::io::ErrorKind::ConnectionRefused => {
                        format!(
                            "{addr} refused the connection (nothing listens on port {})",
                            target.port
                        )
                    }
                    _ => format!("couldn't connect to {addr}: {err}"),
                }
            }
            Err(_) => last_err = format!("connecting to {addr} timed out (a firewall or VPN?)"),
        }
    }
    run.fail(StepKind::Network, t, last_err, |_| {});
    false
}

/// The cluster entry of the context.
fn cluster_of<'a>(
    kube: &'a Kubeconfig,
    info: &kubyl_kube::kubeconfig::ContextInfo,
) -> Option<&'a kube::config::Cluster> {
    kube.clusters
        .iter()
        .find(|c| c.name == info.cluster)
        .and_then(|c| c.cluster.as_ref())
}

fn user_of<'a>(
    kube: &'a Kubeconfig,
    info: &kubyl_kube::kubeconfig::ContextInfo,
) -> Option<&'a kube::config::AuthInfo> {
    let user = info.user.as_deref()?;
    kube.auth_infos
        .iter()
        .find(|u| u.name == user)
        .and_then(|u| u.auth_info.as_ref())
}

async fn tls_step(
    run: &mut Run,
    target: &Target,
    kube: &Kubeconfig,
    info: &kubyl_kube::kubeconfig::ContextInfo,
) -> bool {
    let t = run.start(StepKind::Tls);
    if !target.https {
        run.finish(StepKind::Tls, Status::Warn, t, |s| {
            s.lines = vec!["plain HTTP: no TLS, credentials travel unencrypted".into()];
        });
        return true;
    }
    let cluster = cluster_of(kube, info);
    let (trust, trust_label) = if info.insecure_skip_tls_verify {
        (
            Trust::Insecure,
            "not verified (insecure-skip-tls-verify)".to_string(),
        )
    } else {
        let pem = match cluster {
            Some(c) if c.certificate_authority_data.is_some() => {
                certs::pem_from_data(c.certificate_authority_data.as_deref().unwrap_or_default())
                    .map(Some)
            }
            Some(c) => match &c.certificate_authority {
                Some(path) => std::fs::read_to_string(path)
                    .map(Some)
                    .map_err(|e| format!("can't read the CA file {path}: {e}")),
                None => Ok(None),
            },
            None => Ok(None),
        };
        match pem {
            Err(err) => {
                run.fail_with(
                    StepKind::Tls,
                    t,
                    err,
                    "Skipped: no credentials were sent",
                    |_| {},
                );
                return false;
            }
            Ok(Some(pem)) => {
                let label = certs::certificates(&pem)
                    .ok()
                    .and_then(|c| c.first().map(|c| c.common_name()))
                    .map(|cn| format!("verified by the kubeconfig's CA \"{cn}\""))
                    .unwrap_or_else(|| "verified by the kubeconfig's CA".into());
                (Trust::Pem(pem), label)
            }
            Ok(None) => (Trust::System, "verified by the system trust store".into()),
        }
    };
    match tls::check(target, &trust).await {
        Ok(handshake) => {
            let status = if matches!(trust, Trust::Insecure) {
                Status::Warn
            } else {
                Status::Ok
            };
            run.finish(StepKind::Tls, status, t, |s| {
                s.lines = vec![format!("{} · {trust_label}", handshake.protocol)];
                if let Some(leaf) = handshake.chain.first() {
                    s.lines.push(format!(
                        "server certificate {} · valid until {}",
                        leaf.common_name(),
                        leaf.not_after_date()
                    ));
                }
            });
            true
        }
        Err((err, served)) => {
            let fix = matches!(err, TlsError::UnknownIssuer).then_some(Fix::FetchCa);
            run.fail_with(
                StepKind::Tls,
                t,
                err.message(),
                "Skipped: no credentials were sent",
                |s| {
                    s.fix = fix;
                    if let Some(leaf) = served.first() {
                        s.lines = vec![format!(
                            "the server presented {} issued by {}",
                            leaf.common_name(),
                            certs::common_name(&leaf.issuer)
                        )];
                    }
                },
            );
            false
        }
    }
}

async fn credentials(
    run: &mut Run,
    input: &Input,
    kube: &Kubeconfig,
    info: &kubyl_kube::kubeconfig::ContextInfo,
) -> Option<client::BuiltClient> {
    let t = run.start(StepKind::Credentials);
    let user = user_of(kube, info);
    let mut lines = Vec::new();
    let mut status = Status::Ok;
    match &info.auth {
        AuthMethod::Exec(summary) => {
            let Some(exec_config) = user.and_then(|u| u.exec.clone()) else {
                run.fail(
                    StepKind::Credentials,
                    t,
                    "the user has no exec config".into(),
                    |_| {},
                );
                return None;
            };
            if !input.allow_exec {
                run.fail(
                    StepKind::Credentials,
                    t,
                    format!("Kubyl needs your OK to run `{}`", summary.command),
                    |_| {},
                );
                return None;
            }
            let hint = exec_config.install_hint.clone();
            let cluster = cluster_of(kube, info).and_then(|c| ExecAuthCluster::try_from(c).ok());
            let auth = ExecAuth::new(info.name.clone(), exec_config, cluster);
            // A test always runs the plugin, like kubectl would (nothing cached).
            auth.invalidate().await;
            match auth.credential().await {
                Ok(exec::Credential::Token(token)) => {
                    let expires = auth
                        .expires_at()
                        .await
                        .or_else(|| kubyl_kube::auth::jwt_expiry(token.expose_secret()));
                    lines.push(format!(
                        "{} · token{}",
                        summary_line(summary),
                        expiry_note(expires)
                    ));
                }
                Ok(exec::Credential::ClientCertificate { certificate, .. }) => {
                    let cert = certs::certificates(&certificate)
                        .ok()
                        .and_then(|c| c.into_iter().next());
                    lines.push(format!(
                        "{} · client certificate{}",
                        summary_line(summary),
                        cert.map(|c| format!(
                            " {} · valid until {}",
                            c.common_name(),
                            c.not_after_date()
                        ))
                        .unwrap_or_default()
                    ));
                }
                Err(AuthError::Exec { message, stderr }) => {
                    let not_found = message.contains("was not found");
                    let fix = match (&hint, not_found) {
                        (Some(hint), true) => Some(Fix::InstallHint(hint.trim().to_string())),
                        _ => None,
                    };
                    let message = if not_found && hint.is_none() {
                        format!(
                            "`{}` isn't installed (or not in your login shell's PATH)",
                            summary.command
                        )
                    } else if not_found {
                        format!(
                            "`{}` isn't installed: see the install hint",
                            summary.command
                        )
                    } else {
                        message
                    };
                    run.fail(StepKind::Credentials, t, message, |s| {
                        s.stderr = stderr;
                        s.fix = fix;
                    });
                    return None;
                }
                Err(err) => {
                    run.fail(StepKind::Credentials, t, err.to_string(), |_| {});
                    return None;
                }
            }
        }
        AuthMethod::Token => {
            let token = user
                .and_then(|u| u.token.as_ref())
                .map(|t| t.expose_secret().to_string());
            let expiry = token.as_deref().and_then(kubyl_kube::auth::jwt_expiry);
            if let Some(exp) = expiry.filter(|e| *e < jiff::Timestamp::now()) {
                status = Status::Warn;
                lines.push(format!(
                    "bearer token · expired on {} (the server decides)",
                    exp.strftime("%Y-%m-%d %H:%M UTC")
                ));
            } else {
                lines.push(format!("bearer token{}", expiry_note(expiry)));
            }
        }
        AuthMethod::TokenFile => {
            let path = user.and_then(|u| u.token_file.clone()).unwrap_or_default();
            if let Err(err) = std::fs::metadata(&path) {
                run.fail(
                    StepKind::Credentials,
                    t,
                    format!("can't read the token file {path}: {err}"),
                    |_| {},
                );
                return None;
            }
            lines.push(format!("token file {path}"));
        }
        AuthMethod::ClientCertificate => {
            let pem =
                user.and_then(
                    |u| match (&u.client_certificate_data, &u.client_certificate) {
                        (Some(data), _) => certs::pem_from_data(data).ok(),
                        (None, Some(path)) => std::fs::read_to_string(path).ok(),
                        _ => None,
                    },
                );
            match pem
                .and_then(|p| certs::certificates(&p).ok())
                .and_then(|c| c.into_iter().next())
            {
                Some(cert) if cert.expired() => {
                    run.fail(
                        StepKind::Credentials,
                        t,
                        format!(
                            "the client certificate expired on {}",
                            cert.not_after_date()
                        ),
                        |_| {},
                    );
                    return None;
                }
                Some(cert) => lines.push(format!(
                    "client certificate {} · issued by {} · valid until {}",
                    cert.common_name(),
                    certs::common_name(&cert.issuer),
                    cert.not_after_date()
                )),
                None => {
                    run.fail(
                        StepKind::Credentials,
                        t,
                        "the client certificate can't be read".into(),
                        |_| {},
                    );
                    return None;
                }
            }
        }
        AuthMethod::Basic => {
            status = Status::Warn;
            lines.push("basic auth (removed in Kubernetes 1.19)".into());
        }
        AuthMethod::None => lines.push("no credentials: requests are anonymous".into()),
        AuthMethod::Oidc(params) => lines.push(format!("OIDC · {}", params.issuer_host())),
        AuthMethod::OpenShift => lines.push("OpenShift OAuth token".into()),
        AuthMethod::Provider(name) => {
            status = Status::Warn;
            lines.push(format!("auth provider {name} (removed from kubectl 1.26)"));
        }
    }

    let built = match client::build(info, Arc::new(kube.clone())).await {
        Ok(built) => built,
        Err(err) => {
            let (message, stderr) = describe_connect(&err);
            run.fail(StepKind::Credentials, t, message, |s| s.stderr = stderr);
            return None;
        }
    };
    if let Some(CredentialSource::Oidc(oidc)) = &built.credentials {
        run.report.oidc = Some(oidc.clone());
        match oidc.token().await {
            Ok(_) => lines.push(format!("ID token{}", expiry_note(oidc.expires_at().await))),
            Err(AuthError::SignInRequired) => {
                run.fail(
                    StepKind::Credentials,
                    t,
                    "sign in with your identity provider to test this context".into(),
                    |s| {
                        s.fix = Some(Fix::SignIn);
                        s.lines = lines;
                    },
                );
                return None;
            }
            Err(err) => {
                run.fail(StepKind::Credentials, t, err.to_string(), |s| {
                    s.lines = lines
                });
                return None;
            }
        }
    }
    run.finish(StepKind::Credentials, status, t, |s| s.lines = lines);
    Some(built)
}

/// `38 ms`, or `0.4 ms` below 10 ms.
pub fn format_latency(latency: Duration) -> String {
    let ms = latency.as_secs_f64() * 1000.0;
    if ms < 10.0 {
        format!("{ms:.1} ms")
    } else {
        format!("{ms:.0} ms")
    }
}

fn summary_line(summary: &kubyl_kube::auth::ExecSummary) -> String {
    let mut line = summary.command.clone();
    if !summary.subcommands.is_empty() {
        line.push(' ');
        line.push_str(&summary.subcommands.join(" "));
    }
    line
}

fn expiry_note(expires: Option<jiff::Timestamp>) -> String {
    match expires {
        Some(at) => {
            let minutes = (at.as_second() - jiff::Timestamp::now().as_second()) / 60;
            if minutes < 0 {
                format!(" · expired {}", at.strftime("%Y-%m-%d %H:%M UTC"))
            } else if minutes < 120 {
                format!(" · valid for {minutes} min")
            } else {
                format!(" · valid until {}", at.strftime("%Y-%m-%d %H:%M UTC"))
            }
        }
        None => String::new(),
    }
}

/// Why a 401 happened, in words, for the auth method in use.
fn unauthorized_message(
    _doc: &Doc,
    info: &kubyl_kube::kubeconfig::ContextInfo,
    kube: &Kubeconfig,
) -> (String, Option<Fix>) {
    let user = user_of(kube, info);
    match &info.auth {
        AuthMethod::Token | AuthMethod::OpenShift => {
            let expired = user
                .and_then(|u| u.token.as_ref())
                .and_then(|t| kubyl_kube::auth::jwt_expiry(t.expose_secret()))
                .filter(|e| *e < jiff::Timestamp::now());
            match expired {
                Some(at) => (
                    format!(
                        "the token expired on {}: the API server rejected it (401)",
                        at.strftime("%Y-%m-%d %H:%M UTC")
                    ),
                    Some(Fix::ReplaceToken),
                ),
                None => (
                    "the API server rejected the token (401): it's wrong, revoked or expired".into(),
                    Some(Fix::ReplaceToken),
                ),
            }
        }
        AuthMethod::TokenFile => ("the API server rejected the token from the token file (401)".into(), None),
        AuthMethod::Exec(summary) => (
            format!("the API server rejected the token from `{}` (401)", summary.command),
            None,
        ),
        AuthMethod::ClientCertificate => (
            "the API server didn't accept the client certificate (401): is it signed by this cluster's client CA?".into(),
            None,
        ),
        AuthMethod::Oidc(_) => (
            "the API server rejected the ID token (401): is the cluster set up for this issuer and client ID?".into(),
            Some(Fix::SignIn),
        ),
        AuthMethod::None => ("anonymous requests aren't allowed (401): add credentials".into(), None),
        _ => ("the API server rejected the credentials (401)".into(), None),
    }
}

fn describe_connect(err: &ConnectError) -> (String, Option<String>) {
    match err {
        ConnectError::Auth { message, detail } => (message.clone(), detail.clone()),
        ConnectError::SignInRequired => ("sign-in required".into(), None),
        ConnectError::Forbidden(m) => (format!("forbidden: {m}"), None),
        ConnectError::Unreachable(m) => (m.clone(), None),
        ConnectError::Config(m) => (format!("the kubeconfig can't be used: {m}"), None),
    }
}

enum Failure {
    Kube(kube::Error),
    Timeout,
    /// Known to be rejected already (`/version` said 401).
    Unauthorized,
}

impl Failure {
    fn describe(&self) -> (String, Option<String>) {
        match self {
            Failure::Timeout => ("the API server didn't answer within 15 s".into(), None),
            Failure::Unauthorized => ("the API server rejected the credentials (401)".into(), None),
            Failure::Kube(err) => describe_connect(&ConnectError::from_kube(err)),
        }
    }
}

async fn timeout<T>(
    fut: impl std::future::Future<Output = Result<T, kube::Error>>,
) -> Result<T, Failure> {
    match tokio::time::timeout(REQUEST_TIMEOUT, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(Failure::Kube(err)),
        Err(_) => Err(Failure::Timeout),
    }
}

/// A SelfSubjectAccessReview.
async fn can(
    client: &Client,
    verb: &str,
    group: &str,
    resource: &str,
    namespace: Option<&str>,
) -> Option<bool> {
    let api: Api<SelfSubjectAccessReview> = Api::all(client.clone());
    let review = SelfSubjectAccessReview {
        spec: SelfSubjectAccessReviewSpec {
            resource_attributes: Some(ResourceAttributes {
                verb: Some(verb.into()),
                group: Some(group.into()),
                resource: Some(resource.into()),
                namespace: namespace.map(String::from),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    timeout(api.create(&PostParams::default(), &review))
        .await
        .ok()
        .and_then(|r| r.status)
        .map(|s| s.allowed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certs::tests::{CA, OTHER_CA};

    fn doc(server: &str, ca: &str, user: &str) -> Doc {
        Doc::parse(&format!(
            "apiVersion: v1\nkind: Config\nclusters:\n- name: c\n  cluster:\n    server: {server}\n    certificate-authority-data: {}\ncontexts:\n- name: ctx\n  context: {{cluster: c, user: u}}\nusers:\n- name: u\n  user:\n{user}",
            certs::data_from_pem(ca)
        ))
        .unwrap()
    }

    async fn run_test(doc: Doc, allow_exec: bool) -> Report {
        let (tx, _rx) = mpsc::unbounded();
        run(
            Input {
                doc,
                context: "ctx".into(),
                file: PathBuf::from("/tmp/kubyl-test/config"),
                allow_exec,
            },
            tx,
        )
        .await
    }

    #[tokio::test]
    async fn a_wrong_ca_fails_at_tls_before_credentials_are_sent() {
        let (port, requests) = crate::tls::tests::server(String::new()).await;
        let report = run_test(
            doc(
                &format!("https://127.0.0.1:{port}"),
                OTHER_CA,
                "    token: s3cret\n",
            ),
            false,
        )
        .await;
        assert!(report.done && !report.passed());
        assert_eq!(report.step(StepKind::Network).status, Status::Ok);
        let tls = report.step(StepKind::Tls);
        assert_eq!(tls.status, Status::Fail);
        assert_eq!(tls.fix, Some(Fix::FetchCa));
        assert!(tls.error.as_deref().unwrap().contains("unknown authority"));
        assert_eq!(report.step(StepKind::Credentials).status, Status::Skipped);
        assert_eq!(
            report.step(StepKind::Credentials).lines,
            ["Skipped: no credentials were sent"]
        );
        // Nothing was sent to the server.
        assert!(requests.lock().is_empty());
        assert!(report.summary().starts_with("Failed at TLS"));
    }

    #[tokio::test]
    async fn exec_plugins_need_consent_and_missing_ones_say_so() {
        let (port, _) = crate::tls::tests::server(String::new()).await;
        let exec = "    exec:\n      apiVersion: client.authentication.k8s.io/v1\n      command: kubyl-missing-plugin-zzz\n      installHint: brew install kubyl-missing-plugin\n";
        let server = format!("https://127.0.0.1:{port}");
        let report = run_test(doc(&server, CA, exec), false).await;
        let step = report.step(StepKind::Credentials);
        assert_eq!(step.status, Status::Fail);
        assert!(step.error.as_deref().unwrap().contains("needs your OK"));

        let report = run_test(doc(&server, CA, exec), true).await;
        let step = report.step(StepKind::Credentials);
        assert_eq!(step.status, Status::Fail, "{report:?}");
        assert!(
            step.error.as_deref().unwrap().contains("isn't installed"),
            "{step:?}"
        );
        assert_eq!(
            step.fix,
            Some(Fix::InstallHint("brew install kubyl-missing-plugin".into()))
        );
        assert_eq!(report.step(StepKind::Tls).status, Status::Ok);
    }

    #[tokio::test]
    async fn unreachable_servers_fail_at_the_network_step() {
        let report = run_test(doc("https://127.0.0.1:1", CA, "    token: x\n"), false).await;
        let step = report.step(StepKind::Network);
        assert_eq!(step.status, Status::Fail);
        assert!(
            step.error.as_deref().unwrap().contains("refused"),
            "{step:?}"
        );
        let report = run_test(doc("ftp://nope", CA, "    token: x\n"), false).await;
        assert_eq!(report.step(StepKind::Network).status, Status::Fail);
    }
}
