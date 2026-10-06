//! Kubyl's cluster tools for agents (the `kubyl` MCP server's tools), read-only.
//!
//! Every tool works on the thread's cluster through the user's own client from
//! `ConnectionManager` (the API server applies their RBAC), masks Secret values and Route keys
//! ([`kubyl_resources_core::redact`]), never returns Helm release storage, scrubs token shapes
//! from text, and caps its output at the configured size (the newest part of logs, the start of
//! everything else).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, DynamicObject, ListParams, LogParams};
use kubyl_base::{CellValue, ClusterCaps, ClusterId, ResourceRef};
use kubyl_kube_core::access::{self, AccessQuery};
use kubyl_kube_core::discovery::{ApiResourceInfo, Discovery};
use kubyl_metrics_core::prometheus::PromClient;
use kubyl_resources_core::{columns, describe, format, redact, store};
use serde_json::{Value, json};

/// How long one tool call may take.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// Most objects a list returns.
const MAX_LIST: u32 = 500;
const DEFAULT_LIST: u32 = 100;

/// An alert as the `alerts` tool reports it (filled from `kubyl_alerts` by the app).
#[derive(Clone, Debug, PartialEq)]
pub struct AlertSummary {
    pub name: String,
    pub severity: String,
    pub state: String,
    pub since: Option<Timestamp>,
    pub summary: String,
    pub labels: BTreeMap<String, String>,
}

/// What the tools of one thread can reach. The app refreshes it when the connection, the
/// selection or the alerts change.
#[derive(Clone, Default)]
pub struct ToolContext {
    pub cluster: Option<ClusterId>,
    pub cluster_name: String,
    pub client: Option<kube::Client>,
    pub discovery: Option<Arc<Discovery>>,
    pub caps: ClusterCaps,
    /// The namespace the title bar shows for this cluster.
    pub namespace: Option<String>,
    /// What Kubyl has selected on this cluster.
    pub selection: Option<ResourceRef>,
    pub selection_kind: Option<String>,
    pub prometheus: Option<PromClient>,
    /// `None`: no alert source.
    pub alerts: Option<Arc<Vec<AlertSummary>>>,
    pub max_output: usize,
    pub log_lines: usize,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("cluster", &self.cluster)
            .field("connected", &self.client.is_some())
            .finish_non_exhaustive()
    }
}

/// A tool's answer: text for the agent, and whether it failed.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

