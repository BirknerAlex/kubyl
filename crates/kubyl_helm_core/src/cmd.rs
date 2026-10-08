//! The `helm` command lines Kubyl runs: install, upgrade, rollback and uninstall (with their dry
//! runs), and the reads behind them. Values always go on stdin (`-f -`); the kubeconfig, context
//! and token are added by [`crate::cli::run`].
//!
//! Flags that differ between Helm 3 and 4 are picked by version: `--atomic` (3) /
//! `--rollback-on-failure` (4).

use std::time::Duration;

use crate::cli::{Invocation, Version};
use crate::decode::Driver;
use crate::repo::ChartRef;

/// How an install or upgrade runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `--dry-run=server`: rendered with server-side lookups and validation, nothing applied.
    DryRunServer,
    /// `--dry-run=client`: the fallback when the server-side dry run isn't allowed.
    DryRunClient,
    Apply,
}

impl Mode {
    pub fn is_dry_run(self) -> bool {
        !matches!(self, Mode::Apply)
    }
}

/// Options shared by install and upgrade.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteOptions {
    /// `--wait`: until the resources are ready (up to the timeout).
    pub wait: bool,
    /// Helm's `--timeout` (`5m0s`, `90s`).
    pub timeout: String,
    /// Roll back (uninstall, for an install) on failure: `--atomic` / `--rollback-on-failure`.
    pub atomic: bool,
    /// `--skip-crds` (install only: Helm never upgrades CRDs).
    pub skip_crds: bool,
    pub description: Option<String>,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            wait: false,
            timeout: "5m0s".into(),
            atomic: true,
            skip_crds: false,
            description: None,
        }
    }
}

/// What to install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallSpec {
    pub name: String,
    pub namespace: String,
    pub create_namespace: bool,
    pub chart: ChartRef,
    /// `None`: the latest.
    pub version: Option<String>,
    pub options: WriteOptions,
}

/// Where an upgrade's values start from (decision: the editor holds the user-supplied values).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ValuesMode {
    /// `--reset-values -f -`: the chart's defaults plus exactly the editor's values.
    #[default]
    Replace,
    /// `--reuse-values -f -`: the last release's values with the editor's merged on top.
    Reuse,
}

/// What to upgrade to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpgradeSpec {
    pub name: String,
    pub namespace: String,
    /// Where the release is stored (`HELM_DRIVER`).
    pub driver: Driver,
    pub chart: ChartRef,
    pub version: Option<String>,
    pub values_mode: ValuesMode,
    pub options: WriteOptions,
}

/// Rollback options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RollbackSpec {
    pub name: String,
    pub namespace: String,
    /// Where the release is stored (`HELM_DRIVER`).
    pub driver: Driver,
    pub revision: u32,
    pub wait: bool,
    pub timeout: String,
    pub cleanup_on_fail: bool,
    pub no_hooks: bool,
}

/// Uninstall options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UninstallSpec {
    pub name: String,
    pub namespace: String,
    /// Where the release is stored (`HELM_DRIVER`).
    pub driver: Driver,
    pub keep_history: bool,
    pub no_hooks: bool,
    pub wait: bool,
    pub timeout: String,
}

/// Parses Helm's duration like Go's `time.ParseDuration` does (`5m0s`, `90s`, `1.5h`, `250ms`;
/// every number needs a unit, `0` alone is fine), for Kubyl's own timeout. `None` for what Helm
/// would reject, and for durations too long to be one (Go's limit is about 292 years).
pub fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    if text == "0" {
        return Some(Duration::ZERO);
    }
    if text.is_empty() {
        return None;
    }
    let mut total = 0f64;
    let mut rest = text;
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let value: f64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let unit_len = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let seconds = match &rest[..unit_len] {
            "ns" => 1e-9,
            "us" | "µs" | "μs" => 1e-6,
            "ms" => 1e-3,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            // A bare number (`90`): Helm says "missing unit".
            _ => return None,
        };
        rest = &rest[unit_len..];
        total += value * seconds;
    }
    // Go's durations are i64 nanoseconds.
    if total > i64::MAX as f64 / 1e9 {
        return None;
    }
    Duration::try_from_secs_f64(total).ok()
}

/// Kubyl's own timeout for a write: Helm's plus a minute for hooks and the release record.
pub fn write_timeout(helm_timeout: &str) -> Duration {
    parse_duration(helm_timeout).unwrap_or(Duration::from_secs(300)) + Duration::from_secs(60)
}

