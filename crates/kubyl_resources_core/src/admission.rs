//! Admission control: ValidatingAdmissionPolicies, MutatingAdmissionPolicies, their bindings
//! and the webhook configurations, plus label selectors.
//!
//! The policies are read the same in `admissionregistration.k8s.io/v1`, `v1beta1` and
//! `v1alpha1` (MutatingAdmissionPolicy is beta before Kubernetes 1.36). CEL expressions are kept
//! as written.

use serde_json::Value;

use crate::format::{array_at, int_at, str_at};

/// The API group.
pub const GROUP: &str = "admissionregistration.k8s.io";

/// Whether `kind` is a policy or a policy binding.
pub fn is_policy(kind: &str) -> bool {
    matches!(
        kind,
        "ValidatingAdmissionPolicy" | "MutatingAdmissionPolicy"
    )
}

pub fn is_binding(kind: &str) -> bool {
    matches!(
        kind,
        "ValidatingAdmissionPolicyBinding" | "MutatingAdmissionPolicyBinding"
    )
}

/// The binding kind's plural for a policy kind and the other way round.
pub fn bindings_resource(policy_kind: &str) -> &'static str {
    match policy_kind {
        "MutatingAdmissionPolicy" => "mutatingadmissionpolicybindings",
        _ => "validatingadmissionpolicybindings",
    }
}

pub fn policy_resource(binding_kind: &str) -> &'static str {
    match binding_kind {
        "MutatingAdmissionPolicyBinding" => "mutatingadmissionpolicies",
        _ => "validatingadmissionpolicies",
    }
}

/// A named CEL expression (match condition, variable).
#[derive(Clone, Debug, PartialEq)]
pub struct Named {
    pub name: String,
    pub expression: String,
}