impl ToolOutput {
    fn ok(text: String) -> Self {
        Self {
            text,
            is_error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// The tools for `tools/list`: name, description and JSON schema of the arguments.
pub fn definitions() -> Vec<Value> {
    let tool = |name: &str, description: &str, properties: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": {
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false },
        })
    };
    let kind = json!({"type": "string", "description": "Kind, plural or short name, optionally with the group: `pods`, `deploy`, `Deployment`, `certificates.cert-manager.io`."});
    let namespace = json!({"type": "string", "description": "Namespace. Omit for cluster-scoped kinds or (lists) all namespaces."});
    let name = json!({"type": "string", "description": "Object name."});
    vec![
        tool(
            "cluster_info",
            "The cluster this conversation is about: name, Kubernetes version, flags (production, read-only), what it serves (OpenShift, metrics, Prometheus, alerts) and what the user has open in Kubyl. Call this first.",
            json!({}),
            &[],
        ),
        tool(
            "list_resources",
            "List objects of a kind, like `kubectl get` (the same columns Kubyl shows). Secrets are listed without their values.",
            json!({
                "kind": kind,
                "namespace": namespace,
                "label_selector": {"type": "string", "description": "e.g. `app=web,tier!=cache`"},
                "field_selector": {"type": "string", "description": "e.g. `status.phase!=Running`"},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIST, "description": "At most this many (default 100)."},
            }),
            &["kind"],
        ),
        tool(
            "get_resource",
            "One object as YAML, like `kubectl get -o yaml`. Secret values and private keys are masked; Helm release storage isn't returned.",
            json!({ "kind": kind, "name": name, "namespace": namespace }),
            &["kind", "name"],
        ),
        tool(
            "describe",
            "Describe an object like `kubectl describe`: status, conditions, containers, owners and its recent events.",
            json!({ "kind": kind, "name": name, "namespace": namespace }),
            &["kind", "name"],
        ),
        tool(
            "events",
            "Recent Kubernetes events, newest first: in a namespace, for one object, or only warnings.",
            json!({
                "namespace": namespace,
                "kind": {"type": "string", "description": "Kind of the involved object, e.g. `Pod`."},
                "name": {"type": "string", "description": "Name of the involved object."},
                "warnings_only": {"type": "boolean"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 500},
            }),
            &[],
        ),
        tool(
            "logs",
            "A pod's recent log lines (the newest). Token-like strings are masked.",
            json!({
                "namespace": {"type": "string"},
                "pod": {"type": "string"},
                "container": {"type": "string", "description": "Default: the pod's only or first container."},
                "previous": {"type": "boolean", "description": "The previous (crashed) container's logs."},
                "tail_lines": {"type": "integer", "minimum": 1, "maximum": 5000},
                "since_seconds": {"type": "integer", "minimum": 1},
                "grep": {"type": "string", "description": "Only lines containing this text (case-insensitive)."},
            }),
            &["namespace", "pod"],
        ),
        tool(
            "top",
            "CPU and memory usage of pods or nodes from metrics-server, like `kubectl top`.",
            json!({
                "kind": {"type": "string", "enum": ["pods", "nodes"]},
                "namespace": {"type": "string", "description": "Pods only; omit for all namespaces."},
            }),
            &["kind"],
        ),
        tool(
            "query_prometheus",
            "Run a PromQL query against the Prometheus Kubyl found for this cluster: an instant query, or a range over the last `range_minutes`.",
            json!({
                "query": {"type": "string"},
                "range_minutes": {"type": "integer", "minimum": 1, "maximum": 10080},
                "step_seconds": {"type": "integer", "minimum": 1},
            }),
            &["query"],
        ),
        tool(
            "alerts",
            "Firing and pending alerts on this cluster (Alertmanager / Prometheus), as Kubyl shows them.",
            json!({ "namespace": {"type": "string", "description": "Only alerts with this namespace label."} }),
            &[],
        ),
        tool(
            "can_i",
            "Whether the user may do something, like `kubectl auth can-i`.",
            json!({
                "verb": {"type": "string", "description": "get, list, watch, create, update, patch, delete…"},
                "kind": kind,
                "namespace": namespace,
                "name": {"type": "string"},
                "subresource": {"type": "string", "description": "e.g. `log`, `exec`, `scale`"},
            }),
            &["verb", "kind"],
        ),
        tool(
            "api_resources",
            "The kinds the cluster serves (including CRDs), like `kubectl api-resources`.",
            json!({ "filter": {"type": "string", "description": "Only kinds whose name or group contains this."} }),
            &[],
        ),
    ]
}

/// Runs tool `name` with `args`. Never panics on bad input; errors come back as text.
pub async fn call(context: &ToolContext, name: &str, args: &Value) -> ToolOutput {
    let result = tokio::time::timeout(TIMEOUT, dispatch(context, name, args)).await;
    let output = match result {
        Ok(Ok(text)) => ToolOutput::ok(text),
        Ok(Err(message)) => ToolOutput::error(message),
        Err(_) => ToolOutput::error(format!(
            "{name} took longer than {}s; narrow it down.",
            TIMEOUT.as_secs()
        )),
    };
    let tail = name == "logs";
    ToolOutput {
        text: cap(
            &redact::scrub_text(&output.text),
            context.max_output.max(1024),
            tail,
        ),
        is_error: output.is_error,
    }
}

async fn dispatch(context: &ToolContext, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "cluster_info" => cluster_info(context).await,
        "list_resources" => list_resources(context, args).await,
        "get_resource" => get_resource(context, args).await,
        "describe" => describe_object(context, args).await,
        "events" => events(context, args).await,
        "logs" => logs(context, args).await,
        "top" => top(context, args).await,
        "query_prometheus" => query_prometheus(context, args).await,
        "alerts" => alerts(context, args),
        "can_i" => can_i(context, args).await,
        "api_resources" => api_resources(context, args),
        other => Err(format!("Kubyl has no tool named `{other}`.")),
    }
}

/// `text` cut to `max` bytes: the end of it (`tail`) or the start, with a note.
fn cap(text: &str, max: usize, tail: bool) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let dropped = text.len() - max;
    if tail {
        let mut start = dropped;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        format!(
            "[{dropped} earlier bytes not shown; use tail_lines, since_seconds or grep]\n{}",
            &text[start..]
        )
    } else {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!(
            "{}\n[{dropped} more bytes not shown; narrow it with namespace, label_selector or limit]",
            &text[..end]
        )
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    str_arg(args, key).ok_or_else(|| format!("`{key}` is required."))
}

