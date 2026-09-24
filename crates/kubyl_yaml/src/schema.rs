//! Schemas from the cluster's OpenAPI v3 documents (`/openapi/v3/apis/<group>/<version>`).
//!
//! Built-in kinds and CRDs (`openAPIV3Schema`) come from the same place, so a CRD installed
//! while Kubyl runs gets a schema as soon as discovery sees it (the index is revalidated and
//! [`Schemas`] drops a cluster's documents on `ConnectionEvent::DiscoveryChanged`).
//!
//! [`Schema`] is a merged view of one node: `$ref`s and `allOf` wrappers (how the API server
//! attaches descriptions to referenced types) are followed.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use kubyl_core::{ClusterId, Gvk};
use kubyl_kube::openapi::OpenApiIndex;
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use serde_json::Value;

use crate::parse::{Path, Seg};

/// A node of an OpenAPI document with its `$ref`/`allOf` parts merged.
#[derive(Clone, Debug)]
pub struct Schema<'a> {
    doc: &'a Value,
    parts: Vec<&'a Value>,
}

const MAX_DEPTH: usize = 16;

impl<'a> Schema<'a> {
    /// Wraps `node` of `doc`, following references.
    pub fn new(doc: &'a Value, node: &'a Value) -> Self {
        let mut parts = Vec::new();
        collect(doc, node, &mut parts, 0);
        Self { doc, parts }
    }

    /// The schema of `gvk` in an OpenAPI v3 document (by `x-kubernetes-group-version-kind`).
    pub fn for_gvk(doc: &'a Value, gvk: &Gvk) -> Option<Self> {
        let schemas = doc.pointer("/components/schemas")?.as_object()?;
        schemas
            .values()
            .find(|schema| {
                schema
                    .get("x-kubernetes-group-version-kind")
                    .and_then(Value::as_array)
                    .is_some_and(|gvks| {
                        gvks.iter().any(|g| {
                            g["group"].as_str() == Some(&gvk.group)
                                && g["version"].as_str() == Some(&gvk.version)
                                && g["kind"].as_str() == Some(&gvk.kind)
                        })
                    })
            })
            .map(|node| Self::new(doc, node))
    }

    fn first<T>(&self, f: impl Fn(&'a Value) -> Option<T>) -> Option<T> {
        self.parts.iter().find_map(|p| f(p))
    }

    fn flag(&self, name: &str) -> bool {
        self.parts
            .iter()
            .any(|p| p.get(name).and_then(Value::as_bool) == Some(true))
    }

    pub fn description(&self) -> Option<&'a str> {
        self.first(|p| p.get("description")?.as_str())
    }

