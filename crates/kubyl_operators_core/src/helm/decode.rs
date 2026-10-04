//! Decoding Helm v3 releases from their storage objects, off the UI thread.
//!
//! Helm stores a release as JSON, gzipped and base64-encoded, in `data.release` of a Secret
//! (`HELM_DRIVER=secret`, the default; the API adds its own base64 on top) or a ConfigMap
//! (`HELM_DRIVER=configmap`). Releases hold the user's values, which commonly contain passwords:
//! [`Release`]'s `Debug` never prints values, the manifest or notes, and nothing here logs them.

use std::fmt;
use std::io::Read as _;

use base64::Engine as _;
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::Value;

/// Where a release is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Driver {
    Secret,
    ConfigMap,
}

impl Driver {
    pub fn label(self) -> &'static str {
        match self {
            Driver::Secret => "Secret",
            Driver::ConfigMap => "ConfigMap",
        }
    }
}

/// Why a release couldn't be read. Never contains any of the release's content.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("no release key in the {0}")]
    Missing(&'static str),
    #[error("the release isn't valid base64")]
    Base64,
    #[error("the release isn't valid gzip")]
    Gzip,
    #[error("the release isn't a Helm release (JSON: {0})")]
    Json(String),
    #[error("the release is larger than {0} MB unpacked")]
    TooLarge(usize),
}

/// Unpacked releases above this are refused (Kubernetes objects are ≤ 1 MB packed; this leaves
/// room for well-compressed manifests).
const MAX_UNPACKED: usize = 64 << 20;

const GZIP_MAGIC: &[u8] = &[0x1f, 0x8b, 0x08];

/// Decodes Helm's encoding: base64 text → (gzip) → JSON bytes. `encoded` is `data.release` as
/// stored: for a Secret that's what the API's base64 decodes to, for a ConfigMap the string.
pub fn unpack(encoded: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let trimmed: Vec<u8> = encoded
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&trimmed)
        .map_err(|_| DecodeError::Base64)?;
    if !bytes.starts_with(GZIP_MAGIC) {
        // Helm reads plain JSON too.
        return Ok(bytes);
    }
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes.as_slice())
        .take(MAX_UNPACKED as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|_| DecodeError::Gzip)?;
    if out.len() > MAX_UNPACKED {
        return Err(DecodeError::TooLarge(MAX_UNPACKED >> 20));
    }
    Ok(out)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawRelease {
    name: String,
    namespace: String,
    version: u32,
    info: RawInfo,
    chart: RawChart,
    config: Option<Value>,
    manifest: String,
    hooks: Option<Vec<RawHook>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawInfo {
    first_deployed: Option<String>,
    last_deployed: Option<String>,
    deleted: Option<String>,
    description: Option<String>,
    status: Option<String>,
    notes: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawChart {
    metadata: ChartMetadata,
    values: Option<Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawHook {
    name: String,
    kind: String,
    path: String,
    events: Vec<String>,
    manifest: String,
}

/// `Chart.yaml` of the release's chart. Not secret.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChartMetadata {
    pub name: String,
    pub version: String,
    pub app_version: Option<String>,
    pub description: Option<String>,
    pub home: Option<String>,
    pub icon: Option<String>,
    pub sources: Vec<String>,
    pub kube_version: Option<String>,
    pub deprecated: bool,
}

/// What the list shows: nothing secret.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub name: String,
    pub namespace: String,
    pub revision: u32,
    pub chart: ChartMetadata,
    pub status: String,
    pub description: Option<String>,
    pub first_deployed: Option<Timestamp>,
    pub last_deployed: Option<Timestamp>,
}

impl Summary {
    /// `kube-prometheus-stack-84.1.0`, like `helm list`'s CHART column.
    pub fn chart_label(&self) -> String {
        if self.chart.version.is_empty() {
            self.chart.name.clone()
        } else {
            format!("{}-{}", self.chart.name, self.chart.version)
        }
    }
}

/// A hook of the release (manifest included: it can hold Secrets too).
#[derive(Clone, PartialEq)]
pub struct Hook {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub events: Vec<String>,
    pub manifest: String,
}

/// A whole release: the summary plus values, manifest and notes. Kept only while a release tab
/// shows it. `Debug` prints the summary and sizes only.
#[derive(Clone, PartialEq)]
pub struct Release {
    pub summary: Summary,
    /// The user-supplied values (`helm get values`).
    pub values: Value,
    /// The chart's default values (`helm get values --all` merges them under `values`).
    pub chart_values: Value,
    pub manifest: String,
    pub notes: Option<String>,
    pub hooks: Vec<Hook>,
}

impl fmt::Debug for Release {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Release")
            .field("summary", &self.summary)
            .field("manifest_bytes", &self.manifest.len())
            .field("hooks", &self.hooks.len())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Hook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hook")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl Release {
    /// Chart defaults with the user's values on top (`helm get values --all`).
    pub fn all_values(&self) -> Value {
        let mut merged = self.chart_values.clone();
        merge(&mut merged, &self.values);
        merged
    }
}

/// Deep-merges `over` into `base` (maps merge, everything else replaces; `null` removes a key,
/// like Helm).
pub fn merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(base), Value::Object(over)) => {
            for (key, value) in over {
                if value.is_null() {
                    base.remove(key);
                    continue;
                }
                match base.get_mut(key) {
                    Some(existing) if existing.is_object() && value.is_object() => {
                        merge(existing, value)
                    }
                    _ => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, over) if !over.is_null() => *base = over.clone(),
        _ => {}
    }
}

