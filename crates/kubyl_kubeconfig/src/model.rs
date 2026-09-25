//! The kubeconfig document being edited.
//!
//! [`Doc`] keeps the parsed YAML as JSON (`serde_json` keeps key order), so fields Kubyl doesn't
//! know (extensions, preferences, newer kubectl fields) survive every edit. Forms read and write
//! single fields of an entry by path ([`get_str`], [`set_str`]…); [`crate::yaml::write`] turns
//! the edited document back into text.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

/// The three lists of a kubeconfig.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Context,
    Cluster,
    User,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Context, Kind::Cluster, Kind::User];

    /// `contexts`, `clusters`, `users`.
    pub fn list_key(self) -> &'static str {
        match self {
            Kind::Context => "contexts",
            Kind::Cluster => "clusters",
            Kind::User => "users",
        }
    }

    /// The key of an entry's body: `context`, `cluster`, `user`.
    pub fn body_key(self) -> &'static str {
        match self {
            Kind::Context => "context",
            Kind::Cluster => "cluster",
            Kind::User => "user",
        }
    }

    pub fn label(self) -> &'static str {
        self.body_key()
    }

    /// `Contexts`, `Clusters`, `Users`.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Context => "Contexts",
            Kind::Cluster => "Clusters",
            Kind::User => "Users",
        }
    }
}

/// An entry of a list.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntryRef {
    pub kind: Kind,
    pub name: String,
}

impl EntryRef {
    pub fn new(kind: Kind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
        }
    }
}

/// A kubeconfig document.
#[derive(Clone, Debug, PartialEq)]
pub struct Doc(pub Value);

impl Default for Doc {
    fn default() -> Self {
        Self::empty()
    }
}

impl Doc {
    /// A new, empty kubeconfig.
    pub fn empty() -> Self {
        Self(json!({
            "apiVersion": "v1",
            "kind": "Config",
            "clusters": [],
            "contexts": [],
            "users": [],
            "preferences": {},
        }))
    }

    /// Parses kubeconfig YAML (comments and key order are kept by [`crate::yaml::write`]).
    pub fn parse(text: &str) -> Result<Self, String> {
        let value = crate::yaml::load(text)?;
        let doc = Self(value);
        for kind in Kind::ALL {
            match doc.0.get(kind.list_key()) {
                None | Some(Value::Null) | Some(Value::Array(_)) => {}
                Some(_) => return Err(format!("`{}` isn't a list", kind.list_key())),
            }
        }
        Ok(doc)
    }

    fn root(&mut self) -> &mut Map<String, Value> {
        if !self.0.is_object() {
            self.0 = Value::Object(Map::new());
        }
        self.0.as_object_mut().expect("an object")
    }

    fn list(&self, kind: Kind) -> &[Value] {
        self.0
            .get(kind.list_key())
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }

    fn list_mut(&mut self, kind: Kind) -> &mut Vec<Value> {
        let root = self.root();
        let slot = root
            .entry(kind.list_key())
            .or_insert_with(|| Value::Array(Vec::new()));
        if !slot.is_array() {
            *slot = Value::Array(Vec::new());
        }
        slot.as_array_mut().expect("an array")
    }

    /// Entry names in file order (entries without a name are skipped).
    pub fn names(&self, kind: Kind) -> Vec<String> {
        self.list(kind)
            .iter()
            .filter_map(|e| e.get("name").and_then(Value::as_str))
            .map(String::from)
            .collect()
    }

    fn index(&self, kind: Kind, name: &str) -> Option<usize> {
        self.list(kind)
            .iter()
            .position(|e| e.get("name").and_then(Value::as_str) == Some(name))
    }

    pub fn contains(&self, kind: Kind, name: &str) -> bool {
        self.index(kind, name).is_some()
    }

    /// The whole list entry (`name` plus body and anything else).
    pub fn entry(&self, kind: Kind, name: &str) -> Option<&Value> {
        self.list(kind).get(self.index(kind, name)?)
    }

    /// An entry's body (`cluster:`, `user:` or `context:`).
    pub fn body(&self, kind: Kind, name: &str) -> Option<&Map<String, Value>> {
        self.entry(kind, name)?
            .get(kind.body_key())
            .and_then(Value::as_object)
    }

