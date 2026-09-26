//! Messages for failed requests that say what's missing (a 403 names the verb, resource and
//! scope) instead of the API server's raw text.

/// A user-facing message for `err` while doing `verb` on `resource` (`namespace`: `None` for
/// cluster-wide).
pub fn describe(err: &kube::Error, verb: &str, resource: &str, namespace: Option<&str>) -> String {
    let scope = match namespace {
        Some(ns) => format!("in {ns}"),
        None => "cluster-wide".to_string(),
    };
    match err {
        kube::Error::Api(status) if status.code == 403 => {
            format!(
                "Forbidden: you can't {verb} {resource} {scope}. Ask for a role that can {verb} {resource}."
            )
        }
        kube::Error::Api(status) if status.code == 404 => {
            format!("Not found: {resource} ({})", status.message)
        }
        kube::Error::Api(status) if status.code == 409 => {
            format!("Conflict: {}", status.message)
        }
        kube::Error::Api(status) if status.code == 422 => {
            format!("Invalid: {}", status.message)
        }
        kube::Error::Api(status) => status.message.clone(),
        other => other.to_string(),
    }
}

/// Whether `err` is a 403.
pub fn is_forbidden(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(status) if status.code == 403)
}

/// Whether `err` is a 404.
pub fn is_not_found(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(status) if status.code == 404)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(code: u16, message: &str) -> kube::Error {
        kube::Error::Api(
            kube::core::Status::failure(message, "Reason")
                .with_code(code)
                .boxed(),
        )
    }

    #[test]
    fn forbidden_says_what_is_missing() {
        let err = api(
            403,
            "secrets is forbidden: User \"jane\" cannot list resource",
        );
        let text = describe(&err, "list", "secrets", Some("shop"));
        assert_eq!(
            text,
            "Forbidden: you can't list secrets in shop. Ask for a role that can list secrets."
        );
        assert!(is_forbidden(&err));
        assert!(describe(&api(403, ""), "get", "configmaps", None).contains("cluster-wide"));
        assert!(is_not_found(&api(404, "gone")));
    }
}
