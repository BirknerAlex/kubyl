//! [`ToolsHandler`]: Kubyl's tools behind an MCP server, for one cluster or for all of them.

use std::fmt::Write as _;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::approval::Approver;
use crate::changes;
use crate::server::Handler;
use crate::tools::{self, ToolContext, ToolOutput, str_arg};

/// Which clusters a server's tools reach.
pub enum Scope {
    /// One cluster, the thread's. Tools take no `cluster` argument.
    Cluster(watch::Receiver<ToolContext>),
    /// Every cluster Kubyl knows. Tools take `cluster`; `list_clusters` lists them.
    Clusters(watch::Receiver<Arc<Vec<ToolContext>>>),
}

/// Whether and how a server may change clusters.
#[derive(Clone)]
pub struct Writes {
    /// Switched on by the user (per thread, or by the `mcp.writes` setting).
    pub enabled: watch::Receiver<bool>,
    /// Asks the user about every change.
    pub approver: Arc<dyn Approver>,
    /// What a tool call says while changes are off.
    pub off_message: String,
}

/// Starts a read-only server for one cluster on a free port (tests and simple hosts).
pub async fn serve_cluster(
    context: watch::Receiver<ToolContext>,
) -> std::io::Result<crate::server::McpServer> {
    crate::server::serve(
        Arc::new(ToolsHandler::new(Scope::Cluster(context), None)),
        crate::server::ServeOptions::default(),
    )
    .await
}

pub struct ToolsHandler {
    scope: Scope,
    writes: Option<Writes>,
}

impl ToolsHandler {
    pub fn new(scope: Scope, writes: Option<Writes>) -> Self {
        Self { scope, writes }
    }

    fn all(&self) -> bool {
        matches!(self.scope, Scope::Clusters(_))
    }

    async fn change(&self, context: &ToolContext, name: &str, args: &Value) -> ToolOutput {
        let Some(writes) = &self.writes else {
            return ToolOutput::error("Changes through Kubyl aren't available here.");
        };
        if !*writes.enabled.borrow() {
            return ToolOutput::error(writes.off_message.clone());
        }
        if context.caps.read_only {
            return ToolOutput::error(format!(
                "{} is read-only in Kubyl, so no change can be made to it from here.",
                context.cluster_name
            ));
        }
        let cluster = context.cluster.clone();
        let name_of = context.cluster_name.clone();
        let recheck = move || self.still_allowed(&cluster, &name_of, writes);
        changes::run_with(context, &writes.approver, name, args, &recheck).await
    }

    /// Right before an approved change is made: changes are still on and the cluster still
    /// isn't read-only (either may have changed while the user decided).
    fn still_allowed(
        &self,
        cluster: &Option<kubyl_base::ClusterId>,
        name: &str,
        writes: &Writes,
    ) -> Result<(), String> {
        if !*writes.enabled.borrow() {
            return Err(writes.off_message.clone());
        }
        let read_only = match &self.scope {
            Scope::Cluster(context) => context.borrow().caps.read_only,
            Scope::Clusters(clusters) => {
                match clusters.borrow().iter().find(|c| &c.cluster == cluster) {
                    Some(found) => found.caps.read_only,
                    // Gone from the list (removed or hidden meanwhile): don't guess.
                    None => {
                        return Err(format!(
                            "{name} is no longer in Kubyl's list of clusters, so nothing was changed."
                        ));
                    }
                }
            }
        };
        if read_only {
            return Err(format!(
                "{name} became read-only in Kubyl while the change waited, so nothing was changed."
            ));
        }
        Ok(())
    }
}

impl Handler for ToolsHandler {
    fn instructions(&self) -> String {
        let mut text = if self.all() {
            "Kubyl's view of every Kubernetes cluster the user is connected to, through their own accounts. \
Call list_clusters first, then pass `cluster` to the other tools (it can be left out when only one cluster is connected). "
        } else {
            "Kubyl's view of one Kubernetes cluster, through the user's own account. Start with cluster_info. "
        }
        .to_string();
        text.push_str(
            "Secret values, private keys and token-like strings are masked; Kubyl never reveals them, \
so don't try to work around the masking. ",
        );
        if self.writes.is_some() {
            text.push_str(
                "The read tools can't change anything. The change tools (apply_manifest, patch_resource, delete_resource, \
scale_resource, restart_resource) work only while the user allows changes, and each change is shown to them \
for approval in Kubyl before it happens; if they reject it, don't retry it unchanged.",
            );
        } else {
            text.push_str(
                "These tools can't change the cluster; when a change would help, show the user the manifest or command.",
            );
        }
        text
    }