    /// An entry's body for editing (created when missing or not a mapping).
    pub fn body_mut(&mut self, kind: Kind, name: &str) -> Option<&mut Map<String, Value>> {
        let ix = self.index(kind, name)?;
        let entry = self.list_mut(kind).get_mut(ix)?.as_object_mut()?;
        let body = entry
            .entry(kind.body_key())
            .or_insert_with(|| Value::Object(Map::new()));
        if !body.is_object() {
            *body = Value::Object(Map::new());
        }
        body.as_object_mut()
    }

    /// Adds an entry (`name` first, then the body). Fails on a taken name.
    pub fn add(&mut self, kind: Kind, name: &str, body: Map<String, Value>) -> Result<(), String> {
        validate_name(name)?;
        if self.contains(kind, name) {
            return Err(format!(
                "a {} named \"{name}\" already exists",
                kind.label()
            ));
        }
        let mut entry = Map::new();
        entry.insert("name".into(), Value::String(name.to_string()));
        entry.insert(kind.body_key().into(), Value::Object(body));
        self.list_mut(kind).push(Value::Object(entry));
        Ok(())
    }

    /// `base`, or `base-2`, `base-3`… if taken.
    pub fn unique_name(&self, kind: Kind, base: &str) -> String {
        let base = if base.trim().is_empty() {
            kind.label()
        } else {
            base.trim()
        };
        if !self.contains(kind, base) {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|n| !self.contains(kind, n))
            .expect("some name is free")
    }

    /// Copies an entry under a new name (`<name>-copy`). Returns the new name.
    pub fn duplicate(&mut self, kind: Kind, name: &str) -> Option<String> {
        let mut copy = self.entry(kind, name)?.clone();
        let new_name = self.unique_name(kind, &format!("{name}-copy"));
        copy["name"] = Value::String(new_name.clone());
        let ix = self.index(kind, name)?;
        self.list_mut(kind).insert(ix + 1, copy);
        Some(new_name)
    }

    /// Renames an entry and everything that refers to it (contexts, `current-context`).
    pub fn rename(&mut self, kind: Kind, old: &str, new: &str) -> Result<(), String> {
        if old == new {
            return Ok(());
        }
        validate_name(new)?;
        if self.contains(kind, new) {
            return Err(format!("a {} named \"{new}\" already exists", kind.label()));
        }
        let ix = self
            .index(kind, old)
            .ok_or_else(|| format!("no {} named \"{old}\"", kind.label()))?;
        self.list_mut(kind)[ix]["name"] = Value::String(new.to_string());
        match kind {
            Kind::Context => {
                if self.current_context() == Some(old) {
                    self.set_current_context(Some(new));
                }
            }
            Kind::Cluster | Kind::User => {
                let field = kind.body_key();
                for context in self.list_mut(Kind::Context) {
                    if let Some(body) = context.get_mut("context").and_then(Value::as_object_mut)
                        && body.get(field).and_then(Value::as_str) == Some(old)
                    {
                        body.insert(field.into(), Value::String(new.to_string()));
                    }
                }
            }
        }
        Ok(())
    }

    /// Contexts that use a cluster or user.
    pub fn used_by(&self, kind: Kind, name: &str) -> Vec<String> {
        if kind == Kind::Context {
            return Vec::new();
        }
        let field = kind.body_key();
        self.list(Kind::Context)
            .iter()
            .filter(|c| {
                c.get("context")
                    .and_then(|b| b.get(field))
                    .and_then(Value::as_str)
                    == Some(name)
            })
            .filter_map(|c| c.get("name").and_then(Value::as_str))
            .map(String::from)
            .collect()
    }

    /// Removes an entry. A removed current context clears `current-context`.
    pub fn remove(&mut self, kind: Kind, name: &str) -> bool {
        let Some(ix) = self.index(kind, name) else {
            return false;
        };
        self.list_mut(kind).remove(ix);
        if kind == Kind::Context && self.current_context() == Some(name) {
            self.set_current_context(None);
        }
        true
    }

    pub fn current_context(&self) -> Option<&str> {
        self.0
            .get("current-context")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    }

    pub fn set_current_context(&mut self, name: Option<&str>) {
        let root = self.root();
        match name {
            Some(name) => {
                root.insert("current-context".into(), Value::String(name.into()));
            }
            None => {
                if root.contains_key("current-context") {
                    root.insert("current-context".into(), Value::String(String::new()));
                }
            }
        }
    }

    /// The context's cluster and user names.
    pub fn context_refs(&self, context: &str) -> (Option<String>, Option<String>) {
        let body = self.body(Kind::Context, context);
        let get = |k: &str| {
            body.and_then(|b| b.get(k))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(String::from)
        };
        (get("cluster"), get("user"))
    }

