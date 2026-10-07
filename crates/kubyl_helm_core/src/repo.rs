//! Repositories and charts (decisions 3 and 4): Helm's own repository list (`helm repo …`),
//! OCI registries (`helm registry login`), search across the added repositories
//! (`helm search repo -o json`), OCI references typed by the user, and a chart's details (one
//! `helm pull --untar` into a temporary folder: `Chart.yaml`, README, default values, the values
//! schema and the CRDs).
//!
//! Kubyl keeps no repository credentials of its own: `helm repo add` stores them in Helm's
//! `repositories.yaml` (plain text), registry logins go to Docker's credential store. Passwords
//! reach `helm` on stdin (`--password-stdin`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use secrecy::SecretString;
use serde::Deserialize;
use serde_json::Value;

use crate::cli::Invocation;

/// A chart as `helm install` takes it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ChartRef {
    /// `repo/name` from an added repository.
    Repo { repo: String, name: String },
    /// `oci://registry/path/name` (the version goes into `--version`).
    Oci { reference: String },
    /// A chart from a repository that isn't added (Artifact Hub): `--repo <url> <name>`.
    Url { repo_url: String, name: String },
}

impl ChartRef {
    /// Whether `helm` would take the chart's arguments as what they are: no part that starts
    /// with `-` (it would read as a flag), no whitespace.
    pub fn validate(&self) -> Result<(), String> {
        let parts: Vec<&str> = match self {
            ChartRef::Repo { repo, name } => vec![repo, name],
            ChartRef::Oci { reference } => vec![reference],
            ChartRef::Url { repo_url, name } => vec![repo_url, name],
        };
        if parts
            .iter()
            .any(|p| p.is_empty() || p.starts_with('-') || p.contains(char::is_whitespace))
        {
            return Err(format!(
                "{} isn't a chart reference Kubyl can pass to helm.",
                self.label()
            ));
        }
        Ok(())
    }

    /// The arguments that name the chart.
    pub fn args(&self) -> Vec<String> {
        match self {
            ChartRef::Repo { repo, name } => vec![format!("{repo}/{name}")],
            ChartRef::Oci { reference } if is_local_registry(reference) => {
                vec![reference.clone(), "--plain-http".into()]
            }
            ChartRef::Oci { reference } => vec![reference.clone()],
            ChartRef::Url { repo_url, name } => {
                vec![name.clone(), "--repo".into(), repo_url.clone()]
            }
        }
    }

    /// The chart's own name (`demo`).
    pub fn name(&self) -> &str {
        match self {
            ChartRef::Repo { name, .. } | ChartRef::Url { name, .. } => name,
            ChartRef::Oci { reference } => reference.rsplit('/').next().unwrap_or(reference),
        }
    }

    /// `kubyl-dev/demo`, `oci://…/demo`, `demo (https://…)`.
    pub fn label(&self) -> String {
        match self {
            ChartRef::Repo { repo, name } => format!("{repo}/{name}"),
            ChartRef::Oci { reference } => reference.clone(),
            ChartRef::Url { repo_url, name } => format!("{name} ({repo_url})"),
        }
    }

    /// The repository it comes from (`None` for OCI).
    pub fn repo(&self) -> Option<&str> {
        match self {
            ChartRef::Repo { repo, .. } => Some(repo),
            ChartRef::Url { repo_url, .. } => Some(repo_url),
            ChartRef::Oci { .. } => None,
        }
    }
}

/// Whether an OCI reference (or a registry host) points at a registry on this machine
/// (`localhost:5001`, kind's local registry): those serve plain HTTP, which Helm 4 only talks
/// with `--plain-http`.
pub fn is_local_registry(reference: &str) -> bool {
    let authority = reference
        .trim_start_matches("oci://")
        .split('/')
        .next()
        .unwrap_or_default();
    // `[::1]` and `[::1]:5000`; `localhost:5000`.
    let host = match authority.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

/// Whether `version` is a SemVer pre-release (`1.2.0-rc.1`; build metadata after `+` isn't).
pub fn is_prerelease(version: &str) -> bool {
    version.split('+').next().unwrap_or_default().contains('-')
}

/// The version `helm install` picks without `--version`: the newest that isn't a
/// pre-release. `versions` are newest first, as `helm search` sorts them.
pub fn latest_stable<'a>(versions: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    versions.into_iter().find(|v| !is_prerelease(v))
}

