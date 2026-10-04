//! OperatorHub: the packages every CatalogSource offers, from PackageManifests
//! (`packages.operators.coreos.com/v1`, get and list only).
//!
//! The list is big (~11 MB for the operatorhub.io catalog), so it's fetched and parsed on Tokio
//! straight into compact [`Package`]s; the view never sees the JSON.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use super::model::{InstallMode, OwnedCrd, examples, install_modes, owned_crds};

/// The five capability levels, in order.
pub const CAPABILITY_LEVELS: [&str; 5] = [
    "Basic Install",
    "Seamless Upgrades",
    "Full Lifecycle",
    "Deep Insights",
    "Auto Pilot",
];

#[derive(Deserialize)]
struct List {
    #[serde(default)]
    items: Vec<RawPackage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawPackage {
    metadata: RawMeta,
    status: RawStatus,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawMeta {
    name: String,
    namespace: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawStatus {
    catalog_source: String,
    catalog_source_display_name: Option<String>,
    catalog_source_namespace: String,
    catalog_source_publisher: Option<String>,
    default_channel: Option<String>,
    package_name: String,
    provider: Option<RawProvider>,
    channels: Vec<RawChannel>,
    deprecation: Option<RawDeprecation>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawProvider {
    name: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawDeprecation {
    message: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawChannel {
    name: String,
    #[serde(rename = "currentCSV")]
    current_csv: String,
    #[serde(rename = "currentCSVDesc")]
    current_csv_desc: RawDesc,
    entries: Vec<RawEntry>,
    deprecation: Option<RawDeprecation>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawEntry {
    name: String,
    version: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawDesc {
    display_name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    annotations: BTreeMap<String, String>,
    keywords: Vec<String>,
    install_modes: Value,
    customresourcedefinitions: Value,
    min_kube_version: Option<String>,
    links: Vec<RawLink>,
    maturity: Option<String>,
    provider: Option<RawProvider>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct RawLink {
    name: String,
    url: String,
}

/// A version of a channel (`entries`), newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub csv: String,
    pub version: Option<String>,
}

/// One channel of a package, described by its head CSV.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub name: String,
    pub head: String,
    pub version: Option<String>,
    pub install_modes: BTreeSet<InstallMode>,
    pub min_kube_version: Option<String>,
    pub owned: Vec<OwnedCrd>,
    pub entries: Vec<Entry>,
    pub deprecated: Option<String>,
    /// `alm-examples` of the head (JSON text).
    pub alm_examples: Option<String>,
    pub suggested_namespace: Option<String>,
}

/// One package of a catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    pub name: String,
    /// The namespace the PackageManifest was listed in (its catalog's for global catalogs).
    pub namespace: String,
    pub catalog: String,
    pub catalog_namespace: String,
    pub catalog_display: String,
    pub publisher: Option<String>,
    pub provider: String,
    pub display_name: String,
    /// The short description (the head's `description` annotation).
    pub summary: String,
    /// The long description (Markdown).
    pub description: String,
    pub capability: Option<String>,
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
    pub repository: Option<String>,
    pub container_image: Option<String>,
    pub links: Vec<(String, String)>,
    pub maturity: Option<String>,
    pub default_channel: String,
    pub channels: Vec<Channel>,
    pub deprecated: Option<String>,
}

impl Package {
    /// `catalog-namespace/catalog/name`: unique across catalogs.
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.catalog_namespace, self.catalog, self.name)
    }

    pub fn channel(&self, name: &str) -> Option<&Channel> {
        self.channels.iter().find(|c| c.name == name)
    }

    /// The default channel (else the first).
    pub fn head(&self) -> Option<&Channel> {
        self.channel(&self.default_channel)
            .or_else(|| self.channels.first())
    }

    pub fn initials(&self) -> String {
        super::join::initials(&self.display_name)
    }

    /// The `alm-examples` of the default channel's head.
    pub fn examples(&self) -> Vec<Value> {
        examples(self.head().and_then(|c| c.alm_examples.as_deref()))
    }

    fn haystack(&self) -> String {
        let mut text = format!(
            "{} {} {} {} {}",
            self.name,
            self.display_name,
            self.provider,
            self.summary,
            self.keywords.join(" ")
        );
        if let Some(head) = self.head() {
            for crd in &head.owned {
                text.push(' ');
                text.push_str(&crd.kind);
            }
        }
        text.to_lowercase()
    }
}

fn parse_package(raw: RawPackage) -> Package {
    let status = raw.status;
    let default_channel = status.default_channel.clone().unwrap_or_default();
    let channels: Vec<Channel> = status
        .channels
        .iter()
        .map(|c| {
            let desc = &c.current_csv_desc;
            Channel {
                name: c.name.clone(),
                head: c.current_csv.clone(),
                version: desc.version.clone(),
                install_modes: install_modes(Some(&desc.install_modes)),
                min_kube_version: desc.min_kube_version.clone(),
                owned: owned_crds(desc.customresourcedefinitions.get("owned")),
                entries: c
                    .entries
                    .iter()
                    .map(|e| Entry {
                        csv: e.name.clone(),
                        version: e.version.clone(),
                    })
                    .collect(),
                deprecated: c.deprecation.as_ref().and_then(|d| d.message.clone()),
                alm_examples: desc.annotations.get("alm-examples").cloned(),
                suggested_namespace: desc
                    .annotations
                    .get("operatorframework.io/suggested-namespace")
                    .cloned(),
            }
        })
        .collect();
    let head = status
        .channels
        .iter()
        .find(|c| c.name == default_channel)
        .or(status.channels.first());
    let desc = head.map(|c| &c.current_csv_desc);
    let annotation = |key: &str| {
        desc.and_then(|d| d.annotations.get(key))
            .filter(|v| !v.is_empty())
            .cloned()
    };
    let display_name = desc
        .and_then(|d| d.display_name.clone())
        .unwrap_or_else(|| status.package_name.clone());
    Package {
        name: if status.package_name.is_empty() {
            raw.metadata.name.clone()
        } else {
            status.package_name.clone()
        },
        namespace: raw.metadata.namespace,
        catalog: status.catalog_source.clone(),
        catalog_namespace: status.catalog_source_namespace.clone(),
        catalog_display: status
            .catalog_source_display_name
            .clone()
            .unwrap_or_else(|| status.catalog_source.clone()),
        publisher: status.catalog_source_publisher.clone(),
        provider: status
            .provider
            .as_ref()
            .and_then(|p| p.name.clone())
            .or_else(|| desc.and_then(|d| d.provider.as_ref()?.name.clone()))
            .unwrap_or_default(),
        deprecated: status
            .deprecation
            .as_ref()
            .and_then(|d| d.message.clone())
            .or_else(|| {
                display_name
                    .starts_with("[DEPRECATED]")
                    .then(|| "The package is deprecated.".to_string())
            }),
        display_name: display_name
            .trim_start_matches("[DEPRECATED]")
            .trim()
            .to_string(),
        summary: annotation("description").unwrap_or_default(),
        description: desc.and_then(|d| d.description.clone()).unwrap_or_default(),
        capability: annotation("capabilities"),
        categories: annotation("categories")
            .map(|c| {
                c.split(',')
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        keywords: desc.map(|d| d.keywords.clone()).unwrap_or_default(),
        repository: annotation("repository"),
        container_image: annotation("containerImage"),
        links: desc
            .map(|d| {
                d.links
                    .iter()
                    .filter(|l| l.url.starts_with("https://") || l.url.starts_with("http://"))
                    .map(|l| (l.name.clone(), l.url.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        maturity: desc.and_then(|d| d.maturity.clone()),
        default_channel,
        channels,
    }
}

/// Parses a PackageManifest list (the API's JSON) into packages, sorted by display name.
/// Packages seen in several namespaces (a namespaced list repeats global catalogs) count once.
pub fn parse_list(json: &[u8]) -> Result<Vec<Package>, String> {
    let list: List = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let mut seen = BTreeSet::new();
    let mut out: Vec<Package> = list
        .items
        .into_iter()
        .map(parse_package)
        .filter(|p| seen.insert(p.key()))
        .collect();
    out.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
            .then_with(|| a.key().cmp(&b.key()))
    });
    Ok(out)
}

/// Fetches every PackageManifest (all namespaces) and parses them, on Tokio.
pub async fn fetch(client: kube::Client) -> Result<Vec<Arc<Package>>, String> {
    let request = http::Request::get("/apis/packages.operators.coreos.com/v1/packagemanifests")
        .body(Vec::new())
        .map_err(|e| e.to_string())?;
    let text = client
        .request_text(request)
        .await
        .map_err(|e| crate::errors::describe(&e, "list", "packagemanifests", None))?;
    let packages = parse_list(text.as_bytes())?;
    Ok(packages.into_iter().map(Arc::new).collect())
}

/// An icon from the packageserver (`packagemanifests/<name>/icon`): its format and bytes.
pub async fn fetch_icon(
    client: kube::Client,
    namespace: String,
    name: String,
) -> Option<(gpui::ImageFormat, Vec<u8>)> {
    use http_body_util::BodyExt as _;
    let path = format!(
        "/apis/packages.operators.coreos.com/v1/namespaces/{namespace}/packagemanifests/{name}/icon"
    );
    let request = http::Request::get(path)
        .body(kube::client::Body::empty())
        .ok()?;
    let response = client.send(request).await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let content_type = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();
    let bytes = response.into_body().collect().await.ok()?.to_bytes();
    // Icons are small; refuse anything that isn't.
    if bytes.is_empty() || bytes.len() > 512 * 1024 {
        return None;
    }
    let format = image_format(&content_type, &bytes)?;
    Some((format, bytes.to_vec()))
}

fn image_format(content_type: &str, bytes: &[u8]) -> Option<gpui::ImageFormat> {
    use gpui::ImageFormat;
    if content_type.contains("svg") || bytes.starts_with(b"<?xml") || bytes.starts_with(b"<svg") {
        Some(ImageFormat::Svg)
    } else if content_type.contains("png") || bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some(ImageFormat::Png)
    } else if content_type.contains("jpeg") || bytes.starts_with(&[0xff, 0xd8]) {
        Some(ImageFormat::Jpeg)
    } else if content_type.contains("gif") {
        Some(ImageFormat::Gif)
    } else {
        None
    }
}

/// What the OperatorHub filters select.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    pub query: String,
    pub category: Option<String>,
    pub capabilities: BTreeSet<String>,
    pub provider: Option<String>,
    /// Catalogs (`namespace/name`) the user turned off.
    pub hidden_catalogs: BTreeSet<String>,
    pub installed_only: bool,
}

impl Filters {
    pub fn matches(&self, package: &Package, installed: bool) -> bool {
        if self.installed_only && !installed {
            return false;
        }
        if self.hidden_catalogs.contains(&format!(
            "{}/{}",
            package.catalog_namespace, package.catalog
        )) {
            return false;
        }
        if let Some(category) = &self.category
            && !package.categories.iter().any(|c| c == category)
        {
            return false;
        }
        if !self.capabilities.is_empty()
            && !package
                .capability
                .as_ref()
                .is_some_and(|c| self.capabilities.contains(c))
        {
            return false;
        }
        if let Some(provider) = &self.provider
            && &package.provider != provider
        {
            return false;
        }
        let query = self.query.trim().to_lowercase();
        query.is_empty() || {
            let haystack = package.haystack();
            query.split_whitespace().all(|word| haystack.contains(word))
        }
    }

    /// How well a package matches the query (lower is better), for "Relevance".
    pub fn rank(&self, package: &Package) -> u8 {
        let query = self.query.trim().to_lowercase();
        if query.is_empty() {
            return 0;
        }
        let name = package.name.to_lowercase();
        let display = package.display_name.to_lowercase();
        if name == query || display == query {
            0
        } else if name.starts_with(&query) || display.starts_with(&query) {
            1
        } else if name.contains(&query) || display.contains(&query) {
            2
        } else {
            3
        }
    }
}

/// A package description (Markdown) as plain text: no heading, emphasis or code markers,
/// links as their text, images and HTML tags dropped, at most one blank line in a row.
pub fn plain_text(markdown: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim();
        let mut text = trimmed
            .trim_start_matches('#')
            .trim_start_matches('>')
            .trim();
        let bullet = text.starts_with("* ") || text.starts_with("- ");
        if bullet {
            text = &text[2..];
        }
        let mut plain = String::with_capacity(text.len());
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            // `![alt](url)` goes, `[text](url)` keeps its text.
            if (c == '!' && chars.get(i + 1) == Some(&'[')) || c == '[' {
                let image = c == '!';
                let start = if image { i + 2 } else { i + 1 };
                if let Some(close) = chars[start..].iter().position(|&c| c == ']')
                    && chars.get(start + close + 1) == Some(&'(')
                    && let Some(end) = chars[start + close + 2..].iter().position(|&c| c == ')')
                {
                    if !image {
                        plain.extend(&chars[start..start + close]);
                    }
                    i = start + close + 2 + end + 1;
                    continue;
                }
            }
            if c == '<'
                && let Some(end) = chars[i..].iter().position(|&c| c == '>')
                && chars
                    .get(i + 1)
                    .is_some_and(|n| n.is_ascii_alphabetic() || *n == '/')
            {
                i += end + 1;
                continue;
            }
            if matches!(c, '*' | '`') || (c == '_' && chars.get(i + 1) == Some(&'_')) {
                i += if c == '_' { 2 } else { 1 };
                continue;
            }
            plain.push(c);
            i += 1;
        }
        let plain = plain.trim().to_string();
        if plain.is_empty() {
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
        } else if bullet {
            out.push(format!("• {plain}"));
        } else {
            out.push(plain);
        }
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out.join("\n")
}

/// Categories with their package counts (most first), for the filter column.
pub fn categories(packages: &[Arc<Package>]) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for package in packages {
        for category in &package.categories {
            *counts.entry(category.clone()).or_default() += 1;
        }
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Providers (sorted), for the provider menu.
pub fn providers(packages: &[Arc<Package>]) -> Vec<String> {
    let set: BTreeSet<String> = packages
        .iter()
        .map(|p| p.provider.clone())
        .filter(|p| !p.is_empty())
        .collect();
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn list() -> Vec<u8> {
        json!({"items": [
            {"metadata": {"name": "cert-manager", "namespace": "olm"},
             "status": {"catalogSource": "operatorhubio-catalog", "catalogSourceDisplayName": "Community Operators",
                "catalogSourceNamespace": "olm", "defaultChannel": "stable", "packageName": "cert-manager",
                "provider": {"name": "The cert-manager maintainers"},
                "channels": [
                    {"name": "candidate", "currentCSV": "cert-manager.v1.17.0-beta.0",
                     "currentCSVDesc": {"version": "1.17.0-beta.0", "installModes": []}},
                    {"name": "stable", "currentCSV": "cert-manager.v1.16.5",
                     "entries": [{"name": "cert-manager.v1.16.5", "version": "1.16.5"}, {"name": "cert-manager.v1.16.1", "version": "1.16.1"}],
                     "currentCSVDesc": {"displayName": "[DEPRECATED] cert-manager", "version": "1.16.5",
                        "annotations": {"capabilities": "Deep Insights", "categories": "Security",
                            "description": "Cloud native certificate management",
                            "operatorframework.io/suggested-namespace": "cert-manager",
                            "alm-examples": "[{\"apiVersion\":\"cert-manager.io/v1\",\"kind\":\"Issuer\",\"metadata\":{\"name\":\"x\"}}]"},
                        "keywords": ["TLS"], "minKubeVersion": "1.19.0-0",
                        "links": [{"name": "Docs", "url": "https://cert-manager.io/"}, {"name": "Bad", "url": "javascript:alert(1)"}],
                        "installModes": [{"type": "OwnNamespace", "supported": false}, {"type": "AllNamespaces", "supported": true}],
                        "customresourcedefinitions": {"owned": [{"name": "certificates.cert-manager.io", "version": "v1", "kind": "Certificate"}]}}}]}},
            {"metadata": {"name": "cloudnative-pg", "namespace": "olm"},
             "status": {"catalogSource": "operatorhubio-catalog", "catalogSourceNamespace": "olm", "defaultChannel": "stable-v1",
                "packageName": "cloudnative-pg", "provider": {"name": "CloudNativePG"},
                "channels": [{"name": "stable-v1", "currentCSV": "cloudnative-pg.v1.30.1",
                   "currentCSVDesc": {"displayName": "CloudNativePG", "version": "1.30.1",
                      "annotations": {"capabilities": "Auto Pilot", "categories": "Database"}}}]}},
            // The same package listed in another namespace (a namespaced list) counts once.
            {"metadata": {"name": "cloudnative-pg", "namespace": "shop"},
             "status": {"catalogSource": "operatorhubio-catalog", "catalogSourceNamespace": "olm", "packageName": "cloudnative-pg"}}
        ]})
        .to_string()
        .into_bytes()
    }

    #[test]
    fn package_manifests_parse_into_packages() {
        let packages = parse_list(&list()).unwrap();
        assert_eq!(packages.len(), 2);
        let cm = &packages[0];
        assert_eq!(cm.display_name, "cert-manager");
        assert!(cm.deprecated.is_some());
        assert_eq!(cm.catalog_display, "Community Operators");
        let head = cm.head().unwrap();
        assert_eq!(head.name, "stable");
        assert_eq!(head.version.as_deref(), Some("1.16.5"));
        assert_eq!(head.entries.len(), 2);
        assert_eq!(
            head.install_modes.iter().copied().collect::<Vec<_>>(),
            [InstallMode::AllNamespaces]
        );
        assert_eq!(head.suggested_namespace.as_deref(), Some("cert-manager"));
        assert_eq!(cm.links.len(), 1, "only http(s) links");
        assert_eq!(cm.examples().len(), 1);
        assert_eq!(cm.initials(), "CM");
    }

    #[test]
    fn descriptions_become_plain_text() {
        let markdown = "## Features\n\n* **Fast** backups to `S3`\n- See [the docs](https://example.com/docs) ![logo](https://example.com/l.png)\n\n\n\n<br/>Plain __text__ <b>here</b>\n";
        assert_eq!(
            plain_text(markdown),
            "Features\n\n• Fast backups to S3\n• See the docs\n\nPlain text here"
        );
        assert_eq!(plain_text("a < b and c > d"), "a < b and c > d");
    }

    #[test]
    fn filters() {
        let packages: Vec<Arc<Package>> = parse_list(&list())
            .unwrap()
            .into_iter()
            .map(Arc::new)
            .collect();
        let mut filters = Filters::default();
        assert!(packages.iter().all(|p| filters.matches(p, false)));
        filters.query = "certificate".into(); // an owned kind
        assert!(filters.matches(&packages[0], false));
        assert!(!filters.matches(&packages[1], false));
        filters.query.clear();
        filters.capabilities.insert("Auto Pilot".into());
        assert!(!filters.matches(&packages[0], false));
        assert!(filters.matches(&packages[1], false));
        filters.capabilities.clear();
        filters.category = Some("Security".into());
        assert!(filters.matches(&packages[0], false));
        filters.category = None;
        filters.installed_only = true;
        assert!(!filters.matches(&packages[0], false));
        assert!(filters.matches(&packages[0], true));
        filters.installed_only = false;
        filters
            .hidden_catalogs
            .insert("olm/operatorhubio-catalog".into());
        assert!(!filters.matches(&packages[0], false));
        assert_eq!(
            categories(&packages),
            [("Database".to_string(), 1), ("Security".to_string(), 1)]
        );
        let ranked = Filters {
            query: "cert".into(),
            ..Default::default()
        };
        assert_eq!(ranked.rank(&packages[0]), 1);
    }
}
