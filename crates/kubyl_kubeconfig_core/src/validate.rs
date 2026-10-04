//! Problems of a kubeconfig, shown while editing: in the list, on the form, in the YAML tab.
//!
//! Reads the certificate, key and token files the document points at, so run it off the UI
//! thread. Never includes secret values in messages.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::certs::{self, CertInfo};
use crate::model::{self, AuthKind, Doc, EntryRef, Kind, PemSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// One problem (or noteworthy fact, for [`Severity::Info`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub severity: Severity,
    /// The entry it's about (`None`: the whole file).
    pub entry: Option<EntryRef>,
    /// The key inside the entry's body (`server`, `exec.command`…), or at the top level.
    pub field: Option<String>,
    pub message: String,
}

impl Problem {
    fn new(
        severity: Severity,
        entry: Option<EntryRef>,
        field: Option<&str>,
        message: String,
    ) -> Self {
        Self {
            severity,
            entry,
            field: field.map(String::from),
            message,
        }
    }
}

/// Everything wrong with `doc`. `dir`: the kubeconfig's folder (relative paths).
pub fn validate(doc: &Doc, dir: Option<&Path>) -> Vec<Problem> {
    let mut out = Vec::new();
    for kind in Kind::ALL {
        for name in model::duplicate_names(doc, kind) {
            out.push(Problem::new(
                Severity::Error,
                Some(EntryRef::new(kind, &name)),
                None,
                format!(
                    "two {}s are named \"{name}\"; kubectl uses the first",
                    kind.label()
                ),
            ));
        }
        let unnamed = doc
            .0
            .get(kind.list_key())
            .and_then(Value::as_array)
            .map_or(0, |l| {
                l.iter()
                    .filter(|e| {
                        e.get("name")
                            .and_then(Value::as_str)
                            .is_none_or(str::is_empty)
                    })
                    .count()
            });
        if unnamed > 0 {
            out.push(Problem::new(
                Severity::Error,
                None,
                Some(kind.list_key()),
                format!("{unnamed} {} without a name", kind.list_key()),
            ));
        }
    }
    if let Some(current) = doc.current_context()
        && !doc.contains(Kind::Context, current)
    {
        out.push(Problem::new(
            Severity::Warning,
            None,
            Some("current-context"),
            format!("current-context \"{current}\" doesn't exist"),
        ));
    }
    for name in doc.names(Kind::Context) {
        context(doc, &name, &mut out);
    }
    let empty = Map::new();
    for (name, body) in doc.bodies(Kind::Cluster) {
        cluster(&name, body.unwrap_or(&empty), dir, &mut out);
    }
    for (name, body) in doc.bodies(Kind::User) {
        user(&name, body.unwrap_or(&empty), dir, &mut out);
    }
    out
}

fn context(doc: &Doc, name: &str, out: &mut Vec<Problem>) {
    let entry = Some(EntryRef::new(Kind::Context, name));
    let body = doc.body(Kind::Context, name);
    let cluster = body
        .map(|b| model::get_str(b, &["cluster"]))
        .unwrap_or_default();
    if cluster.is_empty() {
        out.push(Problem::new(
            Severity::Error,
            entry.clone(),
            Some("cluster"),
            "no cluster".into(),
        ));
    } else if !doc.contains(Kind::Cluster, &cluster) {
        out.push(Problem::new(
            Severity::Error,
            entry.clone(),
            Some("cluster"),
            format!("cluster \"{cluster}\" doesn't exist in this file"),
        ));
    }
    let user = body
        .map(|b| model::get_str(b, &["user"]))
        .unwrap_or_default();
    if !user.is_empty() && !doc.contains(Kind::User, &user) {
        out.push(Problem::new(
            Severity::Error,
            entry.clone(),
            Some("user"),
            format!("user \"{user}\" doesn't exist in this file"),
        ));
    }
    let namespace = body
        .map(|b| model::get_str(b, &["namespace"]))
        .unwrap_or_default();
    if !namespace.is_empty() && !is_dns_label(&namespace) {
        out.push(Problem::new(
            Severity::Warning,
            entry,
            Some("namespace"),
            format!("\"{namespace}\" isn't a valid namespace name"),
        ));
    }
}