fn named(items: &[Value]) -> Vec<Named> {
    items
        .iter()
        .map(|v| Named {
            name: str_at(v, "/name").to_string(),
            expression: str_at(v, "/expression").trim().to_string(),
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Validation {
    pub expression: String,
    pub message: String,
    pub message_expression: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mutation {
    /// `ApplyConfiguration` or `JSONPatch`.
    pub patch_type: String,
    pub expression: String,
}

/// What a policy or binding matches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Match {
    /// `apps/v1 deployments, statefulsets · CREATE, UPDATE`.
    pub rules: Vec<String>,
    pub excluded: Vec<String>,
    pub namespace_selector: Option<Value>,
    pub object_selector: Option<Value>,
    pub match_policy: String,
}

impl Match {
    pub fn parse(value: Option<&Value>) -> Option<Self> {
        let value = value?;
        let selector = |key: &str| {
            value
                .get(key)
                .filter(|s| s.as_object().is_some_and(|m| !m.is_empty()))
                .cloned()
        };
        Some(Self {
            rules: array_at(value, "/resourceRules").iter().map(rule).collect(),
            excluded: array_at(value, "/excludeResourceRules")
                .iter()
                .map(rule)
                .collect(),
            namespace_selector: selector("namespaceSelector"),
            object_selector: selector("objectSelector"),
            match_policy: str_at(value, "/matchPolicy").to_string(),
        })
    }
}

fn strings(value: &Value, pointer: &str) -> Vec<String> {
    array_at(value, pointer)
        .iter()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect()
}

/// A rule as text: `apps/v1 deployments, statefulsets · CREATE, UPDATE` (`core` for the core
/// group, `*` stays `*`).
pub fn rule(rule: &Value) -> String {
    let groups: Vec<String> = strings(rule, "/apiGroups")
        .into_iter()
        .map(|g| if g.is_empty() { "core".into() } else { g })
        .collect();
    let versions = strings(rule, "/apiVersions");
    let mut resources = strings(rule, "/resources");
    resources.extend(
        strings(rule, "/resourceNames")
            .into_iter()
            .map(|n| format!("named {n}")),
    );
    let operations = strings(rule, "/operations");
    let mut out = format!(
        "{}/{} {}",
        or_star(&groups),
        or_star(&versions),
        or_star(&resources)
    );
    if !operations.is_empty() {
        out.push_str(" · ");
        out.push_str(&operations.join(", "));
    }
    match str_at(rule, "/scope") {
        "" | "*" => {}
        scope => out.push_str(&format!(" · {scope}")),
    }
    out
}

fn or_star(items: &[String]) -> String {
    if items.is_empty() {
        "*".into()
    } else {
        items.join(", ")
    }
}

/// A ValidatingAdmissionPolicy or MutatingAdmissionPolicy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Policy {
    pub failure_policy: String,
    /// `(apiVersion, kind)` of the param objects.
    pub param_kind: Option<(String, String)>,
    pub constraints: Option<Match>,
    pub match_conditions: Vec<Named>,
    pub variables: Vec<Named>,
    pub validations: Vec<Validation>,
    /// `(key, valueExpression)`.
    pub audit_annotations: Vec<(String, String)>,
    pub mutations: Vec<Mutation>,
    pub reinvocation_policy: String,
    /// Type-checking warnings: `(field, warning)`.
    pub warnings: Vec<(String, String)>,
}

impl Policy {
    pub fn parse(policy: &Value) -> Self {
        let spec = policy.get("spec").unwrap_or(&Value::Null);
        let param_kind = spec.get("paramKind").map(|p| {
            (
                str_at(p, "/apiVersion").to_string(),
                str_at(p, "/kind").to_string(),
            )
        });
        Self {
            failure_policy: match str_at(spec, "/failurePolicy") {
                "" => "Fail".into(),
                p => p.to_string(),
            },
            param_kind,
            constraints: Match::parse(spec.get("matchConstraints")),
            match_conditions: named(array_at(spec, "/matchConditions")),
            variables: named(array_at(spec, "/variables")),
            validations: array_at(spec, "/validations")
                .iter()
                .map(|v| Validation {
                    expression: str_at(v, "/expression").trim().to_string(),
                    message: str_at(v, "/message").to_string(),
                    message_expression: str_at(v, "/messageExpression").trim().to_string(),
                    reason: str_at(v, "/reason").to_string(),
                })
                .collect(),
            audit_annotations: array_at(spec, "/auditAnnotations")
                .iter()
                .map(|a| {
                    (
                        str_at(a, "/key").to_string(),
                        str_at(a, "/valueExpression").trim().to_string(),
                    )
                })
                .collect(),
            mutations: array_at(spec, "/mutations")
                .iter()
                .map(|m| {
                    let patch_type = str_at(m, "/patchType");
                    let expression = match patch_type {
                        "JSONPatch" => str_at(m, "/jsonPatch/expression"),
                        _ => str_at(m, "/applyConfiguration/expression"),
                    };
                    Mutation {
                        patch_type: patch_type.to_string(),
                        expression: expression.trim().to_string(),
                    }
                })
                .collect(),
            reinvocation_policy: match (str_at(spec, "/reinvocationPolicy"), spec.get("mutations"))
            {
                ("", Some(_)) => "Never".into(),
                (p, _) => p.to_string(),
            },
            warnings: array_at(policy, "/status/typeChecking/expressionWarnings")
                .iter()
                .map(|w| {
                    (
                        str_at(w, "/fieldRef").to_string(),
                        str_at(w, "/warning").to_string(),
                    )
                })
                .collect(),
        }
    }

    /// `ConfigMap` (v1), `Widget (example.com/v1)`.
    pub fn param_label(&self) -> Option<String> {
        let (api_version, kind) = self.param_kind.as_ref()?;
        Some(match api_version.as_str() {
            "v1" | "" => kind.clone(),
            other => format!("{kind} ({other})"),
        })
    }
}

/// A param reference of a binding.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParamRef {
    pub name: String,
    pub namespace: String,
    pub selector: Option<Value>,
    /// `Allow` or `Deny`.
    pub not_found: String,
}

impl ParamRef {
    /// `kubyl-views/replica-limit`, `selector team=a`, `replica-limit` (a cluster-scoped
    /// param kind).
    pub fn label(&self) -> String {
        self.label_for(false)
    }

    /// The label when the param kind is `namespaced`: without a namespace the params come
    /// from the namespace of each admitted request (`per namespace: replica-limit`).
    pub fn label_for(&self, namespaced: bool) -> String {
        let what = match &self.selector {
            Some(selector) => format!("selector {}", selector_text(selector)),
            None => self.name.clone(),
        };
        match self.namespace.as_str() {
            "" if namespaced => format!("per namespace: {what}"),
            "" => what,
            ns if self.selector.is_some() => format!("{what} in {ns}"),
            ns => format!("{ns}/{what}"),
        }
    }

    /// Whether it names one object (`name` set, and a namespace when the kind is namespaced).
    pub fn names_one(&self, namespaced: bool) -> bool {
        self.selector.is_none()
            && !self.name.is_empty()
            && !(namespaced && self.namespace.is_empty())
    }
}

/// A ValidatingAdmissionPolicyBinding or MutatingAdmissionPolicyBinding.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Binding {
    pub policy: String,
    /// `Deny`, `Warn`, `Audit` (validating bindings only).
    pub validation_actions: Vec<String>,
    pub param_ref: Option<ParamRef>,
    pub resources: Option<Match>,
}

