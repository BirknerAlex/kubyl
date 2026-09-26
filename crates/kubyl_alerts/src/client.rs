//! Talking to Alertmanager (API v2, 0.22 or newer) and gathering what discovery needs.
//!
//! Transports, in order (see the README's "Alertmanager credentials"):
//! - the API server's service proxy with the cluster's client: nothing reaches Alertmanager but
//!   the API server's request, which authenticated the user;
//! - behind an auth proxy (401/403 through the proxy), and only when [`token_allowed`]: the
//!   Service's admitted Route with the user's token or a minted service-account token
//!   (OpenShift's `alertmanager-main`), else a temporary loopback forward verified against the
//!   service CA (OpenShift's user workload Alertmanager, which has no Route);
//! - an external URL with the Authorization header from the keychain (never with
//!   `insecure_skip_tls_verify`), an optional CA file and client certificate.

use std::time::Duration;

use kubyl_kube::auth::BearerToken;
use kubyl_metrics::openshift::{self, Bearer, ServiceAccountToken};
use kubyl_metrics::prometheus::{PromClient, PromError};
use kubyl_metrics::transport::{ExternalTls, Transport};
use secrecy::SecretString;
use serde_json::{Value, json};

use crate::discover::{
    self, AmTarget, Candidate, OPENSHIFT_MONITORING, OPENSHIFT_USER_WORKLOAD,
    WELL_KNOWN_NAMESPACES, token_allowed,
};
use crate::matchers::Matcher;
use crate::model::{AmStatus, parse_status};
use crate::settings::ClusterSettings;

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
/// Candidates probed at most (settings entries are always probed).
const MAX_PROBES: usize = 6;

/// How Kubyl reaches an Alertmanager.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Via {
    /// The API server's service proxy.
    Proxy,
    /// An OpenShift Route (`https://<admitted host>`).
    Route { url: String },
    /// A temporary loopback forward (kept by the service while it's used).
    Forward { local_port: u16 },
    /// An external URL.
    Url,
}

/// A working connection to one Alertmanager.
#[derive(Clone, Debug)]
pub struct AmConn {
    pub target: AmTarget,
    pub transport: Transport,
    pub via: Via,
    pub status: AmStatus,
}

impl AmConn {
    /// `monitoring/alertmanager-operated`.
    pub fn label(&self) -> String {
        self.target.label()
    }

    /// Writes use a service account's token (the silence dialog asks first): its
    /// `namespace/name`.
    pub fn service_account(&self) -> Option<String> {
        match self.transport.bearer()? {
            bearer @ Bearer::ServiceAccount(_) => Some(
                bearer
                    .describe()
                    .trim_start_matches("service account ")
                    .to_string(),
            ),
            Bearer::User(_) => None,
        }
    }

    async fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Value, PromError> {
        self.transport.get(path, params).await
    }

    pub async fn status(&self) -> Result<AmStatus, PromError> {
        status(&self.transport).await
    }

    /// Active, silenced, inhibited and unprocessed alerts, filtered by `matchers` (a central
    /// Alertmanager's cluster matchers).
    pub async fn alerts(&self, matchers: &[Matcher]) -> Result<Value, PromError> {
        let mut params: Vec<(&str, String)> = vec![
            ("active", "true".into()),
            ("silenced", "true".into()),
            ("inhibited", "true".into()),
            ("unprocessed", "true".into()),
        ];
        params.extend(matchers.iter().map(|m| ("filter", m.to_string())));
        self.get("/api/v2/alerts", &params).await
    }

    pub async fn silences(&self, matchers: &[Matcher]) -> Result<Value, PromError> {
        let params: Vec<(&str, String)> =
            matchers.iter().map(|m| ("filter", m.to_string())).collect();
        self.get("/api/v2/silences", &params).await
    }

    pub async fn receivers(&self) -> Result<Value, PromError> {
        self.get("/api/v2/receivers", &[]).await
    }

    /// Creates (or, with an `id`, replaces) a silence. Returns its id.
    pub async fn post_silence(&self, silence: &Value) -> Result<String, PromError> {
        let answer = self
            .transport
            .post_json("/api/v2/silences", silence)
            .await?;
        answer["silenceID"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| PromError::Invalid("no silenceID in the answer".into()))
    }

