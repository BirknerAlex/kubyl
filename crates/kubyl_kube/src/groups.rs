//! One sidebar entry per cluster and user (context grouping, decided in phase 15).
//!
//! `oc login` writes one `cluster` entry per API server and one `user` entry per user and
//! server; every `oc project <ns>` then adds a context `<ns>/<cluster>/<user>`. Contexts of one
//! file that differ only in their namespace become one entry: the same cluster and user entries
//! (or entries with identical contents) and every other context field equal. Anything else
//! (another user, token, impersonation, proxy, TLS setting, another file) stays separate.
//!
//! Entries are compared by their serialized contents in memory, only while grouping: nothing
//! derived from credentials is kept, logged or written anywhere.
//!
//! Ids: a group is `group:<cluster entry>,<user entry>@<file>/` ([`group_id`]). Context ids
//! (`<context>@<file>`) end with a file path and so never with `/`: the forms can't collide.
//! The id stays the same when `oc` adds members or switches the current context.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kube::config::Kubeconfig;
use kubyl_core::ClusterId;

use crate::kubeconfig::{ContextInfo, Member};

/// Prefix of group ids.
pub const GROUP_PREFIX: &str = "group:";

/// How contexts are grouped (settings `kubernetes.group_contexts`, `separate_groups`).
#[derive(Clone, Debug, Default)]
pub struct GroupOptions {
    pub enabled: bool,
    /// Groups shown as separate contexts ("Show Contexts Separately").
    pub separate: Vec<String>,
}

fn escape(name: &str) -> String {
    name.replace('%', "%25")
        .replace(',', "%2C")
        .replace('@', "%40")
        .replace('#', "%23")
}

fn unescape(name: &str) -> String {
    name.replace("%23", "#")
        .replace("%40", "@")
        .replace("%2C", ",")
        .replace("%25", "%")
}

/// `group:<cluster>,<user>@<file>/`; `n > 1` adds `#n` for groups that share both entry names
/// (their contexts differ in another field).
pub fn group_id(file: &Path, cluster: &str, user: &str, n: usize) -> ClusterId {
    let counter = if n > 1 {
        format!("#{n}")
    } else {
        String::new()
    };
    ClusterId::new(format!(
        "{GROUP_PREFIX}{},{}{counter}@{}/",
        escape(cluster),
        escape(user),
        file.display()
    ))
}

/// The cluster entry, user entry and file of a group id.
pub fn parse_group_id(id: &str) -> Option<(String, String, PathBuf)> {
    let rest = id.strip_prefix(GROUP_PREFIX)?.strip_suffix('/')?;
    let (names, file) = rest.split_once('@')?;
    let names = names.split_once('#').map_or(names, |(names, _)| names);
    let (cluster, user) = names.split_once(',')?;
    Some((unescape(cluster), unescape(user), PathBuf::from(file)))
}

/// What must be equal for two contexts to be one entry. Holds serialized credentials: never
/// `Debug`, never logged, dropped after grouping.
#[derive(PartialEq, Eq, Hash)]
struct Identity {
    file: PathBuf,
    cluster: String,
    user: String,
    context: String,
}

fn identity(info: &ContextInfo, config: &Kubeconfig) -> Option<Identity> {
    if info.error.is_some() {
        return None;
    }
    let cluster = config
        .clusters
        .iter()
        .find(|c| c.name == info.cluster)?
        .cluster
        .as_ref()?;
    let user = info.user.as_ref().and_then(|name| {
        config
            .auth_infos
            .iter()
            .find(|u| &u.name == name)
            .and_then(|u| u.auth_info.as_ref())
    });
    let context = config
        .contexts
        .iter()
        .find(|c| c.name == info.context)?
        .context
        .as_ref()?;
    // Every context field but the namespace and the entry names.
    let mut rest = serde_json::to_value(context).ok()?;
    if let Some(map) = rest.as_object_mut() {
        map.remove("namespace");
        map.remove("cluster");
        map.remove("user");
    }
    Some(Identity {
        file: info.file.clone(),
        cluster: serde_json::to_string(cluster).ok()?,
        user: serde_json::to_string(&user).ok()?,
        context: rest.to_string(),
    })
}

/// `ocp.eu1.example.com` from `https://api.ocp.eu1.example.com:6443`: without `api.` and
/// without the default ports (443, OpenShift's 6443).
pub fn short_host(server: &str) -> Option<String> {
    let url = url::Url::parse(server).ok()?;
    let host = url.host_str()?;
    let host = host.strip_prefix("api.").unwrap_or(host);
    Some(match url.port() {
        Some(443 | 6443) | None => host.to_string(),
        Some(port) => format!("{host}:{port}"),
    })
}