impl Binding {
    pub fn parse(binding: &Value) -> Self {
        let spec = binding.get("spec").unwrap_or(&Value::Null);
        Self {
            policy: str_at(spec, "/policyName").to_string(),
            validation_actions: strings(spec, "/validationActions"),
            param_ref: spec.get("paramRef").map(|p| ParamRef {
                name: str_at(p, "/name").to_string(),
                namespace: str_at(p, "/namespace").to_string(),
                selector: p.get("selector").cloned(),
                not_found: str_at(p, "/parameterNotFoundAction").to_string(),
            }),
            resources: Match::parse(spec.get("matchResources")),
        }
    }
}

/// The bindings of `policy` among `bindings`.
pub fn bindings_of<'a>(
    policy: &str,
    bindings: impl IntoIterator<Item = &'a Value>,
) -> Vec<&'a Value> {
    let mut out: Vec<&Value> = bindings
        .into_iter()
        .filter(|b| str_at(b, "/spec/policyName") == policy)
        .collect();
    out.sort_by_key(|b| crate::format::name(b));
    out
}

/// One webhook of a webhook configuration.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Webhook {
    pub name: String,
    /// `Service ns/name:443/path` or the URL without its query.
    pub client: String,
    pub failure_policy: String,
    pub side_effects: String,
    pub timeout: i64,
    pub rules: Vec<String>,
    pub match_conditions: Vec<Named>,
    pub namespace_selector: Option<Value>,
    pub object_selector: Option<Value>,
    pub reinvocation_policy: String,
}

/// The webhooks of a Validating/MutatingWebhookConfiguration.
pub fn webhooks(configuration: &Value) -> Vec<Webhook> {
    array_at(configuration, "/webhooks")
        .iter()
        .map(|w| {
            let client = match w.pointer("/clientConfig/service") {
                Some(svc) => {
                    let port = match int_at(svc, "/port") {
                        0 => 443,
                        p => p,
                    };
                    format!(
                        "Service {}/{}:{port}{}",
                        str_at(svc, "/namespace"),
                        str_at(svc, "/name"),
                        str_at(svc, "/path")
                    )
                }
                // A URL can carry a token in its query: keep only scheme, host and path.
                None => str_at(w, "/clientConfig/url")
                    .split(['?', '#'])
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            };
            let selector = |key: &str| {
                w.get(key)
                    .filter(|s| s.as_object().is_some_and(|m| !m.is_empty()))
                    .cloned()
            };
            Webhook {
                name: str_at(w, "/name").to_string(),
                client,
                failure_policy: str_at(w, "/failurePolicy").to_string(),
                side_effects: str_at(w, "/sideEffects").to_string(),
                timeout: int_at(w, "/timeoutSeconds"),
                rules: array_at(w, "/rules").iter().map(rule).collect(),
                match_conditions: named(array_at(w, "/matchConditions")),
                namespace_selector: selector("namespaceSelector"),
                object_selector: selector("objectSelector"),
                reinvocation_policy: str_at(w, "/reinvocationPolicy").to_string(),
            }
        })
        .collect()
}

// ----- Label selectors -----

/// A label selector as text: `team=a, env in (prod,stage), !legacy`; `everything` when empty.
pub fn selector_text(selector: &Value) -> String {
    let mut parts = crate::format::map_pairs(selector.get("matchLabels"));
    for e in array_at(selector, "/matchExpressions") {
        let key = str_at(e, "/key");
        let values = strings(e, "/values").join(",");
        parts.push(match str_at(e, "/operator") {
            "In" => format!("{key} in ({values})"),
            "NotIn" => format!("{key} notin ({values})"),
            "Exists" => key.to_string(),
            "DoesNotExist" => format!("!{key}"),
            op => format!("{key} {op} ({values})"),
        });
    }
    if parts.is_empty() {
        "everything".into()
    } else {
        parts.join(", ")
    }
}