    /// A standalone document with one context, its cluster and user.
    /// `credentials: false` removes every secret from the user.
    pub fn extract(&self, context: &str, credentials: bool) -> Option<Doc> {
        let mut out = Doc::empty();
        out.list_mut(Kind::Context)
            .push(self.entry(Kind::Context, context)?.clone());
        let (cluster, user) = self.context_refs(context);
        if let Some(entry) = cluster.and_then(|c| self.entry(Kind::Cluster, &c)) {
            out.list_mut(Kind::Cluster).push(entry.clone());
        }
        if let Some(mut entry) = user.and_then(|u| self.entry(Kind::User, &u).cloned()) {
            if !credentials && let Some(body) = entry.get_mut("user").and_then(Value::as_object_mut)
            {
                strip_secrets(body);
            }
            out.list_mut(Kind::User).push(entry);
        }
        out.set_current_context(Some(context));
        Some(out)
    }

    /// Copies a context with its cluster and user into this document. Names already taken get
    /// a counter; returns the context's name here.
    pub fn import_context(&mut self, from: &Doc, context: &str) -> Option<String> {
        let mut entry = from.entry(Kind::Context, context)?.clone();
        let (cluster, user) = from.context_refs(context);
        let copy_ref = |this: &mut Doc, kind: Kind, name: Option<String>| -> Option<String> {
            let name = name?;
            let theirs = from.entry(kind, &name)?.clone();
            // The same entry already here: share it.
            if this.entry(kind, &name) == Some(&theirs) {
                return Some(name);
            }
            let new_name = this.unique_name(kind, &name);
            let mut theirs = theirs;
            theirs["name"] = Value::String(new_name.clone());
            this.list_mut(kind).push(theirs);
            Some(new_name)
        };
        let cluster = copy_ref(self, Kind::Cluster, cluster);
        let user = copy_ref(self, Kind::User, user);
        let name = self.unique_name(Kind::Context, context);
        entry["name"] = Value::String(name.clone());
        if let Some(body) = entry.get_mut("context").and_then(Value::as_object_mut) {
            if let Some(cluster) = cluster {
                body.insert("cluster".into(), Value::String(cluster));
            }
            if let Some(user) = user {
                body.insert("user".into(), Value::String(user));
            }
        }
        self.list_mut(Kind::Context).push(entry);
        Some(name)
    }

    /// Removes a context and the cluster and user no other context uses.
    pub fn remove_context_with_refs(&mut self, context: &str) {
        let (cluster, user) = self.context_refs(context);
        self.remove(Kind::Context, context);
        for (kind, name) in [(Kind::Cluster, cluster), (Kind::User, user)] {
            if let Some(name) = name
                && self.used_by(kind, &name).is_empty()
            {
                self.remove(kind, &name);
            }
        }
    }

    /// Whether any user holds a secret inline (the file must be 0600).
    pub fn has_inline_credentials(&self) -> bool {
        self.list(Kind::User).iter().any(|u| {
            u.get("user")
                .and_then(Value::as_object)
                .is_some_and(|body| !secret_paths(body).is_empty())
        })
    }

    /// For the client builder: the document as kube's type.
    pub fn to_kube(&self) -> Result<kube::config::Kubeconfig, String> {
        let mut value = self.0.clone();
        // kube wants lists where kubectl tolerates `null`.
        if let Some(root) = value.as_object_mut() {
            for kind in Kind::ALL {
                if root.get(kind.list_key()).is_some_and(Value::is_null) {
                    root.insert(kind.list_key().into(), Value::Array(Vec::new()));
                }
            }
        }
        serde_json::from_value(value).map_err(|e| format!("invalid kubeconfig: {e}"))
    }

    /// The YAML of the document rendered from scratch (new files).
    pub fn to_yaml(&self) -> String {
        crate::yaml::render(&self.0)
    }
}

/// Names can't be empty or have surrounding spaces.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the name can't be empty".into());
    }
    if name.trim() != name {
        return Err("the name can't start or end with a space".into());
    }
    Ok(())
}

// ----- Fields by path -----

fn walk<'a>(map: &'a Map<String, Value>, path: &[&str]) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    let value = map.get(*first)?;
    if rest.is_empty() {
        return Some(value);
    }
    walk(value.as_object()?, rest)
}

