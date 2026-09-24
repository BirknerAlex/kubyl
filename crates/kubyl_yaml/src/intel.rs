//! Hover docs and completion, as plain functions over the buffer text and a schema lookup.
//! [`crate::lsp`] adapts them to gpui-component's editor providers.

use std::ops::Range;

use kubyl_core::{Gvk, Gvr};
use serde_json::Value;

use crate::parse::{self, Part, Path, Seg};
use crate::schema::Schema;

/// Finds the OpenAPI document for a kind (`None` while loading or unknown).
pub type SchemaLookup<'a> = &'a dyn Fn(&Gvk) -> Option<(std::sync::Arc<Value>, String)>;

/// `apiVersion` and `kind` of the document around `offset`, read line by line so it works
/// while the buffer doesn't parse.
pub fn doc_header(text: &str, offset: usize) -> Option<Gvk> {
    let offset = offset.min(text.len());
    let start = text[..offset].rfind("\n---").map_or(0, |i| i + 4);
    let end = text[offset..]
        .find("\n---")
        .map_or(text.len(), |i| offset + i);
    let mut api_version = None;
    let mut kind = None;
    for line in text[start..end].lines() {
        if let Some(v) = line.strip_prefix("apiVersion:") {
            api_version = Some(clean(v));
        } else if let Some(v) = line.strip_prefix("kind:") {
            kind = Some(clean(v));
        }
    }
    let api_version = api_version?;
    let kind = kind?;
    let (group, version) = match api_version.split_once('/') {
        Some((g, v)) => (g.to_string(), v.to_string()),
        None => (String::new(), api_version),
    };
    Some(Gvk::new(group, version, kind))
}

fn clean(value: &str) -> String {
    value
        .split(" #")
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

/// What the hover popup shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverInfo {
    pub path: String,
    pub type_label: String,
    pub required: bool,
    pub description: Option<String>,
    pub source: String,
}

impl HoverInfo {
    /// Markdown for the popup (board 3: path, type, required, description, source).
    pub fn markdown(&self) -> String {
        let mut out = format!(
            "`{}`: {} · {}",
            self.path,
            self.type_label,
            if self.required {
                "required"
            } else {
                "optional"
            }
        );
        if let Some(description) = &self.description {
            out.push_str("\n\n");
            out.push_str(&summary(description));
        }
        out.push_str("\n\n---\n\n");
        out.push_str(&self.source);
        out
    }
}

/// The first paragraph of a schema description (CRD descriptions run for pages), joined into
/// one line and cut at about 400 characters.
fn summary(description: &str) -> String {
    let first = description.split("\n\n").next().unwrap_or(description);
    let joined = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= 400 {
        return joined;
    }
    let cut: String = joined.chars().take(400).collect();
    match cut.rfind(' ') {
        Some(at) => format!("{}…", &cut[..at]),
        None => format!("{cut}…"),
    }
}

/// Hover information for the key or value at `offset`, and the range it applies to.
pub fn hover(text: &str, offset: usize, lookup: SchemaLookup) -> Option<(Range<usize>, HoverInfo)> {
    let parsed = parse::parse(text);
    let root = parsed.doc_at(offset)?.root.as_ref()?;
    let (path, part) = root.path_at(offset);
    if path.0.is_empty() {
        return None;
    }
    let range = match part {
        Part::Key => root.find_entry(&path)?.key.span.clone(),
        Part::Value => root.find(&path)?.span.clone(),
    };
    if !range.contains(&offset) && range.end != offset {
        return None;
    }
    let gvk = doc_header(text, offset)?;
    let (doc, source) = lookup(&gvk)?;
    let schema = Schema::for_gvk(&doc, &gvk)?;
    // For list items, describe the list.
    let mut field_path = path.clone();
    while matches!(field_path.0.last(), Some(Seg::Index(_))) {
        field_path.0.pop();
    }
    let field = schema.at(&field_path)?;
    let required = match field_path.0.split_last() {
        Some((Seg::Key(key), parent)) => schema
            .at(&Path(parent.to_vec()))
            .is_some_and(|p| p.required().contains(&key.as_str())),
        _ => false,
    };
    Some((
        range,
        HoverInfo {
            path: field_path.to_string(),
            type_label: field.type_label(),
            required,
            description: field.description().map(str::to_string),
            source,
        },
    ))
}

/// Resources whose names can be completed, by field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefKind {
    pub gvrs: Vec<Gvr>,
    pub namespaced: bool,
}

