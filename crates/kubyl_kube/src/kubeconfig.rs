//! Kubeconfig sources: which files to read, reading them, and merging their contexts.
//!
//! Everything here is plain blocking code without GPUI, so it runs on a background thread and
//! is easy to test. [`crate::ConnectionManager`] calls [`load`] whenever a source changes.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kube::config::{Kubeconfig, NamedAuthInfo, NamedCluster};
use kubyl_core::ClusterId;

use crate::auth::AuthMethod;
use crate::settings::{KubeSettings, expand_home};

/// Where a source comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceKind {
    /// `~/.kube/config`.
    Default,
    /// The files in `$KUBECONFIG`, merged.
    Env,
    /// A file or folder the user added.
    User,
    /// Kubeconfigs pasted into Kubyl (`<config dir>/kubeconfigs/`).
    Pasted,
}

/// A configured source before reading it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceSpec {
    pub kind: SourceKind,
    /// The file or folder. For [`SourceKind::Env`], the first file.
    pub path: PathBuf,
    /// The files to read. A folder lists its files when loading.
    pub files: Vec<PathBuf>,
    pub is_dir: bool,
}

impl SourceSpec {
    /// The label of the source row: `~/.kube/config`, `$KUBECONFIG`…
    pub fn label(&self) -> String {
        match self.kind {
            SourceKind::Env => "$KUBECONFIG".into(),
            SourceKind::Pasted => "Pasted kubeconfigs".into(),
            _ => crate::settings::display_path(&self.path),
        }
    }

    /// Paths to watch for changes. Folders are watched themselves so new files show up.
    pub fn watch_paths(&self) -> Vec<PathBuf> {
        if self.is_dir {
            vec![self.path.clone()]
        } else {
            self.files.clone()
        }
    }
}

/// One file of a source after reading it.
#[derive(Clone, Debug)]
pub struct SourceFile {
    pub path: PathBuf,
    pub contexts: usize,
    /// The file couldn't be read or parsed. Shown on the source row.
    pub error: Option<String>,
    /// At least one context signs in with OIDC.
    pub oidc: bool,
}

/// A source after reading it.
#[derive(Clone, Debug)]
pub struct Source {
    pub spec: SourceSpec,
    pub files: Vec<SourceFile>,
    /// The file or folder is missing (only reported for user-added sources).
    pub error: Option<String>,
}

impl Source {
    pub fn context_count(&self) -> usize {
        self.files.iter().map(|f| f.contexts).sum()
    }

    pub fn errors(&self) -> impl Iterator<Item = &str> {
        self.error
            .as_deref()
            .into_iter()
            .chain(self.files.iter().filter_map(|f| f.error.as_deref()))
    }

    pub fn has_oidc(&self) -> bool {
        self.files.iter().any(|f| f.oidc)
    }
}

/// Where the cluster CA comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaSource {
    /// The system trust store.
    System,
    File(PathBuf),
    Inline,
}

/// A context from one of the kubeconfig files, with what the UI needs to show it.
///
/// Contains no credentials: tokens and keys stay in the parsed [`Kubeconfig`].
#[derive(Clone, Debug)]
pub struct ContextInfo {
    pub id: ClusterId,
    /// Unique name across all sources: the context name, plus `@<file-stem>` on a collision.
    pub name: String,
    /// The context name inside its kubeconfig.
    pub context: String,
    /// The kubeconfig file that defines the context.
    pub file: PathBuf,
    /// The source (file, folder or `$KUBECONFIG`) the file belongs to.
    pub source: SourceKind,
    pub source_path: PathBuf,
    pub cluster: String,
    pub user: Option<String>,
    pub server: Option<String>,
    /// The kubeconfig's namespace for this context.
    pub namespace: Option<String>,
    pub auth: AuthMethod,
    pub insecure_skip_tls_verify: bool,
    pub proxy_url: Option<String>,
    pub tls_server_name: Option<String>,
    pub ca: CaSource,
    /// The context references a missing cluster or user.
    pub error: Option<String>,
}