fn time(text: &Option<String>) -> Option<Timestamp> {
    text.as_deref().and_then(|t| t.parse().ok())
}

fn parse(json: &[u8]) -> Result<RawRelease, DecodeError> {
    serde_json::from_slice::<RawRelease>(json).map_err(|e| {
        // Only the position: serde's message may quote content.
        DecodeError::Json(format!("line {}, column {}", e.line(), e.column()))
    })
}

fn summary_of(raw: &RawRelease) -> Summary {
    Summary {
        name: raw.name.clone(),
        namespace: raw.namespace.clone(),
        revision: raw.version,
        chart: raw.chart.metadata.clone(),
        status: raw.info.status.clone().unwrap_or_default(),
        description: raw.info.description.clone(),
        first_deployed: time(&raw.info.first_deployed),
        last_deployed: time(&raw.info.last_deployed),
    }
}

/// Decodes only what the list needs. The values in the JSON are dropped right away.
pub fn decode_summary(encoded: &[u8]) -> Result<Summary, DecodeError> {
    let json = unpack(encoded)?;
    let raw = parse(&json)?;
    Ok(summary_of(&raw))
}

/// Decodes the whole release.
pub fn decode(encoded: &[u8]) -> Result<Release, DecodeError> {
    let json = unpack(encoded)?;
    let raw = parse(&json)?;
    let summary = summary_of(&raw);
    Ok(Release {
        summary,
        values: raw.config.unwrap_or(Value::Object(Default::default())),
        chart_values: raw
            .chart
            .values
            .unwrap_or(Value::Object(Default::default())),
        manifest: raw.manifest,
        notes: raw.info.notes.filter(|n| !n.trim().is_empty()),
        hooks: raw
            .hooks
            .unwrap_or_default()
            .into_iter()
            .map(|h| Hook {
                name: h.name,
                kind: h.kind,
                path: h.path,
                events: h.events,
                manifest: h.manifest,
            })
            .collect(),
    })
}

/// Helm's encoding of a release (for tests and fixtures): JSON → gzip → base64.
pub fn encode(json: &Value) -> String {
    use std::io::Write as _;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(json.to_string().as_bytes())
        .expect("writing to memory");
    let bytes = gz.finish().expect("writing to memory");
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release() -> Value {
        json!({
            "name": "shop-db", "namespace": "shop", "version": 3,
            "info": {"first_deployed": "2026-09-25T14:21:18.836393+02:00",
                     "last_deployed": "2026-09-26T12:16:04+02:00",
                     "description": "Upgrade complete", "status": "deployed",
                     "notes": "Get the password with kubectl get secret …"},
            "chart": {"metadata": {"name": "postgresql", "version": "16.2.1", "appVersion": "17.2.0"},
                      "values": {"auth": {"username": "postgres", "database": "app"}, "replicas": 1},
                      "templates": [{"name": "templates/x.yaml", "data": "aGVsbG8="}]},
            "config": {"auth": {"password": "hunter2-very-secret", "username": null}},
            "manifest": "---\n# Source: postgresql/templates/secret.yaml\napiVersion: v1\nkind: Secret\n",
            "hooks": [{"name": "migrate", "kind": "Job", "path": "t/job.yaml", "events": ["post-upgrade"], "manifest": "kind: Job"}]
        })
    }

    #[test]
    fn decodes_secret_and_config_map_encodings() {
        let helm = encode(&release());
        // ConfigMap: `data.release` is Helm's string.
        let full = decode(helm.as_bytes()).unwrap();
        assert_eq!(full.summary.chart_label(), "postgresql-16.2.1");
        assert_eq!(full.summary.chart.app_version.as_deref(), Some("17.2.0"));
        assert_eq!(full.summary.revision, 3);
        assert_eq!(full.summary.status, "deployed");
        assert!(full.summary.last_deployed.is_some());
        assert_eq!(full.hooks.len(), 1);
        // Secret: the API's base64 decodes to the same string, so the input is identical; the
        // whitespace kubectl may wrap it with doesn't matter.
        let wrapped = format!("{}\n{}", &helm[..40], &helm[40..]);
        assert_eq!(decode_summary(wrapped.as_bytes()).unwrap(), full.summary);
        // Plain JSON without gzip is read as well.
        let plain = base64::engine::general_purpose::STANDARD.encode(release().to_string());
        assert_eq!(decode_summary(plain.as_bytes()).unwrap().name, "shop-db");
        // `--all`: chart defaults under the user's values; `null` drops a key.
        let all = full.all_values();
        assert_eq!(all["auth"]["password"], "hunter2-very-secret");
        assert_eq!(all["auth"]["database"], "app");
        assert!(all["auth"].get("username").is_none());
        assert_eq!(all["replicas"], 1);
    }

    #[test]
    fn errors_and_debug_never_show_values() {
        let helm = encode(&release());
        let full = decode(helm.as_bytes()).unwrap();
        let debug = format!("{full:?}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(!debug.contains("kubectl get secret"), "{debug}");
        assert!(!format!("{:?}", full.hooks).contains("kind: Job"));
        assert_eq!(decode(b"not base64!").unwrap_err(), DecodeError::Base64);
        let bad = base64::engine::general_purpose::STANDARD
            .encode(br#"{"name": "x", "config": {"password": "hunter2"#);
        let err = decode(bad.as_bytes()).unwrap_err();
        assert!(!err.to_string().contains("hunter2"), "{err}");
        let broken = base64::engine::general_purpose::STANDARD.encode([0x1f, 0x8b, 0x08, 0, 1, 2]);
        assert_eq!(decode(broken.as_bytes()).unwrap_err(), DecodeError::Gzip);
    }
}