    fn tools(&self) -> Vec<Value> {
        let mut defs = tools::definitions();
        if self.writes.is_some() {
            defs.extend(changes::definitions());
        }
        if self.all() {
            for def in &mut defs {
                if let Some(properties) = def.pointer_mut("/inputSchema/properties")
                    && let Some(map) = properties.as_object_mut()
                {
                    map.insert("cluster".into(), cluster_property());
                }
            }
            defs.insert(
                0,
                json!({
                    "name": "list_clusters",
                    "description": "The clusters Kubyl knows: name, connection state, flags (production, read-only) and which one the user has open. Only connected clusters can be queried.",
                    "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
                    "annotations": { "readOnlyHint": true, "openWorldHint": false },
                }),
            );
        }
        defs
    }

    fn call<'a>(&'a self, name: &'a str, arguments: Value) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let context = match &self.scope {
                Scope::Cluster(context) => context.borrow().clone(),
                Scope::Clusters(clusters) => {
                    let clusters = clusters.borrow().clone();
                    if name == "list_clusters" {
                        return ToolOutput::ok(list_clusters(&clusters));
                    }
                    match pick_cluster(&clusters, str_arg(&arguments, "cluster")) {
                        Ok(context) => context,
                        Err(message) => return ToolOutput::error(message),
                    }
                }
            };
            if changes::is_change_tool(name) {
                self.change(&context, name, &arguments).await
            } else {
                tools::call(&context, name, &arguments).await
            }
        })
    }
}

fn cluster_property() -> Value {
    json!({
        "type": "string",
        "description": "The cluster, by the name list_clusters shows. Optional when only one cluster is connected.",
    })
}

/// The text of `list_clusters`.
fn list_clusters(clusters: &[ToolContext]) -> String {
    if clusters.is_empty() {
        return "Kubyl knows no clusters. The user adds kubeconfigs in Kubyl.".into();
    }
    let mut out = String::from("Clusters in Kubyl (pass the name as `cluster`):\n");
    for context in clusters {
        let mut flags = Vec::new();
        if context.caps.production {
            flags.push("production");
        }
        if context.caps.read_only {
            flags.push("read-only");
        }
        if context.caps.openshift {
            flags.push("OpenShift");
        }
        if context.active {
            flags.push("open in Kubyl");
        }
        let status = if context.status.is_empty() {
            if context.client.is_some() {
                "connected"
            } else {
                "not connected"
            }
        } else {
            context.status.as_str()
        };
        write!(out, "- {}: {status}", context.cluster_name).ok();
        if !flags.is_empty() {
            write!(out, " ({})", flags.join(", ")).ok();
        }
        out.push('\n');
    }
    out
}