    /// The declared `type` (`object`, `array`, `string`, `integer`, `number`, `boolean`).
    pub fn type_name(&self) -> Option<&'a str> {
        self.first(|p| p.get("type")?.as_str())
    }

    pub fn format(&self) -> Option<&'a str> {
        self.first(|p| p.get("format")?.as_str())
    }

    pub fn is_int_or_string(&self) -> bool {
        self.flag("x-kubernetes-int-or-string")
    }

    pub fn is_nullable(&self) -> bool {
        self.flag("nullable")
    }

    /// `x-kubernetes-preserve-unknown-fields` or `additionalProperties: true`, or an object
    /// without declared properties: anything goes below.
    pub fn allows_unknown(&self) -> bool {
        if self.flag("x-kubernetes-preserve-unknown-fields")
            || self.flag("x-kubernetes-embedded-resource") && self.properties().is_empty()
        {
            return true;
        }
        if self.parts.iter().any(|p| {
            p.get("additionalProperties")
                .is_some_and(|a| a.as_bool() == Some(true))
        }) {
            return true;
        }
        self.properties().is_empty() && self.additional().is_none()
    }

    /// The JSON types a value may have (`oneOf`/`anyOf` alternatives included). Empty = any.
    pub fn allowed_types(&self) -> Vec<&'a str> {
        if self.is_int_or_string() {
            return vec!["integer", "string"];
        }
        if let Some(t) = self.type_name() {
            return vec![t];
        }
        let mut types = Vec::new();
        for part in &self.parts {
            for key in ["oneOf", "anyOf"] {
                for alt in part
                    .get(key)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let alt = Schema::new(self.doc, alt);
                    types.extend(alt.allowed_types());
                }
            }
        }
        types.sort_unstable();
        types.dedup();
        types
    }

    pub fn enum_values(&self) -> Option<&'a [Value]> {
        self.first(|p| p.get("enum")?.as_array().map(Vec::as_slice))
    }

    pub fn required(&self) -> Vec<&'a str> {
        let mut out: Vec<&str> = self
            .parts
            .iter()
            .filter_map(|p| p.get("required")?.as_array())
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        out.dedup();
        out
    }

    /// Declared properties, alphabetically.
    pub fn properties(&self) -> Vec<(&'a str, Schema<'a>)> {
        let mut out: Vec<(&str, Schema)> = Vec::new();
        for part in &self.parts {
            if let Some(props) = part.get("properties").and_then(Value::as_object) {
                for (name, node) in props {
                    if !out.iter().any(|(n, _)| n == name) {
                        out.push((name.as_str(), Schema::new(self.doc, node)));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(b.0));
        out
    }

    pub fn property(&self, name: &str) -> Option<Schema<'a>> {
        self.first(|p| p.get("properties")?.get(name))
            .map(|node| Schema::new(self.doc, node))
    }

    /// The schema of map values (`additionalProperties: {…}`).
    pub fn additional(&self) -> Option<Schema<'a>> {
        self.first(|p| p.get("additionalProperties").filter(|a| a.is_object()))
            .map(|node| Schema::new(self.doc, node))
    }

    pub fn items(&self) -> Option<Schema<'a>> {
        self.first(|p| p.get("items").filter(|i| i.is_object()))
            .map(|node| Schema::new(self.doc, node))
    }

    pub fn pattern(&self) -> Option<&'a str> {
        self.first(|p| p.get("pattern")?.as_str())
    }

    pub fn number(&self, key: &str) -> Option<f64> {
        self.first(|p| p.get(key)?.as_f64())
    }

    pub fn list_type(&self) -> Option<&'a str> {
        self.first(|p| p.get("x-kubernetes-list-type")?.as_str())
    }

    pub fn list_map_keys(&self) -> Vec<&'a str> {
        self.first(|p| p.get("x-kubernetes-list-map-keys")?.as_array())
            .map(|keys| keys.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }

    pub fn default_value(&self) -> Option<&'a Value> {
        self.first(|p| p.get("default"))
    }

    /// The schema one step down.
    pub fn child(&self, seg: &Seg) -> Option<Schema<'a>> {
        match seg {
            Seg::Key(key) => self.property(key).or_else(|| self.additional()),
            Seg::Index(_) => self.items(),
        }
    }

    pub fn at(&self, path: &Path) -> Option<Schema<'a>> {
        let mut schema = self.clone();
        for seg in &path.0 {
            schema = schema.child(seg)?;
        }
        Some(schema)
    }

    /// A short type label for hover and the outline: `string`, `[]string`, `map[string]string`,
    /// `object`, `int-or-string`, `enum`, `string (date-time)`.
    pub fn type_label(&self) -> String {
        if self.is_int_or_string() {
            return "int-or-string".into();
        }
        if self.enum_values().is_some() {
            return "enum".into();
        }
        match self.type_name() {
            Some("array") => match self.items() {
                Some(items) => format!("[]{}", items.type_label()),
                None => "array".into(),
            },
            Some("object") => match self.additional() {
                Some(values) if self.properties().is_empty() => {
                    format!("map[string]{}", values.type_label())
                }
                _ => "object".into(),
            },
            Some(t) => match self.format() {
                Some(f) if !matches!(f, "int32" | "int64" | "double" | "float") => {
                    format!("{t} ({f})")
                }
                _ => t.to_string(),
            },
            None => {
                let types = self.allowed_types();
                if types.is_empty() {
                    "any".into()
                } else {
                    types.join(" | ")
                }
            }
        }
    }
}

fn collect<'a>(doc: &'a Value, node: &'a Value, parts: &mut Vec<&'a Value>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    parts.push(node);
    if let Some(target) = node
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.strip_prefix('#'))
        .and_then(|pointer| doc.pointer(pointer))
    {
        collect(doc, target, parts, depth + 1);
    }
    for sub in node
        .get("allOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        collect(doc, sub, parts, depth + 1);
    }
}

/// Where a schema comes from, for hover docs.
pub fn source_label(gvk: &Gvk, plural: Option<&str>) -> String {
    if is_builtin_group(&gvk.group) {
        format!("Kubernetes API · {}", gvk.api_version())
    } else {
        let name = match plural {
            Some(plural) => format!("{plural}.{}", gvk.group),
            None => gvk.group.clone(),
        };
        format!("From CRD {name} · openAPIV3Schema {}", gvk.version)
    }
}

