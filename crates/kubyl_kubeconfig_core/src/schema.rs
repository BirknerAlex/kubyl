//! The kubeconfig schema for the YAML tab: an OpenAPI v3 document like the ones the API server
//! serves, so phase 04's validation, hover docs and completion work on kubeconfigs
//! (`apiVersion: v1`, `kind: Config`). Written from client-go's `clientcmd/api/v1` types.

use std::sync::{Arc, OnceLock};

use kubyl_core::Gvk;
use serde_json::{Value, json};

/// The group/version/kind of a kubeconfig document.
pub fn gvk() -> Gvk {
    Gvk::new("", "v1", "Config")
}

/// A label for hover popups.
pub const SOURCE: &str = "kubeconfig (client-go clientcmd/api/v1)";

fn string(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn boolean(description: &str) -> Value {
    json!({"type": "boolean", "description": description})
}

fn named(name_desc: &str, key: &str, reference: &str, body_desc: &str) -> Value {
    json!({
        "type": "object",
        "required": ["name", key],
        "properties": {
            "name": string(name_desc),
            key: {"allOf": [{"$ref": format!("#/components/schemas/{reference}")}], "description": body_desc},
        },
    })
}

fn extensions() -> Value {
    json!({
        "type": "array",
        "description": "Additional information for extenders; kept as they are.",
        "items": {"$ref": "#/components/schemas/NamedExtension"},
    })
}

/// The OpenAPI document (built once).
pub fn document() -> Arc<Value> {
    static DOC: OnceLock<Arc<Value>> = OnceLock::new();
    DOC.get_or_init(|| Arc::new(build())).clone()
}

fn build() -> Value {
    json!({
        "openapi": "3.0.0",
        "components": {"schemas": {
            "Config": {
                "type": "object",
                "description": "A kubeconfig: how to reach clusters (clusters), who you are (users) and which pairs to use (contexts).",
                "x-kubernetes-group-version-kind": [{"group": "", "version": "v1", "kind": "Config"}],
                "properties": {
                    "apiVersion": string("Always v1."),
                    "kind": string("Always Config."),
                    "preferences": {"type": "object", "description": "Legacy kubectl preferences.", "x-kubernetes-preserve-unknown-fields": true},
                    "clusters": {"type": "array", "description": "Named clusters: API server address and how to verify it.", "items": {"$ref": "#/components/schemas/NamedCluster"}},
                    "users": {"type": "array", "description": "Named users: credentials for API servers.", "items": {"$ref": "#/components/schemas/NamedAuthInfo"}},
                    "contexts": {"type": "array", "description": "Named contexts: a cluster, a user and a default namespace.", "items": {"$ref": "#/components/schemas/NamedContext"}},
                    "current-context": string("The context kubectl uses by default."),
                    "extensions": extensions(),
                },
            },
            "NamedCluster": named("The cluster's name, referenced by contexts.", "cluster", "Cluster", "How to reach and verify the API server."),
            "Cluster": {
                "type": "object",
                "required": ["server"],
                "properties": {
                    "server": string("The API server's address (https://host:port)."),
                    "tls-server-name": string("The name the server's certificate is checked against, when it differs from the host of `server`."),
                    "insecure-skip-tls-verify": boolean("Don't verify the server's certificate. Insecure: anyone on the network path can read your credentials."),
                    "certificate-authority": string("Path to a PEM file with the CA that signed the server's certificate."),
                    "certificate-authority-data": string("The CA certificate (PEM, base64-encoded). Overrides certificate-authority."),
                    "proxy-url": string("An http, https or socks5 proxy for requests to this cluster."),
                    "disable-compression": boolean("Don't request compressed responses."),
                    "extensions": extensions(),
                },
            },
            "NamedContext": named("The context's name.", "context", "Context", "A cluster, a user and a namespace."),
            "Context": {
                "type": "object",
                "required": ["cluster"],
                "properties": {
                    "cluster": string("The name of a cluster in this file."),
                    "user": string("The name of a user in this file."),
                    "namespace": string("The default namespace."),
                    "extensions": extensions(),
                },
            },
            "NamedAuthInfo": named("The user's name, referenced by contexts.", "user", "AuthInfo", "The user's credentials."),
            "AuthInfo": {
                "type": "object",
                "properties": {
                    "client-certificate": string("Path to a PEM client certificate."),
                    "client-certificate-data": string("A PEM client certificate, base64-encoded."),
                    "client-key": string("Path to the PEM key of the client certificate."),
                    "client-key-data": string("The PEM key of the client certificate, base64-encoded. Secret."),
                    "token": string("A bearer token. Secret."),
                    "tokenFile": string("Path to a file with a bearer token (read on every use)."),
                    "as": string("A user to impersonate."),
                    "as-uid": string("A UID to impersonate."),
                    "as-groups": {"type": "array", "description": "Groups to impersonate.", "items": {"type": "string"}},
                    "as-user-extra": {"type": "object", "description": "Extra fields to impersonate.", "additionalProperties": {"type": "array", "items": {"type": "string"}}},
                    "username": string("Basic auth user name (removed in Kubernetes 1.19)."),
                    "password": string("Basic auth password. Secret."),
                    "auth-provider": {"$ref": "#/components/schemas/AuthProviderConfig"},
                    "exec": {"$ref": "#/components/schemas/ExecConfig"},
                    "extensions": extensions(),
                },
            },
            "AuthProviderConfig": {
                "type": "object",
                "required": ["name"],
                "description": "A legacy auth provider (oidc; gcp and azure were removed from kubectl 1.26).",
                "properties": {
                    "name": string("oidc, gcp or azure."),
                    "config": {"type": "object", "description": "Provider settings (idp-issuer-url, client-id, client-secret, extra-scopes, id-token, refresh-token…).", "additionalProperties": {"type": "string"}},
                },
            },
            "ExecConfig": {
                "type": "object",
                "required": ["command", "apiVersion"],
                "description": "A credential plugin kubectl (and Kubyl) runs to get a token or client certificate.",
                "properties": {
                    "command": string("The program to run (looked up in PATH)."),
                    "args": {"type": "array", "description": "Arguments.", "items": {"type": "string"}},
                    "env": {"type": "array", "description": "Extra environment variables.", "items": {"$ref": "#/components/schemas/ExecEnvVar"}},
                    "apiVersion": {"type": "string", "description": "The ExecCredential API version the plugin speaks.", "enum": ["client.authentication.k8s.io/v1", "client.authentication.k8s.io/v1beta1", "client.authentication.k8s.io/v1alpha1"]},
                    "installHint": string("Shown when the program isn't installed."),
                    "provideClusterInfo": boolean("Pass the cluster's details to the plugin (KUBERNETES_EXEC_INFO)."),
                    "interactiveMode": {"type": "string", "description": "Whether the plugin may prompt: Never, IfAvailable or Always.", "enum": ["Never", "IfAvailable", "Always"]},
                },
            },
            "ExecEnvVar": {
                "type": "object",
                "required": ["name", "value"],
                "properties": {"name": string("Variable name."), "value": string("Value.")},
            },
            "NamedExtension": {
                "type": "object",
                "required": ["name", "extension"],
                "properties": {
                    "name": string("The extension's name."),
                    "extension": {"type": "object", "description": "Anything.", "x-kubernetes-preserve-unknown-fields": true},
                },
            },
        }},
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_yaml::schema::Schema;
    use kubyl_yaml::{parse, validate};

    #[test]
    fn validates_kubeconfigs() {
        let doc = document();
        let schema = Schema::for_gvk(&doc, &gvk()).unwrap();
        let text = "apiVersion: v1\nkind: Config\nclusters:\n- name: a\n  cluster:\n    server: https://x\n    insecure-skip-tls-verify: maybe\n    servr: typo\nusers:\n- name: u\n  user:\n    exec:\n      command: aws\n      apiVersion: client.authentication.k8s.io/v1\n      interactiveMode: Sometimes\n";
        let parsed = parse::parse(text);
        let problems = validate::validate(parsed.roots().next().unwrap(), &schema);
        let messages: Vec<&str> = problems.iter().map(|p| p.message.as_str()).collect();
        assert!(messages.iter().any(|m| m.contains("servr")), "{messages:?}");
        assert!(
            messages.iter().any(|m| m.contains("Sometimes")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("boolean")),
            "{messages:?}"
        );
        let lookup = |g: &Gvk| (g == &gvk()).then(|| (document(), SOURCE.to_string()));
        let offset = text.find("servr").unwrap() - 8;
        let hover = kubyl_yaml::intel::hover(text, offset, &lookup);
        assert!(hover.is_some());
    }
}