fn int_arg(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

fn bool_arg(args: &Value, key: &str) -> bool {
    args.get(key)
        .and_then(|v| v.as_bool().or_else(|| v.as_str().map(|s| s == "true")))
        .unwrap_or(false)
}

fn client(context: &ToolContext) -> Result<kube::Client, String> {
    context.client.clone().ok_or_else(|| {
        format!(
            "{} isn't connected in Kubyl. Ask the user to connect it.",
            context.cluster_name
        )
    })
}

fn resolve<'a>(context: &'a ToolContext, kind: &str) -> Result<&'a ApiResourceInfo, String> {
    let discovery = context
        .discovery
        .as_ref()
        .ok_or_else(|| format!("{} isn't connected in Kubyl.", context.cluster_name))?;
    discovery.resolve(kind).ok_or_else(|| {
        format!("The cluster doesn't serve `{kind}`. Call api_resources to see what it serves.")
    })
}

fn api_for(
    client: kube::Client,
    info: &ApiResourceInfo,
    namespace: Option<&str>,
) -> Api<DynamicObject> {
    let resource = store::api_resource(info);
    match (info.namespaced, namespace) {
        (true, Some(ns)) => Api::namespaced_with(client, ns, &resource),
        _ => Api::all_with(client, &resource),
    }
}

fn kube_error(err: kube::Error) -> String {
    match err {
        kube::Error::Api(status) if status.code == 403 => format!(
            "Forbidden: the user's account may not do this ({}).",
            status.message
        ),
        kube::Error::Api(status) if status.code == 404 => format!("Not found: {}", status.message),
        kube::Error::Api(status) => status.message,
        other => other.to_string(),
    }
}

/// An object as JSON with its `apiVersion` and `kind` (list items come without them).
fn typed_json(object: &DynamicObject, info: &ApiResourceInfo) -> Option<Value> {
    let mut value = store::to_json(object)?;
    value["apiVersion"] = Value::String(info.gvk.api_version());
    value["kind"] = Value::String(info.gvk.kind.clone());
    Some(value)
}

fn refuse_helm_release(info: &ApiResourceInfo, object: &Value) -> Result<(), String> {
    if info.gvk.group.is_empty() && redact::is_helm_release(object) {
        return Err(format!(
            "{} holds a Helm release (its values and manifest), which Kubyl doesn't give to agents. Use list_resources on the release's objects instead.",
            format::name(object)
        ));
    }
    Ok(())
}

async fn cluster_info(context: &ToolContext) -> Result<String, String> {
    let mut out = String::new();
    writeln!(out, "Cluster: {}", context.cluster_name).ok();
    let Some(client) = context.client.clone() else {
        writeln!(out, "Connection: not connected in Kubyl").ok();
        return Ok(out);
    };
    match client.apiserver_version().await {
        Ok(version) => writeln!(
            out,
            "Kubernetes: {} ({})",
            version.git_version, version.platform
        )
        .ok(),
        Err(err) => writeln!(out, "Kubernetes: unknown ({})", kube_error(err)).ok(),
    };
    let caps = &context.caps;
    let mut flags = Vec::new();
    if caps.production {
        flags.push("production");
    }
    if caps.read_only {
        flags.push("read-only in Kubyl");
    }
    if caps.openshift {
        flags.push("OpenShift");
    }
    if caps.olm {
        flags.push("OLM");
    }
    if caps.argocd.any() {
        flags.push("Argo CD");
    }
    writeln!(out, "Flags: {}", format::join_or_none(&flags)).ok();
    writeln!(
        out,
        "Metrics: metrics-server {}, Prometheus {}",
        if caps.metrics_server { "yes" } else { "no" },
        if context.prometheus.is_some() {
            "yes (query_prometheus)"
        } else {
            "not found"
        }
    )
    .ok();
    writeln!(
        out,
        "Alerts: {}",
        match &context.alerts {
            Some(alerts) => format!("{} active (alerts)", alerts.len()),
            None => "no Alertmanager or Prometheus rules found".into(),
        }
    )
    .ok();
    if let Some(ns) = &context.namespace {
        writeln!(out, "Namespace in Kubyl's title bar: {ns}").ok();
    }
    if let Some(selection) = &context.selection {
        let kind = context
            .selection_kind
            .clone()
            .unwrap_or_else(|| selection.gvr.resource.clone());
        match (&selection.namespace, &selection.name) {
            (Some(ns), Some(name)) => writeln!(out, "Selected in Kubyl: {kind} {ns}/{name}").ok(),
            (None, Some(name)) => writeln!(out, "Selected in Kubyl: {kind} {name}").ok(),
            _ => writeln!(out, "Open in Kubyl: {kind} list").ok(),
        };
    }
    if let Some(discovery) = &context.discovery {
        writeln!(out, "Kinds served: {}", discovery.kind_count()).ok();
    }
    Ok(out)
}