/// Parses what the user typed: `oci://host/path/chart[:version]` or `repo/chart[@version]`.
/// Returns the chart and the version it names, if any.
pub fn parse_reference(text: &str) -> Option<(ChartRef, Option<String>)> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("oci://") {
        if rest.is_empty() || !rest.contains('/') {
            return None;
        }
        // A tag after the last path segment (`chart:1.2.3`); `host:5000/…` is a port.
        let (path, version) = match rest.rsplit_once('/') {
            Some((head, last)) => match last.split_once(':') {
                Some((name, tag)) if !tag.is_empty() => {
                    (format!("{head}/{name}"), Some(tag.to_string()))
                }
                _ => (rest.to_string(), None),
            },
            None => (rest.to_string(), None),
        };
        if path.ends_with('/') || path.contains(char::is_whitespace) {
            return None;
        }
        return Some((
            ChartRef::Oci {
                reference: format!("oci://{path}"),
            },
            version.filter(|v| !v.starts_with('-')),
        ));
    }
    let (chart, version) = match text.split_once('@') {
        Some((chart, version)) if !version.is_empty() => (chart, Some(version.to_string())),
        _ => (text, None),
    };
    let (repo, name) = chart.split_once('/')?;
    if name.contains('/') || version.as_deref().is_some_and(|v| v.starts_with('-')) {
        return None;
    }
    let chart = ChartRef::Repo {
        repo: repo.into(),
        name: name.into(),
    };
    chart.validate().ok()?;
    Some((chart, version))
}

/// One repository from `helm repo list -o json`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Repository {
    pub name: String,
    pub url: String,
}

/// Parses `helm repo list -o json` (Helm prints `[]` or nothing when there are none).
pub fn parse_repositories(stdout: &str) -> Result<Vec<Repository>, String> {
    if stdout.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(stdout).map_err(|e| format!("unexpected output of helm repo list: {e}"))
}

/// `helm repo list -o json`. With no repositories Helm exits 1 with "no repositories to
/// show": callers treat that as an empty list ([`is_no_repositories`]).
pub fn list_repositories() -> Invocation {
    Invocation::read(["repo", "list", "--output", "json"]).timeout(Duration::from_secs(30))
}

/// Whether `helm repo list` failed only because there are none.
pub fn is_no_repositories(message: &str) -> bool {
    message.contains("no repositories")
}

/// A repository to add.
#[derive(Clone, Default)]
pub struct NewRepository {
    pub name: String,
    pub url: String,
    pub username: Option<String>,
    /// Sent on stdin (`--password-stdin`); Helm stores it in `repositories.yaml`.
    pub password: Option<SecretString>,
    pub ca_file: Option<PathBuf>,
    pub cert_file: Option<PathBuf>,
    pub key_file: Option<PathBuf>,
    pub insecure_skip_tls_verify: bool,
}

/// `helm repo add` (`--force-update`: the dialog asks before it replaces a repository of the
/// same name, so changing a repository's URL works like adding it).
pub fn add_repository(repo: &NewRepository) -> Invocation {
    let mut args = vec!["repo".to_string(), "add".into(), "--force-update".into()];
    if let Some(user) = repo.username.as_ref().filter(|u| !u.trim().is_empty()) {
        args.extend(["--username".into(), user.trim().into()]);
        if repo.password.is_some() {
            args.push("--password-stdin".into());
        }
    }
    for (flag, path) in [
        ("--ca-file", &repo.ca_file),
        ("--cert-file", &repo.cert_file),
        ("--key-file", &repo.key_file),
    ] {
        if let Some(path) = path {
            args.extend([flag.into(), path.to_string_lossy().into_owned()]);
        }
    }
    if repo.insecure_skip_tls_verify {
        args.push("--insecure-skip-tls-verify".into());
    }
    // `--`: the name and URL are never read as flags.
    args.extend(["--".into(), repo.name.trim().into(), repo.url.trim().into()]);
    let stdin = repo.username.as_ref().and(repo.password.clone());
    Invocation::read(args)
        .stdin(stdin)
        .timeout(Duration::from_secs(120))
}

