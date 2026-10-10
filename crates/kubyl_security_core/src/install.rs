//! Installing the operator: what Kubyl offers where Trivy Operator isn't found.

/// The chart repository (Helm's own `--repo`, so no repository has to be added).
pub const CHART_REPO: &str = "https://aquasecurity.github.io/helm-charts/";
pub const CHART: &str = "trivy-operator";
/// The namespace the operator's docs install it into.
pub const NAMESPACE: &str = "trivy-system";
pub const RELEASE: &str = "trivy-operator";

/// The commands that do the same as Kubyl's install (shown, and copied, where Kubyl may not
/// write: read-only clusters, no `helm` found).
pub fn command() -> String {
    format!(
        "helm repo add aqua {CHART_REPO}\nhelm repo update\nhelm install {RELEASE} aqua/{CHART} \\\n  --namespace {NAMESPACE} --create-namespace"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_installs_into_trivy_system() {
        let text = command();
        assert!(text.contains("aqua/trivy-operator") && text.contains("--namespace trivy-system"));
        assert!(text.contains("--create-namespace"));
        assert!(text.contains("https://aquasecurity.github.io/helm-charts/"));
    }
}