fn cell_text(cell: CellValue) -> String {
    match cell {
        CellValue::Text(s)
        | CellValue::Tinted { label: s, .. }
        | CellValue::Status { label: s, .. }
        | CellValue::Usage { label: s, .. } => s.to_string(),
        CellValue::Buttons(_) | CellValue::Empty => "-".into(),
    }
}

/// Rows as an aligned text table.
fn table(headers: &[String], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(w) = widths.get_mut(i) {
                *w = (*w).max(cell.chars().count().min(80));
            }
        }
    }
    let mut out = String::new();
    let line = |cells: &[String], out: &mut String| {
        let last = cells.len().saturating_sub(1);
        for (i, cell) in cells.iter().enumerate() {
            let cell: String = cell.replace('\n', " ");
            if i == last {
                out.push_str(&cell);
            } else {
                let pad = widths[i].saturating_sub(cell.chars().count()) + 3;
                out.push_str(&cell);
                out.extend(std::iter::repeat_n(' ', pad));
            }
        }
        out.push('\n');
    };
    line(headers, &mut out);
    for row in rows {
        line(row, &mut out);
    }
    out
}

async fn list_resources(context: &ToolContext, args: &Value) -> Result<String, String> {
    let info = resolve(context, required(args, "kind")?)?;
    if !info.supports("list") {
        return Err(format!("`{}` can't be listed.", info.gvr));
    }
    let namespace = str_arg(args, "namespace");
    let limit = int_arg(args, "limit")
        .map(|l| l.clamp(1, MAX_LIST as i64) as u32)
        .unwrap_or(DEFAULT_LIST);
    let mut params = ListParams::default().limit(limit);
    if let Some(selector) = str_arg(args, "label_selector") {
        params = params.labels(selector);
    }
    if let Some(selector) = str_arg(args, "field_selector") {
        params = params.fields(selector);
    }
    let api = api_for(client(context)?, info, namespace);
    let list = api.list(&params).await.map_err(kube_error)?;
    let more = list
        .metadata
        .continue_
        .as_deref()
        .is_some_and(|c| !c.is_empty());
    let objects: Vec<Value> = list
        .items
        .iter()
        .filter_map(|o| typed_json(o, info))
        .filter(|o| !(info.gvk.group.is_empty() && redact::is_helm_release(o)))
        .collect();
    if objects.is_empty() {
        let scope = match namespace {
            Some(ns) if info.namespaced => format!(" in {ns}"),
            _ => String::new(),
        };
        return Ok(format!("No {}{scope}.", info.gvr));
    }

    let builtin = columns::builtin();
    let kind = builtin
        .iter()
        .find(|(group, kind, _)| *group == info.gvk.group && *kind == info.gvk.kind)
        .map(|(_, _, kind)| kind);
    let all_namespaces = info.namespaced && namespace.is_none();
    let mut headers = Vec::new();
    if all_namespaces {
        headers.push("NAMESPACE".to_string());
    }
    let mut rows: Vec<Vec<String>> = objects
        .iter()
        .map(|o| {
            if all_namespaces {
                vec![format::namespace(o).unwrap_or("").to_string()]
            } else {
                Vec::new()
            }
        })
        .collect();
    match kind {
        Some(kind) => {
            let defs: Vec<_> = kind
                .columns()
                .into_iter()
                .filter(|c| !matches!(c.id.as_ref(), "cpu" | "memory") && c.id != "namespace")
                .collect();
            headers.extend(defs.iter().map(|c| c.title.to_uppercase()));
            for (row, object) in rows.iter_mut().zip(&objects) {
                row.extend(defs.iter().map(|c| cell_text(kind.cell(object, &c.id))));
            }
        }
        None => {
            let now = Timestamp::now();
            headers.extend(["NAME".to_string(), "AGE".to_string()]);
            for (row, object) in rows.iter_mut().zip(&objects) {
                row.push(format::name(object).to_string());
                row.push(format::object_age(object, now));
            }
        }
    }
    let mut out = table(&headers, &rows);
    if more {
        writeln!(
            out,
            "[more than {limit} objects: narrow it with namespace or selectors, or raise limit]"
        )
        .ok();
    }
    Ok(out)
}

