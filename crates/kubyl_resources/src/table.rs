//! Server-side printing: `Accept: application/json;as=Table;v=v1;g=meta.k8s.io`.
//!
//! Kinds without a hand-written [`kubyl_core::ColumnProvider`] (CRDs, aggregated APIs, rare
//! built-ins) get their columns from the API server. For CRDs these are the
//! `additionalPrinterColumns`, so a newly installed CRD lists nicely without code changes.
//! List views watch the objects' metadata for liveness and refetch the table when it changes.

use std::collections::HashMap;
use std::sync::Arc;

use kube::Client;
use kubyl_core::{Align, CellValue, ColumnDef, ColumnWidth, Gvr};
use serde_json::Value;

use crate::store::{ObjectKey, object_key};

const TABLE_ACCEPT: &str = "application/json;as=Table;v=v1;g=meta.k8s.io,application/json";

/// A column of a server-side table.
#[derive(Clone, Debug, PartialEq)]
pub struct TableColumn {
    pub name: String,
    /// `string`, `integer`, `number`, `boolean` or `date`.
    pub kind: String,
    pub format: String,
    pub description: String,
    /// `0`: default view. Higher: only in the wide view.
    pub priority: i64,
}

/// A fetched table: columns and the cells of each object.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerTable {
    pub columns: Vec<TableColumn>,
    pub rows: HashMap<ObjectKey, Vec<Value>>,
}

impl ServerTable {
    /// Column definitions for the list view. Column ids are `t:<index>`; the `Name` column is
    /// `name` and a `date` column called `Age` is `age`, so the view renders both itself (ages
    /// tick live).
    pub fn column_defs(&self) -> Vec<ColumnDef> {
        self.columns
            .iter()
            .enumerate()
            .map(|(ix, column)| {
                let id = self.column_id(ix);
                let title = column.name.clone();
                let mut def = match id.as_str() {
                    "name" => ColumnDef::new(
                        id,
                        title,
                        ColumnWidth::Flex {
                            weight: 1.0,
                            min: 180.0,
                        },
                    )
                    .mono(),
                    "age" => ColumnDef::new(id, title, ColumnWidth::Fixed(60.0)).mono(),
                    _ => {
                        let width = (column.name.len() as f32 * 9.0 + 40.0).clamp(80.0, 220.0);
                        let mut def = ColumnDef::new(id, title, ColumnWidth::Fixed(width));
                        if matches!(column.kind.as_str(), "integer" | "number") {
                            def = def.mono().align_end();
                        }
                        def
                    }
                };
                if column.priority > 0 {
                    def = def.wide();
                }
                def
            })
            .collect()
    }

    fn column_id(&self, ix: usize) -> String {
        let column = &self.columns[ix];
        if column.name.eq_ignore_ascii_case("name") && column.format == "name" || ix == 0 {
            "name".into()
        } else if column.name.eq_ignore_ascii_case("age") && column.kind == "date" {
            "age".into()
        } else {
            format!("t:{ix}")
        }
    }

    /// The cell of `column` (a `t:<index>` id) for an object.
    pub fn cell(&self, key: &str, column: &str) -> CellValue {
        let Some(ix) = column
            .strip_prefix("t:")
            .and_then(|i| i.parse::<usize>().ok())
        else {
            return CellValue::Empty;
        };
        let Some(value) = self.rows.get(key).and_then(|cells| cells.get(ix)) else {
            return CellValue::Empty;
        };
        match value {
            Value::Null => CellValue::Empty,
            Value::String(s) if s.is_empty() || s == "<none>" => CellValue::Empty,
            Value::String(s) => CellValue::Text(s.clone().into()),
            Value::Array(items) => CellValue::Text(
                items
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(String::from)
                            .unwrap_or_else(|| v.to_string())
                    })
                    .collect::<Vec<_>>()
                    .join(",")
                    .into(),
            ),
            other => CellValue::Text(other.to_string().into()),
        }
    }

    /// Alignment hint for numeric columns.
    pub fn align(&self, ix: usize) -> Align {
        match self.columns.get(ix).map(|c| c.kind.as_str()) {
            Some("integer" | "number") => Align::End,
            _ => Align::Start,
        }
    }
}