/// An atomic write may wait `--timeout` for the release, then again for its rollback.
fn atomic_write_timeout(options: &WriteOptions) -> Duration {
    let helm = parse_duration(&options.timeout).unwrap_or(Duration::from_secs(300));
    let helm = if options.atomic {
        helm.saturating_mul(2)
    } else {
        helm
    };
    helm + Duration::from_secs(60)
}

fn push_chart(args: &mut Vec<String>, chart: &ChartRef, version: Option<&str>) {
    args.extend(chart.args());
    if let Some(version) = version.filter(|v| !v.trim().is_empty()) {
        args.push("--version".into());
        args.push(version.trim().to_string());
    }
}

fn push_mode(args: &mut Vec<String>, mode: Mode) {
    match mode {
        Mode::DryRunServer => {
            args.push("--dry-run=server".into());
            args.extend(["--output".into(), "json".into()]);
        }
        Mode::DryRunClient => {
            args.push("--dry-run=client".into());
            args.extend(["--output".into(), "json".into()]);
        }
        Mode::Apply => {
            args.extend(["--output".into(), "json".into()]);
        }
    }
}

fn push_options(args: &mut Vec<String>, options: &WriteOptions, mode: Mode, helm: Version) {
    if options.wait {
        args.push("--wait".into());
    }
    if !options.timeout.trim().is_empty() {
        args.push("--timeout".into());
        args.push(options.timeout.trim().into());
    }
    // A dry run doesn't roll anything back.
    if options.atomic && !mode.is_dry_run() {
        args.push(if helm.is_v4() {
            "--rollback-on-failure".into()
        } else {
            "--atomic".into()
        });
    }
    if let Some(description) = options
        .description
        .as_ref()
        .filter(|d| !d.trim().is_empty())
    {
        args.push("--description".into());
        args.push(description.trim().into());
    }
}

/// `helm install` (always reads values from stdin; empty values are `{}`).
pub fn install(spec: &InstallSpec, values: &str, mode: Mode, helm: Version) -> Invocation {
    let mut args = vec!["install".to_string(), spec.name.clone()];
    push_chart(&mut args, &spec.chart, spec.version.as_deref());
    args.extend(["--namespace".into(), spec.namespace.clone()]);
    if spec.create_namespace {
        args.push("--create-namespace".into());
    }
    if spec.options.skip_crds {
        args.push("--skip-crds".into());
    }
    push_options(&mut args, &spec.options, mode, helm);
    args.extend(["--values".into(), "-".into()]);
    push_mode(&mut args, mode);
    let timeout = if mode.is_dry_run() {
        Duration::from_secs(180)
    } else {
        atomic_write_timeout(&spec.options)
    };
    Invocation::cluster(args)
        .timeout(timeout)
        .stdin(Some(values_text(values)))
}

/// `helm upgrade`.
pub fn upgrade(spec: &UpgradeSpec, values: &str, mode: Mode, helm: Version) -> Invocation {
    let mut args = vec!["upgrade".to_string(), spec.name.clone()];
    push_chart(&mut args, &spec.chart, spec.version.as_deref());
    args.extend(["--namespace".into(), spec.namespace.clone()]);
    args.push(match spec.values_mode {
        ValuesMode::Replace => "--reset-values".into(),
        ValuesMode::Reuse => "--reuse-values".into(),
    });
    push_options(&mut args, &spec.options, mode, helm);
    if spec.options.atomic && !mode.is_dry_run() {
        args.push("--cleanup-on-fail".into());
    }
    args.extend(["--values".into(), "-".into()]);
    push_mode(&mut args, mode);
    let timeout = if mode.is_dry_run() {
        Duration::from_secs(180)
    } else {
        atomic_write_timeout(&spec.options)
    };
    Invocation::cluster(args)
        .timeout(timeout)
        .stdin(Some(values_text(values)))
        .driver(spec.driver)
}

/// `helm rollback` (Helm has no rollback dry run with output: Kubyl previews from the stored
/// revisions).
pub fn rollback(spec: &RollbackSpec) -> Invocation {
    let mut args = vec![
        "rollback".to_string(),
        spec.name.clone(),
        spec.revision.to_string(),
        "--namespace".into(),
        spec.namespace.clone(),
    ];
    if spec.wait {
        args.push("--wait".into());
    }
    if !spec.timeout.trim().is_empty() {
        args.extend(["--timeout".into(), spec.timeout.trim().into()]);
    }
    if spec.cleanup_on_fail {
        args.push("--cleanup-on-fail".into());
    }
    if spec.no_hooks {
        args.push("--no-hooks".into());
    }
    Invocation::cluster(args)
        .timeout(write_timeout(&spec.timeout))
        .driver(spec.driver)
}