impl ContextInfo {
    /// `<context>@<file>`: stable as long as the file and context name don't change.
    pub fn make_id(context: &str, file: &Path) -> ClusterId {
        ClusterId::new(format!("{context}@{}", file.display()))
    }
}

/// The result of reading every source.
#[derive(Clone, Default)]
pub struct Loaded {
    pub sources: Vec<Source>,
    pub contexts: Vec<ContextInfo>,
    /// Parsed files by path, used to build clients.
    pub configs: HashMap<PathBuf, Arc<Kubeconfig>>,
}

/// The sources to read, in order: `$KUBECONFIG`, `~/.kube/config`, user-added, pasted.
pub fn source_specs(
    settings: &KubeSettings,
    kubeconfig_env: Option<OsString>,
    home: Option<PathBuf>,
    pasted_dir: &Path,
) -> Vec<SourceSpec> {
    let mut specs = Vec::new();
    if settings.load_kubeconfig_env
        && let Some(value) = kubeconfig_env
    {
        let files: Vec<PathBuf> = std::env::split_paths(&value)
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
        if let Some(first) = files.first() {
            specs.push(SourceSpec {
                kind: SourceKind::Env,
                path: first.clone(),
                files,
                is_dir: false,
            });
        }
    }
    if settings.load_default_kubeconfig
        && let Some(home) = home
    {
        let path = home.join(".kube").join("config");
        specs.push(SourceSpec {
            kind: SourceKind::Default,
            files: vec![path.clone()],
            path,
            is_dir: false,
        });
    }
    for path in &settings.kubeconfigs {
        let path = expand_home(path);
        let is_dir = path.is_dir();
        specs.push(SourceSpec {
            kind: SourceKind::User,
            files: if is_dir {
                Vec::new()
            } else {
                vec![path.clone()]
            },
            path,
            is_dir,
        });
    }
    specs.push(SourceSpec {
        kind: SourceKind::Pasted,
        path: pasted_dir.to_path_buf(),
        files: Vec::new(),
        is_dir: true,
    });
    specs
}

/// Files of a folder source: regular, non-hidden files, sorted.
fn folder_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && !path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .collect();
    files.sort();
    Ok(files)
}

/// Reads every source and merges their contexts. Blocking: call it off the UI thread.
pub fn load(specs: &[SourceSpec]) -> Loaded {
    load_with(specs, |path| {
        Kubeconfig::read_from(path).map_err(|err| describe_error(&err))
    })
}

fn describe_error(err: &kube::config::KubeconfigError) -> String {
    use std::error::Error as _;
    // `ReadConfig` wraps the IO error; show both parts without the file path twice.
    match err.source() {
        Some(source) => format!("{err}: {source}"),
        None => err.to_string(),
    }
}

/// [`load`] with an injectable reader, for tests.
pub fn load_with(
    specs: &[SourceSpec],
    read: impl Fn(&Path) -> Result<Kubeconfig, String>,
) -> Loaded {
    let mut loaded = Loaded::default();
    let mut seen_files = HashSet::new();
    // (spec index, file, parsed) in merge order.
    let mut parsed: Vec<(usize, PathBuf, Arc<Kubeconfig>)> = Vec::new();

    for (index, spec) in specs.iter().enumerate() {
        let mut source = Source {
            spec: spec.clone(),
            files: Vec::new(),
            error: None,
        };
        let files = if spec.is_dir {
            match folder_files(&spec.path) {
                Ok(files) => files,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    if spec.kind == SourceKind::User {
                        source.error = Some("folder not found".into());
                    }
                    Vec::new()
                }
                Err(err) => {
                    source.error = Some(err.to_string());
                    Vec::new()
                }
            }
        } else {
            spec.files.clone()
        };
        source.spec.files = files.clone();

        for file in files {
            // A file listed by two sources (e.g. `$KUBECONFIG` and `~/.kube/config`) belongs
            // to the first one.
            let key = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
            if !seen_files.insert(key) {
                continue;
            }
            if !file.exists() {
                if spec.kind == SourceKind::User || spec.kind == SourceKind::Env {
                    source.files.push(SourceFile {
                        path: file,
                        contexts: 0,
                        error: Some("file not found".into()),
                        oidc: false,
                    });
                }
                continue;
            }
            match read(&file) {
                Ok(config) => {
                    let config = Arc::new(config);
                    let oidc = config.contexts.iter().any(|c| {
                        user_of(&config, c.context.as_ref().and_then(|c| c.user.as_deref()))
                            .map(AuthMethod::detect)
                            .is_some_and(|auth| auth.is_oidc())
                    });
                    source.files.push(SourceFile {
                        path: file.clone(),
                        contexts: config.contexts.len(),
                        error: None,
                        oidc,
                    });
                    loaded.configs.insert(file.clone(), config.clone());
                    parsed.push((index, file, config));
                }
                Err(error) => source.files.push(SourceFile {
                    path: file,
                    contexts: 0,
                    error: Some(error),
                    oidc: false,
                }),
            }
        }
        loaded.sources.push(source);
    }

    loaded.contexts = merge_contexts(specs, &parsed);
    loaded
}

