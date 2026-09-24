//! Starting points for "New resource": hand-written templates for common kinds and a skeleton
//! from any kind's schema (its required fields), so CRDs work too.

use kubyl_core::Gvk;
use serde_json::{Map, Value, json};

use crate::render::to_yaml;
use crate::schema::Schema;

/// A built-in template.
pub struct Template {
    pub title: &'static str,
    pub gvk: (&'static str, &'static str, &'static str),
    body: fn(&str) -> String,
}

impl Template {
    pub fn text(&self, namespace: &str) -> String {
        (self.body)(namespace)
    }

    pub fn gvk(&self) -> Gvk {
        Gvk::new(self.gvk.0, self.gvk.1, self.gvk.2)
    }
}

pub const TEMPLATES: &[Template] = &[
    Template {
        title: "Deployment",
        gvk: ("apps", "v1", "Deployment"),
        body: |ns| {
            format!(
                "apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  namespace: {ns}
  labels:
    app: web
spec:
  replicas: 2
  selector:
    matchLabels:
      app: web
  template:
    metadata:
      labels:
        app: web
    spec:
      containers:
        - name: web
          image: nginx:1.27
          ports:
            - containerPort: 80
          resources:
            requests:
              cpu: 100m
              memory: 128Mi
"
            )
        },
    },
    Template {
        title: "Service",
        gvk: ("", "v1", "Service"),
        body: |ns| {
            format!(
                "apiVersion: v1
kind: Service
metadata:
  name: web
  namespace: {ns}
spec:
  selector:
    app: web
  ports:
    - name: http
      port: 80
      targetPort: 80
"
            )
        },
    },
    Template {
        title: "Ingress",
        gvk: ("networking.k8s.io", "v1", "Ingress"),
        body: |ns| {
            format!(
                "apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: web
  namespace: {ns}
spec:
  rules:
    - host: web.example.com
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: web
                port:
                  number: 80
"
            )
        },
    },
    Template {
        title: "CronJob",
        gvk: ("batch", "v1", "CronJob"),
        body: |ns| {
            format!(
                "apiVersion: batch/v1
kind: CronJob
metadata:
  name: nightly
  namespace: {ns}
spec:
  schedule: \"0 3 * * *\"
  concurrencyPolicy: Forbid
  jobTemplate:
    spec:
      template:
        spec:
          restartPolicy: OnFailure
          containers:
            - name: job
              image: busybox:1.37
              command: [\"sh\", \"-c\", \"date\"]
"
            )
        },
    },
    Template {
        title: "ConfigMap",
        gvk: ("", "v1", "ConfigMap"),
        body: |ns| {
            format!(
                "apiVersion: v1
kind: ConfigMap
metadata:
  name: settings
  namespace: {ns}
data:
  LOG_LEVEL: info
"
            )
        },
    },
    Template {
        title: "Secret",
        gvk: ("", "v1", "Secret"),
        body: |ns| {
            // `data` values are typed in plain text; Kubyl encodes them on apply.
            format!(
                "apiVersion: v1
kind: Secret
metadata:
  name: credentials
  namespace: {ns}
type: Opaque
data:
  username: admin
  password: \"\"
"
            )
        },
    },
    Template {
        title: "PersistentVolumeClaim",
        gvk: ("", "v1", "PersistentVolumeClaim"),
        body: |ns| {
            format!(
                "apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: data
  namespace: {ns}
spec:
  accessModes:
    - ReadWriteOnce
  resources:
    requests:
      storage: 1Gi
"
            )
        },
    },
];

pub fn template_for(gvk: &Gvk) -> Option<&'static Template> {
    TEMPLATES
        .iter()
        .find(|t| t.gvk.0 == gvk.group && t.gvk.2 == gvk.kind)
}

/// A skeleton with the kind's required fields (placeholders by type, enums get their first
/// value). `namespace: None` for cluster-scoped kinds.
pub fn skeleton(schema: Option<&Schema>, gvk: &Gvk, namespace: Option<&str>) -> String {
    let mut object = Map::new();
    object.insert("apiVersion".into(), Value::String(gvk.api_version()));
    object.insert("kind".into(), Value::String(gvk.kind.clone()));
    let mut meta = Map::new();
    meta.insert("name".into(), Value::String(String::new()));
    if let Some(ns) = namespace {
        meta.insert("namespace".into(), Value::String(ns.to_string()));
    }
    object.insert("metadata".into(), Value::Object(meta));
    if let Some(schema) = schema {
        for (name, child) in schema.properties() {
            if matches!(name, "apiVersion" | "kind" | "metadata" | "status") {
                continue;
            }
            let required = schema.required().contains(&name);
            // `spec` is almost always wanted even when the schema doesn't require it.
            if required || name == "spec" {
                object.insert(name.to_string(), placeholder(&child, 0));
            }
        }
    }
    to_yaml(&Value::Object(object))
}

fn placeholder(schema: &Schema, depth: usize) -> Value {
    if let Some(first) = schema.enum_values().and_then(|v| v.first()) {
        return first.clone();
    }
    if let Some(default) = schema.default_value() {
        return default.clone();
    }
    match schema.type_name() {
        Some("object") if depth < 5 => {
            let mut map = Map::new();
            let required = schema.required();
            for (name, child) in schema.properties() {
                if required.contains(&name) {
                    map.insert(name.to_string(), placeholder(&child, depth + 1));
                }
            }
            Value::Object(map)
        }
        Some("object") => json!({}),
        Some("array") => match schema.items() {
            Some(items) if items.type_name() == Some("object") && !items.required().is_empty() => {
                json!([placeholder(&items, depth + 1)])
            }
            _ => json!([]),
        },
        Some("integer") | Some("number") => json!(0),
        Some("boolean") => json!(false),
        _ if schema.is_int_or_string() => json!(0),
        _ => json!(""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;
    use crate::schema::tests::cert_doc;

    #[test]
    fn templates_parse() {
        for template in TEMPLATES {
            let parsed = parse(&template.text("payments"));
            assert!(parsed.error.is_none(), "{}", template.title);
            let root = parsed.roots().next().unwrap().to_json();
            assert_eq!(root["kind"], template.gvk.2);
            assert_eq!(root["metadata"]["namespace"], "payments");
        }
    }

    #[test]
    fn skeletons_have_the_required_fields() {
        let doc = cert_doc();
        let gvk = Gvk::new("cert-manager.io", "v1", "Certificate");
        let schema = Schema::for_gvk(&doc, &gvk).unwrap();
        let text = skeleton(Some(&schema), &gvk, Some("payments"));
        let object = parse(&text).roots().next().unwrap().to_json();
        assert_eq!(object["apiVersion"], "cert-manager.io/v1");
        assert_eq!(object["metadata"]["namespace"], "payments");
        assert_eq!(object["spec"]["secretName"], "");
        assert_eq!(object["spec"]["issuerRef"], json!({"name": ""}));
        assert!(object.get("status").is_none());
    }
}