async fn get_object(
    context: &ToolContext,
    args: &Value,
) -> Result<(Value, ApiResourceInfo), String> {
    let info = resolve(context, required(args, "kind")?)?.clone();
    let name = required(args, "name")?;
    let namespace = str_arg(args, "namespace").or(context.namespace.as_deref());
    if info.namespaced && namespace.is_none() {
        return Err(format!("`namespace` is required for {}.", info.gvr));
    }
    let api = api_for(client(context)?, &info, namespace);
    let object = api.get(name).await.map_err(kube_error)?;
    let value = typed_json(&object, &info).ok_or("The object couldn't be read.")?;
    refuse_helm_release(&info, &value)?;
    Ok((value, info))
}

async fn get_resource(context: &ToolContext, args: &Value) -> Result<String, String> {
    let (mut value, _) = get_object(context, args).await?;
    redact::mask_object(&mut value);
    Ok(format::to_yaml(&value))
}

async fn object_events(client: kube::Client, object: &Value) -> Vec<Value> {
    let meta = &object["metadata"];
    let namespace = meta["namespace"].as_str();
    let fields = match (namespace, meta["uid"].as_str()) {
        (Some(_), Some(uid)) => format!("involvedObject.uid={uid}"),
        _ => format!(
            "involvedObject.kind={},involvedObject.name={}",
            object["kind"].as_str().unwrap_or_default(),
            format::name(object)
        ),
    };
    let resource = kube::api::ApiResource::erase::<k8s_openapi::api::core::v1::Event>(&());
    let api: Api<DynamicObject> = match namespace {
        Some(ns) => Api::namespaced_with(client, ns, &resource),
        None => Api::all_with(client, &resource),
    };
    match api
        .list(&ListParams::default().fields(&fields).limit(200))
        .await
    {
        Ok(list) => list.items.iter().filter_map(store::to_json).collect(),
        Err(_) => Vec::new(),
    }
}

async fn describe_object(context: &ToolContext, args: &Value) -> Result<String, String> {
    let (original, info) = get_object(context, args).await?;
    // `describe` prints a Secret's value sizes, never the values: give it the real `data` (so
    // the sizes are right) and mask everything else, such as kubectl's last-applied copy.
    let mut value = original.clone();
    redact::mask_object(&mut value);
    if value["kind"] == "Secret" {
        value["data"] = original["data"].clone();
        value["stringData"] = original["stringData"].clone();
    }
    let events = object_events(client(context)?, &value).await;
    let mut events: Vec<Value> = events;
    for event in &mut events {
        if let Some(message) = event.get_mut("message")
            && let Some(text) = message.as_str()
        {
            *message = Value::String(redact::scrub_text(text).into_owned());
        }
    }
    let text = describe::describe(&info.gvk.kind, &value, &events, Timestamp::now());
    Ok(text)
}