/// `helm uninstall`.
pub fn uninstall(spec: &UninstallSpec) -> Invocation {
    let mut args = vec![
        "uninstall".to_string(),
        spec.name.clone(),
        "--namespace".into(),
        spec.namespace.clone(),
    ];
    if spec.keep_history {
        args.push("--keep-history".into());
    }
    if spec.no_hooks {
        args.push("--no-hooks".into());
    }
    if spec.wait {
        args.push("--wait".into());
    }
    if !spec.timeout.trim().is_empty() {
        args.extend(["--timeout".into(), spec.timeout.trim().into()]);
    }
    Invocation::cluster(args)
        .timeout(write_timeout(&spec.timeout))
        .driver(spec.driver)
}

/// `helm get values <name> -o json` (the user-supplied values; `all`: with the chart's).
pub fn get_values(name: &str, namespace: &str, driver: Driver, all: bool) -> Invocation {
    let mut args = vec![
        "get".to_string(),
        "values".into(),
        name.into(),
        "--namespace".into(),
        namespace.into(),
        "--output".into(),
        "json".into(),
    ];
    if all {
        args.push("--all".into());
    }
    Invocation::cluster(args)
        .timeout(Duration::from_secs(60))
        .driver(driver)
}

/// `helm status <name> -o json`.
pub fn status(name: &str, namespace: &str, driver: Driver) -> Invocation {
    Invocation::cluster(["status", name, "--namespace", namespace, "--output", "json"])
        .timeout(Duration::from_secs(60))
        .driver(driver)
}

/// Values for stdin: `{}` when empty (Helm reads an empty file fine, but be explicit).
fn values_text(values: &str) -> secrecy::SecretString {
    if values.trim().is_empty() {
        "{}\n".to_string().into()
    } else {
        values.to_string().into()
    }
}

/// Whether `name` is a valid release name (Helm: a DNS-1123 label of at most 53 characters).
pub fn validate_release_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Name the release.".into());
    }
    if name.len() > 53 {
        return Err("Release names are at most 53 characters.".into());
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.ends_with(|c: char| c.is_ascii_alphanumeric());
    if !ok {
        return Err(
            "Use lowercase letters, digits and '-', starting and ending with a letter or digit."
                .into(),
        );
    }
    Ok(())
}