/// The value at `path` in an entry body.
pub fn get<'a>(map: &'a Map<String, Value>, path: &[&str]) -> Option<&'a Value> {
    walk(map, path)
}

/// A string (or number/bool as text) at `path`; `""` when missing.
pub fn get_str(map: &Map<String, Value>, path: &[&str]) -> String {
    match walk(map, path) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

pub fn get_bool(map: &Map<String, Value>, path: &[&str]) -> bool {
    matches!(walk(map, path), Some(Value::Bool(true)))
        || matches!(walk(map, path), Some(Value::String(s)) if s == "true")
}

/// The object at `path`, created along the way.
fn parent_mut<'a>(map: &'a mut Map<String, Value>, path: &[&str]) -> &'a mut Map<String, Value> {
    let mut current = map;
    for key in path {
        let slot = current
            .entry(key.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !slot.is_object() {
            *slot = Value::Object(Map::new());
        }
        current = slot.as_object_mut().expect("an object");
    }
    current
}

/// Sets `value` at `path`, or removes the key for `None` (and parents left empty by it).
pub fn set(map: &mut Map<String, Value>, path: &[&str], value: Option<Value>) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    match value {
        Some(value) => {
            parent_mut(map, parents).insert(last.to_string(), value);
        }
        None => {
            remove_path(map, path);
        }
    }
}

fn remove_path(map: &mut Map<String, Value>, path: &[&str]) -> bool {
    match path {
        [] => false,
        [last] => map.remove(*last).is_some(),
        [first, rest @ ..] => {
            let Some(child) = map.get_mut(*first).and_then(Value::as_object_mut) else {
                return false;
            };
            let removed = remove_path(child, rest);
            if removed && child.is_empty() {
                map.remove(*first);
            }
            removed
        }
    }
}

/// Sets a string; an empty one removes the key.
pub fn set_str(map: &mut Map<String, Value>, path: &[&str], value: &str) {
    let value = (!value.is_empty()).then(|| Value::String(value.to_string()));
    set(map, path, value);
}

/// Sets a flag. `false` removes a missing key's line rather than writing `false`, but keeps an
/// existing key (as `false`).
pub fn set_bool(map: &mut Map<String, Value>, path: &[&str], value: bool) {
    if value || walk(map, path).is_some() {
        set(map, path, Some(Value::Bool(value)));
    }
}

// ----- Clusters -----

/// Where a cluster's CA (or a user's certificate/key) comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PemSource {
    /// No CA: the system trust store. (For client certificates: none.)
    None,
    File(String),
    /// Base64 of the PEM (`*-data`).
    Data(String),
}

impl PemSource {
    pub fn read(map: &Map<String, Value>, file_key: &str, data_key: &str) -> Self {
        let data = get_str(map, &[data_key]);
        if !data.is_empty() {
            return PemSource::Data(data);
        }
        let file = get_str(map, &[file_key]);
        if !file.is_empty() {
            return PemSource::File(file);
        }
        PemSource::None
    }

    pub fn write(&self, map: &mut Map<String, Value>, file_key: &str, data_key: &str) {
        match self {
            PemSource::None => {
                set(map, &[file_key], None);
                set(map, &[data_key], None);
            }
            PemSource::File(path) => {
                set(map, &[data_key], None);
                set_str(map, &[file_key], path);
            }
            PemSource::Data(data) => {
                set(map, &[file_key], None);
                set_str(map, &[data_key], data);
            }
        }
    }
}

pub const CA_FILE: &str = "certificate-authority";
pub const CA_DATA: &str = "certificate-authority-data";
pub const CERT_FILE: &str = "client-certificate";
pub const CERT_DATA: &str = "client-certificate-data";
pub const KEY_FILE: &str = "client-key";
pub const KEY_DATA: &str = "client-key-data";

// ----- Users -----

/// How a user authenticates (what the user form shows).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AuthKind {
    None,
    Token,
    TokenFile,
    ClientCertificate,
    Exec,
    OidcProvider,
    Basic,
    /// A legacy auth provider other than OIDC (`gcp`, `azure`): shown read-only.
    OtherProvider,
}