/// `helm repo update [names…]`.
pub fn update_repositories(names: &[String]) -> Invocation {
    let mut args = vec!["repo".to_string(), "update".into()];
    if !names.is_empty() {
        args.push("--".into());
        args.extend(names.iter().cloned());
    }
    Invocation::read(args).timeout(Duration::from_secs(300))
}

/// `helm repo remove <name>`.
pub fn remove_repository(name: &str) -> Invocation {
    Invocation::read(["repo", "remove", "--", name]).timeout(Duration::from_secs(30))
}

/// `helm registry login --username <u> --password-stdin -- <host>` (Helm 4 needs
/// `--plain-http` for a registry on this machine, like for its charts).
pub fn registry_login(
    host: &str,
    username: &str,
    password: SecretString,
    insecure: bool,
    helm: crate::cli::Version,
) -> Invocation {
    let host = registry_host(host);
    let mut args = vec![
        "registry".to_string(),
        "login".into(),
        "--username".into(),
        username.trim().into(),
        "--password-stdin".into(),
    ];
    if insecure {
        args.push("--insecure".into());
    }
    if helm.is_v4() && is_local_registry(&host) {
        args.push("--plain-http".into());
    }
    args.extend(["--".into(), host]);
    Invocation::read(args)
        .stdin(Some(password))
        .timeout(Duration::from_secs(60))
}

/// The registry of a login, as typed (`oci://ghcr.io` → `ghcr.io`).
pub fn registry_host(text: &str) -> String {
    text.trim()
        .trim_start_matches("oci://")
        .trim_end_matches('/')
        .to_string()
}

/// Whether `host` can be a registry to log in to.
pub fn validate_registry_host(host: &str) -> Result<(), String> {
    let host = registry_host(host);
    if host.is_empty() {
        return Err("Name the registry (ghcr.io, registry.example.com:5000).".into());
    }
    if host.starts_with('-') || host.contains(char::is_whitespace) {
        return Err("Registry names can't start with '-' or hold spaces.".into());
    }
    Ok(())
}

/// Whether `name` is a valid repository name.
pub fn validate_repository_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Name the repository.".into());
    }
    if name.contains('/') || name.contains(char::is_whitespace) {
        return Err("Repository names can't hold '/' or spaces.".into());
    }
    if name.starts_with('-') {
        return Err("Repository names can't start with '-'.".into());
    }
    Ok(())
}

/// Whether `url` can be a repository URL.
pub fn validate_repository_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.starts_with("oci://") {
        return Err(
            "OCI registries aren't added as repositories: log in to the registry and type oci:// references into the search."
                .into(),
        );
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Repository URLs start with https:// (or http://).".into());
    }
    if url.contains('@') {
        return Err("Put the user name and password into their fields, not the URL.".into());
    }
    Ok(())
}

/// A search hit (one chart version).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChartHit {
    pub chart: ChartRef,
    pub version: String,
    pub app_version: Option<String>,
    pub description: Option<String>,
    /// Where it comes from: the repository name, `Artifact Hub · <repo>` or the registry.
    pub source: String,
}