/// Whether `labels` (a `metadata.labels` map, or null) match a label selector. An empty
/// selector matches everything; an unknown operator matches nothing.
pub fn selector_matches(selector: &Value, labels: &Value) -> bool {
    let label = |key: &str| labels.get(key).and_then(Value::as_str);
    let labels_ok = selector
        .get("matchLabels")
        .and_then(Value::as_object)
        .is_none_or(|m| m.iter().all(|(k, v)| label(k) == v.as_str()));
    labels_ok
        && array_at(selector, "/matchExpressions").iter().all(|e| {
            let key = str_at(e, "/key");
            let values = strings(e, "/values");
            match (str_at(e, "/operator"), label(key)) {
                ("In", Some(v)) => values.iter().any(|x| x == v),
                ("In", None) => false,
                ("NotIn", Some(v)) => !values.iter().any(|x| x == v),
                ("NotIn", None) => true,
                ("Exists", found) => found.is_some(),
                ("DoesNotExist", found) => found.is_none(),
                _ => false,
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn vap(api_version: &str) -> Value {
        json!({"apiVersion": format!("admissionregistration.k8s.io/{api_version}"),
            "kind": "ValidatingAdmissionPolicy", "metadata": {"name": "kubyl-replica-limit"},
            "spec": {"failurePolicy": "Fail", "paramKind": {"apiVersion": "v1", "kind": "ConfigMap"},
                "matchConstraints": {"resourceRules": [{"apiGroups": ["apps"], "apiVersions": ["v1"],
                    "operations": ["CREATE", "UPDATE"], "resources": ["deployments", "statefulsets"]}],
                    "namespaceSelector": {}, "matchPolicy": "Equivalent"},
                "matchConditions": [{"name": "not-a-system-user",
                    "expression": "!request.userInfo.username.startsWith('system:')"}],
                "variables": [{"name": "replicas", "expression": "object.spec.replicas"}],
                "validations": [{"expression": "variables.replicas <= 5",
                    "messageExpression": "'too many'", "reason": "Invalid"}],
                "auditAnnotations": [{"key": "replicas", "valueExpression": "string(variables.replicas)"}]},
            "status": {"typeChecking": {"expressionWarnings": [
                {"fieldRef": "spec.validations[0].expression", "warning": "no such key"}]}}})
    }

    #[test]
    fn policies_read_the_same_in_every_version() {
        for version in ["v1", "v1beta1", "v1alpha1"] {
            let policy = Policy::parse(&vap(version));
            assert_eq!(policy.failure_policy, "Fail");
            assert_eq!(policy.param_label().as_deref(), Some("ConfigMap"));
            let constraints = policy.constraints.as_ref().unwrap();
            assert_eq!(
                constraints.rules,
                ["apps/v1 deployments, statefulsets · CREATE, UPDATE"]
            );
            // An empty selector is no selector.
            assert!(constraints.namespace_selector.is_none());
            assert_eq!(policy.match_conditions[0].name, "not-a-system-user");
            assert_eq!(policy.variables[0].expression, "object.spec.replicas");
            assert_eq!(policy.validations[0].reason, "Invalid");
            assert_eq!(policy.validations[0].message_expression, "'too many'");
            assert_eq!(policy.audit_annotations[0].0, "replicas");
            assert_eq!(policy.warnings[0].1, "no such key");
            assert!(policy.mutations.is_empty());
            assert_eq!(policy.reinvocation_policy, "");
        }
        let mutating = Policy::parse(&json!({"spec": {
            "mutations": [{"patchType": "ApplyConfiguration",
                "applyConfiguration": {"expression": " Object{} "}},
                {"patchType": "JSONPatch", "jsonPatch": {"expression": "[JSONPatch{op: 'add'}]"}}],
            "paramKind": {"apiVersion": "example.com/v1", "kind": "Widget"}}}));
        assert_eq!(mutating.failure_policy, "Fail");
        assert_eq!(mutating.reinvocation_policy, "Never");
        assert_eq!(mutating.mutations[0].expression, "Object{}");
        assert_eq!(mutating.mutations[1].patch_type, "JSONPatch");
        assert_eq!(
            mutating.param_label().as_deref(),
            Some("Widget (example.com/v1)")
        );
    }

    #[test]
    fn bindings_and_their_policies() {
        let binding = json!({"metadata": {"name": "b"}, "spec": {"policyName": "kubyl-replica-limit",
            "validationActions": ["Deny", "Audit"],
            "paramRef": {"name": "replica-limit", "namespace": "kubyl-views", "parameterNotFoundAction": "Deny"},
            "matchResources": {"namespaceSelector": {"matchLabels": {"kubyl.dev/views": "true"}}}}});
        let parsed = Binding::parse(&binding);
        assert_eq!(parsed.validation_actions, ["Deny", "Audit"]);
        let param = parsed.param_ref.as_ref().unwrap();
        assert_eq!(param.label(), "kubyl-views/replica-limit");
        assert!(param.names_one(true));
        // A namespaced param kind without a namespace: each request's namespace.
        let per_namespace = ParamRef {
            name: "limits".into(),
            ..ParamRef::default()
        };
        assert_eq!(per_namespace.label_for(true), "per namespace: limits");
        assert_eq!(per_namespace.label_for(false), "limits");
        assert!(!per_namespace.names_one(true));
        assert!(per_namespace.names_one(false));
        assert_eq!(param.not_found, "Deny");
        let selector = parsed.resources.unwrap().namespace_selector.unwrap();
        assert_eq!(selector_text(&selector), "kubyl.dev/views=true");
        let other = json!({"metadata": {"name": "a"}, "spec": {"policyName": "other"}});
        let all = [binding.clone(), other];
        assert_eq!(bindings_of("kubyl-replica-limit", all.iter()).len(), 1);
        assert_eq!(bindings_of("missing", all.iter()).len(), 0);
        assert_eq!(
            bindings_resource("MutatingAdmissionPolicy"),
            "mutatingadmissionpolicybindings"
        );
        assert_eq!(
            policy_resource("ValidatingAdmissionPolicyBinding"),
            "validatingadmissionpolicies"
        );
    }

    #[test]
    fn webhooks_with_match_conditions() {
        let config = json!({"webhooks": [
            {"name": "validate.example.com", "failurePolicy": "Fail", "sideEffects": "None",
             "timeoutSeconds": 5,
             "clientConfig": {"service": {"namespace": "ops", "name": "hook", "path": "/validate"}},
             "rules": [{"apiGroups": [""], "apiVersions": ["v1"], "operations": ["CREATE"],
                        "resources": ["pods"], "scope": "Namespaced"}],
             "matchConditions": [{"name": "not-kube-system", "expression": "request.namespace != 'kube-system'"}]},
            {"name": "url.example.com", "clientConfig": {"url": "https://hook.example.com/v?token=s3cr3t"}}]});
        let hooks = webhooks(&config);
        assert_eq!(hooks[0].client, "Service ops/hook:443/validate");
        assert_eq!(hooks[0].rules, ["core/v1 pods · CREATE · Namespaced"]);
        assert_eq!(hooks[0].match_conditions[0].name, "not-kube-system");
        assert_eq!(hooks[1].client, "https://hook.example.com/v");
        assert!(!format!("{hooks:?}").contains("s3cr3t"));
    }

    #[test]
    fn label_selectors() {
        let selector = json!({"matchLabels": {"team": "a"},
            "matchExpressions": [{"key": "env", "operator": "In", "values": ["prod", "stage"]},
                                 {"key": "legacy", "operator": "DoesNotExist"}]});
        assert_eq!(
            selector_text(&selector),
            "team=a, env in (prod,stage), !legacy"
        );
        assert!(selector_matches(
            &selector,
            &json!({"team": "a", "env": "prod"})
        ));
        assert!(!selector_matches(
            &selector,
            &json!({"team": "a", "env": "dev"})
        ));
        assert!(!selector_matches(
            &selector,
            &json!({"team": "a", "env": "prod", "legacy": "1"})
        ));
        assert!(selector_matches(&json!({}), &Value::Null));
        assert_eq!(selector_text(&json!({})), "everything");
    }
}
