//! URLs as Kubyl shows them: credentials stripped (`https://user:token@host` and query strings
//! of webhook addresses never reach the screen), and commit links for GitHub, GitLab and
//! Bitbucket repositories.

use url::Url;

/// `url` without its password (and without a user name other than `git`), with query values
/// replaced by `…`. Text that isn't a URL (`git@github.com:org/repo`) keeps only a `git` user.
pub fn strip_credentials(text: &str) -> String {
    let Ok(mut url) = Url::parse(text) else {
        return strip_scp_user(text);
    };
    if url.cannot_be_a_base() {
        return strip_scp_user(text);
    }
    let _ = url.set_password(None);
    if url.username() != "git" {
        let _ = url.set_username("");
    }
    if url.query().is_some() {
        let names: Vec<String> = url
            .query_pairs()
            .map(|(name, _)| format!("{name}=…"))
            .collect();
        url.set_query(None);
        let mut shown = url.to_string();
        shown.push('?');
        shown.push_str(&names.join("&"));
        return shown;
    }
    url.to_string()
}

/// SCP-like text (`user:token@host:path`, which `Url` reads as a scheme): without its user
/// info unless that's just `git`.
fn strip_scp_user(text: &str) -> String {
    match text.split_once('@') {
        Some((user, rest)) if !user.contains('/') && user != "git" => rest.to_string(),
        _ => text.to_string(),
    }
}

/// `github.com/stefanprodan/podinfo` for a repository URL (HTTPS, SSH or SCP-like).
pub fn short_repo(url: &str) -> String {
    let text = url
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string();
    let rest = text.split_once("://").map_or(text.as_str(), |(_, r)| r);
    // Drop user info (`git@`, `user:token@`).
    let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
    // `github.com:org/repo` (SCP-like) → `github.com/org/repo`.
    let rest = match rest.split_once(':') {
        Some((host, path)) if !path.starts_with(|c: char| c.is_ascii_digit()) => {
            format!("{host}/{path}")
        }
        Some((host, port_path)) => {
            // `host:22/org/repo` → drop the port.
            let path = port_path.split_once('/').map_or("", |(_, p)| p);
            format!("{host}/{path}")
        }
        None => rest.to_string(),
    };
    rest.trim_end_matches('/').to_string()
}

/// The web page of a commit in `repo_url`, for GitHub, GitLab and Bitbucket (also self-hosted
/// `github.*`/`gitlab.*` hosts).
pub fn commit_url(repo_url: &str, sha: &str) -> Option<String> {
    if repo_url.starts_with("oci://")
        || sha.len() < 7
        || !sha.chars().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }
    let short = short_repo(repo_url);
    let host = short.split('/').next()?.to_lowercase();
    if short.split('/').count() < 3 {
        return None;
    }
    let web = format!("https://{short}");
    if host == "github.com" || host.starts_with("github.") {
        Some(format!("{web}/commit/{sha}"))
    } else if host == "gitlab.com" || host.starts_with("gitlab.") {
        Some(format!("{web}/-/commit/{sha}"))
    } else if host == "bitbucket.org" {
        Some(format!("{web}/commits/{sha}"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_never_shown() {
        assert_eq!(
            strip_credentials("https://deploy:hunter2@gitlab.example.com/platform/infra.git"),
            "https://gitlab.example.com/platform/infra.git"
        );
        assert_eq!(
            strip_credentials("ssh://git@github.com/acme/infra"),
            "ssh://git@github.com/acme/infra"
        );
        assert_eq!(
            strip_credentials("http://user:pass@alerts.flux-demo.invalid/hook?token=abc&x=1"),
            "http://alerts.flux-demo.invalid/hook?token=…&x=…"
        );
        assert_eq!(
            strip_credentials("git@github.com:acme/infra.git"),
            "git@github.com:acme/infra.git"
        );
        assert_eq!(
            strip_credentials("deploy:hunter2@gitlab.example.com:acme/infra.git"),
            "gitlab.example.com:acme/infra.git"
        );
        assert_eq!(
            strip_credentials("bucket.example.com:9000"),
            "bucket.example.com:9000"
        );
        assert_eq!(
            strip_credentials("oci://ghcr.io/stefanprodan/manifests/podinfo"),
            "oci://ghcr.io/stefanprodan/manifests/podinfo"
        );
    }

    #[test]
    fn commit_links() {
        let sha = "3e0ff8ae123b710bc91de1315cba0f996a8896c2";
        assert_eq!(
            commit_url("https://github.com/stefanprodan/podinfo", sha).unwrap(),
            format!("https://github.com/stefanprodan/podinfo/commit/{sha}")
        );
        assert_eq!(
            commit_url("ssh://git@gitlab.com/acme/infra.git", sha).unwrap(),
            format!("https://gitlab.com/acme/infra/-/commit/{sha}")
        );
        assert_eq!(
            commit_url("git@bitbucket.org:acme/infra.git", sha).unwrap(),
            format!("https://bitbucket.org/acme/infra/commits/{sha}")
        );
        assert_eq!(
            commit_url(
                "https://deploy:x@gitlab.example.com/platform/infra.git",
                sha
            )
            .unwrap(),
            format!("https://gitlab.example.com/platform/infra/-/commit/{sha}")
        );
        assert_eq!(commit_url("https://git.example.com/a/b", sha), None);
        assert_eq!(commit_url("oci://ghcr.io/a/b", sha), None);
        assert_eq!(commit_url("https://github.com/a/b", "6.15.0"), None);
        assert_eq!(
            short_repo("ssh://git@github.com:22/acme/infra.git"),
            "github.com/acme/infra"
        );
    }
}
