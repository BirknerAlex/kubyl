//! How to run a user's CLI (`helm`, an agent's `kubectl`) against a context: the kubeconfig
//! file and context name, and for contexts that sign in through Kubyl (OIDC, OpenShift OAuth)
//! the current bearer token, which the CLI can't get from the kubeconfig (Kubyl keeps those
//! sign-ins in the keychain).
//!
//! The token only ever goes into a child's environment (never into its arguments, which `ps`
//! shows, and never into a file). Exec plugins, client certificates and static tokens stay with
//! the kubeconfig: the CLI reads them itself.
//!
//! The sign-in follows the manager's [`Credentials`] handle: a scoped handle belongs to someone
//! other than the kubeconfig's author, so for its OIDC and OpenShift contexts a missing sign-in
//! is an error ([`AuthError::SignInRequired`]), never "let the CLI use the tokens written into
//! the kubeconfig" (what the default handle, the desktop's one user, may do).

use std::fmt;
use std::path::PathBuf;

use secrecy::SecretString;

use crate::auth::{AuthError, CredentialSource, Credentials};

/// A context as a CLI reaches it.
#[derive(Clone)]
pub struct CliTarget {
    /// The kubeconfig file that defines the context (`--kubeconfig`).
    pub kubeconfig: PathBuf,
    /// The context's name in that file (`--kube-context`).
    pub context: String,
    /// The API server, for CLIs that take it next to a token (`HELM_KUBEAPISERVER`).
    pub server: Option<String>,
    /// The sign-in Kubyl manages for the context, when the CLI can't sign in by itself.
    signed_in: Option<CredentialSource>,
    /// The context signs in through Kubyl and only Kubyl's token may reach the CLI (a scoped
    /// handle): no token means no command, not the kubeconfig's own tokens.
    sign_in_required: bool,
}

impl fmt::Debug for CliTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CliTarget")
            .field("kubeconfig", &self.kubeconfig)
            .field("context", &self.context)
            .field("server", &self.server)
            .field("signed_in", &self.signed_in.is_some())
            .field("sign_in_required", &self.sign_in_required)
            .finish()
    }
}

impl CliTarget {
    /// A context the CLI signs in to by itself (its kubeconfig has what it needs).
    pub fn new(kubeconfig: impl Into<PathBuf>, context: impl Into<String>) -> Self {
        Self {
            kubeconfig: kubeconfig.into(),
            context: context.into(),
            server: None,
            signed_in: None,
            sign_in_required: false,
        }
    }

    pub fn with_server(mut self, server: Option<String>) -> Self {
        self.server = server;
        self
    }

    /// The context signs in through Kubyl: [`Self::token`] hands the CLI Kubyl's token.
    /// Only OIDC and OpenShift OAuth count; an exec plugin runs in the CLI as configured.
    pub fn with_sign_in(mut self, source: Option<CredentialSource>) -> Self {
        self.signed_in = source.filter(|s| {
            matches!(
                s,
                CredentialSource::Oidc(_) | CredentialSource::OpenShift(_)
            )
        });
        self
    }

    /// Whose sign-in this is: for a context that signs in through Kubyl (`kubyl_signs_in`: OIDC,
    /// OpenShift OAuth), a handle that mustn't use the kubeconfig's own tokens
    /// ([`Credentials::uses_kubeconfig_tokens`]) makes Kubyl's token required.
    pub fn with_credentials(mut self, credentials: &Credentials, kubyl_signs_in: bool) -> Self {
        self.sign_in_required = kubyl_signs_in && !credentials.uses_kubeconfig_tokens();
        self
    }

    /// Whether the CLI may only run with Kubyl's token (see [`Self::with_credentials`]): when
    /// [`Self::token`] fails, don't run it with the kubeconfig's own credentials.
    pub fn sign_in_required(&self) -> bool {
        self.sign_in_required
    }

    /// The current bearer token for a context that signs in through Kubyl (refreshed when
    /// needed), `None` for others. Put it into the child's environment only.
    pub async fn token(&self) -> Result<Option<SecretString>, AuthError> {
        match &self.signed_in {
            Some(source) => source.token().await.map(Some),
            None if self.sign_in_required => Err(AuthError::SignInRequired),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_a_sign_in() {
        let target = CliTarget::new("/tmp/kubeconfig", "kind-dev")
            .with_server(Some("https://127.0.0.1:6443".into()));
        let text = format!("{target:?}");
        assert!(text.contains("kind-dev") && text.contains("signed_in: false"));
    }

    #[tokio::test]
    async fn scoped_handles_require_kubyls_own_sign_in() {
        // The default handle: a context without a Kubyl sign-in lets the CLI use the kubeconfig.
        let desktop = CliTarget::new("/tmp/kubeconfig", "oidc")
            .with_sign_in(None)
            .with_credentials(&Credentials::default(), true);
        assert!(!desktop.sign_in_required());
        assert!(matches!(desktop.token().await, Ok(None)));
        // A scoped handle never falls back to the tokens written into the kubeconfig.
        let scoped = CliTarget::new("/tmp/kubeconfig", "oidc")
            .with_sign_in(None)
            .with_credentials(&Credentials::scoped("alice"), true);
        assert!(scoped.sign_in_required());
        assert!(matches!(
            scoped.token().await,
            Err(AuthError::SignInRequired)
        ));
        // Contexts the CLI signs in to by itself (exec plugins, certificates) are the same for
        // every handle.
        let exec = CliTarget::new("/tmp/kubeconfig", "eks")
            .with_credentials(&Credentials::scoped("alice"), false);
        assert!(!exec.sign_in_required());
        assert!(matches!(exec.token().await, Ok(None)));
    }
}