/// Which objects a field names, from its path (`secretName`, `configMapKeyRef.name`,
/// `issuerRef.name`…).
pub fn reference_kind(path: &Path) -> Option<RefKind> {
    let keys: Vec<&str> = path
        .0
        .iter()
        .filter_map(|s| match s {
            Seg::Key(k) => Some(k.as_str()),
            Seg::Index(_) => None,
        })
        .collect();
    let last = *keys.last()?;
    let parent = keys.len().checked_sub(2).map(|i| keys[i]).unwrap_or("");
    let core = |r: &str| Gvr::new("", "v1", r);
    let one = |gvr: Gvr, namespaced: bool| {
        Some(RefKind {
            gvrs: vec![gvr],
            namespaced,
        })
    };
    match (parent, last) {
        (_, "secretName") => one(core("secrets"), true),
        ("secretRef" | "secretKeyRef" | "imagePullSecrets" | "tlsSecretRef", "name") => {
            one(core("secrets"), true)
        }
        ("configMapRef" | "configMapKeyRef" | "configMap", "name") => one(core("configmaps"), true),
        (_, "serviceAccountName") => one(core("serviceaccounts"), true),
        (_, "claimName") => one(core("persistentvolumeclaims"), true),
        (_, "storageClassName") => one(Gvr::new("storage.k8s.io", "v1", "storageclasses"), false),
        (_, "ingressClassName") => {
            one(Gvr::new("networking.k8s.io", "v1", "ingressclasses"), false)
        }
        (_, "priorityClassName") => one(
            Gvr::new("scheduling.k8s.io", "v1", "priorityclasses"),
            false,
        ),
        (_, "nodeName") => one(core("nodes"), false),
        ("issuerRef", "name") => Some(RefKind {
            gvrs: vec![
                Gvr::new("cert-manager.io", "v1", "issuers"),
                Gvr::new("cert-manager.io", "v1", "clusterissuers"),
            ],
            namespaced: true,
        }),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateKind {
    Field,
    Value,
    Kind,
    Reference,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub label: String,
    pub detail: String,
    pub insert: String,
    pub kind: CandidateKind,
    /// Required fields first.
    pub required: bool,
}

/// What a completion request needs from the cluster.
pub struct CompletionContext<'a> {
    pub lookup: SchemaLookup<'a>,
    /// `(apiVersion, kind)` of every served kind.
    pub kinds: &'a [(String, String)],
    /// Names of objects of the given kind (from the watch caches).
    pub names: &'a dyn Fn(&RefKind) -> Vec<String>,
}

/// Completions at `offset`, and the range they replace.
pub fn complete(
    text: &str,
    offset: usize,
    ctx: &CompletionContext,
) -> (Range<usize>, Vec<Candidate>) {
    let offset = offset.min(text.len());
    let line_range = parse::line_range(text, offset);
    let before = &text[line_range.start..offset];
    let word_start = offset
        - before
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
            .map(char::len_utf8)
            .sum::<usize>();
    let word = text[word_start..offset].to_lowercase();
    let body = before.trim_start().trim_start_matches("- ").trim_start();
    let mut candidates = match body.split_once(':') {
        None => complete_keys(text, offset, ctx),
        Some((key, _)) => complete_values(text, offset, key.trim(), ctx),
    };
    if !word.is_empty() {
        candidates.retain(|c| c.label.to_lowercase().contains(&word));
        candidates.sort_by_key(|c| (!c.label.to_lowercase().starts_with(&word), !c.required));
    }
    (word_start..offset, candidates)
}

fn complete_keys(text: &str, offset: usize, ctx: &CompletionContext) -> Vec<Candidate> {
    let (parent, key_col) = parse::parent_path_by_indent(text, offset);
    let Some(gvk) = doc_header(text, offset) else {
        // A new document: offer the header first.
        return ["apiVersion", "kind", "metadata"]
            .iter()
            .filter(|_| parent.0.is_empty())
            .map(|k| Candidate {
                label: k.to_string(),
                detail: "object header".into(),
                insert: format!("{k}: "),
                kind: CandidateKind::Field,
                required: true,
            })
            .collect();
    };
    let Some((doc, _)) = (ctx.lookup)(&gvk) else {
        return Vec::new();
    };
    let Some(root) = Schema::for_gvk(&doc, &gvk) else {
        return Vec::new();
    };
    let Some(schema) = root.at(&parent) else {
        return Vec::new();
    };
    let existing = sibling_keys(text, offset, key_col);
    let required = schema.required();
    let indent = " ".repeat(key_col + 2);
    schema
        .properties()
        .into_iter()
        .filter(|(name, _)| !existing.iter().any(|e| e == name))
        .filter(|(name, _)| !(parent.0.is_empty() && *name == "status"))
        .map(|(name, child)| {
            let insert = match child.type_name() {
                Some("object") => format!("{name}:\n{indent}"),
                Some("array") => format!("{name}:\n{indent}- "),
                _ => format!("{name}: "),
            };
            Candidate {
                label: name.to_string(),
                detail: child.type_label(),
                insert,
                kind: CandidateKind::Field,
                required: required.contains(&name),
            }
        })
        .collect()
}

/// Keys already present in the block around `offset` at column `key_col`.
fn sibling_keys(text: &str, offset: usize, key_col: usize) -> Vec<String> {
    let current = parse::line_range(text, offset);
    let mut keys = Vec::new();
    let mut scan = |range: Range<usize>| -> bool {
        let line = &text[range];
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return true;
        }
        let col = line.len() - trimmed.len();
        let (col, body) = match trimmed.strip_prefix("- ") {
            Some(rest) => (col + 2, rest),
            None => (col, trimmed),
        };
        if col < key_col || trimmed == "---" {
            return false;
        }
        if col == key_col
            && let Some((key, _)) = body.split_once(':')
        {
            keys.push(key.trim().trim_matches('"').to_string());
        }
        // A new list item at this level ends the block.
        !(trimmed.starts_with("- ") && col == key_col)
    };
    let mut end = current.start;
    while end > 0 {
        let range = parse::line_range(text, end - 1);
        end = range.start;
        if !scan(range) {
            break;
        }
    }
    let mut start = current.end;
    while start < text.len() {
        let range = parse::line_range(text, start + 1);
        start = range.end;
        if !scan(range) {
            break;
        }
    }
    keys
}