/// Groups served by the API server itself (the rest come from CRDs or aggregated APIs).
pub fn is_builtin_group(group: &str) -> bool {
    const K8S: &[&str] = &[
        "admissionregistration.k8s.io",
        "apiextensions.k8s.io",
        "apiregistration.k8s.io",
        "authentication.k8s.io",
        "authorization.k8s.io",
        "certificates.k8s.io",
        "coordination.k8s.io",
        "discovery.k8s.io",
        "events.k8s.io",
        "flowcontrol.apiserver.k8s.io",
        "internal.apiserver.k8s.io",
        "networking.k8s.io",
        "node.k8s.io",
        "rbac.authorization.k8s.io",
        "resource.k8s.io",
        "scheduling.k8s.io",
        "storage.k8s.io",
        "storagemigration.k8s.io",
    ];
    !group.contains('.') || K8S.contains(&group)
}

// ----- Cache -----

enum Entry {
    Loading(#[allow(dead_code)] Task<()>),
    Ready(Arc<Value>),
    Failed(String),
}

/// OpenAPI documents per (cluster, group-version), loaded on demand and shared by all editors.
/// Observe the entity to hear when one finishes loading.
pub struct Schemas {
    docs: HashMap<(ClusterId, String), Entry>,
}

struct GlobalSchemas(Entity<Schemas>);

impl Global for GlobalSchemas {}

impl Schemas {
    pub(crate) fn install(cx: &mut App) {
        let entity = cx.new(|cx| {
            if let Some(manager) = ConnectionManager::try_global(cx) {
                cx.subscribe(&manager, |this: &mut Schemas, _, event, cx| {
                    if let ConnectionEvent::DiscoveryChanged(cluster) = event {
                        // CRDs may have been added, changed or removed.
                        this.docs.retain(|(c, _), _| c != cluster);
                        cx.notify();
                    }
                })
                .detach();
            }
            Schemas {
                docs: HashMap::new(),
            }
        });
        cx.set_global(GlobalSchemas(entity));
    }

    pub fn global(cx: &App) -> Option<Entity<Schemas>> {
        cx.try_global::<GlobalSchemas>().map(|g| g.0.clone())
    }