/// RFC 1123 label: what namespace names must be.
pub fn is_dns_label(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

fn cluster(name: &str, body: &Map<String, Value>, dir: Option<&Path>, out: &mut Vec<Problem>) {
    let entry = Some(EntryRef::new(Kind::Cluster, name));
    let server = model::get_str(body, &["server"]);
    if server.is_empty() {
        out.push(Problem::new(
            Severity::Error,
            entry.clone(),
            Some("server"),
            "no server URL".into(),
        ));
    } else {
        match crate::tls::Target::parse(&server, None) {
            Err(err) => out.push(Problem::new(
                Severity::Error,
                entry.clone(),
                Some("server"),
                err,
            )),
            Ok(target) if !target.https => out.push(Problem::new(
                Severity::Warning,
                entry.clone(),
                Some("server"),
                "plain HTTP: credentials and data travel unencrypted".into(),
            )),
            Ok(_) => {}
        }
    }
    let insecure = model::get_bool(body, &["insecure-skip-tls-verify"]);
    let ca = PemSource::read(body, model::CA_FILE, model::CA_DATA);
    if insecure && ca != PemSource::None {
        out.push(Problem::new(
            Severity::Error,
            entry.clone(),
            Some("insecure-skip-tls-verify"),
            "skipping TLS verification can't be combined with a CA; kubectl refuses this".into(),
        ));
    } else if insecure {
        out.push(Problem::new(
            Severity::Warning,
            entry.clone(),
            Some("insecure-skip-tls-verify"),
            "TLS isn't verified: anyone on the network path can read the credentials".into(),
        ));
    }
    let field = match ca {
        PemSource::File(_) => model::CA_FILE,
        _ => model::CA_DATA,
    };
    match load_pem(&ca, dir) {
        Ok(None) => {}
        Err(err) => out.push(Problem::new(
            Severity::Error,
            entry,
            Some(field),
            format!("CA: {err}"),
        )),
        Ok(Some(pem)) => match certs::certificates(&pem) {
            Err(err) => out.push(Problem::new(
                Severity::Error,
                entry,
                Some(field),
                format!("CA: {err}"),
            )),
            Ok(found) => {
                for cert in found {
                    expiry("CA", &cert, entry.clone(), field, true, out);
                }
            }
        },
    }
}

fn user(name: &str, body: &Map<String, Value>, dir: Option<&Path>, out: &mut Vec<Problem>) {
    let entry = Some(EntryRef::new(Kind::User, name));
    match AuthKind::detect(body) {
        AuthKind::ClientCertificate => {
            let cert = PemSource::read(body, model::CERT_FILE, model::CERT_DATA);
            let key = PemSource::read(body, model::KEY_FILE, model::KEY_DATA);
            let cert_field = match cert {
                PemSource::File(_) => model::CERT_FILE,
                _ => model::CERT_DATA,
            };
            let key_field = match key {
                PemSource::File(_) => model::KEY_FILE,
                _ => model::KEY_DATA,
            };
            match load_pem(&cert, dir) {
                Ok(None) => out.push(Problem::new(
                    Severity::Error,
                    entry.clone(),
                    Some(cert_field),
                    "no client certificate".into(),
                )),
                Err(err) => out.push(Problem::new(
                    Severity::Error,
                    entry.clone(),
                    Some(cert_field),
                    format!("client certificate: {err}"),
                )),
                Ok(Some(pem)) => match certs::certificates(&pem) {
                    Err(err) => out.push(Problem::new(
                        Severity::Error,
                        entry.clone(),
                        Some(cert_field),
                        format!("client certificate: {err}"),
                    )),
                    Ok(found) => {
                        if let Some(cert) = found.first() {
                            expiry(
                                "client certificate",
                                cert,
                                entry.clone(),
                                cert_field,
                                false,
                                out,
                            );
                        }
                    }
                },
            }
            match load_pem(&key, dir) {
                Ok(None) => out.push(Problem::new(
                    Severity::Error,
                    entry,
                    Some(key_field),
                    "no client key".into(),
                )),
                Err(err) => out.push(Problem::new(
                    Severity::Error,
                    entry,
                    Some(key_field),
                    format!("client key: {err}"),
                )),
                Ok(Some(pem)) => {
                    if let Err(err) = certs::check_private_key(&pem) {
                        out.push(Problem::new(
                            Severity::Error,
                            entry,
                            Some(key_field),
                            format!("client key: {err}"),
                        ));
                    }
                }
            }
        }
        AuthKind::Token => {
            let token = model::get_str(body, &["token"]);
            if token.is_empty() {
                out.push(Problem::new(
                    Severity::Error,
                    entry,
                    Some("token"),
                    "the token is empty".into(),
                ));
            } else if let Some(exp) = kubyl_kube::auth::jwt_expiry(&token)
                && exp < jiff::Timestamp::now()
            {
                out.push(Problem::new(
                    Severity::Warning,
                    entry,
                    Some("token"),
                    format!(
                        "the token expired on {}",
                        exp.strftime("%Y-%m-%d %H:%M UTC")
                    ),
                ));
            }
        }
        AuthKind::TokenFile => {
            let file = model::get_str(body, &["tokenFile"]);
            if file.is_empty() {
                out.push(Problem::new(
                    Severity::Error,
                    entry,
                    Some("tokenFile"),
                    "no token file".into(),
                ));
            } else if let Err(err) = std::fs::metadata(model::resolve_path(&file, dir)) {
                out.push(Problem::new(
                    Severity::Error,
                    entry,
                    Some("tokenFile"),
                    format!("can't read {file}: {}", io_message(&err)),
                ));
            }
        }
        AuthKind::Exec => {
            let command = model::get_str(body, &["exec", "command"]);
            let api = model::get_str(body, &["exec", "apiVersion"]);
            if command.is_empty() {
                out.push(Problem::new(
                    Severity::Error,
                    entry.clone(),
                    Some("exec.command"),
                    "the exec plugin has no command".into(),
                ));
            } else if find_command(&command, dir).is_none() {
                out.push(Problem::new(
                    Severity::Warning,
                    entry.clone(),
                    Some("exec.command"),
                    format!("`{command}` wasn't found in PATH (login shell)"),
                ));
            }
            if api.is_empty() {
                out.push(Problem::new(
                    Severity::Error,
                    entry,
                    Some("exec.apiVersion"),
                    "the exec plugin needs an apiVersion".into(),
                ));
            } else if api.ends_with("v1alpha1") {
                out.push(Problem::new(
                    Severity::Warning,
                    entry,
                    Some("exec.apiVersion"),
                    format!(
                        "{api} was removed from kubectl 1.24; use client.authentication.k8s.io/v1"
                    ),
                ));
            }
        }
        AuthKind::OidcProvider => {
            for (key, label) in [("idp-issuer-url", "issuer URL"), ("client-id", "client ID")] {
                if model::get_str(body, &["auth-provider", "config", key]).is_empty() {
                    out.push(Problem::new(
                        Severity::Error,
                        entry.clone(),
                        Some(&format!("auth-provider.config.{key}")),
                        format!("the OIDC provider needs an {label}"),
                    ));
                }
            }
        }
        AuthKind::Basic => out.push(Problem::new(
            Severity::Warning,
            entry,
            Some("username"),
            "basic auth was removed in Kubernetes 1.19".into(),
        )),
        AuthKind::OtherProvider => out.push(Problem::new(
            Severity::Warning,
            entry,
            Some("auth-provider"),
            "this auth provider was removed from kubectl 1.26; use its exec plugin".into(),
        )),
        AuthKind::None => {}
    }
}

fn expiry(
    what: &str,
    cert: &CertInfo,
    entry: Option<EntryRef>,
    field: &str,
    info_when_ok: bool,
    out: &mut Vec<Problem>,
) {
    if cert.expired() {
        out.push(Problem::new(
            Severity::Error,
            entry,
            Some(field),
            format!("the {what} expired on {}", cert.not_after_date()),
        ));
    } else if cert.expires_soon() {
        out.push(Problem::new(
            Severity::Warning,
            entry,
            Some(field),
            format!(
                "the {what} expires on {} (in {} days)",
                cert.not_after_date(),
                cert.days_left()
            ),
        ));
    } else if info_when_ok {
        out.push(Problem::new(
            Severity::Info,
            entry,
            Some(field),
            format!(
                "{what} {} valid until {}",
                cert.common_name(),
                cert.not_after_date()
            ),
        ));
    }
}

fn io_message(err: &std::io::Error) -> String {
    match err.kind() {
        std::io::ErrorKind::NotFound => "the file doesn't exist".into(),
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        _ => err.to_string(),
    }
}

/// The PEM of a file or `*-data` value; `None` when there is none.
pub fn load_pem(source: &PemSource, dir: Option<&Path>) -> Result<Option<String>, String> {
    match source {
        PemSource::None => Ok(None),
        PemSource::Data(data) => certs::pem_from_data(data).map(Some),
        PemSource::File(path) => std::fs::read_to_string(model::resolve_path(path, dir))
            .map(Some)
            .map_err(|e| format!("can't read {path}: {}", io_message(&e))),
    }
}

/// Where `command` resolves: a path (relative ones against `dir`), or a match in the login
/// shell's `PATH`.
pub fn find_command(command: &str, dir: Option<&Path>) -> Option<PathBuf> {
    if command.contains('/') || command.contains(std::path::MAIN_SEPARATOR) {
        let path = model::resolve_path(command, dir);
        return path.is_file().then_some(path);
    }
    let path = kubyl_kube::auth::shell_env::path()?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(command);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        for ext in ["exe", "cmd", "bat"] {
            let candidate = dir.join(format!("{command}.{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// The problems of one entry, worst first.
pub fn of_entry<'a>(problems: &'a [Problem], entry: &EntryRef) -> Vec<&'a Problem> {
    let mut found: Vec<&Problem> = problems
        .iter()
        .filter(|p| p.entry.as_ref() == Some(entry))
        .collect();
    found.sort_by_key(|p| p.severity);
    found
}

/// The worst severity of an entry's problems (Info doesn't count).
pub fn worst(problems: &[Problem], entry: &EntryRef) -> Option<Severity> {
    problems
        .iter()
        .filter(|p| p.entry.as_ref() == Some(entry) && p.severity != Severity::Info)
        .map(|p| p.severity)
        .min()
}

/// The YAML path of a problem: the entry's list item, then its body field.
pub fn yaml_path(doc: &Doc, problem: &Problem) -> kubyl_yaml::parse::Path {
    use kubyl_yaml::parse::{Path as YPath, Seg};
    let mut segs = Vec::new();
    match &problem.entry {
        Some(entry) => {
            segs.push(Seg::Key(entry.kind.list_key().into()));
            if let Some(ix) = doc.index(entry.kind, &entry.name) {
                segs.push(Seg::Index(ix));
                match &problem.field {
                    Some(field) => {
                        segs.push(Seg::Key(entry.kind.body_key().into()));
                        segs.extend(field.split('.').map(|k| Seg::Key(k.to_string())));
                    }
                    None => segs.push(Seg::Key("name".into())),
                }
            }
        }
        None => {
            if let Some(field) = &problem.field {
                segs.push(Seg::Key(field.clone()));
            }
        }
    }
    YPath(segs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certs::tests::{CA, CLIENT, CLIENT_KEY};

    #[test]
    fn finds_problems() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("client.pem"), CLIENT).unwrap();
        std::fs::write(dir.path().join("client-key.pem"), CLIENT_KEY).unwrap();
        let text = format!(
            r#"apiVersion: v1
kind: Config
current-context: gone
clusters:
- name: ok
  cluster: {{server: "https://127.0.0.1:6443", certificate-authority-data: {ca}}}
- name: bad
  cluster: {{server: "ftp://x", insecure-skip-tls-verify: true, certificate-authority: /nope/ca.pem}}
- name: ok
  cluster: {{server: "http://plain:80"}}
contexts:
- name: a
  context: {{cluster: ok, user: certs, namespace: Payments}}
- name: b
  context: {{cluster: missing, user: nobody}}
users:
- name: certs
  user: {{client-certificate: client.pem, client-key: client-key.pem}}
- name: exec
  user:
    exec: {{command: kubyl-no-such-plugin-xyz}}
- name: token
  user: {{token: ""}}
"#,
            ca = certs::data_from_pem(CA)
        );
        let doc = Doc::parse(&text).unwrap();
        let problems = validate(&doc, Some(dir.path()));
        let messages: Vec<String> = problems.iter().map(|p| p.message.clone()).collect();
        let has = |s: &str| messages.iter().any(|m| m.contains(s));
        assert!(has("two clusters are named \"ok\""), "{messages:#?}");
        assert!(has("current-context \"gone\""));
        assert!(has("cluster \"missing\" doesn't exist"));
        assert!(has("user \"nobody\" doesn't exist"));
        assert!(has("\"Payments\" isn't a valid namespace"));
        assert!(has("must start with https://"));
        assert!(has("can't be combined with a CA"));
        assert!(has("CA: can't read /nope/ca.pem"));
        assert!(has("plain HTTP"));
        assert!(has("`kubyl-no-such-plugin-xyz` wasn't found"));
        assert!(has("needs an apiVersion"));
        assert!(has("the token is empty"));
        assert!(has("CA kubyl-test-ca valid until"));
        // Relative certificate paths resolve against the kubeconfig's folder: no problem.
        let certs = EntryRef::new(Kind::User, "certs");
        assert_eq!(
            worst(&problems, &certs),
            None,
            "{:#?}",
            of_entry(&problems, &certs)
        );
        assert_eq!(
            worst(&problems, &EntryRef::new(Kind::Context, "b")),
            Some(Severity::Error)
        );
        let path = yaml_path(
            &doc,
            of_entry(&problems, &EntryRef::new(Kind::Context, "b"))[0],
        );
        assert_eq!(path.to_string(), "contexts[1].context.cluster");
    }

    #[test]
    fn yaml_paths_count_unnamed_entries() {
        let doc = Doc::parse(
            "clusters:\n- cluster: {server: https://a}\n- name: b\n  cluster: {server: \"ftp://b\"}\n",
        )
        .unwrap();
        let problem = Problem {
            severity: Severity::Error,
            entry: Some(EntryRef::new(Kind::Cluster, "b")),
            field: Some("server".into()),
            message: String::new(),
        };
        assert_eq!(
            yaml_path(&doc, &problem).to_string(),
            "clusters[1].cluster.server"
        );
    }
}