#[derive(Deserialize)]
struct RawHit {
    name: String,
    version: String,
    #[serde(default)]
    app_version: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// `helm search repo <query> -o json` (the latest version of each chart; `versions`: all of
/// them).
pub fn search(query: &str, versions: bool) -> Invocation {
    let mut args = vec!["search".to_string(), "repo".into()];
    if versions {
        args.push("--versions".into());
    }
    args.extend(["--output".into(), "json".into()]);
    // `--`: a query is never read as a flag.
    if !query.trim().is_empty() {
        args.extend(["--".into(), query.trim().into()]);
    }
    Invocation::read(args).timeout(Duration::from_secs(60))
}

/// The versions of one chart of an added repository (Helm's `--regexp` also matches
/// descriptions, so the keyword search is filtered by [`parse_versions`]).
pub fn versions(repo: &str, name: &str) -> Invocation {
    Invocation::read([
        "search".to_string(),
        "repo".into(),
        "--versions".into(),
        "--devel".into(),
        "--output".into(),
        "json".into(),
        "--".into(),
        format!("{repo}/{name}"),
    ])
    .timeout(Duration::from_secs(60))
}

/// The hits of [`versions`] that are exactly `repo/name`, newest first (as Helm sorts them).
pub fn parse_versions(stdout: &str, repo: &str, name: &str) -> Result<Vec<ChartHit>, String> {
    let wanted = ChartRef::Repo {
        repo: repo.into(),
        name: name.into(),
    };
    Ok(parse_search(stdout)?
        .into_iter()
        .filter(|hit| hit.chart == wanted)
        .collect())
}

/// Parses `helm search repo -o json`.
pub fn parse_search(stdout: &str) -> Result<Vec<ChartHit>, String> {
    if stdout.trim().is_empty() {
        return Ok(Vec::new());
    }
    let raw: Vec<RawHit> = serde_json::from_str(stdout)
        .map_err(|e| format!("unexpected output of helm search: {e}"))?;
    Ok(raw
        .into_iter()
        .filter_map(|hit| {
            let (repo, name) = hit.name.split_once('/')?;
            Some(ChartHit {
                source: repo.to_string(),
                chart: ChartRef::Repo {
                    repo: repo.into(),
                    name: name.into(),
                },
                version: hit.version,
                app_version: hit.app_version.filter(|v| !v.is_empty()),
                description: hit.description.filter(|d| !d.is_empty()),
            })
        })
        .collect())
}

/// `helm show chart <ref>` (for OCI references, which `helm search` doesn't cover).
pub fn show_chart(chart: &ChartRef, version: Option<&str>) -> Invocation {
    let mut args = vec!["show".to_string(), "chart".into()];
    args.extend(chart.args());
    if let Some(version) = version.filter(|v| !v.is_empty()) {
        args.extend(["--version".into(), version.into()]);
    }
    Invocation::read(args).timeout(Duration::from_secs(120))
}

/// `helm pull <ref> --untar --untardir <dir>`: the chart's files for its details.
pub fn pull(chart: &ChartRef, version: Option<&str>, dir: &Path) -> Invocation {
    let mut args = vec!["pull".to_string()];
    args.extend(chart.args());
    if let Some(version) = version.filter(|v| !v.is_empty()) {
        args.extend(["--version".into(), version.into()]);
    }
    args.extend([
        "--untar".into(),
        "--untardir".into(),
        dir.to_string_lossy().into_owned(),
    ]);
    Invocation::read(args).timeout(Duration::from_secs(180))
}

/// A maintainer from `Chart.yaml`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Maintainer {
    pub name: String,
    pub email: Option<String>,
    pub url: Option<String>,
}

/// `Chart.yaml`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChartYaml {
    pub api_version: String,
    pub name: String,
    pub version: String,
    pub app_version: Option<String>,
    pub description: Option<String>,
    pub home: Option<String>,
    pub icon: Option<String>,
    pub sources: Vec<String>,
    pub keywords: Vec<String>,
    pub maintainers: Vec<Maintainer>,
    pub kube_version: Option<String>,
    #[serde(rename = "type")]
    pub chart_type: Option<String>,
    pub deprecated: bool,
}

impl ChartYaml {
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_saphyr::from_str(text).map_err(|e| format!("Chart.yaml: {e}"))
    }
}

/// What the details pane and the install dialog show about a chart version. Nothing secret:
/// chart files are public.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartDetails {
    pub chart: ChartYaml,
    pub readme: Option<String>,
    /// `values.yaml` as written (comments kept).
    pub values: String,
    /// `values.schema.json`, when the chart has one.
    pub schema: Option<Value>,
    /// The CRDs in `crds/` (names): installed once, never upgraded or deleted by Helm.
    pub crds: Vec<String>,
}

/// Reads a pulled chart folder (`<dir>/<chart>/…`).
pub fn read_pulled(dir: &Path) -> Result<ChartDetails, String> {
    let root = std::fs::read_dir(dir)
        .map_err(|e| format!("reading the chart: {e}"))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.join("Chart.yaml").is_file())
        .ok_or("the pulled chart has no Chart.yaml")?;
    let chart = ChartYaml::parse(
        &std::fs::read_to_string(root.join("Chart.yaml")).map_err(|e| e.to_string())?,
    )?;
    let readme = ["README.md", "README.txt", "README"]
        .iter()
        .find_map(|name| std::fs::read_to_string(root.join(name)).ok());
    let values = std::fs::read_to_string(root.join("values.yaml")).unwrap_or_default();
    let schema = std::fs::read_to_string(root.join("values.schema.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let mut crds = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.join("crds")) {
        let mut files: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        files.sort();
        for file in files {
            if let Ok(text) = std::fs::read_to_string(&file) {
                crds.extend(crate::preview::crd_names(&text));
            }
        }
    }
    Ok(ChartDetails {
        chart,
        readme,
        values,
        schema,
        crds,
    })
}

