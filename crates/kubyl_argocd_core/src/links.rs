//! Web links for sources: repositories, commits and comparisons on GitHub, GitLab and Bitbucket
//! (github.com/gitlab.com/bitbucket.org and self-hosted hosts with those names), and matching an
//! app's destination to a Kubyl context.

use kubyl_core::ClusterId;

use crate::model::{Destination, is_sha, short_repo};

/// Which web UI a repository has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    GitHub,
    GitLab,
    Bitbucket,
}

/// The repository's web URL and host kind, when it's a Git repository on a known host.
pub fn repo_web(repo_url: &str) -> Option<(String, Host)> {
    let short = short_repo(repo_url);
    let host = short.split('/').next()?.to_lowercase();
    // Helm repositories and OCI registries aren't browsable this way.
    if short.split('/').count() < 3 || repo_url.starts_with("oci://") {
        return None;
    }
    let kind = if host == "github.com" || host.starts_with("github.") {
        Host::GitHub
    } else if host == "gitlab.com" || host.starts_with("gitlab.") {
        Host::GitLab
    } else if host == "bitbucket.org" {
        Host::Bitbucket
    } else {
        return None;
    };
    Some((format!("https://{short}"), kind))
}

/// The page of a commit.
pub fn commit_url(repo_url: &str, revision: &str) -> Option<String> {
    if !is_sha(revision) {
        return None;
    }
    let (web, host) = repo_web(repo_url)?;
    Some(match host {
        Host::GitHub => format!("{web}/commit/{revision}"),
        Host::GitLab => format!("{web}/-/commit/{revision}"),
        Host::Bitbucket => format!("{web}/commits/{revision}"),
    })
}

/// A comparison of two commits (`from` → `to`).
pub fn compare_url(repo_url: &str, from: &str, to: &str) -> Option<String> {
    if !is_sha(from) || !is_sha(to) || from == to {
        return None;
    }
    let (web, host) = repo_web(repo_url)?;
    Some(match host {
        Host::GitHub => format!("{web}/compare/{from}...{to}"),
        Host::GitLab => format!("{web}/-/compare/{from}...{to}"),
        Host::Bitbucket => format!("{web}/branches/compare/{to}%0D{from}"),
    })
}

/// The repository page (or a path in it at a revision).
pub fn tree_url(repo_url: &str, revision: &str, path: Option<&str>) -> Option<String> {
    let (web, host) = repo_web(repo_url)?;
    let path = path.filter(|p| !p.is_empty() && *p != ".");
    Some(match (host, path) {
        (_, None) => web,
        (Host::GitHub, Some(path)) => format!("{web}/tree/{revision}/{path}"),
        (Host::GitLab, Some(path)) => format!("{web}/-/tree/{revision}/{path}"),
        (Host::Bitbucket, Some(path)) => format!("{web}/src/{revision}/{path}"),
    })
}

/// A Kubyl context by its API server URL.
#[derive(Clone, Debug)]
pub struct KnownCluster {
    pub id: ClusterId,
    pub name: String,
    pub server: String,
}

/// The Kubyl cluster an app's destination points at: in-cluster is the app's own cluster;
/// otherwise the context whose API server matches the destination server (or whose context or
/// cluster name matches a named destination).
pub fn destination_cluster(
    destination: &Destination,
    own: &ClusterId,
    known: &[KnownCluster],
) -> Option<ClusterId> {
    if destination.is_in_cluster() {
        return Some(own.clone());
    }
    let normalize = |s: &str| s.trim().trim_end_matches('/').to_lowercase();
    if let Some(server) = destination.server.as_deref().filter(|s| !s.is_empty()) {
        let server = normalize(server);
        return known
            .iter()
            .find(|k| normalize(&k.server) == server)
            .map(|k| k.id.clone());
    }
    let name = destination.name.as_deref()?;
    known.iter().find(|k| k.name == name).map(|k| k.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA_A: &str = "68657670d9131dc5bc5f538b14c1de3377d74591";
    const SHA_B: &str = "8088f4c0d970abb09e250248cc97e35623447cb5";

    #[test]
    fn commit_links_per_host() {
        assert_eq!(
            commit_url("https://github.com/argoproj/argocd-example-apps.git", SHA_A).unwrap(),
            format!("https://github.com/argoproj/argocd-example-apps/commit/{SHA_A}")
        );
        assert_eq!(
            commit_url("git@gitlab.com:group/sub/project.git", SHA_A).unwrap(),
            format!("https://gitlab.com/group/sub/project/-/commit/{SHA_A}")
        );
        assert_eq!(
            commit_url("https://bitbucket.org/ws/repo.git", SHA_A).unwrap(),
            format!("https://bitbucket.org/ws/repo/commits/{SHA_A}")
        );
        assert!(commit_url("https://git.example.com/a/b.git", SHA_A).is_none());
        assert!(commit_url("https://github.com/a/b.git", "main").is_none());
        // Helm repositories have no commits.
        assert!(repo_web("https://charts.jetstack.io").is_none());
        assert!(repo_web("oci://ghcr.io/acme/charts").is_none());
    }

    #[test]
    fn compare_links() {
        assert_eq!(
            compare_url("https://github.com/a/b", SHA_A, SHA_B).unwrap(),
            format!("https://github.com/a/b/compare/{SHA_A}...{SHA_B}")
        );
        assert!(compare_url("https://github.com/a/b", SHA_A, SHA_A).is_none());
        assert_eq!(
            tree_url("https://github.com/a/b.git", "HEAD", Some("guestbook")).unwrap(),
            "https://github.com/a/b/tree/HEAD/guestbook"
        );
        assert_eq!(
            tree_url("https://gitlab.example.com/a/b.git", "main", None).unwrap(),
            "https://gitlab.example.com/a/b"
        );
    }

    #[test]
    fn destinations_map_to_contexts() {
        let own = ClusterId::new("kind-dev@/k");
        let known = vec![
            KnownCluster {
                id: ClusterId::new("prod@/k"),
                name: "prod-us-east-1".into(),
                server: "https://ABC.eks.amazonaws.com/".into(),
            },
            KnownCluster {
                id: own.clone(),
                name: "kind-dev".into(),
                server: "https://127.0.0.1:6443".into(),
            },
        ];
        let in_cluster = Destination {
            server: Some("https://kubernetes.default.svc".into()),
            ..Default::default()
        };
        assert_eq!(
            destination_cluster(&in_cluster, &own, &known),
            Some(own.clone())
        );
        let prod = Destination {
            server: Some("https://abc.eks.amazonaws.com".into()),
            namespace: Some("ingress".into()),
            ..Default::default()
        };
        assert_eq!(
            destination_cluster(&prod, &own, &known),
            Some(ClusterId::new("prod@/k"))
        );
        let named = Destination {
            name: Some("prod-us-east-1".into()),
            ..Default::default()
        };
        assert_eq!(
            destination_cluster(&named, &own, &known),
            Some(ClusterId::new("prod@/k"))
        );
        let unknown = Destination {
            server: Some("https://10.0.0.1".into()),
            ..Default::default()
        };
        assert_eq!(destination_cluster(&unknown, &own, &known), None);
    }
}