fn complete_values(
    text: &str,
    offset: usize,
    key: &str,
    ctx: &CompletionContext,
) -> Vec<Candidate> {
    let (parent, key_col) = parse::parent_path_by_indent(text, offset);
    let path = parent.push_key(key);
    // The document header: apiVersion/kind pairs from discovery.
    if parent.0.is_empty() && key_col == 0 {
        match key {
            "apiVersion" => {
                let mut versions: Vec<&String> = ctx.kinds.iter().map(|(v, _)| v).collect();
                versions.sort();
                versions.dedup();
                return versions
                    .into_iter()
                    .map(|v| Candidate {
                        label: v.clone(),
                        detail: ctx
                            .kinds
                            .iter()
                            .filter(|(av, _)| av == v)
                            .map(|(_, k)| k.as_str())
                            .take(4)
                            .collect::<Vec<_>>()
                            .join(", "),
                        insert: v.clone(),
                        kind: CandidateKind::Kind,
                        required: false,
                    })
                    .collect();
            }
            "kind" => {
                let api_version = doc_header(text, offset)
                    .map(|g| g.api_version())
                    .or_else(|| header_value(text, offset, "apiVersion"));
                return ctx
                    .kinds
                    .iter()
                    .filter(|(v, _)| {
                        api_version
                            .as_ref()
                            .is_none_or(|av| av.is_empty() || av == v)
                    })
                    .map(|(v, k)| Candidate {
                        label: k.clone(),
                        detail: v.clone(),
                        insert: k.clone(),
                        kind: CandidateKind::Kind,
                        required: false,
                    })
                    .collect();
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Some(gvk) = doc_header(text, offset)
        && let Some((doc, _)) = (ctx.lookup)(&gvk)
        && let Some(root) = Schema::for_gvk(&doc, &gvk)
        && let Some(field) = root.at(&path)
    {
        if let Some(values) = field.enum_values() {
            out.extend(values.iter().map(|v| {
                let label = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                Candidate {
                    insert: crate::render::yaml_scalar(&label, key_col),
                    label,
                    detail: "enum".into(),
                    kind: CandidateKind::Value,
                    required: false,
                }
            }));
        } else if field.type_name() == Some("boolean") {
            out.extend(["true", "false"].map(|b| Candidate {
                label: b.into(),
                detail: "boolean".into(),
                insert: b.into(),
                kind: CandidateKind::Value,
                required: false,
            }));
        }
    }
    if let Some(refs) = reference_kind(&path) {
        let kind_label = refs
            .gvrs
            .iter()
            .map(|g| g.resource.trim_end_matches('s').to_string())
            .collect::<Vec<_>>()
            .join("/");
        out.extend((ctx.names)(&refs).into_iter().map(|name| Candidate {
            insert: name.clone(),
            label: name,
            detail: kind_label.clone(),
            kind: CandidateKind::Reference,
            required: false,
        }));
    }
    out
}

fn header_value(text: &str, offset: usize, key: &str) -> Option<String> {
    let start = text[..offset.min(text.len())]
        .rfind("\n---")
        .map_or(0, |i| i + 4);
    text[start..]
        .lines()
        .take_while(|l| *l != "---")
        .find_map(|l| l.strip_prefix(&format!("{key}:")).map(clean))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::cert_doc;
    use std::sync::Arc;

    fn lookup() -> impl Fn(&Gvk) -> Option<(Arc<Value>, String)> {
        let doc = Arc::new(cert_doc());
        move |gvk: &Gvk| {
            (gvk.kind == "Certificate").then(|| {
                (
                    doc.clone(),
                    "From CRD certificates.cert-manager.io · openAPIV3Schema v1".to_string(),
                )
            })
        }
    }

    const CERT: &str = "apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: api-tls
spec:
  secretName: api-tls
  renewBefore: 720h # 30d
  issuerRef:
    name: letsencrypt-prod
  privateKey:
    rotationPolicy: Always
";

    #[test]
    fn hover_on_renew_before() {
        let lookup = lookup();
        let offset = CERT.find("renewBefore").unwrap() + 4;
        let (range, info) = hover(CERT, offset, &lookup).unwrap();
        assert_eq!(&CERT[range], "renewBefore");
        assert_eq!(info.path, "spec.renewBefore");
        assert_eq!(info.type_label, "string");
        assert!(!info.required);
        assert!(info.description.unwrap().starts_with("How long before"));
        let md = hover(CERT, CERT.find("secretName").unwrap(), &lookup)
            .unwrap()
            .1
            .markdown();
        assert!(
            md.starts_with("`spec.secretName`: string · required"),
            "{md}"
        );
        assert!(md.ends_with("From CRD certificates.cert-manager.io · openAPIV3Schema v1"));
        assert_eq!(summary("One\nline.\n\nMore."), "One line.");
    }

    fn ctx_complete(text: &str) -> Vec<String> {
        let lookup = lookup();
        let kinds = vec![
            ("cert-manager.io/v1".to_string(), "Certificate".to_string()),
            ("cert-manager.io/v1".to_string(), "Issuer".to_string()),
            ("apps/v1".to_string(), "Deployment".to_string()),
        ];
        let names = |r: &RefKind| {
            if r.gvrs[0].resource == "secrets" {
                vec!["api-tls".to_string(), "db".to_string()]
            } else {
                vec!["letsencrypt-prod".to_string()]
            }
        };
        let ctx = CompletionContext {
            lookup: &lookup,
            kinds: &kinds,
            names: &names,
        };
        complete(text, text.len(), &ctx)
            .1
            .into_iter()
            .map(|c| c.label)
            .collect()
    }

    #[test]
    fn completes_fields_values_kinds_and_names() {
        // Missing fields of spec (existing ones are skipped), required first.
        let text = format!("{CERT}  ");
        let fields = ctx_complete(&text);
        assert!(fields.contains(&"dnsNames".to_string()));
        assert!(!fields.contains(&"secretName".to_string()));
        let text = format!("{CERT}  du");
        assert_eq!(ctx_complete(&text), ["duration"]);
        // Enum values.
        let text = CERT.replace("rotationPolicy: Always\n", "rotationPolicy: ");
        assert_eq!(ctx_complete(&text), ["Never", "Always"]);
        // References.
        let text = CERT.replace("secretName: api-tls\n  renewBefore: 720h # 30d\n  issuerRef:\n    name: letsencrypt-prod\n  privateKey:\n    rotationPolicy: Always\n", "secretName: d");
        assert_eq!(ctx_complete(&text), ["db"]);
        let text = "apiVersion: cert-manager.io/v1\nkind: ";
        assert_eq!(ctx_complete(text), ["Certificate", "Issuer"]);
        let text = "apiVersion: ap";
        assert_eq!(ctx_complete(text), ["apps/v1"]);
    }

    #[test]
    fn reference_kinds() {
        assert_eq!(
            reference_kind(&Path::parse(
                "spec.template.spec.containers[0].env[0].valueFrom.secretKeyRef.name"
            ))
            .unwrap()
            .gvrs[0]
                .resource,
            "secrets"
        );
        assert_eq!(
            reference_kind(&Path::parse("spec.issuerRef.name"))
                .unwrap()
                .gvrs
                .len(),
            2
        );
        assert!(reference_kind(&Path::parse("metadata.name")).is_none());
    }
}