/// The cluster a call is about: the one named, else the only connected one, else the one open in
/// Kubyl.
pub fn pick_cluster(clusters: &[ToolContext], wanted: Option<&str>) -> Result<ToolContext, String> {
    let names = |list: &[&ToolContext]| {
        list.iter()
            .map(|c| c.cluster_name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let connected: Vec<&ToolContext> = clusters.iter().filter(|c| c.client.is_some()).collect();
    match wanted {
        Some(wanted) => {
            let lower = wanted.to_lowercase();
            let exact: Vec<&ToolContext> = clusters
                .iter()
                .filter(|c| {
                    c.cluster_name.eq_ignore_ascii_case(wanted)
                        || c.cluster
                            .as_ref()
                            .is_some_and(|id| id.to_string() == wanted)
                })
                .collect();
            let found = if exact.is_empty() {
                clusters
                    .iter()
                    .filter(|c| c.cluster_name.to_lowercase().contains(&lower))
                    .collect()
            } else {
                exact
            };
            match found.as_slice() {
                [one] => Ok((*one).clone()),
                [] => Err(format!(
                    "No cluster named `{wanted}`. Known: {}.",
                    names(&clusters.iter().collect::<Vec<_>>())
                )),
                many => Err(format!(
                    "`{wanted}` matches several clusters ({}); use the full name.",
                    names(many)
                )),
            }
        }
        None => match connected.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err("No cluster is connected in Kubyl. Ask the user to connect one.".into()),
            many => match many.iter().find(|c| c.active) {
                Some(active) => Ok((*active).clone()),
                None => Err(format!(
                    "Several clusters are connected ({}); pass `cluster`.",
                    names(many)
                )),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::{Decision, Fixed};
    use kubyl_base::ClusterId;

    fn cluster(name: &str, connected: bool, active: bool) -> ToolContext {
        ToolContext {
            cluster: Some(ClusterId::new(name)),
            cluster_name: name.into(),
            client: None,
            active,
            status: if connected {
                "connected"
            } else {
                "not connected"
            }
            .into(),
            max_output: 4096,
            ..ToolContext::default()
        }
    }

    /// `pick_cluster` only looks at `client.is_some()`; a context that claims to be connected
    /// without a client is made by the tests through `status` and this helper.
    fn pick(list: &[ToolContext], wanted: Option<&str>) -> Result<String, String> {
        pick_cluster(list, wanted).map(|c| c.cluster_name)
    }

    #[test]
    fn clusters_are_found_by_name_prefix_and_id() {
        let list = vec![
            cluster("prod-eu", false, false),
            cluster("prod-us", false, false),
            cluster("kind-dev", false, false),
        ];
        assert_eq!(pick(&list, Some("kind-dev")).unwrap(), "kind-dev");
        assert_eq!(pick(&list, Some("KIND")).unwrap(), "kind-dev");
        assert!(pick(&list, Some("prod")).unwrap_err().contains("several"));
        assert!(pick(&list, Some("nope")).unwrap_err().contains("Known:"));
        // Nothing is connected (no client), so a call without a name has nothing to pick.
        assert!(pick(&list, None).unwrap_err().contains("No cluster"));
    }

    #[test]
    fn the_clusters_list_names_flags_and_state() {
        let mut prod = cluster("prod-eu", true, true);
        prod.caps.production = true;
        let text = list_clusters(&[prod, cluster("kind-dev", false, false)]);
        assert!(
            text.contains("prod-eu: connected (production, open in Kubyl)"),
            "{text}"
        );
        assert!(text.contains("kind-dev: not connected"));
        assert!(list_clusters(&[]).contains("no clusters"));
    }

    #[tokio::test]
    async fn a_thread_server_offers_changes_only_with_a_gate_and_refuses_them_while_off() {
        let (_tx, context) = watch::channel(cluster("kind-dev", true, true));
        let read_only = ToolsHandler::new(Scope::Cluster(context.clone()), None);
        assert_eq!(read_only.tools().len(), 11);
        assert!(read_only.instructions().contains("can't change"));

        let (enabled_tx, enabled) = watch::channel(false);
        let handler = ToolsHandler::new(
            Scope::Cluster(context),
            Some(Writes {
                enabled,
                approver: Arc::new(Fixed(Decision::Approved)),
                off_message: "Changes are off for this conversation.".into(),
            }),
        );
        let names: Vec<String> = handler
            .tools()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names.len(), 16);
        assert!(names.contains(&"scale_resource".to_string()));
        assert!(handler.instructions().contains("approval"));

        let off = handler.call("scale_resource", json!({})).await;
        assert!(off.is_error);
        assert_eq!(off.text, "Changes are off for this conversation.");

        // On, but this cluster isn't connected: the tool fails before anyone is asked.
        enabled_tx.send(true).unwrap();
        let on = handler
            .call(
                "scale_resource",
                json!({"kind": "deploy", "name": "web", "replicas": 2}),
            )
            .await;
        assert!(on.is_error);
        assert!(!on.text.contains("Changes are off"));
    }

    #[tokio::test]
    async fn a_read_only_cluster_refuses_changes_even_when_they_are_on() {
        let mut context = cluster("prod", true, true);
        context.caps.read_only = true;
        let (_tx, rx) = watch::channel(Arc::new(vec![context]));
        let (_enabled_tx, enabled) = watch::channel(true);
        let handler = ToolsHandler::new(
            Scope::Clusters(rx),
            Some(Writes {
                enabled,
                approver: Arc::new(Fixed(Decision::Approved)),
                off_message: String::new(),
            }),
        );
        // `prod` has no client in this test, so a call without a name finds no connected cluster;
        // naming it reaches the read-only check.
        let output = handler
            .call(
                "delete_resource",
                json!({"cluster": "prod", "kind": "pod", "name": "x"}),
            )
            .await;
        assert!(output.is_error);
        assert!(output.text.contains("read-only"), "{}", output.text);
        let names: Vec<String> = handler
            .tools()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names[0], "list_clusters");
        assert!(
            handler
                .tools()
                .iter()
                .skip(1)
                .all(|t| t["inputSchema"]["properties"]["cluster"].is_object())
        );
    }
}
