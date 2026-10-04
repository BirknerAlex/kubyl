//! AWS credentials, read the way `aws eks get-token` reads them: `aws configure
//! export-credentials --format process` for the profile (or `aws sts assume-role` when the
//! exec plugin passes `--role-arn`). Held in memory until 5 min before they expire; never in
//! `Debug`, errors or logs.

use std::fmt;
use std::time::Duration;

use jiff::Timestamp;
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;

use super::EksTarget;
use crate::provider::ProviderError;
use crate::providers::cloud::{self, AWS, CliError};

/// Static keys don't say when they expire: read them again after this long.
const STATIC_KEYS_FOR: Duration = Duration::from_secs(15 * 60);

/// Temporary (or static) AWS credentials.
#[derive(Clone)]
pub struct AwsCredentials {
    pub access_key_id: SecretString,
    pub secret_access_key: SecretString,
    pub session_token: Option<SecretString>,
    pub expiration: Option<Timestamp>,
}

impl fmt::Debug for AwsCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwsCredentials")
            .field("access_key_id", &"[REDACTED]")
            .field("secret_access_key", &"[REDACTED]")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expiration", &self.expiration)
            .finish()
    }
}

impl AwsCredentials {
    /// The keys for one signature.
    pub fn keys(&self) -> super::sigv4::Keys<'_> {
        super::sigv4::Keys {
            access_key_id: self.access_key_id.expose_secret(),
            secret_access_key: self.secret_access_key.expose_secret(),
            session_token: self.session_token.as_ref().map(|t| t.expose_secret()),
        }
    }
}