fn user_of<'a>(config: &'a Kubeconfig, user: Option<&str>) -> Option<&'a kube::config::AuthInfo> {
    let user = user?;
    config
        .auth_infos
        .iter()
        .find(|a: &&NamedAuthInfo| a.name == user)
        .and_then(|a| a.auth_info.as_ref())
}

fn cluster_of<'a>(config: &'a Kubeconfig, name: &str) -> Option<&'a kube::config::Cluster> {
    config
        .clusters
        .iter()
        .find(|c: &&NamedCluster| c.name == name)
        .and_then(|c| c.cluster.as_ref())
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Contexts from all files in one list. The first context with a name keeps it; later ones get
/// `@<file-stem>` (and a counter if that is taken too).
fn merge_contexts(
    specs: &[SourceSpec],
    parsed: &[(usize, PathBuf, Arc<Kubeconfig>)],
) -> Vec<ContextInfo> {
    let mut taken = HashSet::new();
    let mut contexts = Vec::new();
    for (index, file, config) in parsed {
        let spec = &specs[*index];
        for named in &config.contexts {
            let Some(context) = &named.context else {
                continue;
            };
            let mut name = named.name.clone();
            if taken.contains(&name) {
                let base = format!("{}@{}", named.name, file_stem(file));
                name = base.clone();
                let mut n = 2;
                while taken.contains(&name) {
                    name = format!("{base}-{n}");
                    n += 1;
                }
            }
            taken.insert(name.clone());

            contexts.push(build_info(
                config,
                &named.name,
                context,
                name,
                file,
                spec.kind,
                &spec.path,
            ));
        }
    }
    contexts
}

/// The [`ContextInfo`] of one context of a parsed kubeconfig that isn't loaded as a source:
/// the kubeconfig editor tests unsaved edits with it. `file` names the kubeconfig (it doesn't
/// have to exist); the context's merged name is its own name.
pub fn context_info(config: &Kubeconfig, context: &str, file: &Path) -> Option<ContextInfo> {
    let named = config.contexts.iter().find(|c| c.name == context)?;
    let body = named.context.as_ref()?;
    Some(build_info(
        config,
        &named.name,
        body,
        named.name.clone(),
        file,
        SourceKind::User,
        file,
    ))
}