    /// Expires a silence.
    pub async fn expire_silence(&self, id: &str) -> Result<(), PromError> {
        self.transport
            .delete(&format!("/api/v2/silence/{}", url_segment(id)))
            .await
    }
}

fn url_segment(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// The body of `POST /api/v2/silences`.
pub fn silence_body(
    id: Option<&str>,
    matchers: &[Matcher],
    starts_at: jiff::Timestamp,
    ends_at: jiff::Timestamp,
    created_by: &str,
    comment: &str,
) -> Value {
    let mut body = json!({
        "matchers": matchers.iter().map(Matcher::to_json).collect::<Vec<_>>(),
        "startsAt": starts_at.to_string(),
        "endsAt": ends_at.to_string(),
        "createdBy": created_by,
        "comment": comment,
    });
    if let Some(id) = id {
        body["id"] = json!(id);
    }
    body
}

async fn status(transport: &Transport) -> Result<AmStatus, PromError> {
    let body = match tokio::time::timeout(PROBE_TIMEOUT, transport.get("/api/v2/status", &[])).await
    {
        Ok(result) => result?,
        Err(_) => return Err(PromError::Timeout),
    };
    parse_status(&body).map_err(PromError::Invalid)
}

/// What a connection may authenticate with. Never logged.
#[derive(Clone)]
pub struct Credentials {
    /// The user's bearer token (not for client-certificate users).
    pub user_token: Option<BearerToken>,
    /// Authorization headers of external URLs from the keychain, by URL.
    pub headers: Vec<(String, SecretString)>,
}

/// The outcome of trying one candidate.
#[derive(Clone, Debug)]
pub enum Probed {
    Connected(Box<AmConn>),
    /// Behind an auth proxy without a Route: needs a temporary forward to its port.
    NeedsForward(AmTarget, String),
    Failed(AmTarget, String),
}

/// A plain-language reason, naming what's missing on a 403.
pub fn explain(target: &AmTarget, via: &str, err: &PromError) -> String {
    match (target, err) {
        (AmTarget::Service { namespace, .. }, PromError::Http(403, _)) if via == "proxy" => {
            format!("needs get services/proxy in {namespace}")
        }
        (AmTarget::Service { namespace, .. }, PromError::Http(401 | 403, _))
            if namespace == OPENSHIFT_MONITORING =>
        {
            "needs the monitoring-alertmanager-view role in openshift-monitoring \
             (monitoring-alertmanager-edit for silences)"
                .into()
        }
        (AmTarget::Service { namespace, .. }, PromError::Http(401 | 403, _))
            if namespace == OPENSHIFT_USER_WORKLOAD =>
        {
            "needs the monitoring-alertmanager-api-reader role in \
             openshift-user-workload-monitoring (-api-writer for silences)"
                .into()
        }
        (_, PromError::Http(404, _)) => "no Alertmanager API there (404)".into(),
        (_, PromError::Invalid(_)) => "not an Alertmanager API v2 (needs 0.22 or newer)".into(),
        (_, err) => err.to_string(),
    }
}

/// Tries one candidate: the service proxy, then (only for [`token_allowed`] targets that
/// answered 401/403) the Route; external URLs directly.
pub async fn probe(client: &kube::Client, target: &AmTarget, credentials: &Credentials) -> Probed {
    match target {
        AmTarget::Url {
            url,
            tenant,
            ca_file,
            client_certificate,
            client_key,
            insecure,
        } => {
            let roots = match ca_file {
                Some(path) => match tokio::fs::read_to_string(path).await {
                    Ok(pem) => Some(openshift::pem_certificates(&pem)),
                    Err(err) => {
                        return Probed::Failed(target.clone(), format!("CA file {path}: {err}"));
                    }
                },
                None => None,
            };
            let tls = ExternalTls {
                insecure: *insecure,
                roots,
                client_certificate: client_certificate.as_ref().map(Into::into),
                client_key: client_key.as_ref().map(Into::into),
            };
            let header = credentials
                .headers
                .iter()
                .find(|(u, _)| u == url)
                .map(|(_, h)| h.clone());
            // The header needs verified TLS.
            let header = header.filter(|_| !insecure);
            let transport = match Transport::external(url, header.as_ref(), &tls)
                .and_then(|t| with_tenant(t, tenant.as_deref()))
            {
                Ok(t) => t,
                Err(err) => return Probed::Failed(target.clone(), err.to_string()),
            };
            match status(&transport).await {
                Ok(status) => Probed::Connected(Box::new(AmConn {
                    target: target.clone(),
                    transport,
                    via: Via::Url,
                    status,
                })),
                Err(err) => Probed::Failed(target.clone(), explain(target, "url", &err)),
            }
        }
        AmTarget::Service {
            namespace,
            service,
            port,
            scheme,
            path,
            service_account,
            tenant,
            ..
        } => {
            let transport =
                Transport::service_proxy(client.clone(), namespace, service, port, scheme, path);
            let transport = match with_tenant(transport, tenant.as_deref()) {
                Ok(t) => t,
                Err(err) => return Probed::Failed(target.clone(), err.to_string()),
            };
            let err = match status(&transport).await {
                Ok(status) => {
                    return Probed::Connected(Box::new(AmConn {
                        target: target.clone(),
                        transport,
                        via: Via::Proxy,
                        status,
                    }));
                }
                Err(err) => err,
            };
            // Behind an auth proxy the API server's stripped credentials get a 401/403. Only
            // trusted Services may get a token.
            if !matches!(err, PromError::Http(401 | 403, _)) || !token_allowed(target) {
                return Probed::Failed(target.clone(), explain(target, "proxy", &err));
            }
            match openshift::through_route(
                client,
                namespace,
                service,
                path,
                credentials.user_token.clone(),
                service_account.clone(),
                "/api/v2/status",
            )
            .await
            {
                Ok(routed) => match status(&routed.transport).await {
                    Ok(status) => Probed::Connected(Box::new(AmConn {
                        target: target.clone(),
                        transport: routed.transport,
                        via: Via::Route { url: routed.url },
                        status,
                    })),
                    Err(err) => Probed::Failed(target.clone(), explain(target, "route", &err)),
                },
                Err(route) if route.contains("has no Route") => Probed::NeedsForward(
                    target.clone(),
                    format!(
                        "{} through the API server; no Route",
                        explain(target, "proxy", &err)
                    ),
                ),
                Err(route) => Probed::Failed(
                    target.clone(),
                    format!("{} ({route})", explain(target, "route", &err)),
                ),
            }
        }
    }
}

fn with_tenant(transport: Transport, tenant: Option<&str>) -> Result<Transport, PromError> {
    match tenant.filter(|t| !t.is_empty()) {
        Some(tenant) => transport.with_header("x-scope-orgid", tenant),
        None => Ok(transport),
    }
}

/// The service CA OpenShift injects into every namespace (`openshift-service-ca.crt`).
pub async fn service_ca(client: &kube::Client, namespace: &str) -> Option<Vec<Vec<u8>>> {
    let request = http::Request::get(format!(
        "/api/v1/namespaces/{namespace}/configmaps/openshift-service-ca.crt"
    ))
    .body(Vec::new())
    .ok()?;
    let map: Value = client.request(request).await.ok()?;
    let certs = openshift::pem_certificates(map.pointer("/data/service-ca.crt")?.as_str()?);
    (!certs.is_empty()).then_some(certs)
}

/// Connects through a temporary loopback forward to a trusted Service behind an auth proxy:
/// HTTPS verified against the service CA with the Service's DNS name, with the user's token,
/// else the settings' service account.
pub async fn probe_forward(
    client: &kube::Client,
    target: &AmTarget,
    local_port: u16,
    credentials: &Credentials,
) -> Probed {
    let AmTarget::Service {
        namespace,
        service,
        path,
        service_account,
        tenant,
        ..
    } = target
    else {
        return Probed::Failed(target.clone(), "not a Service".into());
    };
    if !token_allowed(target) {
        return Probed::Failed(target.clone(), "not a trusted Service".into());
    }
    let Some(roots) = service_ca(client, namespace).await else {
        return Probed::Failed(
            target.clone(),
            format!("no service CA in {namespace} to verify the forward"),
        );
    };
    let server_name = format!("{service}.{namespace}.svc");
    let url = format!(
        "https://127.0.0.1:{local_port}{}",
        kubyl_metrics::transport::normalize_prefix(path)
    );
    let mut bearers = Vec::new();
    if let Some(user) = &credentials.user_token {
        bearers.push(Bearer::User(user.clone()));
    }
    if let Some((ns, name)) = service_account {
        bearers.push(Bearer::ServiceAccount(std::sync::Arc::new(
            ServiceAccountToken::new(client.clone(), ns, name),
        )));
    }
    if bearers.is_empty() {
        return Probed::Failed(
            target.clone(),
            "no token for its auth proxy: you sign in with a client certificate; name a \
             service_account in alerts.clusters"
                .into(),
        );
    }
    let mut errors = Vec::new();
    for bearer in bearers {
        let transport = match Transport::direct(
            &url,
            Some(roots.clone()),
            Some(server_name.clone()),
            bearer.clone(),
        )
        .and_then(|t| with_tenant(t, tenant.as_deref()))
        {
            Ok(t) => t,
            Err(err) => {
                errors.push(err.to_string());
                continue;
            }
        };
        match status(&transport).await {
            Ok(status) => {
                return Probed::Connected(Box::new(AmConn {
                    target: target.clone(),
                    transport,
                    via: Via::Forward { local_port },
                    status,
                }));
            }
            Err(err) => errors.push(format!(
                "with {}: {}",
                bearer.describe(),
                explain(target, "forward", &err)
            )),
        }
    }
    Probed::Failed(target.clone(), errors.join("; "))
}

/// What discovery reads from the cluster.
#[derive(Clone, Debug, Default)]
pub struct Gathered {
    pub services: Vec<Value>,
    pub prometheuses: Vec<Value>,
    pub alertmanagers: Vec<Value>,
    pub slices: Vec<Value>,
    /// Prometheus' `activeAlertmanagers` (`None`: unknown, e.g. Thanos).
    pub prometheus_alertmanagers: Option<Vec<String>>,
}

async fn list(client: &kube::Client, path: &str) -> Result<Vec<Value>, kube::Error> {
    let request = http::Request::get(path)
        .body(Vec::new())
        .map_err(kube::Error::HttpError)?;
    let list: Value = client.request(request).await?;
    Ok(list["items"].as_array().cloned().unwrap_or_default())
}

/// Services cluster-wide, or in the well-known namespaces when that's forbidden.
pub async fn list_services(client: &kube::Client) -> Vec<Value> {
    match list(client, "/api/v1/services").await {
        Ok(items) => items,
        Err(_) => {
            let mut items = Vec::new();
            for ns in WELL_KNOWN_NAMESPACES {
                if let Ok(found) = list(client, &format!("/api/v1/namespaces/{ns}/services")).await
                {
                    items.extend(found);
                }
            }
            items
        }
    }
}

/// Reads Services, prometheus-operator objects and Prometheus' Alertmanager list.
pub async fn gather(client: &kube::Client, prometheus: Option<&PromClient>) -> Gathered {
    let services = list_services(client).await;
    let prometheuses = list(client, "/apis/monitoring.coreos.com/v1/prometheuses")
        .await
        .unwrap_or_default();
    let alertmanagers = list(client, "/apis/monitoring.coreos.com/v1/alertmanagers")
        .await
        .unwrap_or_default();
    let prometheus_alertmanagers = match prometheus {
        Some(prom) => prom
            .api("/api/v1/alertmanagers", &[])
            .await
            .ok()
            .and_then(|body| crate::model::parse_alertmanagers(&body).ok()),
        None => None,
    };
    // Only needed to map Prometheus' pod URLs when the operator objects said nothing.
    let slices = match &prometheus_alertmanagers {
        Some(urls) if !urls.is_empty() && prometheuses.is_empty() && alertmanagers.is_empty() => {
            list(client, "/apis/discovery.k8s.io/v1/endpointslices")
                .await
                .unwrap_or_default()
        }
        _ => Vec::new(),
    };
    Gathered {
        services,
        prometheuses,
        alertmanagers,
        slices,
        prometheus_alertmanagers,
    }
}

/// One line of "what was tried".
#[derive(Clone, Debug, PartialEq)]
pub struct Tried {
    pub label: String,
    pub found_by: &'static str,
    /// `None`: it answered.
    pub error: Option<String>,
}

/// The outcome of looking for Alertmanagers.
#[derive(Clone, Debug, Default)]
pub struct Discovered {
    pub connected: Vec<AmConn>,
    /// Trusted Services behind an auth proxy without a Route (need a forward).
    pub forwards: Vec<AmTarget>,
    pub tried: Vec<Tried>,
    /// Steps that found nothing, for the "no Alertmanager" explanation.
    pub notes: Vec<String>,
    /// How many Alertmanagers Prometheus sends to, when known.
    pub prometheus_alertmanagers: Option<usize>,
}

/// Finds and connects the cluster's Alertmanagers (settings first, else discovery).
pub async fn discover(
    client: &kube::Client,
    settings: &ClusterSettings,
    discover_enabled: bool,
    prometheus: Option<&PromClient>,
    credentials: &Credentials,
) -> Discovered {
    let mut out = Discovered::default();
    let candidates: Vec<Candidate> = if !settings.alertmanagers.is_empty() {
        discover::from_settings(&settings.alertmanagers)
    } else if !discover_enabled || !settings.discover {
        out.notes
            .push("Discovery is off (alerts.discover); name an Alertmanager in settings.".into());
        return out;
    } else {
        let gathered = gather(client, prometheus).await;
        out.prometheus_alertmanagers = gathered.prometheus_alertmanagers.as_ref().map(Vec::len);
        let operator = discover::from_operator(
            &gathered.prometheuses,
            &gathered.alertmanagers,
            &gathered.services,
        );
        if gathered.prometheuses.is_empty() && gathered.alertmanagers.is_empty() {
            out.notes
                .push("prometheus-operator: no Prometheus or Alertmanager objects".into());
        }
        let from_prometheus = match &gathered.prometheus_alertmanagers {
            Some(urls) if urls.is_empty() => {
                out.notes
                    .push("Prometheus: sends to no Alertmanager".into());
                Vec::new()
            }
            Some(urls) => discover::from_prometheus(urls, &gathered.slices, &gathered.services),
            None if prometheus.is_none() => {
                out.notes
                    .push("Prometheus: none found (phase 07's discovery)".into());
                Vec::new()
            }
            None => Vec::new(),
        };
        let by_name = discover::from_services(&gathered.services);
        if by_name.is_empty() {
            out.notes.push(format!(
                "Services: none named or labelled like an Alertmanager among {}",
                gathered.services.len()
            ));
        }
        discover::rank(vec![operator, from_prometheus, by_name])
    };
    let limit = if settings.alertmanagers.is_empty() {
        MAX_PROBES
    } else {
        usize::MAX
    };
    for candidate in candidates.into_iter().take(limit) {
        let found_by = candidate.found_by;
        let label = candidate.target.label();
        match probe(client, &candidate.target, credentials).await {
            Probed::Connected(conn) => {
                // The same Alertmanager cluster through another Service.
                let duplicate = conn.status.cluster_name.as_ref().is_some_and(|name| {
                    !name.is_empty()
                        && out
                            .connected
                            .iter()
                            .any(|c| c.status.cluster_name.as_ref() == Some(name))
                });
                out.tried.push(Tried {
                    label,
                    found_by,
                    error: duplicate.then(|| "same Alertmanager as another Service".into()),
                });
                if !duplicate {
                    tracing::info!(alertmanager = %conn.label(), via = ?conn.via, "found Alertmanager");
                    out.connected.push(*conn);
                }
            }
            Probed::NeedsForward(target, why) => {
                out.tried.push(Tried {
                    label,
                    found_by,
                    error: Some(why),
                });
                out.forwards.push(target);
            }
            Probed::Failed(_, why) => out.tried.push(Tried {
                label,
                found_by,
                error: Some(why),
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matchers::MatchOp;

    #[test]
    fn explains_what_is_missing() {
        let proxy = AmTarget::service("monitoring", "am", "9093", "http", "");
        assert_eq!(
            explain(&proxy, "proxy", &PromError::Http(403, String::new())),
            "needs get services/proxy in monitoring"
        );
        let platform = AmTarget::service(
            OPENSHIFT_MONITORING,
            "alertmanager-main",
            "9094",
            "https",
            "",
        );
        assert!(
            explain(&platform, "route", &PromError::Http(403, String::new()))
                .contains("monitoring-alertmanager-view")
        );
        let uwm = AmTarget::service(
            OPENSHIFT_USER_WORKLOAD,
            "alertmanager-user-workload",
            "9095",
            "https",
            "",
        );
        assert!(
            explain(&uwm, "forward", &PromError::Http(403, String::new())).contains("api-reader")
        );
    }

    #[test]
    fn silence_bodies() {
        let start: jiff::Timestamp = "2026-09-26T10:00:00Z".parse().unwrap();
        let end: jiff::Timestamp = "2026-09-26T12:00:00Z".parse().unwrap();
        let body = silence_body(
            Some("abc"),
            &[Matcher::new("alertname", MatchOp::Equal, "X")],
            start,
            end,
            "alice@example.com",
            "load test",
        );
        assert_eq!(body["id"], "abc");
        assert_eq!(body["matchers"][0]["isEqual"], true);
        assert_eq!(body["endsAt"], "2026-09-26T12:00:00Z");
        assert_eq!(url_segment("0f3a-12/../x"), "0f3a-12x");
    }

    #[tokio::test]
    async fn no_secret_reaches_debug_output() {
        let header = SecretString::from("Bearer kubyl-secret-header".to_string());
        let transport = Transport::external(
            "https://alertmanager.example.com",
            Some(&header),
            &ExternalTls::default(),
        )
        .unwrap();
        let conn = AmConn {
            target: AmTarget::Url {
                url: "https://alertmanager.example.com".into(),
                tenant: None,
                ca_file: None,
                client_certificate: None,
                client_key: None,
                insecure: false,
            },
            transport,
            via: Via::Url,
            status: AmStatus::default(),
        };
        let debug = format!("{conn:?} {:?}", Probed::Connected(Box::new(conn.clone())));
        assert!(!debug.contains("kubyl-secret-header"), "{debug}");
        let token = BearerToken::Static(SecretString::from("kubyl-secret-token".to_string()));
        let direct = Transport::direct(
            "https://127.0.0.1:9443",
            None,
            Some("am.monitoring.svc".into()),
            Bearer::User(token),
        )
        .unwrap();
        let debug = format!("{direct:?}");
        assert!(!debug.contains("kubyl-secret-token"), "{debug}");
        assert!(debug.contains("your token"));
    }

    /// A look-alike Service outside the trusted namespaces never gets a token: after its 401 the
    /// probe stops instead of calling its Route. (The fake API server answers 401 to the proxy
    /// request and would record any other request.)
    #[tokio::test]
    async fn look_alike_services_never_receive_a_token() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            while let Ok(Ok((mut socket, _))) =
                tokio::time::timeout(Duration::from_millis(800), listener.accept()).await
            {
                let mut buf = vec![0; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                requests.push(String::from_utf8_lossy(&buf[..n]).to_string());
                let body = r#"{"kind":"Status","apiVersion":"v1","status":"Failure","message":"Unauthorized","reason":"Unauthorized","code":401}"#;
                let response = format!(
                    "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.ok();
            }
            requests
        });
        let config = kube::Config::new(format!("http://{address}").parse().unwrap());
        let client = kube::Client::try_from(config).unwrap();
        let credentials = Credentials {
            user_token: Some(BearerToken::Static(SecretString::from(
                "s3cr3t-user-token".to_string(),
            ))),
            headers: Vec::new(),
        };
        let look_alike =
            AmTarget::service("monitoring-evil", "alertmanager-main", "9094", "https", "");
        match probe(&client, &look_alike, &credentials).await {
            Probed::Failed(_, why) => {
                assert!(why.contains("401") || why.contains("Unauthorized"), "{why}")
            }
            other => panic!("{other:?}"),
        }
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1, "only the proxy request: {requests:?}");
        assert!(requests[0].starts_with(
            "GET /api/v1/namespaces/monitoring-evil/services/https:alertmanager-main:9094/proxy/api/v2/status"
        ));
        assert!(!requests.iter().any(|r| r.contains("s3cr3t-user-token")));
        assert!(!requests.iter().any(|r| r.contains("/routes")));
    }
}