/// Pulls a chart into a temporary folder and reads it (on Tokio). The folder is removed.
pub async fn details(
    helm: &crate::cli::HelmInfo,
    chart: &ChartRef,
    version: Option<&str>,
) -> Result<ChartDetails, String> {
    let dir = tempfile::Builder::new()
        .prefix("kubyl-chart-")
        .tempdir()
        .map_err(|e| format!("couldn't make a temporary folder: {e}"))?;
    crate::cli::run(helm, None, pull(chart, version, dir.path()), None, None)
        .await
        .map_err(|e| e.message)?;
    let path = dir.path().to_path_buf();
    tokio::task::spawn_blocking(move || read_pulled(&path))
        .await
        .map_err(|e| e.to_string())?
}

/// Artifact Hub (decision 4, opt-in): `GET /api/v1/packages/search?kind=0&ts_query_web=…`.
pub mod artifact_hub {
    use super::*;

    pub const BASE: &str = "https://artifacthub.io";

    #[derive(Deserialize)]
    struct Page {
        #[serde(default)]
        packages: Vec<Package>,
    }

    #[derive(Deserialize)]
    struct Package {
        name: String,
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        app_version: Option<String>,
        #[serde(default)]
        description: Option<String>,
        repository: Repo,
    }

    #[derive(Deserialize)]
    struct Repo {
        name: String,
        url: String,
    }

    /// The search URL for `query`.
    pub fn search_url(base: &str, query: &str) -> String {
        let mut url =
            url::Url::parse(&format!("{base}/api/v1/packages/search")).expect("a valid base URL");
        url.query_pairs_mut()
            .append_pair("kind", "0")
            .append_pair("ts_query_web", query.trim())
            .append_pair("limit", "40")
            .append_pair("offset", "0");
        url.to_string()
    }

    /// Parses a search page into hits that install with `--repo <url>` (or as OCI references).
    pub fn parse(body: &str) -> Result<Vec<ChartHit>, String> {
        let page: Page = serde_json::from_str(body)
            .map_err(|e| format!("unexpected Artifact Hub answer: {e}"))?;
        Ok(page
            .packages
            .into_iter()
            .filter_map(|p| {
                let chart = if let Some(rest) = p.repository.url.strip_prefix("oci://") {
                    ChartRef::Oci {
                        reference: format!("oci://{}/{}", rest.trim_end_matches('/'), p.name),
                    }
                } else if p.repository.url.starts_with("http") {
                    ChartRef::Url {
                        repo_url: p.repository.url.clone(),
                        name: p.name.clone(),
                    }
                } else {
                    return None;
                };
                chart.validate().ok()?;
                Some(ChartHit {
                    chart,
                    version: p.version.unwrap_or_default(),
                    app_version: p.app_version.filter(|v| !v.is_empty()),
                    description: p.description,
                    source: format!("Artifact Hub · {}", p.repository.name),
                })
            })
            .collect())
    }