async fn events(context: &ToolContext, args: &Value) -> Result<String, String> {
    let namespace = str_arg(args, "namespace");
    let limit = int_arg(args, "limit")
        .map(|l| l.clamp(1, 500) as usize)
        .unwrap_or(100);
    let mut fields = Vec::new();
    if let Some(kind) = str_arg(args, "kind") {
        let kind = context
            .discovery
            .as_ref()
            .and_then(|d| d.resolve(kind))
            .map(|i| i.gvk.kind.clone())
            .unwrap_or_else(|| kind.to_string());
        fields.push(format!("involvedObject.kind={kind}"));
    }
    if let Some(name) = str_arg(args, "name") {
        fields.push(format!("involvedObject.name={name}"));
    }
    if bool_arg(args, "warnings_only") {
        fields.push("type=Warning".into());
    }
    let resource = kube::api::ApiResource::erase::<k8s_openapi::api::core::v1::Event>(&());
    let client = client(context)?;
    let api: Api<DynamicObject> = match namespace {
        Some(ns) => Api::namespaced_with(client, ns, &resource),
        None => Api::all_with(client, &resource),
    };
    let mut params = ListParams::default().limit(1000);
    if !fields.is_empty() {
        params = params.fields(&fields.join(","));
    }
    let list = api.list(&params).await.map_err(kube_error)?;
    let mut items: Vec<Value> = list.items.iter().filter_map(store::to_json).collect();
    items.sort_by_key(|e| std::cmp::Reverse(kubyl_resources_core::status::event_time(e)));
    items.truncate(limit);
    if items.is_empty() {
        return Ok("No events (Kubernetes keeps them for about an hour).".into());
    }
    let now = Timestamp::now();
    let headers: Vec<String> = ["LAST", "TYPE", "REASON", "OBJECT", "COUNT", "MESSAGE"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|e| {
            let last = kubyl_resources_core::status::event_time(e)
                .map(|t| format::human_duration(format::seconds_since(t, now)))
                .unwrap_or_else(|| "-".into());
            let object = kubyl_resources_core::status::event_object(e);
            let object = match (namespace, e["metadata"]["namespace"].as_str()) {
                (None, Some(ns)) => format!("{ns}/{object}"),
                _ => object,
            };
            vec![
                last,
                e["type"].as_str().unwrap_or("").into(),
                e["reason"].as_str().unwrap_or("").into(),
                object,
                e["count"].as_i64().unwrap_or(1).to_string(),
                kubyl_resources_core::status::event_message(e).to_string(),
            ]
        })
        .collect();
    Ok(table(&headers, &rows))
}

async fn logs(context: &ToolContext, args: &Value) -> Result<String, String> {
    let namespace = required(args, "namespace")?;
    let pod = required(args, "pod")?;
    let client = client(context)?;
    let api: Api<Pod> = Api::namespaced(client, namespace);
    let container = match str_arg(args, "container") {
        Some(c) => Some(c.to_string()),
        None => {
            let pod = api.get(pod).await.map_err(kube_error)?;
            let containers = pod.spec.map(|s| s.containers).unwrap_or_default();
            if containers.len() > 1 {
                let names: Vec<_> = containers.iter().map(|c| c.name.as_str()).collect();
                return Err(format!(
                    "{pod_name} has several containers; pass `container` (one of: {}).",
                    names.join(", "),
                    pod_name = args["pod"].as_str().unwrap_or_default()
                ));
            }
            containers.first().map(|c| c.name.clone())
        }
    };
    let tail = int_arg(args, "tail_lines")
        .map(|t| t.clamp(1, 5000))
        .unwrap_or(context.log_lines.max(1) as i64);
    let params = LogParams {
        container,
        previous: bool_arg(args, "previous"),
        tail_lines: Some(tail),
        since_seconds: int_arg(args, "since_seconds").filter(|s| *s > 0),
        limit_bytes: Some((context.max_output.max(1024) * 4) as i64),
        ..LogParams::default()
    };
    let text = api.logs(pod, &params).await.map_err(kube_error)?;
    let text = match str_arg(args, "grep") {
        Some(needle) => {
            let needle = needle.to_lowercase();
            let lines: Vec<&str> = text
                .lines()
                .filter(|l| l.to_lowercase().contains(&needle))
                .collect();
            if lines.is_empty() {
                return Ok(format!("No lines contain `{needle}`."));
            }
            lines.join("\n")
        }
        None => text,
    };
    if text.trim().is_empty() {
        return Ok("No log lines.".into());
    }
    Ok(text)
}