/// The label of an `oc`-style context (`<namespace>/<cluster entry>/<user>` with a user entry
/// `<user>/<cluster entry>`): `ocp.eu1.example.com · jane.doe@example.com`.
pub fn oc_label(
    context: &str,
    cluster: &str,
    user: Option<&str>,
    server: Option<&str>,
) -> Option<String> {
    let bare = user?.strip_suffix(&format!("/{cluster}"))?;
    let namespace = context.strip_suffix(&format!("/{cluster}/{bare}"))?;
    if namespace.is_empty() || namespace.contains('/') || bare.is_empty() {
        return None;
    }
    let host = server
        .and_then(short_host)
        .unwrap_or_else(|| cluster.to_string());
    Some(format!("{host} · {bare}"))
}

/// The default label of an entry: `oc`-style names as [`oc_label`], other groups
/// `<cluster entry> · <user entry>`, other contexts their (merged) name.
pub fn label(info: &ContextInfo) -> String {
    if let Some(label) = oc_label(
        &info.context,
        &info.cluster,
        info.user.as_deref(),
        info.server.as_deref(),
    ) {
        return label;
    }
    if info.is_group() {
        match &info.user {
            Some(user) => format!("{} · {user}", info.cluster),
            None => info.cluster.clone(),
        }
    } else {
        info.name.clone()
    }
}

enum Slot {
    Single(usize),
    Bucket(usize),
}

/// The sidebar entries of `contexts` (every context of every file, in source order): groups of
/// contexts that differ only in their namespace, and the other contexts. `current` names each
/// file's current context (it comes first in a group, and gives the group its namespace).
pub fn entries(
    contexts: &[ContextInfo],
    configs: &HashMap<PathBuf, Arc<Kubeconfig>>,
    options: &GroupOptions,
) -> Vec<ContextInfo> {
    let mut buckets: Vec<Vec<usize>> = Vec::new();
    let mut slots = Vec::new();
    {
        let mut index: HashMap<Identity, usize> = HashMap::new();
        for (ix, info) in contexts.iter().enumerate() {
            let key = configs.get(&info.file).and_then(|c| identity(info, c));
            match key {
                Some(key) => match index.get(&key) {
                    Some(&bucket) => buckets[bucket].push(ix),
                    None => {
                        index.insert(key, buckets.len());
                        slots.push(Slot::Bucket(buckets.len()));
                        buckets.push(vec![ix]);
                    }
                },
                None => slots.push(Slot::Single(ix)),
            }
        }
        // The serialized entries (credentials) go out of scope here.
    }

    let mut ids: HashSet<ClusterId> = HashSet::new();
    let mut out = Vec::new();
    for slot in slots {
        match slot {
            Slot::Single(ix) => out.push(single(&contexts[ix], None)),
            Slot::Bucket(bucket) => {
                let members = &buckets[bucket];
                if members.len() == 1 {
                    out.push(single(&contexts[members[0]], None));
                    continue;
                }
                let first = &contexts[members[0]];
                let min = |f: fn(&ContextInfo) -> String| {
                    members
                        .iter()
                        .map(|&ix| f(&contexts[ix]))
                        .min()
                        .unwrap_or_default()
                };
                let cluster = min(|c| c.cluster.clone());
                let user = min(|c| c.user.clone().unwrap_or_default());
                let mut n = 1;
                let mut id = group_id(&first.file, &cluster, &user, n);
                while !ids.insert(id.clone()) {
                    n += 1;
                    id = group_id(&first.file, &cluster, &user, n);
                }
                if !options.enabled || options.separate.iter().any(|s| s == id.as_str()) {
                    out.extend(
                        members
                            .iter()
                            .map(|&ix| single(&contexts[ix], Some(id.clone()))),
                    );
                    continue;
                }
                let current = configs
                    .get(&first.file)
                    .and_then(|c| c.current_context.clone());
                out.push(group(contexts, members, id, current.as_deref()));
            }
        }
    }
    unique_labels(&mut out);
    out
}

fn single(info: &ContextInfo, group: Option<ClusterId>) -> ContextInfo {
    let mut entry = info.clone();
    entry.name = label(info);
    entry.group = group;
    entry
}