    /// Searches Artifact Hub (on Tokio). Only called when `helm.artifact_hub` is on.
    pub async fn search(base: &str, query: &str) -> Result<Vec<ChartHit>, String> {
        use openidconnect::reqwest;
        let client = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::limited(3))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?;
        let response = client
            .get(search_url(base, query))
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("Artifact Hub: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("Artifact Hub answered {}", response.status()));
        }
        let body = response.text().await.map_err(|e| e.to_string())?;
        parse(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret as _;

    #[test]
    fn references_parse() {
        assert_eq!(
            parse_reference("oci://registry.example.com:5000/charts/demo:0.2.0"),
            Some((
                ChartRef::Oci {
                    reference: "oci://registry.example.com:5000/charts/demo".into()
                },
                Some("0.2.0".into())
            ))
        );
        assert_eq!(
            parse_reference("oci://localhost:5022/kubyl/demo"),
            Some((
                ChartRef::Oci {
                    reference: "oci://localhost:5022/kubyl/demo".into()
                },
                None
            ))
        );
        assert_eq!(parse_reference("oci://"), None);
        assert_eq!(
            ChartRef::Oci {
                reference: "oci://localhost:5022/charts/demo".into()
            }
            .args(),
            ["oci://localhost:5022/charts/demo", "--plain-http"]
        );
        assert!(!is_local_registry("oci://ghcr.io/x/demo"));
        assert_eq!(
            parse_reference("kubyl-dev/demo@0.1.0"),
            Some((
                ChartRef::Repo {
                    repo: "kubyl-dev".into(),
                    name: "demo".into()
                },
                Some("0.1.0".into())
            ))
        );
        assert_eq!(parse_reference("demo"), None);
        let url = ChartRef::Url {
            repo_url: "https://charts.example.com".into(),
            name: "demo".into(),
        };
        assert_eq!(url.args(), ["demo", "--repo", "https://charts.example.com"]);
        assert_eq!(
            ChartRef::Oci {
                reference: "oci://r/x/demo".into()
            }
            .name(),
            "demo"
        );
    }

    #[test]
    fn search_output_parses() {
        let hits = parse_search(
            r#"[{"name":"kubyl-dev/demo","version":"0.2.0","app_version":"1.1.0","description":"A demo"},{"name":"bitnami/redis","version":"21.2.13","app_version":"","description":""}]"#,
        )
        .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].source, "kubyl-dev");
        assert_eq!(hits[0].chart.name(), "demo");
        assert_eq!(hits[1].app_version, None);
        assert!(parse_search("").unwrap().is_empty());
        assert_eq!(
            parse_repositories(r#"[{"name":"a","url":"https://a"}]"#).unwrap()[0].name,
            "a"
        );
        let both = r#"[{"name":"kubyl-dev/demo","version":"0.2.0"},{"name":"kubyl-dev/demo-extra","version":"1.0.0"}]"#;
        assert_eq!(parse_versions(both, "kubyl-dev", "demo").unwrap().len(), 1);
    }

    #[test]
    fn passwords_go_on_stdin() {
        let add = add_repository(&NewRepository {
            name: "private".into(),
            url: "https://charts.example.com".into(),
            username: Some("alice".into()),
            password: Some("s3cret".to_string().into()),
            ..NewRepository::default()
        });
        assert!(!add.args.iter().any(|a| a.contains("s3cret")));
        assert!(add.args.contains(&"--password-stdin".to_string()));
        assert_eq!(add.stdin.unwrap().expose_secret(), "s3cret");
        const V3: crate::cli::Version = crate::cli::Version {
            major: 3,
            minor: 19,
            patch: 0,
        };
        const V4: crate::cli::Version = crate::cli::Version {
            major: 4,
            minor: 3,
            patch: 0,
        };
        let login = registry_login(
            "oci://localhost:5022",
            "bob",
            "pw".to_string().into(),
            true,
            V3,
        );
        assert_eq!(
            login.args,
            [
                "registry",
                "login",
                "--username",
                "bob",
                "--password-stdin",
                "--insecure",
                "--",
                "localhost:5022"
            ]
        );
        // Helm 4 talks plain HTTP to a local registry only with --plain-http.
        let v4 = registry_login("localhost:5022", "bob", "pw".to_string().into(), false, V4);
        assert!(v4.args.contains(&"--plain-http".to_string()));
        let remote = registry_login("ghcr.io", "bob", "pw".to_string().into(), false, V4);
        assert!(!remote.args.contains(&"--plain-http".to_string()));
        assert!(validate_registry_host("--help").is_err());
        assert!(validate_registry_host("oci://ghcr.io/").is_ok());
        assert!(validate_repository_url("https://a:b@x").is_err());
        assert!(validate_repository_url("oci://x").is_err());
        assert!(validate_repository_name("a/b").is_err());
        assert!(validate_repository_name("--debug").is_err());
        let add = add_repository(&NewRepository {
            name: "dev".into(),
            url: "http://127.0.0.1:8879".into(),
            ..NewRepository::default()
        });
        assert_eq!(
            &add.args[add.args.len() - 3..],
            ["--", "dev", "http://127.0.0.1:8879"]
        );
        assert_eq!(
            remove_repository("dev").args,
            ["repo", "remove", "--", "dev"]
        );
    }