    /// The document for `group/version`, or `None` while it loads (observers are notified).
    pub fn get(
        &mut self,
        cluster: &ClusterId,
        group: &str,
        version: &str,
        cx: &mut Context<Self>,
    ) -> Result<Option<Arc<Value>>, String> {
        let key = (cluster.clone(), OpenApiIndex::key(group, version));
        match self.docs.get(&key) {
            Some(Entry::Ready(doc)) => return Ok(Some(doc.clone())),
            Some(Entry::Failed(err)) => return Err(err.clone()),
            Some(Entry::Loading(_)) => return Ok(None),
            None => {}
        }
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return Err("not connected".into());
        };
        if manager.read(cx).client(cluster).is_none() {
            return Ok(None);
        }
        let load = manager.update(cx, |m, cx| m.openapi_spec(cluster, key.1.clone(), cx));
        let task_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                let entry = match result {
                    Ok(doc) => Entry::Ready(Arc::new(doc)),
                    Err(err) => {
                        tracing::warn!(path = %task_key.1, "OpenAPI schema: {err}");
                        Entry::Failed(err)
                    }
                };
                this.docs.insert(task_key, entry);
                cx.notify();
            })
            .ok();
        });
        self.docs.insert(key, Entry::Loading(task));
        Ok(None)
    }

    /// Forgets a failed or stale document so the next [`Self::get`] fetches it again.
    pub fn invalidate(&mut self, cluster: &ClusterId, group: &str, version: &str) {
        self.docs
            .remove(&(cluster.clone(), OpenApiIndex::key(group, version)));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// A cut-down cert-manager `Certificate` document, shaped like the API server's.
    pub fn cert_doc() -> Value {
        json!({
            "components": {"schemas": {
                "io.cert-manager.v1.Certificate": {
                    "type": "object",
                    "description": "A Certificate resource.",
                    "required": ["spec"],
                    "x-kubernetes-group-version-kind": [{"group": "cert-manager.io", "version": "v1", "kind": "Certificate"}],
                    "properties": {
                        "apiVersion": {"type": "string"},
                        "kind": {"type": "string"},
                        "metadata": {"description": "Standard object's metadata.", "allOf": [{"$ref": "#/components/schemas/io.k8s.apimachinery.pkg.apis.meta.v1.ObjectMeta"}]},
                        "spec": {
                            "type": "object",
                            "required": ["issuerRef", "secretName"],
                            "properties": {
                                "secretName": {"type": "string", "description": "Name of the Secret."},
                                "renewBefore": {"type": "string", "description": "How long before the currently issued certificate's expiry cert-manager should renew the certificate."},
                                "duration": {"type": "string"},
                                "commonName": {"type": "string"},
                                "dnsNames": {"type": "array", "items": {"type": "string"}},
                                "usages": {"type": "array", "items": {"type": "string", "enum": ["server auth", "client auth", "digital signature"]}},
                                "issuerRef": {"type": "object", "required": ["name"], "properties": {
                                    "name": {"type": "string"}, "kind": {"type": "string"}, "group": {"type": "string"}}},
                                "privateKey": {"type": "object", "properties": {
                                    "algorithm": {"type": "string", "enum": ["RSA", "ECDSA", "Ed25519"]},
                                    "rotationPolicy": {"type": "string", "enum": ["Never", "Always"]},
                                    "size": {"type": "integer"}}},
                                "secretTemplate": {"type": "object", "properties": {
                                    "labels": {"type": "object", "additionalProperties": {"type": "string"}}}},
                                "keystores": {"type": "object", "x-kubernetes-preserve-unknown-fields": true}
                            }
                        },
                        "status": {"type": "object", "properties": {"revision": {"type": "integer"}, "notAfter": {"type": "string", "format": "date-time"}}}
                    }
                },
                "io.k8s.apimachinery.pkg.apis.meta.v1.ObjectMeta": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Name must be unique within a namespace."},
                        "namespace": {"type": "string"},
                        "labels": {"type": "object", "additionalProperties": {"type": "string", "default": ""}},
                        "annotations": {"type": "object", "additionalProperties": {"type": "string", "default": ""}},
                        "resourceVersion": {"type": "string"},
                        "uid": {"type": "string"},
                        "generation": {"type": "integer", "format": "int64"},
                        "creationTimestamp": {"allOf": [{"$ref": "#/components/schemas/io.k8s.apimachinery.pkg.apis.meta.v1.Time"}]},
                        "managedFields": {"type": "array", "items": {"type": "object", "x-kubernetes-preserve-unknown-fields": true}}
                    }
                },
                "io.k8s.apimachinery.pkg.apis.meta.v1.Time": {"type": "string", "format": "date-time"}
            }}
        })
    }

    #[test]
    fn finds_kinds_and_follows_refs() {
        let doc = cert_doc();
        let gvk = Gvk::new("cert-manager.io", "v1", "Certificate");
        let root = Schema::for_gvk(&doc, &gvk).unwrap();
        assert_eq!(root.required(), ["spec"]);
        let meta = root.property("metadata").unwrap();
        assert_eq!(meta.description(), Some("Standard object's metadata."));
        assert_eq!(meta.type_name(), Some("object"));
        assert_eq!(
            root.at(&Path::parse("metadata.name")).unwrap().type_name(),
            Some("string")
        );
        assert_eq!(
            root.at(&Path::parse("metadata.labels.app"))
                .unwrap()
                .type_name(),
            Some("string")
        );
        assert_eq!(
            root.at(&Path::parse("metadata.creationTimestamp"))
                .unwrap()
                .type_label(),
            "string (date-time)"
        );
        let usages = root.at(&Path::parse("spec.usages")).unwrap();
        assert_eq!(usages.type_label(), "[]enum");
        assert_eq!(
            root.at(&Path::parse("spec.secretTemplate.labels"))
                .unwrap()
                .type_label(),
            "map[string]string"
        );
        assert!(
            root.at(&Path::parse("spec.keystores"))
                .unwrap()
                .allows_unknown()
        );
        assert!(!root.at(&Path::parse("spec")).unwrap().allows_unknown());
    }

    #[test]
    fn source_labels() {
        assert_eq!(
            source_label(
                &Gvk::new("cert-manager.io", "v1", "Certificate"),
                Some("certificates")
            ),
            "From CRD certificates.cert-manager.io · openAPIV3Schema v1"
        );
        assert_eq!(
            source_label(&Gvk::new("apps", "v1", "Deployment"), Some("deployments")),
            "Kubernetes API · apps/v1"
        );
        assert!(is_builtin_group("networking.k8s.io"));
        assert!(!is_builtin_group("gateway.networking.k8s.io"));
    }
}