async fn top(context: &ToolContext, args: &Value) -> Result<String, String> {
    let client = client(context)?;
    let nodes = matches!(required(args, "kind")?, "nodes" | "node" | "no");
    let error = |e: kubyl_metrics_core::metrics_server::FetchError| {
        if e.forbidden {
            "Forbidden: the user may not read metrics.k8s.io.".to_string()
        } else {
            format!("metrics-server isn't available: {}", e.message)
        }
    };
    let mut rows: Vec<(String, f64, f64)> = if nodes {
        kubyl_metrics_core::metrics_server::nodes(&client)
            .await
            .map_err(error)?
            .into_iter()
            .map(|(name, usage)| (name, usage.cpu, usage.memory))
            .collect()
    } else {
        kubyl_metrics_core::metrics_server::pods(&client, str_arg(args, "namespace"))
            .await
            .map_err(error)?
            .into_iter()
            .map(|(key, usage)| (key.to_string(), usage.cpu, usage.memory))
            .collect()
    };
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    let headers: Vec<String> = [if nodes { "NODE" } else { "POD" }, "CPU", "MEMORY"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let rows: Vec<Vec<String>> = rows
        .into_iter()
        .map(|(name, cpu, memory)| {
            vec![name, format::format_cpu(cpu), format::format_bytes(memory)]
        })
        .collect();
    Ok(table(&headers, &rows))
}

async fn query_prometheus(context: &ToolContext, args: &Value) -> Result<String, String> {
    let prometheus = context.prometheus.as_ref().ok_or(
        "Kubyl hasn't found a Prometheus for this cluster (open the cluster's Overview, or set metrics.prometheus in settings).",
    )?;
    let query = required(args, "query")?;
    let format_labels = |labels: &BTreeMap<String, String>| {
        let name = labels.get("__name__").cloned().unwrap_or_default();
        let rest: Vec<String> = labels
            .iter()
            .filter(|(k, _)| *k != "__name__")
            .map(|(k, v)| format!("{k}=\"{v}\""))
            .collect();
        format!("{name}{{{}}}", rest.join(", "))
    };
    match int_arg(args, "range_minutes") {
        Some(minutes) => {
            let minutes = minutes.clamp(1, 10080) as f64;
            let end = Timestamp::now().as_second() as f64;
            let start = end - minutes * 60.0;
            let step = int_arg(args, "step_seconds")
                .map(|s| s.max(1) as f64)
                .unwrap_or_else(|| (minutes * 60.0 / 60.0).max(15.0));
            let series = prometheus
                .query_range(query, start, end, step)
                .await
                .map_err(|e| e.to_string())?;
            if series.is_empty() {
                return Ok("No data.".into());
            }
            let mut out = String::new();
            for s in series.iter().take(50) {
                writeln!(out, "{}", format_labels(&s.labels)).ok();
                for (t, v) in &s.values {
                    let at = Timestamp::from_second(*t as i64)
                        .map(|t| t.to_string())
                        .unwrap_or_default();
                    writeln!(out, "  {at}  {v}").ok();
                }
            }
            if series.len() > 50 {
                writeln!(out, "[{} more series not shown]", series.len() - 50).ok();
            }
            Ok(out)
        }
        None => {
            let samples = prometheus.query(query).await.map_err(|e| e.to_string())?;
            if samples.is_empty() {
                return Ok("No data.".into());
            }
            let mut out = String::new();
            for sample in samples.iter().take(500) {
                writeln!(out, "{}  {}", format_labels(&sample.labels), sample.value).ok();
            }
            if samples.len() > 500 {
                writeln!(out, "[{} more samples not shown]", samples.len() - 500).ok();
            }
            Ok(out)
        }
    }
}

fn alerts(context: &ToolContext, args: &Value) -> Result<String, String> {
    let alerts = context
        .alerts
        .as_ref()
        .ok_or("Kubyl hasn't found an Alertmanager or Prometheus rules for this cluster.")?;
    let namespace = str_arg(args, "namespace");
    let now = Timestamp::now();
    let rows: Vec<Vec<String>> = alerts
        .iter()
        .filter(|a| {
            namespace.is_none_or(|ns| a.labels.get("namespace").map(String::as_str) == Some(ns))
        })
        .map(|a| {
            let labels: Vec<String> = a
                .labels
                .iter()
                .filter(|(k, _)| !matches!(k.as_str(), "alertname" | "severity"))
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            vec![
                a.name.clone(),
                a.severity.clone(),
                a.state.clone(),
                a.since
                    .map(|t| format::human_duration(format::seconds_since(t, now)))
                    .unwrap_or_else(|| "-".into()),
                a.summary.clone(),
                labels.join(" "),
            ]
        })
        .collect();
    if rows.is_empty() {
        return Ok("No active alerts.".into());
    }
    let headers: Vec<String> = ["ALERT", "SEVERITY", "STATE", "SINCE", "SUMMARY", "LABELS"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    Ok(table(&headers, &rows))
}

async fn can_i(context: &ToolContext, args: &Value) -> Result<String, String> {
    let verb = required(args, "verb")?;
    let info = resolve(context, required(args, "kind")?)?;
    let namespace = str_arg(args, "namespace").filter(|_| info.namespaced);
    let mut query = AccessQuery::new(verb, &info.gvr, namespace);
    if let Some(sub) = str_arg(args, "subresource") {
        query = query.subresource(sub);
    }
    if let Some(name) = str_arg(args, "name") {
        query = query.name(name);
    }
    let allowed = access::check(client(context)?, query)
        .await
        .map_err(kube_error)?;
    let scope = namespace.map(|ns| format!(" in {ns}")).unwrap_or_default();
    Ok(format!(
        "{}: {verb} {}{scope}",
        if allowed { "yes" } else { "no" },
        info.gvr
    ))
}

fn api_resources(context: &ToolContext, args: &Value) -> Result<String, String> {
    let discovery = context
        .discovery
        .as_ref()
        .ok_or_else(|| format!("{} isn't connected in Kubyl.", context.cluster_name))?;
    let filter = str_arg(args, "filter").map(str::to_lowercase);
    let mut rows: Vec<Vec<String>> = discovery
        .preferred()
        .filter(|r| {
            filter.as_ref().is_none_or(|f| {
                r.gvr.resource.contains(f.as_str())
                    || r.gvk.group.contains(f.as_str())
                    || r.gvk.kind.to_lowercase().contains(f.as_str())
            })
        })
        .map(|r| {
            vec![
                r.gvr.resource.clone(),
                r.short_names.join(","),
                r.gvk.api_version(),
                r.namespaced.to_string(),
                r.gvk.kind.clone(),
            ]
        })
        .collect();
    rows.sort();
    let headers: Vec<String> = ["NAME", "SHORTNAMES", "APIVERSION", "NAMESPACED", "KIND"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    Ok(table(&headers, &rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_schema_and_a_dispatch_arm() {
        let names: Vec<String> = definitions()
            .iter()
            .map(|d| d["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names.len(), 11);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let context = ToolContext {
            cluster_name: "kind-dev".into(),
            max_output: 4096,
            ..ToolContext::default()
        };
        for name in &names {
            let output = runtime.block_on(call(&context, name, &json!({})));
            assert!(
                !output.text.starts_with("Kubyl has no tool"),
                "{name}: {}",
                output.text
            );
        }
        let unknown = runtime.block_on(call(&context, "delete_everything", &json!({})));
        assert!(unknown.is_error);
    }

    #[test]
    fn output_is_capped_at_the_right_end() {
        let text = "a".repeat(100) + "END";
        let head = cap(&text, 50, false);
        assert!(head.starts_with("aaa") && head.contains("more bytes not shown"));
        let tail = cap(&text, 50, true);
        assert!(tail.ends_with("END") && tail.contains("earlier bytes not shown"));
        assert_eq!(cap("short", 50, false), "short");
    }

    #[test]
    fn tables_align_columns() {
        let out = table(
            &["NAME".into(), "STATUS".into()],
            &[
                vec!["web-0".into(), "Running".into()],
                vec!["db".into(), "CrashLoopBackOff".into()],
            ],
        );
        assert_eq!(
            out,
            "NAME    STATUS\nweb-0   Running\ndb      CrashLoopBackOff\n"
        );
    }

    #[test]
    fn disconnected_clusters_say_so() {
        let context = ToolContext {
            cluster_name: "prod-eu".into(),
            max_output: 4096,
            ..ToolContext::default()
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let info = runtime.block_on(call(&context, "cluster_info", &json!({})));
        assert!(info.text.contains("not connected"));
        let list = runtime.block_on(call(&context, "list_resources", &json!({"kind": "pods"})));
        assert!(list.is_error && list.text.contains("isn't connected"));
    }

    #[test]
    fn alerts_filter_by_namespace() {
        let context = ToolContext {
            alerts: Some(Arc::new(vec![
                AlertSummary {
                    name: "KubePodCrashLooping".into(),
                    severity: "warning".into(),
                    state: "firing".into(),
                    since: None,
                    summary: "Pod is crash looping.".into(),
                    labels: [("namespace".to_string(), "shop".to_string())].into(),
                },
                AlertSummary {
                    name: "Watchdog".into(),
                    severity: "none".into(),
                    state: "firing".into(),
                    since: None,
                    summary: String::new(),
                    labels: BTreeMap::new(),
                },
            ])),
            ..ToolContext::default()
        };
        let all = alerts(&context, &json!({})).unwrap();
        assert!(all.contains("Watchdog") && all.contains("KubePodCrashLooping"));
        let shop = alerts(&context, &json!({"namespace": "shop"})).unwrap();
        assert!(!shop.contains("Watchdog"));
    }
}