/// Parses a `meta.k8s.io/v1` `Table`.
pub fn parse_table(table: &Value) -> ServerTable {
    let columns = table["columnDefinitions"]
        .as_array()
        .map(|defs| {
            defs.iter()
                .map(|d| TableColumn {
                    name: d["name"].as_str().unwrap_or_default().to_string(),
                    kind: d["type"].as_str().unwrap_or_default().to_string(),
                    format: d["format"].as_str().unwrap_or_default().to_string(),
                    description: d["description"].as_str().unwrap_or_default().to_string(),
                    priority: d["priority"].as_i64().unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    let rows = table["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let meta = &row["object"]["metadata"];
                    let name = meta["name"].as_str()?;
                    let key = object_key(meta["namespace"].as_str(), name);
                    let cells = row["cells"].as_array().cloned().unwrap_or_default();
                    Some((key, cells))
                })
                .collect()
        })
        .unwrap_or_default();
    ServerTable { columns, rows }
}

/// Percent-encodes a query value.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The collection path of a resource, e.g. `/apis/apps/v1/namespaces/web/deployments`.
pub fn collection_path(gvr: &Gvr, namespace: Option<&str>) -> String {
    let base = if gvr.group.is_empty() {
        format!("/api/{}", gvr.version)
    } else {
        format!("/apis/{}/{}", gvr.group, gvr.version)
    };
    match namespace {
        Some(ns) => format!("{base}/namespaces/{ns}/{}", gvr.resource),
        None => format!("{base}/{}", gvr.resource),
    }
}

/// Lists `gvr` as a server-side table (objects as metadata only).
pub async fn fetch_table(
    client: Client,
    gvr: Gvr,
    namespace: Option<String>,
    label_selector: Option<String>,
    field_selector: Option<String>,
) -> Result<Arc<ServerTable>, kube::Error> {
    let mut url = format!(
        "{}?includeObject=Metadata",
        collection_path(&gvr, namespace.as_deref())
    );
    if let Some(selector) = label_selector {
        url.push_str(&format!("&labelSelector={}", encode(&selector)));
    }
    if let Some(selector) = field_selector {
        url.push_str(&format!("&fieldSelector={}", encode(&selector)));
    }
    let request = http::Request::get(url)
        .header(http::header::ACCEPT, TABLE_ACCEPT)
        .body(Vec::new())
        .map_err(kube::Error::HttpError)?;
    let table: Value = client.request(request).await?;
    Ok(Arc::new(parse_table(&table)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_crd_printer_columns() {
        let table = json!({
            "kind": "Table",
            "columnDefinitions": [
                {"name": "Name", "type": "string", "format": "name", "priority": 0},
                {"name": "Ready", "type": "string", "format": "", "priority": 0},
                {"name": "Secret", "type": "string", "format": "", "priority": 1},
                {"name": "Age", "type": "date", "format": "", "priority": 0}
            ],
            "rows": [
                {"cells": ["api-tls", "True", "api-tls", "5m"],
                 "object": {"kind": "PartialObjectMetadata", "metadata": {"name": "api-tls", "namespace": "payments"}}}
            ]
        });
        let table = parse_table(&table);
        let defs = table.column_defs();
        let ids: Vec<_> = defs.iter().map(|d| d.id.to_string()).collect();
        assert_eq!(ids, ["name", "t:1", "t:2", "age"]);
        assert!(defs[2].wide);
        assert_eq!(
            table.cell("payments/api-tls", "t:1"),
            CellValue::Text("True".into())
        );
        assert_eq!(table.cell("payments/other", "t:1"), CellValue::Empty);
    }

    #[test]
    fn builds_paths_and_encodes_selectors() {
        assert_eq!(
            collection_path(&Gvr::new("", "v1", "pods"), Some("web")),
            "/api/v1/namespaces/web/pods"
        );
        assert_eq!(
            collection_path(&Gvr::new("cert-manager.io", "v1", "certificates"), None),
            "/apis/cert-manager.io/v1/certificates"
        );
        assert_eq!(encode("app in (a,b)"), "app%20in%20%28a%2Cb%29");
    }
}