fn build_info(
    config: &Kubeconfig,
    context_name: &str,
    context: &kube::config::Context,
    name: String,
    file: &Path,
    source: SourceKind,
    source_path: &Path,
) -> ContextInfo {
    let cluster = cluster_of(config, &context.cluster);
    let user = user_of(config, context.user.as_deref());
    let mut error = None;
    if cluster.is_none() {
        error = Some(format!("cluster \"{}\" not found", context.cluster));
    } else if context.user.is_some() && user.is_none() {
        error = Some(format!(
            "user \"{}\" not found",
            context.user.as_deref().unwrap_or_default()
        ));
    }
    let ca = match cluster {
        Some(c) if c.certificate_authority_data.is_some() => CaSource::Inline,
        Some(c) => match &c.certificate_authority {
            Some(path) => CaSource::File(PathBuf::from(path)),
            None => CaSource::System,
        },
        None => CaSource::System,
    };
    ContextInfo {
        id: ContextInfo::make_id(context_name, file),
        name,
        context: context_name.to_string(),
        file: file.to_path_buf(),
        source,
        source_path: source_path.to_path_buf(),
        cluster: context.cluster.clone(),
        user: context.user.clone(),
        server: cluster.and_then(|c| c.server.clone()),
        namespace: context.namespace.clone(),
        auth: user.map(AuthMethod::detect).unwrap_or(AuthMethod::None),
        insecure_skip_tls_verify: cluster
            .and_then(|c| c.insecure_skip_tls_verify)
            .unwrap_or(false),
        proxy_url: cluster.and_then(|c| c.proxy_url.clone()),
        tls_server_name: cluster.and_then(|c| c.tls_server_name.clone()),
        ca,
        error,
    }
}

/// Validates pasted kubeconfig YAML and returns its context names.
pub fn validate_yaml(yaml: &str) -> Result<Vec<String>, String> {
    let config = Kubeconfig::from_yaml(yaml).map_err(|err| describe_error(&err))?;
    if config.contexts.is_empty() {
        return Err("the kubeconfig has no contexts".into());
    }
    Ok(config.contexts.into_iter().map(|c| c.name).collect())
}

/// An exec credential plugin in pasted YAML, for the user to check before it's added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecCommand {
    /// The users running it.
    pub users: Vec<String>,
    /// `command arg…`.
    pub command: String,
    /// `NAME=value`, secret-looking values masked.
    pub env: Vec<String>,
}

/// The exec plugins of kubeconfig YAML (one entry per distinct command line and env).
pub fn exec_commands(yaml: &str) -> Vec<ExecCommand> {
    let Ok(config) = Kubeconfig::from_yaml(yaml) else {
        return Vec::new();
    };
    let mut found: Vec<ExecCommand> = Vec::new();
    for user in config.auth_infos {
        let Some(exec) = user.auth_info.and_then(|a| a.exec) else {
            continue;
        };
        let mut command = exec.command.unwrap_or_default();
        for arg in exec.args.unwrap_or_default() {
            command.push(' ');
            command.push_str(&arg);
        }
        let env = exec
            .env
            .unwrap_or_default()
            .into_iter()
            .map(|var| {
                let name = var.get("name").cloned().unwrap_or_default();
                let value = var.get("value").cloned().unwrap_or_default();
                let secret = [
                    "SECRET",
                    "TOKEN",
                    "PASSWORD",
                    "PASSWD",
                    "CREDENTIAL",
                    "PRIVATE",
                    "KEY",
                ]
                .iter()
                .any(|s| name.to_ascii_uppercase().contains(s));
                format!("{name}={}", if secret { "••••" } else { &value })
            })
            .collect::<Vec<_>>();
        match found
            .iter_mut()
            .find(|c| c.command == command && c.env == env)
        {
            Some(existing) => existing.users.push(user.name),
            None => found.push(ExecCommand {
                users: vec![user.name],
                command,
                env,
            }),
        }
    }
    found
}