impl AuthKind {
    /// The choices of the user form, in order.
    pub const CHOICES: [AuthKind; 7] = [
        AuthKind::ClientCertificate,
        AuthKind::Token,
        AuthKind::TokenFile,
        AuthKind::Exec,
        AuthKind::OidcProvider,
        AuthKind::Basic,
        AuthKind::None,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AuthKind::None => "None",
            AuthKind::Token => "Token",
            AuthKind::TokenFile => "Token file",
            AuthKind::ClientCertificate => "Client certificate",
            AuthKind::Exec => "Exec plugin",
            AuthKind::OidcProvider => "OIDC provider",
            AuthKind::Basic => "Basic auth",
            AuthKind::OtherProvider => "Auth provider",
        }
    }

    /// Detects the kind from a user body. Exec wins over everything (kubectl's order).
    pub fn detect(body: &Map<String, Value>) -> Self {
        if body.contains_key("exec") {
            AuthKind::Exec
        } else if let Some(provider) = body.get("auth-provider") {
            if provider.get("name").and_then(Value::as_str) == Some("oidc") {
                AuthKind::OidcProvider
            } else {
                AuthKind::OtherProvider
            }
        } else if body.contains_key(CERT_FILE) || body.contains_key(CERT_DATA) {
            AuthKind::ClientCertificate
        } else if body.contains_key("token") {
            AuthKind::Token
        } else if body.contains_key("tokenFile") {
            AuthKind::TokenFile
        } else if body.contains_key("username") || body.contains_key("password") {
            AuthKind::Basic
        } else {
            AuthKind::None
        }
    }

    /// The keys of a user body that belong to this kind.
    fn keys(self) -> &'static [&'static str] {
        match self {
            AuthKind::None | AuthKind::OtherProvider => &[],
            AuthKind::Token => &["token"],
            AuthKind::TokenFile => &["tokenFile"],
            AuthKind::ClientCertificate => &[CERT_FILE, CERT_DATA, KEY_FILE, KEY_DATA],
            AuthKind::Exec => &["exec"],
            AuthKind::OidcProvider => &["auth-provider"],
            AuthKind::Basic => &["username", "password"],
        }
    }
}

/// Every credential key a user body can have.
const AUTH_KEYS: &[&str] = &[
    "token",
    "tokenFile",
    CERT_FILE,
    CERT_DATA,
    KEY_FILE,
    KEY_DATA,
    "exec",
    "auth-provider",
    "username",
    "password",
];

/// Switches a user to `kind`: removes the other kinds' keys (other fields like `as` or
/// `extensions` stay) and adds a starting point for the new one.
pub fn set_auth_kind(body: &mut Map<String, Value>, kind: AuthKind) {
    if AuthKind::detect(body) == kind {
        return;
    }
    let keep = kind.keys();
    body.retain(|k, _| !AUTH_KEYS.contains(&k.as_str()) || keep.contains(&k.as_str()));
    match kind {
        AuthKind::Exec => {
            body.insert(
                "exec".into(),
                json!({
                    "apiVersion": EXEC_API_VERSION,
                    "command": "",
                }),
            );
        }
        AuthKind::OidcProvider => {
            body.insert(
                "auth-provider".into(),
                json!({"name": "oidc", "config": {"idp-issuer-url": "", "client-id": ""}}),
            );
        }
        AuthKind::Token => {
            body.insert("token".into(), Value::String(String::new()));
        }
        AuthKind::TokenFile => {
            body.insert("tokenFile".into(), Value::String(String::new()));
        }
        AuthKind::Basic => {
            body.insert("username".into(), Value::String(String::new()));
            body.insert("password".into(), Value::String(String::new()));
        }
        AuthKind::ClientCertificate => {
            body.insert(CERT_FILE.into(), Value::String(String::new()));
            body.insert(KEY_FILE.into(), Value::String(String::new()));
        }
        AuthKind::None | AuthKind::OtherProvider => {}
    }
}

/// The exec credential API version Kubyl writes for new plugins.
pub const EXEC_API_VERSION: &str = "client.authentication.k8s.io/v1";

/// Paths of secret values in a user body (masked in the UI, removed by exports without
/// credentials).
pub fn secret_paths(body: &Map<String, Value>) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for key in ["token", KEY_DATA, "password"] {
        if body
            .get(key)
            .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))
        {
            out.push(vec![key.to_string()]);
        }
    }
    if let Some(config) = body
        .get("auth-provider")
        .and_then(|p| p.get("config"))
        .and_then(Value::as_object)
    {
        for key in SECRET_PROVIDER_KEYS {
            if config
                .get(*key)
                .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))
            {
                out.push(vec![
                    "auth-provider".into(),
                    "config".into(),
                    key.to_string(),
                ]);
            }
        }
    }
    if let Some(env) = body
        .get("exec")
        .and_then(|e| e.get("env"))
        .and_then(Value::as_array)
    {
        for (ix, var) in env.iter().enumerate() {
            let name = var.get("name").and_then(Value::as_str).unwrap_or_default();
            if is_secret_env(name) {
                out.push(vec![
                    "exec".into(),
                    "env".into(),
                    ix.to_string(),
                    "value".into(),
                ]);
            }
        }
    }
    out
}