/// `credential_process` JSON, as `aws configure export-credentials --format process` prints it
/// (and `Credentials` of `aws sts assume-role`).
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ProcessCredentials {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    expiration: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AssumeRoleOutput {
    credentials: ProcessCredentials,
}

impl ProcessCredentials {
    fn into_credentials(self) -> Result<AwsCredentials, String> {
        let expiration = self
            .expiration
            .filter(|e| !e.is_empty())
            .map(|e| e.parse::<Timestamp>())
            .transpose()
            .map_err(|err| format!("invalid Expiration: {err}"))?;
        if self.access_key_id.is_empty() || self.secret_access_key.is_empty() {
            return Err("the credentials are empty".into());
        }
        Ok(AwsCredentials {
            access_key_id: SecretString::from(self.access_key_id),
            secret_access_key: SecretString::from(self.secret_access_key),
            session_token: self
                .session_token
                .filter(|t| !t.is_empty())
                .map(SecretString::from),
            expiration,
        })
    }
}

/// Errors say where the JSON broke, never what it held.
fn json_error(err: serde_json::Error) -> String {
    format!(
        "the output isn't the expected JSON (line {}, column {})",
        err.line(),
        err.column()
    )
}

/// Parses `aws configure export-credentials --format process`.
pub fn parse_process_credentials(stdout: &[u8]) -> Result<AwsCredentials, String> {
    serde_json::from_slice::<ProcessCredentials>(stdout)
        .map_err(json_error)?
        .into_credentials()
}

/// Parses `aws sts assume-role --output json`.
pub fn parse_assume_role(stdout: &[u8]) -> Result<AwsCredentials, String> {
    serde_json::from_slice::<AssumeRoleOutput>(stdout)
        .map_err(json_error)?
        .credentials
        .into_credentials()
}

/// The CLI arguments that read the target's credentials.
pub fn cli_args(target: &EksTarget) -> Vec<String> {
    let mut args: Vec<String> = match &target.role_arn {
        Some(role) => [
            "sts",
            "assume-role",
            "--role-arn",
            role,
            "--role-session-name",
            "kubyl-updates",
            "--output",
            "json",
            "--region",
            &target.region,
        ]
        .map(String::from)
        .to_vec(),
        None => ["configure", "export-credentials", "--format", "process"]
            .map(String::from)
            .to_vec(),
    };
    if let Some(profile) = &target.profile {
        args.extend(["--profile".to_string(), profile.clone()]);
    }
    args
}

/// `aws sso login --profile prod`.
pub fn sso_login(profile: Option<&str>) -> String {
    match profile {
        Some(profile) => format!("aws sso login --profile {profile}"),
        None => "aws sso login".into(),
    }
}

/// What a failed `aws` run means, and what fixes it.
pub fn classify(error: &CliError, target: &EksTarget) -> ProviderError {
    let profile = target.profile.as_deref();
    let of_profile = profile
        .map(|p| format!(" for the profile {p}"))
        .unwrap_or_default();
    let shown = format!("aws {}", cli_args(target).join(" "));
    let CliError::Failed { stderr, .. } = error else {
        return error.to_provider_error(AWS, &shown, None);
    };
    let lower = stderr.to_lowercase();
    let summary = cloud::stderr_summary(stderr);
    if lower.contains("invalid choice") {
        return ProviderError::Unavailable(format!(
            "Kubyl needs AWS CLI v2 (2.9 or newer) for `aws configure export-credentials`. \
             Update it from {}.",
            AWS.install_url
        ));
    }
    if lower.contains("sso")
        && (lower.contains("token") || lower.contains("expired") || lower.contains("session"))
    {
        return ProviderError::Credentials {
            message: format!("Your AWS SSO session{of_profile} has expired or isn't signed in."),
            command: Some(sso_login(profile)),
        };
    }
    if lower.contains("could not be found") && lower.contains("profile") {
        return ProviderError::Credentials {
            message: format!(
                "The AWS profile{} isn't configured on this computer (~/.aws/config): {summary}",
                profile.map(|p| format!(" {p}")).unwrap_or_default()
            ),
            command: None,
        };
    }
    if lower.contains("unable to locate credentials") || lower.contains("no credentials") {
        return ProviderError::Credentials {
            message: format!("No AWS credentials found{of_profile}."),
            command: Some(match profile {
                Some(_) => sso_login(profile),
                None => "aws configure".into(),
            }),
        };
    }
    if lower.contains("expired") {
        return ProviderError::Credentials {
            message: format!("Your AWS credentials{of_profile} have expired: {summary}"),
            command: profile.map(|p| sso_login(Some(p))),
        };
    }
    if let Some(role) = &target.role_arn
        && lower.contains("accessdenied")
        && lower.contains("assumerole")
    {
        return ProviderError::Forbidden {
            verb: "sts:AssumeRole".into(),
            resource: format!("role {role}"),
        };
    }
    error.to_provider_error(AWS, &shown, None)
}

/// Reads the credentials with the AWS CLI; the answer says until when to use them.
pub async fn fetch(target: &EksTarget) -> Result<(AwsCredentials, Timestamp), ProviderError> {
    let args = cli_args(target);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let stdout = cloud::run(AWS, &refs, &target.env)
        .await
        .map_err(|err| classify(&err, target))?;
    let parsed = if target.role_arn.is_some() {
        parse_assume_role(&stdout)
    } else {
        parse_process_credentials(&stdout)
    };
    let credentials = parsed.map_err(|message| {
        ProviderError::Other(format!("`aws {}`: {message}", args[..2].join(" ")))
    })?;
    let until = cloud::fresh_until(credentials.expiration, STATIC_KEYS_FOR, Timestamp::now());
    Ok((credentials, until))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::cloud::CliEnv;

    fn target(profile: Option<&str>, role: Option<&str>) -> EksTarget {
        EksTarget {
            cluster: "prod-eu-west-1".into(),
            region: "eu-west-1".into(),
            profile: profile.map(str::to_string),
            role_arn: role.map(str::to_string),
            domain: "amazonaws.com".into(),
            env: CliEnv::default(),
        }
    }

    #[test]
    fn parses_export_credentials() {
        let creds = parse_process_credentials(
            br#"{"Version": 1, "AccessKeyId": "ASIAEXAMPLE", "SecretAccessKey": "wJalrEXAMPLE",
                "SessionToken": "FwoGEXAMPLE", "Expiration": "2026-09-26T13:00:00+00:00"}"#,
        )
        .unwrap();
        assert_eq!(creds.access_key_id.expose_secret(), "ASIAEXAMPLE");
        assert_eq!(
            creds.expiration,
            Some("2026-09-26T13:00:00Z".parse().unwrap())
        );
        // Static keys: no token, no expiry.
        let creds = parse_process_credentials(
            br#"{"Version": 1, "AccessKeyId": "AKIAEXAMPLE", "SecretAccessKey": "wJalrEXAMPLE"}"#,
        )
        .unwrap();
        assert!(creds.session_token.is_none() && creds.expiration.is_none());
        let creds = parse_assume_role(
            br#"{"Credentials": {"AccessKeyId": "ASIAROLE", "SecretAccessKey": "s3cr3t",
                "SessionToken": "t0k3n", "Expiration": "2026-09-26T13:00:00Z"},
                "AssumedRoleUser": {"AssumedRoleId": "AROA:kubyl-updates", "Arn": "arn:aws:sts::111122223333:assumed-role/Admin/kubyl-updates"}}"#,
        )
        .unwrap();
        assert_eq!(creds.access_key_id.expose_secret(), "ASIAROLE");
    }

    #[test]
    fn credentials_never_show_in_debug_or_errors() {
        let creds = parse_process_credentials(
            br#"{"AccessKeyId": "ASIASECRETID", "SecretAccessKey": "very-secret-key",
                "SessionToken": "session-secret", "Expiration": "2026-09-26T13:00:00Z"}"#,
        )
        .unwrap();
        let debug = format!("{creds:?}");
        for secret in ["ASIASECRETID", "very-secret-key", "session-secret"] {
            assert!(!debug.contains(secret), "{debug}");
        }
        assert!(debug.contains("2026-09-26"));
        // A broken output never echoes what it held.
        let err = parse_process_credentials(br#"{"AccessKeyId": "ASIASECRETID", "SecretAcc"#)
            .err()
            .unwrap();
        assert!(!err.contains("ASIASECRETID"), "{err}");
        let err = parse_process_credentials(br#"{"AccessKeyId": 5, "SecretAccessKey": "x"}"#)
            .err()
            .unwrap();
        assert!(err.contains("line 1"), "{err}");
    }

    #[test]
    fn cli_arguments() {
        assert_eq!(
            cli_args(&target(Some("prod"), None)).join(" "),
            "configure export-credentials --format process --profile prod"
        );
        assert_eq!(
            cli_args(&target(
                None,
                Some("arn:aws:iam::111122223333:role/EksAdmin")
            ))
            .join(" "),
            "sts assume-role --role-arn arn:aws:iam::111122223333:role/EksAdmin \
             --role-session-name kubyl-updates --output json --region eu-west-1"
        );
    }

    #[test]
    fn classifies_cli_errors() {
        let failed = |stderr: &str| CliError::Failed {
            code: Some(255),
            stderr: stderr.into(),
        };
        let prod = target(Some("prod"), None);
        assert_eq!(
            classify(
                &failed("aws: [ERROR]: Error loading SSO Token: Token for corp does not exist"),
                &prod
            ),
            ProviderError::Credentials {
                message:
                    "Your AWS SSO session for the profile prod has expired or isn't signed in."
                        .into(),
                command: Some("aws sso login --profile prod".into()),
            }
        );
        assert!(matches!(
            classify(
                &failed("Error when retrieving token from sso: Token has expired and refresh failed"),
                &prod
            ),
            ProviderError::Credentials { command: Some(c), .. } if c == "aws sso login --profile prod"
        ));
        assert!(matches!(
            classify(
                &failed("aws: [ERROR]: Unable to retrieve credentials: The config profile (prod) could not be found"),
                &prod
            ),
            ProviderError::Credentials { message, command: None } if message.contains("profile prod isn't configured")
        ));
        assert!(matches!(
            classify(
                &failed("aws: [ERROR]: An error occurred (Configuration): Unable to retrieve credentials: no credentials found"),
                &target(None, None)
            ),
            ProviderError::Credentials { command: Some(c), .. } if c == "aws configure"
        ));
        assert!(matches!(
            classify(
                &failed("aws: error: argument operation: Invalid choice, valid choices are: add-model | get"),
                &prod
            ),
            ProviderError::Unavailable(m) if m.contains("2.9")
        ));
        let role = target(
            Some("prod"),
            Some("arn:aws:iam::111122223333:role/EksAdmin"),
        );
        assert_eq!(
            classify(
                &failed(
                    "An error occurred (AccessDenied) when calling the AssumeRole operation: User is not authorized"
                ),
                &role
            ),
            ProviderError::Forbidden {
                verb: "sts:AssumeRole".into(),
                resource: "role arn:aws:iam::111122223333:role/EksAdmin".into()
            }
        );
        assert!(matches!(
            classify(&CliError::NotFound, &prod),
            ProviderError::Unavailable(m) if m.contains("AWS CLI")
        ));
    }
}