/// Saves pasted kubeconfig YAML as `<dir>/<name>.yaml`, readable only by the user.
/// Returns the path. Doesn't overwrite: a taken name gets a counter.
pub fn save_pasted(dir: &Path, name: &str, yaml: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stem: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let stem = stem.trim_matches(['-', '.']);
    let stem = if stem.is_empty() { "kubeconfig" } else { stem };
    let mut path = dir.join(format!("{stem}.yaml"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem}-{n}.yaml"));
        n += 1;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    use std::io::Write as _;
    let mut file = options.open(&path)?;
    file.write_all(yaml.as_bytes())?;
    file.sync_all()?;
    Ok(path)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const DEV: &str = r#"
apiVersion: v1
kind: Config
current-context: kind-dev
clusters:
  - name: kind-dev
    cluster:
      server: https://127.0.0.1:52341
      certificate-authority-data: Zm9v
  - name: prod
    cluster:
      server: https://prod.example.com
contexts:
  - name: kind-dev
    context: { cluster: kind-dev, user: kind-dev, namespace: payments }
  - name: prod
    context: { cluster: prod, user: aws }
users:
  - name: kind-dev
    user:
      client-certificate-data: Zm9v
      client-key-data: Zm9v
  - name: aws
    user:
      exec:
        apiVersion: client.authentication.k8s.io/v1beta1
        command: aws
        args: [eks, get-token, --cluster-name, prod, --output, json]
"#;

    pub const OTHER: &str = r#"
apiVersion: v1
kind: Config
clusters:
  - name: prod
    cluster: { server: "https://other.example.com", insecure-skip-tls-verify: true }
contexts:
  - name: prod
    context: { cluster: prod, user: oidc }
  - name: broken
    context: { cluster: missing }
users:
  - name: oidc
    user:
      auth-provider:
        name: oidc
        config:
          idp-issuer-url: https://sso.example.com/realms/platform
          client-id: kubernetes
"#;

    fn file_spec(kind: SourceKind, path: &str) -> SourceSpec {
        SourceSpec {
            kind,
            path: PathBuf::from(path),
            files: vec![PathBuf::from(path)],
            is_dir: false,
        }
    }

    /// Parses the fixtures by file name, so paths work on every OS.
    fn fake_read(path: &Path) -> Result<Kubeconfig, String> {
        match path.file_name().and_then(|n| n.to_str()) {
            Some("dev.yaml") => Kubeconfig::from_yaml(DEV).map_err(|e| e.to_string()),
            Some("other.yaml") => Kubeconfig::from_yaml(OTHER).map_err(|e| e.to_string()),
            _ => Err("not a kubeconfig".into()),
        }
    }

    /// `load_with` skips files that don't exist, so tests use real (empty) files.
    fn specs_in(dir: &Path) -> Vec<SourceSpec> {
        for name in ["dev.yaml", "other.yaml", "bad.yaml"] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        let p = |n: &str| dir.join(n).to_string_lossy().into_owned();
        vec![
            file_spec(SourceKind::Default, &p("dev.yaml")),
            file_spec(SourceKind::User, &p("other.yaml")),
            file_spec(SourceKind::User, &p("bad.yaml")),
        ]
    }

    fn read_in(_dir: PathBuf) -> impl Fn(&Path) -> Result<Kubeconfig, String> {
        fake_read
    }

    #[test]
    fn merges_contexts_and_suffixes_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load_with(&specs_in(dir.path()), read_in(dir.path().to_path_buf()));
        let names: Vec<_> = loaded.contexts.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["kind-dev", "prod", "prod@other", "broken"]);

        let dev = &loaded.contexts[0];
        assert_eq!(dev.namespace.as_deref(), Some("payments"));
        assert_eq!(dev.auth, AuthMethod::ClientCertificate);
        assert_eq!(dev.ca, CaSource::Inline);
        assert_eq!(dev.source, SourceKind::Default);

        let prod = &loaded.contexts[1];
        assert_eq!(prod.auth.label(), "exec · aws eks get-token");
        let other = &loaded.contexts[2];
        assert_eq!(other.context, "prod");
        assert!(other.insecure_skip_tls_verify);
        assert!(other.auth.is_oidc());
        assert_ne!(prod.id, other.id);
        assert!(loaded.contexts[3].error.is_some());
    }

    #[test]
    fn reports_parse_errors_on_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load_with(&specs_in(dir.path()), read_in(dir.path().to_path_buf()));
        assert_eq!(loaded.sources.len(), 3);
        assert_eq!(loaded.sources[0].context_count(), 2);
        assert!(!loaded.sources[0].has_oidc());
        assert!(loaded.sources[1].has_oidc());
        assert_eq!(
            loaded.sources[2].errors().collect::<Vec<_>>(),
            ["not a kubeconfig"]
        );
    }

    #[test]
    fn a_file_in_two_sources_is_read_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut specs = specs_in(dir.path());
        specs.push(specs[0].clone());
        let loaded = load_with(&specs, read_in(dir.path().to_path_buf()));
        assert_eq!(loaded.contexts.len(), 4);
        assert!(loaded.sources[3].files.is_empty());
    }

    #[test]
    fn reads_real_files_and_folders() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("configs");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("dev.yaml"), DEV).unwrap();
        std::fs::write(folder.join(".hidden"), "x").unwrap();
        let settings = KubeSettings {
            load_default_kubeconfig: false,
            kubeconfigs: vec![folder.to_string_lossy().into_owned()],
            ..Default::default()
        };
        let pasted = dir.path().join("pasted");
        let specs = source_specs(&settings, None, None, &pasted);
        assert_eq!(specs.len(), 2);
        assert!(specs[0].is_dir);

        let path = save_pasted(&pasted, "other cluster", OTHER).unwrap();
        assert_eq!(path.file_name().unwrap(), "other-cluster.yaml");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let loaded = load(&specs);
        let names: Vec<_> = loaded.contexts.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["kind-dev", "prod", "prod@other-cluster", "broken"]);
        assert_eq!(loaded.sources[0].files.len(), 1);
        assert_eq!(loaded.sources[1].spec.kind, SourceKind::Pasted);
    }

    #[test]
    fn env_source_lists_every_file() {
        let settings = KubeSettings::default();
        let env = std::env::join_paths(["/a/one", "/a/two"]).unwrap();
        let specs = source_specs(
            &settings,
            Some(env),
            Some("/home/me".into()),
            Path::new("/p"),
        );
        assert_eq!(specs[0].kind, SourceKind::Env);
        assert_eq!(specs[0].files.len(), 2);
        assert_eq!(specs[0].label(), "$KUBECONFIG");
        assert_eq!(specs[1].kind, SourceKind::Default);
    }

    #[test]
    fn describes_contexts_of_unloaded_kubeconfigs() {
        let config = Kubeconfig::from_yaml(OTHER).unwrap();
        let info = context_info(&config, "prod", Path::new("/tmp/new.yaml")).unwrap();
        assert!(info.auth.is_oidc());
        assert!(info.insecure_skip_tls_verify);
        assert_eq!(info.id.as_str(), "prod@/tmp/new.yaml");
        let broken = context_info(&config, "broken", Path::new("/tmp/new.yaml")).unwrap();
        assert!(broken.error.is_some());
        assert!(context_info(&config, "nope", Path::new("/x")).is_none());
    }

    #[test]
    fn lists_exec_commands_of_pasted_yaml() {
        let yaml = "apiVersion: v1\nkind: Config\nusers:\n- name: a\n  user:\n    exec:\n      apiVersion: client.authentication.k8s.io/v1\n      command: aws\n      args: [eks, get-token]\n      env: [{name: AWS_PROFILE, value: prod}, {name: SSO_TOKEN, value: hunter2}]\n- name: b\n  user:\n    exec:\n      apiVersion: client.authentication.k8s.io/v1\n      command: aws\n      args: [eks, get-token]\n      env: [{name: AWS_PROFILE, value: prod}, {name: SSO_TOKEN, value: hunter2}]\n- name: c\n  user:\n    token: abc\n";
        let commands = exec_commands(yaml);
        assert_eq!(
            commands,
            vec![ExecCommand {
                users: vec!["a".into(), "b".into()],
                command: "aws eks get-token".into(),
                env: vec!["AWS_PROFILE=prod".into(), "SSO_TOKEN=••••".into()],
            }]
        );
        assert!(exec_commands("users: []").is_empty());
    }

    #[test]
    fn validates_pasted_yaml() {
        assert_eq!(validate_yaml(DEV).unwrap(), ["kind-dev", "prod"]);
        assert!(validate_yaml("apiVersion: v1\nkind: Config\n").is_err());
        assert!(validate_yaml(": not yaml [").is_err());
    }
}