/// Auth-provider config keys that hold secrets.
pub const SECRET_PROVIDER_KEYS: &[&str] =
    &["client-secret", "id-token", "refresh-token", "access-token"];

/// Exec env variables whose value is likely a secret.
pub fn is_secret_env(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    [
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "PRIVATE",
        "API_KEY",
        "ACCESS_KEY",
    ]
    .iter()
    .any(|s| upper.contains(s))
}

/// Removes every secret of a user body (exports without credentials). Files referenced by
/// path stay (they aren't in the document).
pub fn strip_secrets(body: &mut Map<String, Value>) {
    body.remove("token");
    body.remove(KEY_DATA);
    body.remove("password");
    if let Some(config) = body
        .get_mut("auth-provider")
        .and_then(|p| p.get_mut("config"))
        .and_then(Value::as_object_mut)
    {
        for key in SECRET_PROVIDER_KEYS {
            config.remove(*key);
        }
    }
    if let Some(env) = body
        .get_mut("exec")
        .and_then(|e| e.get_mut("env"))
        .and_then(Value::as_array_mut)
    {
        env.retain(|var| {
            !is_secret_env(var.get("name").and_then(Value::as_str).unwrap_or_default())
        });
    }
}

/// Replaces secret values in a whole document with a placeholder (the YAML tab while secrets
/// are masked). Returns the originals by path, to put them back.
pub fn mask_secrets(doc: &mut Doc, placeholder: &str) -> Vec<(String, Vec<String>, String)> {
    let mut masked = Vec::new();
    let names = doc.names(Kind::User);
    for name in names {
        let Some(body) = doc.body_mut(Kind::User, &name) else {
            continue;
        };
        for path in secret_paths(body) {
            let refs: Vec<&str> = path.iter().map(String::as_str).collect();
            if let Some(original) = get_json_mut(body, &refs)
                && let Value::String(s) = original
            {
                masked.push((
                    name.clone(),
                    path.clone(),
                    std::mem::replace(s, placeholder.into()),
                ));
            }
        }
    }
    masked
}