fn group(
    contexts: &[ContextInfo],
    members: &[usize],
    id: ClusterId,
    current: Option<&str>,
) -> ContextInfo {
    let mut by_name: Vec<&ContextInfo> = members.iter().map(|&ix| &contexts[ix]).collect();
    by_name.sort_by(|a, b| a.context.cmp(&b.context));
    let current_member = current.and_then(|c| by_name.iter().find(|m| m.context == c).copied());
    // Settings order: the file's current context first, then by name.
    let mut ordered: Vec<&ContextInfo> = current_member.into_iter().collect();
    ordered.extend(
        by_name
            .iter()
            .copied()
            .filter(|m| Some(m.context.as_str()) != current),
    );
    // Connects with the first member by name (stable while `oc project` switches contexts).
    let mut entry = by_name[0].clone();
    entry.id = id.clone();
    entry.group = Some(id);
    entry.namespace = current_member
        .map(|m| m.namespace.clone())
        .unwrap_or_else(|| by_name[0].namespace.clone());
    entry.members = ordered
        .into_iter()
        .map(|m| Member {
            id: m.id.clone(),
            context: m.context.clone(),
            namespace: m.namespace.clone(),
        })
        .collect();
    // `oc`-style when any member is (a group can mix `oc` contexts with others of the same
    // server and user under other entry names).
    entry.name = by_name
        .iter()
        .find_map(|m| {
            oc_label(
                &m.context,
                &m.cluster,
                m.user.as_deref(),
                m.server.as_deref(),
            )
        })
        .unwrap_or_else(|| label(&entry));
    entry
}