    #[test]
    fn nothing_typed_reads_as_a_flag() {
        assert_eq!(parse_reference("-x/demo"), None);
        assert_eq!(parse_reference("dev/--set"), None);
        assert_eq!(parse_reference("dev/demo@--devel"), None);
        assert_eq!(
            parse_reference("oci://ghcr.io/x/demo:-v"),
            Some((
                ChartRef::Oci {
                    reference: "oci://ghcr.io/x/demo".into()
                },
                None
            ))
        );
        let search = search("--debug", false);
        assert_eq!(&search.args[search.args.len() - 2..], ["--", "--debug"]);
        assert!(
            ChartRef::Url {
                repo_url: "https://charts.example.com".into(),
                name: "-evil".into()
            }
            .validate()
            .is_err()
        );
        let hits = artifact_hub::parse(
            r#"{"packages":[{"name":"--post-renderer","repository":{"name":"x","url":"https://x.example.com"}}]}"#,
        )
        .unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn pre_releases_and_local_registries() {
        assert!(is_prerelease("1.2.0-rc.1"));
        assert!(!is_prerelease("1.2.0+build-7"));
        assert_eq!(
            latest_stable(["0.3.0-beta.1", "0.2.0", "0.1.0"]),
            Some("0.2.0")
        );
        assert_eq!(latest_stable(["0.3.0-beta.1"]), None);
        for local in [
            "oci://localhost/charts/demo",
            "oci://localhost:5022/charts/demo",
            "oci://127.0.0.1:5000/x",
            "oci://[::1]/x",
            "oci://[::1]:5000/x",
            "localhost:5022",
        ] {
            assert!(is_local_registry(local), "{local}");
        }
        assert!(!is_local_registry("oci://localhost.example.com/x"));
    }

    #[test]
    fn pulled_charts_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("demo");
        std::fs::create_dir_all(root.join("crds")).unwrap();
        std::fs::write(
            root.join("Chart.yaml"),
            "apiVersion: v2\nname: demo\nversion: 0.2.0\nappVersion: \"1.1.0\"\nkeywords: [demo]\nmaintainers:\n- name: Kubyl\n",
        )
        .unwrap();
        std::fs::write(root.join("values.yaml"), "# replicas\nreplicaCount: 1\n").unwrap();
        std::fs::write(
            root.join("values.schema.json"),
            r#"{"type":"object","properties":{"replicaCount":{"type":"integer"}}}"#,
        )
        .unwrap();
        std::fs::write(root.join("README.md"), "# Demo\n").unwrap();
        std::fs::write(
            root.join("crds/widgets.yaml"),
            "apiVersion: apiextensions.k8s.io/v1\nkind: CustomResourceDefinition\nmetadata:\n  name: widgets.demo.kubyl.dev\n",
        )
        .unwrap();
        let details = read_pulled(dir.path()).unwrap();
        assert_eq!(details.chart.name, "demo");
        assert_eq!(details.chart.app_version.as_deref(), Some("1.1.0"));
        assert_eq!(details.chart.maintainers[0].name, "Kubyl");
        assert!(details.values.starts_with("# replicas"));
        assert!(details.schema.is_some());
        assert_eq!(details.crds, ["widgets.demo.kubyl.dev"]);
        assert_eq!(details.readme.as_deref(), Some("# Demo\n"));
    }

    #[test]
    fn artifact_hub_hits() {
        let url = artifact_hub::search_url(artifact_hub::BASE, "redis cache");
        assert!(url.contains("ts_query_web=redis+cache"), "{url}");
        let hits = artifact_hub::parse(
            r#"{"packages":[{"name":"redis","version":"21.2.13","app_version":"8.2.1","description":"Redis","repository":{"name":"bitnami","url":"https://charts.bitnami.com/bitnami"}},{"name":"podinfo","version":"6.9.0","repository":{"name":"podinfo","url":"oci://ghcr.io/stefanprodan/charts"}}]}"#,
        )
        .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].chart.args(),
            ["redis", "--repo", "https://charts.bitnami.com/bitnami"]
        );
        assert_eq!(
            hits[1].chart.args(),
            ["oci://ghcr.io/stefanprodan/charts/podinfo"]
        );
        assert_eq!(hits[1].source, "Artifact Hub · podinfo");
    }
}