/// A value by path where numeric segments index arrays.
pub fn get_json_mut<'a>(map: &'a mut Map<String, Value>, path: &[&str]) -> Option<&'a mut Value> {
    let (first, rest) = path.split_first()?;
    let mut current = map.get_mut(*first)?;
    for seg in rest {
        current = match current {
            Value::Object(m) => m.get_mut(*seg)?,
            Value::Array(items) => items.get_mut(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

// ----- Exec plugins -----

/// What the consent dialog shows about an exec plugin (and what consent is given for).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExecSpec {
    pub command: String,
    pub args: Vec<String>,
    /// `NAME=value` pairs.
    pub env: Vec<(String, String)>,
    pub api_version: String,
    pub interactive_mode: String,
    pub provide_cluster_info: bool,
}

impl ExecSpec {
    pub fn read(exec: &Value) -> Option<Self> {
        let command = exec.get("command")?.as_str()?.to_string();
        let strings = |v: Option<&Value>| -> Vec<String> {
            v.and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|x| match x {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let env = exec
            .get("env")
            .and_then(Value::as_array)
            .map(|vars| {
                vars.iter()
                    .map(|v| {
                        (
                            v.get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            v.get("value")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            command,
            args: strings(exec.get("args")),
            env,
            api_version: exec
                .get("apiVersion")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            interactive_mode: exec
                .get("interactiveMode")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            provide_cluster_info: exec
                .get("provideClusterInfo")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// A stable id for consent (never stored; env values are part of it).
    pub fn consent_key(&self) -> u64 {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

/// The exec plugins a context runs (its user's), with the user's name.
pub fn exec_of(doc: &Doc, context: &str) -> Option<(String, ExecSpec)> {
    let (_, user) = doc.context_refs(context);
    let user = user?;
    let exec = doc.body(Kind::User, &user)?.get("exec")?;
    Some((user, ExecSpec::read(exec)?))
}

// ----- Exec presets -----

/// Exec plugin presets for the user form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecPreset {
    Eks,
    Gke,
    AksKubelogin,
    KubeloginOidc,
}

impl ExecPreset {
    pub const ALL: [ExecPreset; 4] = [
        ExecPreset::Eks,
        ExecPreset::Gke,
        ExecPreset::AksKubelogin,
        ExecPreset::KubeloginOidc,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ExecPreset::Eks => "AWS EKS (aws eks get-token)",
            ExecPreset::Gke => "GKE (gke-gcloud-auth-plugin)",
            ExecPreset::AksKubelogin => "AKS (kubelogin)",
            ExecPreset::KubeloginOidc => "OIDC (kubelogin)",
        }
    }

    /// The `exec:` block, as the cloud CLIs write it. Placeholders in angle brackets.
    pub fn exec(self) -> Value {
        match self {
            ExecPreset::Eks => json!({
                "apiVersion": "client.authentication.k8s.io/v1beta1",
                "command": "aws",
                "args": ["--region", "<region>", "eks", "get-token", "--cluster-name", "<cluster>", "--output", "json"],
                "installHint": "Install the AWS CLI: https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html",
            }),
            ExecPreset::Gke => json!({
                "apiVersion": "client.authentication.k8s.io/v1beta1",
                "command": "gke-gcloud-auth-plugin",
                "installHint": "Install gke-gcloud-auth-plugin for use with kubectl by following https://cloud.google.com/kubernetes-engine/docs/how-to/cluster-access-for-kubectl#install_plugin",
                "provideClusterInfo": true,
            }),
            ExecPreset::AksKubelogin => json!({
                "apiVersion": "client.authentication.k8s.io/v1beta1",
                "command": "kubelogin",
                "args": ["get-token", "--login", "azurecli", "--server-id", AKS_SERVER_ID],
                "installHint": "Install kubelogin: https://azure.github.io/kubelogin/install.html",
            }),
            ExecPreset::KubeloginOidc => json!({
                "apiVersion": "client.authentication.k8s.io/v1beta1",
                "command": "kubectl",
                "args": ["oidc-login", "get-token", "--oidc-issuer-url=<issuer>", "--oidc-client-id=<client-id>"],
                "installHint": "Install kubelogin (kubectl oidc-login): https://github.com/int128/kubelogin#setup",
            }),
        }
    }
}

/// The AAD server application id of AKS-managed Microsoft Entra ID.
pub const AKS_SERVER_ID: &str = "6dae42f8-4368-4678-94ff-3960e28e3630";

/// Every `name` used more than once in a list.
pub fn duplicate_names(doc: &Doc, kind: Kind) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut dups = BTreeSet::new();
    for name in doc.names(kind) {
        if !seen.insert(name.clone()) {
            dups.insert(name);
        }
    }
    dups
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"apiVersion: v1
kind: Config
current-context: dev
clusters:
- name: dev
  cluster: {server: "https://127.0.0.1:6443", certificate-authority-data: QUJD}
contexts:
- name: dev
  context: {cluster: dev, user: dev, namespace: payments}
- name: other
  context: {cluster: dev, user: aws}
users:
- name: dev
  user: {token: s3cret, as: admin}
- name: aws
  user:
    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      command: aws
      args: [eks, get-token]
      env: [{name: AWS_PROFILE, value: prod}, {name: AWS_SECRET_ACCESS_KEY, value: shh}]
extensions:
- name: x
  extension: {a: 1}
"#;

    #[test]
    fn renames_update_references() {
        let mut doc = Doc::parse(SAMPLE).unwrap();
        doc.rename(Kind::Cluster, "dev", "kind").unwrap();
        doc.rename(Kind::Context, "dev", "kind-dev").unwrap();
        assert_eq!(doc.current_context(), Some("kind-dev"));
        assert_eq!(doc.context_refs("kind-dev").0.as_deref(), Some("kind"));
        assert_eq!(doc.context_refs("other").0.as_deref(), Some("kind"));
        assert!(doc.rename(Kind::User, "dev", "aws").is_err());
        assert!(doc.rename(Kind::User, "dev", " x").is_err());
        // Unknown fields survive.
        assert_eq!(doc.0["extensions"][0]["extension"]["a"], 1);
        assert_eq!(doc.body(Kind::User, "dev").unwrap()["as"], "admin");
    }

    #[test]
    fn adds_duplicates_and_removes_entries() {
        let mut doc = Doc::parse(SAMPLE).unwrap();
        assert_eq!(doc.used_by(Kind::Cluster, "dev"), ["dev", "other"]);
        let copy = doc.duplicate(Kind::User, "aws").unwrap();
        assert_eq!(copy, "aws-copy");
        assert_eq!(doc.names(Kind::User), ["dev", "aws", "aws-copy"]);
        assert!(doc.add(Kind::Cluster, "dev", Map::new()).is_err());
        doc.add(Kind::Cluster, "new", Map::new()).unwrap();
        assert_eq!(doc.unique_name(Kind::Cluster, "new"), "new-2");
        doc.remove(Kind::Context, "dev");
        assert_eq!(doc.current_context(), None);
        doc.remove_context_with_refs("other");
        // `dev` user is unused now but only the removed context's refs go.
        assert!(!doc.contains(Kind::User, "aws"));
        assert!(!doc.contains(Kind::Cluster, "dev"));
        assert!(doc.contains(Kind::User, "dev"));
    }

    #[test]
    fn switches_auth_kinds_and_keeps_other_fields() {
        let mut doc = Doc::parse(SAMPLE).unwrap();
        let body = doc.body_mut(Kind::User, "dev").unwrap();
        assert_eq!(AuthKind::detect(body), AuthKind::Token);
        set_auth_kind(body, AuthKind::Exec);
        assert_eq!(AuthKind::detect(body), AuthKind::Exec);
        assert!(!body.contains_key("token"));
        assert_eq!(body["as"], "admin");
        set_str(body, &["exec", "command"], "gke-gcloud-auth-plugin");
        set_bool(body, &["exec", "provideClusterInfo"], true);
        assert_eq!(
            get_str(body, &["exec", "command"]),
            "gke-gcloud-auth-plugin"
        );
        set_str(body, &["exec", "command"], "");
        assert_eq!(get(body, &["exec", "command"]), None);
        set_bool(body, &["exec", "provideClusterInfo"], false);
        assert_eq!(body["exec"]["provideClusterInfo"], false);
    }

    #[test]
    fn finds_masks_and_strips_secrets() {
        let mut doc = Doc::parse(SAMPLE).unwrap();
        assert!(doc.has_inline_credentials());
        let aws = doc.body(Kind::User, "aws").unwrap();
        assert_eq!(secret_paths(aws), [vec!["exec", "env", "1", "value"]]);
        let masked = mask_secrets(&mut doc, "••••");
        assert_eq!(masked.len(), 2);
        assert_eq!(doc.body(Kind::User, "dev").unwrap()["token"], "••••");
        let exported = Doc::parse(SAMPLE).unwrap().extract("other", false).unwrap();
        let env = &exported.body(Kind::User, "aws").unwrap()["exec"]["env"];
        assert_eq!(env.as_array().unwrap().len(), 1);
        assert_eq!(exported.names(Kind::Cluster), ["dev"]);
        assert_eq!(exported.current_context(), Some("other"));
        let with = Doc::parse(SAMPLE).unwrap().extract("dev", true).unwrap();
        assert_eq!(with.body(Kind::User, "dev").unwrap()["token"], "s3cret");
    }

    #[test]
    fn imports_contexts_with_their_entries() {
        let from = Doc::parse(SAMPLE).unwrap();
        let mut into = Doc::parse(SAMPLE).unwrap();
        // Identical cluster and user are shared; the context gets a new name.
        assert_eq!(into.import_context(&from, "dev").unwrap(), "dev-2");
        assert_eq!(into.names(Kind::Cluster), ["dev"]);
        let mut empty = Doc::empty();
        assert_eq!(empty.import_context(&from, "other").unwrap(), "other");
        assert_eq!(empty.names(Kind::User), ["aws"]);
    }

    #[test]
    fn converts_to_kube() {
        let doc = Doc::parse(SAMPLE).unwrap();
        let kube = doc.to_kube().unwrap();
        assert_eq!(kube.contexts.len(), 2);
        let exec = ExecSpec::read(&doc.body(Kind::User, "aws").unwrap()["exec"]).unwrap();
        assert_eq!(exec.command, "aws");
        assert_eq!(exec.env[0], ("AWS_PROFILE".into(), "prod".into()));
        assert_eq!(exec_of(&doc, "other").unwrap().0, "aws");
        let mut changed = exec.clone();
        changed.args.push("--x".into());
        assert_ne!(exec.consent_key(), changed.consent_key());
        assert!(Doc::parse("clusters: 3\n").is_err());
    }
}