/// Labels must tell entries apart: later duplicates get `@<file-stem>` (and a counter).
fn unique_labels(entries: &mut [ContextInfo]) {
    let mut taken: HashSet<String> = HashSet::new();
    for entry in entries.iter_mut() {
        if taken.insert(entry.name.clone()) {
            continue;
        }
        let stem = entry
            .file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let base = format!("{}@{stem}", entry.name);
        let mut name = base.clone();
        let mut n = 2;
        while !taken.insert(name.clone()) {
            name = format!("{base}-{n}");
            n += 1;
        }
        entry.name = name;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::kubeconfig::{SourceKind, SourceSpec, load_with};

    /// Shaped like a real `oc` kubeconfig (names and servers made up): 37 contexts, 5 cluster
    /// entries and 8 user entries; all contexts are one of 8 cluster + user pairs.
    pub fn oc_kubeconfig() -> String {
        let clusters = [
            (
                "api-ocp-eu1-example-com:6443",
                "https://api.ocp.eu1.example.com:6443",
            ),
            (
                "api-ocp-us1-example-com:6443",
                "https://api.ocp.us1.example.com:6443",
            ),
            (
                "api-ocp-dev-example-com:6443",
                "https://api.ocp-dev.example.com:6443",
            ),
            ("0-0-0-0:55878", "https://0.0.0.0:55878"),
            ("kind-dev", "https://127.0.0.1:52341"),
        ];
        // (user, cluster index, contexts)
        let users = [
            ("jane.doe@example.com", 0, 14),
            ("kube:admin", 0, 6),
            ("jane.doe@example.com", 1, 9),
            ("kube:admin", 1, 2),
            ("jane.doe@example.com", 2, 2),
            ("developer", 2, 1),
            ("system:admin", 3, 2),
            ("kind-dev", 4, 1),
        ];
        let mut yaml = String::from(
            "apiVersion: v1\nkind: Config\ncurrent-context: \"ns-3/api-ocp-eu1-example-com:6443/jane.doe@example.com\"\nclusters:\n",
        );
        for (name, server) in clusters {
            yaml.push_str(&format!(
                "- name: \"{name}\"\n  cluster:\n    server: {server}\n    certificate-authority-data: Zm9v\n"
            ));
        }
        yaml.push_str("users:\n");
        for (ix, (user, cluster, _)) in users.iter().enumerate() {
            let (cluster, _) = clusters[*cluster];
            let entry = if *user == "kind-dev" {
                "kind-dev".to_string()
            } else {
                format!("{user}/{cluster}")
            };
            yaml.push_str(&format!(
                "- name: \"{entry}\"\n  user:\n    token: sha256~token-{ix}\n"
            ));
        }
        yaml.push_str("contexts:\n");
        for (user, cluster, count) in users {
            let (cluster, _) = clusters[cluster];
            for n in 0..count {
                let (name, entry) = if user == "kind-dev" {
                    ("kind-dev".to_string(), "kind-dev".to_string())
                } else {
                    (
                        format!("ns-{n}/{cluster}/{user}"),
                        format!("{user}/{cluster}"),
                    )
                };
                yaml.push_str(&format!(
                    "- name: \"{name}\"\n  context:\n    cluster: \"{cluster}\"\n    user: \"{entry}\"\n    namespace: ns-{n}\n"
                ));
            }
        }
        yaml
    }

    pub fn load_yaml(files: &[(&str, &str)]) -> (tempfile::TempDir, crate::kubeconfig::Loaded) {
        let dir = tempfile::tempdir().unwrap();
        let mut specs = Vec::new();
        for (name, yaml) in files {
            let path = dir.path().join(name);
            std::fs::write(&path, yaml).unwrap();
            specs.push(SourceSpec {
                kind: SourceKind::User,
                path: path.clone(),
                files: vec![path],
                is_dir: false,
            });
        }
        let loaded = load_with(&specs, |p| {
            Kubeconfig::read_from(p).map_err(|e| e.to_string())
        });
        (dir, loaded)
    }

    fn grouped(loaded: &crate::kubeconfig::Loaded) -> Vec<ContextInfo> {
        entries(
            &loaded.contexts,
            &loaded.configs,
            &GroupOptions {
                enabled: true,
                separate: Vec::new(),
            },
        )
    }

    #[test]
    fn thirty_seven_oc_contexts_become_eight_entries() {
        let (_dir, loaded) = load_yaml(&[("config", &oc_kubeconfig())]);
        assert_eq!(loaded.contexts.len(), 37);
        let entries = grouped(&loaded);
        let labels: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            labels,
            [
                "ocp.eu1.example.com · jane.doe@example.com",
                "ocp.eu1.example.com · kube:admin",
                "ocp.us1.example.com · jane.doe@example.com",
                "ocp.us1.example.com · kube:admin",
                "ocp-dev.example.com · jane.doe@example.com",
                "ocp-dev.example.com · developer",
                "0.0.0.0:55878 · system:admin",
                "kind-dev",
            ]
        );
        let sizes: Vec<usize> = entries.iter().map(|e| e.members.len()).collect();
        assert_eq!(sizes, [14, 6, 9, 2, 2, 1, 2, 1]);
        // The personal user and kube:admin on one server are separate entries.
        assert_ne!(entries[0].id, entries[1].id);
        // The file's current context comes first and gives the group its namespace.
        assert_eq!(
            entries[0].members[0].context,
            "ns-3/api-ocp-eu1-example-com:6443/jane.doe@example.com"
        );
        assert_eq!(entries[0].namespace.as_deref(), Some("ns-3"));
        // It connects with its first member by name.
        assert_eq!(
            entries[0].context,
            "ns-0/api-ocp-eu1-example-com:6443/jane.doe@example.com"
        );
        // Contexts without siblings keep their id; groups get a group id.
        assert!(entries[5].id.as_str().starts_with("ns-0/"));
        assert!(entries[0].id.as_str().starts_with(GROUP_PREFIX));
        assert!(entries[0].id.as_str().ends_with("/config/"));
    }

    #[test]
    fn group_ids_round_trip_and_never_look_like_context_ids() {
        let id = group_id(
            Path::new("/home/me/.kube/config"),
            "api-x:6443",
            "jane@example.com/api-x:6443,a#b%",
            1,
        );
        let (cluster, user, file) = parse_group_id(id.as_str()).unwrap();
        assert_eq!(cluster, "api-x:6443");
        assert_eq!(user, "jane@example.com/api-x:6443,a#b%");
        assert_eq!(file, Path::new("/home/me/.kube/config"));
        assert!(id.as_str().ends_with('/'));
        let second = group_id(Path::new("/k"), "c", "u", 2);
        assert_eq!(parse_group_id(second.as_str()).unwrap().0, "c");
        assert_ne!(second, group_id(Path::new("/k"), "c", "u", 1));
        // A context named like a group id still gets `@<file>` and no trailing slash.
        let context = ContextInfo::make_id("group:c,u@/k/", Path::new("/k"));
        assert!(!context.as_str().ends_with('/'));
        assert!(parse_group_id("kind-dev@/k").is_none());
    }

    #[test]
    fn entries_with_identical_contents_group_near_misses_do_not() {
        let yaml = r#"
apiVersion: v1
kind: Config
clusters:
- name: a
  cluster: {server: "https://k.example.com", certificate-authority-data: Zm9v}
- name: b
  cluster: {server: "https://k.example.com", certificate-authority-data: Zm9v}
- name: proxied
  cluster: {server: "https://k.example.com", certificate-authority-data: Zm9v, proxy-url: "http://proxy.example.com:3128"}
users:
- name: u1
  user: {token: s3cr3t-one}
- name: u2
  user: {token: s3cr3t-one}
- name: other-token
  user: {token: s3cr3t-two}
- name: impersonating
  user: {token: s3cr3t-one, as: admin}
contexts:
- {name: one, context: {cluster: a, user: u1, namespace: x}}
- {name: two, context: {cluster: b, user: u2, namespace: y}}
- {name: three, context: {cluster: a, user: u1}}
- {name: token, context: {cluster: a, user: other-token, namespace: x}}
- {name: as, context: {cluster: a, user: impersonating, namespace: x}}
- {name: proxy, context: {cluster: proxied, user: u1, namespace: x}}
"#;
        let (_dir, loaded) = load_yaml(&[("dev.yaml", yaml), ("copy.yaml", yaml)]);
        let entries = grouped(&loaded);
        // Per file: {one, two, three} group; token, as and proxy stay separate. Files never
        // group with each other.
        assert_eq!(entries.len(), 8);
        let group = &entries[0];
        let names: Vec<&str> = group.member_names().collect();
        assert_eq!(names, ["one", "three", "two"]);
        assert_eq!(group.name, "a · u1");
        assert_eq!(
            entries[1..4]
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["token", "as", "proxy"]
        );
        assert_ne!(entries[0].id, entries[4].id);
        assert_eq!(entries[4].name, "a · u1@copy");
        // Nothing of the credentials shows up in what's kept.
        let debug = format!("{entries:?}");
        assert!(!debug.contains("s3cr3t"), "{debug}");
    }

    #[test]
    fn ids_stay_stable_when_oc_adds_contexts_and_switches() {
        let yaml = oc_kubeconfig();
        let (_dir, before) = load_yaml(&[("config", &yaml)]);
        let before = grouped(&before);
        // `oc project new-ns`: a new context, and it becomes the current one.
        let changed = yaml.replace(
            "current-context: \"ns-3/api-ocp-eu1-example-com:6443/jane.doe@example.com\"",
            "current-context: \"new-ns/api-ocp-eu1-example-com:6443/jane.doe@example.com\"",
        ) + "- name: \"new-ns/api-ocp-eu1-example-com:6443/jane.doe@example.com\"\n  context:\n    cluster: \"api-ocp-eu1-example-com:6443\"\n    user: \"jane.doe@example.com/api-ocp-eu1-example-com:6443\"\n    namespace: new-ns\n";
        let (_dir2, after) = load_yaml(&[("config", &changed)]);
        let after = grouped(&after);
        assert_eq!(after.len(), 8);
        // Same file name in another temp dir: compare the id without the directory.
        let strip = |id: &ClusterId| {
            let (c, u, f) = parse_group_id(id.as_str()).unwrap();
            (c, u, f.file_name().unwrap().to_owned())
        };
        assert_eq!(strip(&before[0].id), strip(&after[0].id));
        assert_eq!(after[0].members.len(), 15);
        assert_eq!(after[0].namespace.as_deref(), Some("new-ns"));
        // What other crates key things by (web view stores) didn't change either; the
        // context it connects with may (any member works: they differ only in the namespace).
        assert_eq!(before[0].stable_key(), after[0].stable_key());
    }

    #[test]
    fn grouping_can_be_turned_off() {
        let (_dir, loaded) = load_yaml(&[("config", &oc_kubeconfig())]);
        let all = entries(
            &loaded.contexts,
            &loaded.configs,
            &GroupOptions {
                enabled: false,
                separate: Vec::new(),
            },
        );
        assert_eq!(all.len(), 37);
        // Each context remembers its group (for "Show as One Cluster").
        assert!(all[0].group.is_some());
        assert_eq!(all.last().unwrap().group, None);
        // Only one group shown separately.
        let grouped_ = grouped(&loaded);
        let separate = grouped_[1].id.to_string();
        let some = entries(
            &loaded.contexts,
            &loaded.configs,
            &GroupOptions {
                enabled: true,
                separate: vec![separate.clone()],
            },
        );
        assert_eq!(some.len(), 8 - 1 + 6);
        assert!(
            some.iter()
                .filter(|e| e.group.as_ref().map(|g| g.as_str()) == Some(separate.as_str()))
                .all(|e| !e.is_group())
        );
    }

    #[test]
    fn labels() {
        assert_eq!(
            oc_label(
                "shop/api-ocp-eu1-example-com:6443/kube:admin",
                "api-ocp-eu1-example-com:6443",
                Some("kube:admin/api-ocp-eu1-example-com:6443"),
                Some("https://api.ocp.eu1.example.com:6443"),
            )
            .as_deref(),
            Some("ocp.eu1.example.com · kube:admin")
        );
        assert_eq!(oc_label("prod", "prod", Some("aws"), None), None);
        assert_eq!(
            short_host("https://api.example.com").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            short_host("https://0.0.0.0:55878").as_deref(),
            Some("0.0.0.0:55878")
        );
    }
}