/// Whether `name` is a valid namespace name (DNS-1123 label, at most 63 characters).
pub fn validate_namespace(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Pick a namespace.".into());
    }
    if name.len() > 63
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || !name.starts_with(|c: char| c.is_ascii_alphanumeric())
        || !name.ends_with(|c: char| c.is_ascii_alphanumeric())
    {
        return Err("Namespaces are lowercase letters, digits and '-' (at most 63).".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret as _;

    const V3: Version = Version {
        major: 3,
        minor: 16,
        patch: 2,
    };
    const V4: Version = Version {
        major: 4,
        minor: 3,
        patch: 0,
    };

    fn spec() -> InstallSpec {
        InstallSpec {
            name: "web".into(),
            namespace: "shop".into(),
            create_namespace: true,
            chart: ChartRef::Repo {
                repo: "kubyl-dev".into(),
                name: "demo".into(),
            },
            version: Some("0.2.0".into()),
            options: WriteOptions {
                wait: true,
                description: Some("from Kubyl".into()),
                ..WriteOptions::default()
            },
        }
    }

    #[test]
    fn install_runs_a_server_dry_run_then_applies() {
        let dry = install(&spec(), "replicaCount: 2\n", Mode::DryRunServer, V3);
        assert_eq!(
            dry.args,
            [
                "install",
                "web",
                "kubyl-dev/demo",
                "--version",
                "0.2.0",
                "--namespace",
                "shop",
                "--create-namespace",
                "--wait",
                "--timeout",
                "5m0s",
                "--description",
                "from Kubyl",
                "--values",
                "-",
                "--dry-run=server",
                "--output",
                "json"
            ]
        );
        assert!(dry.cluster);
        assert_eq!(dry.stdin.unwrap().expose_secret(), "replicaCount: 2\n");
        let apply = install(&spec(), "", Mode::Apply, V3);
        assert!(apply.args.contains(&"--atomic".to_string()));
        assert!(!apply.args.iter().any(|a| a.starts_with("--dry-run")));
        assert_eq!(apply.stdin.unwrap().expose_secret(), "{}\n");
        assert_eq!(apply.timeout, Duration::from_secs(660));
        // Helm 4 renamed --atomic.
        let v4 = install(&spec(), "", Mode::Apply, V4);
        assert!(v4.args.contains(&"--rollback-on-failure".to_string()));
        assert!(!v4.args.contains(&"--atomic".to_string()));
    }

    #[test]
    fn upgrades_replace_or_reuse_values() {
        let spec = UpgradeSpec {
            name: "web".into(),
            namespace: "shop".into(),
            driver: Driver::Secret,
            chart: ChartRef::Oci {
                reference: "oci://registry.example.com/charts/demo".into(),
            },
            version: None,
            values_mode: ValuesMode::Replace,
            options: WriteOptions::default(),
        };
        let args = upgrade(&spec, "a: 1", Mode::Apply, V3).args;
        assert_eq!(
            &args[..4],
            [
                "upgrade",
                "web",
                "oci://registry.example.com/charts/demo",
                "--namespace"
            ]
        );
        assert!(args.contains(&"--reset-values".to_string()));
        assert!(args.contains(&"--cleanup-on-fail".to_string()));
        let reuse = UpgradeSpec {
            values_mode: ValuesMode::Reuse,
            ..spec
        };
        let args = upgrade(&reuse, "", Mode::DryRunClient, V4).args;
        assert!(args.contains(&"--reuse-values".to_string()));
        assert!(args.contains(&"--dry-run=client".to_string()));
        assert!(!args.contains(&"--rollback-on-failure".to_string()));
    }

    #[test]
    fn rollback_and_uninstall_flags() {
        let args = rollback(&RollbackSpec {
            name: "web".into(),
            namespace: "shop".into(),
            driver: Driver::ConfigMap,
            revision: 3,
            wait: true,
            timeout: "2m".into(),
            cleanup_on_fail: true,
            no_hooks: true,
        })
        .args;
        assert_eq!(
            args,
            [
                "rollback",
                "web",
                "3",
                "--namespace",
                "shop",
                "--wait",
                "--timeout",
                "2m",
                "--cleanup-on-fail",
                "--no-hooks"
            ]
        );
        let args = uninstall(&UninstallSpec {
            name: "web".into(),
            namespace: "shop".into(),
            driver: Driver::Secret,
            keep_history: true,
            no_hooks: false,
            wait: false,
            timeout: "5m".into(),
        })
        .args;
        assert_eq!(
            args,
            [
                "uninstall",
                "web",
                "--namespace",
                "shop",
                "--keep-history",
                "--timeout",
                "5m"
            ]
        );
    }

    #[test]
    fn only_applied_writes_outlive_kubyl() {
        assert!(install(&spec(), "", Mode::Apply, V3).is_write());
        assert!(!install(&spec(), "", Mode::DryRunServer, V3).is_write());
        assert!(!get_values("web", "shop", Driver::Secret, false).is_write());
        assert!(!crate::repo::update_repositories(&[]).is_write());
    }

    #[test]
    fn durations_and_names() {
        assert_eq!(parse_duration("5m0s"), Some(Duration::from_secs(300)));
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("1h30m"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_duration("1.5m"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("250ms"), Some(Duration::from_millis(250)));
        assert_eq!(parse_duration("0"), Some(Duration::ZERO));
        // Helm rejects these: a missing unit, an unknown one, a sign, an overflow.
        assert_eq!(parse_duration("45"), None);
        assert_eq!(parse_duration("5x"), None);
        assert_eq!(parse_duration("-5s"), None);
        assert_eq!(parse_duration("m"), None);
        assert_eq!(parse_duration("99999999999999999999h"), None);
        assert_eq!(parse_duration(&format!("{}s", "9".repeat(400))), None);
        assert_eq!(
            write_timeout("99999999999999999999h"),
            Duration::from_secs(360)
        );
        assert!(validate_release_name("web-1").is_ok());
        assert!(validate_release_name("Web").is_err());
        assert!(validate_release_name("-web").is_err());
        assert!(validate_release_name(&"a".repeat(54)).is_err());
        assert!(validate_namespace("kubyl-helm").is_ok());
        assert!(validate_namespace("Bad_ns").is_err());
    }
}
