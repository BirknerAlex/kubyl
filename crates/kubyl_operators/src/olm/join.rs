//! Installed operators: Subscriptions joined with their CSV and pending InstallPlan.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kubyl_core::Tone;

use super::model::{Approval, Csv, InstallPlan, Subscription};

/// Where an installed operator stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OperatorStatus {
    Failed,
    /// A manual subscription's first install waits for approval.
    ApprovalRequired,
    UpgradeAvailable,
    Installing,
    Deleting,
    Succeeded,
    /// Nothing to say yet (no CSV, no plan).
    Unknown,
}

impl OperatorStatus {
    pub fn label(self) -> &'static str {
        match self {
            OperatorStatus::Failed => "Failed",
            OperatorStatus::ApprovalRequired => "Approval required",
            OperatorStatus::UpgradeAvailable => "Upgrade available",
            OperatorStatus::Installing => "Installing",
            OperatorStatus::Deleting => "Deleting",
            OperatorStatus::Succeeded => "Succeeded",
            OperatorStatus::Unknown => "Unknown",
        }
    }

    pub fn tone(self) -> Tone {
        match self {
            OperatorStatus::Failed => Tone::Bad,
            OperatorStatus::ApprovalRequired | OperatorStatus::UpgradeAvailable => Tone::Warning,
            OperatorStatus::Installing => Tone::Info,
            OperatorStatus::Deleting | OperatorStatus::Unknown => Tone::Muted,
            OperatorStatus::Succeeded => Tone::Good,
        }
    }
}

/// One row of Installed Operators.
#[derive(Clone, Debug, PartialEq)]
pub struct Operator {
    /// `sub:<ns>/<name>`, or `csv:<ns>/<name>` for a CSV without a Subscription.
    pub key: String,
    pub subscription: Option<Arc<Subscription>>,
    pub csv: Option<Arc<Csv>>,
    /// The InstallPlan waiting for approval (or installing) for this operator.
    pub plan: Option<Arc<InstallPlan>>,
    pub status: OperatorStatus,
    /// Why (a failure message, what it waits for).
    pub detail: Option<String>,
    /// The version a pending plan installs.
    pub upgrade_to: Option<String>,
}

impl Operator {
    pub fn namespace(&self) -> &str {
        self.subscription
            .as_ref()
            .map(|s| s.namespace.as_str())
            .or_else(|| self.csv.as_ref().map(|c| c.namespace.as_str()))
            .unwrap_or_default()
    }

    pub fn display_name(&self) -> String {
        self.csv
            .as_ref()
            .map(|c| c.display_name.clone())
            .or_else(|| self.subscription.as_ref().map(|s| s.package.clone()))
            .unwrap_or_default()
    }

    pub fn package(&self) -> Option<&str> {
        self.subscription.as_ref().map(|s| s.package.as_str())
    }

    /// What the uninstall dialog asks to type: the package (else the CSV's name).
    pub fn typed_name(&self) -> String {
        self.package()
            .map(str::to_string)
            .or_else(|| self.csv.as_ref().map(|c| c.name.clone()))
            .unwrap_or_default()
    }

    pub fn version(&self) -> Option<&str> {
        self.csv.as_ref().and_then(|c| c.version.as_deref())
    }

    pub fn channel(&self) -> Option<&str> {
        self.subscription
            .as_ref()
            .and_then(|s| s.channel.as_deref())
    }

    pub fn approval(&self) -> Option<Approval> {
        self.subscription.as_ref().map(|s| s.approval)
    }

    /// The pending plan when it needs someone's approval.
    pub fn approvable(&self) -> Option<&Arc<InstallPlan>> {
        self.plan.as_ref().filter(|p| p.needs_approval())
    }

    /// Two letters for the tile: `Strimzi` → `ST`, `cert-manager` → `CM`.
    pub fn initials(&self) -> String {
        initials(&self.display_name())
    }
}

/// Two letters for a letter tile.
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let mut out = String::new();
    match words.as_slice() {
        [] => {}
        [one] => {
            let mut chars = one.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
            }
            // The next capital of a CamelCase name (`CloudNativePG` → `CN`), else the next
            // letter (`Strimzi` → `ST`).
            let rest: Vec<char> = chars.collect();
            let pick = rest.iter().find(|c| c.is_uppercase()).or(rest.first());
            if let Some(c) = pick {
                out.extend(c.to_uppercase());
            }
        }
        [first, second, ..] => {
            for word in [first, second] {
                if let Some(c) = word.chars().next() {
                    out.extend(c.to_uppercase());
                }
            }
        }
    }
    out
}

/// Joins the watches into operator rows, sorted by name. `csvs` are the head CSVs (no copies).
pub fn join(
    subscriptions: &[Arc<Subscription>],
    csvs: &[Arc<Csv>],
    plans: &[Arc<InstallPlan>],
) -> Vec<Operator> {
    let csv_by_key: HashMap<(String, String), &Arc<Csv>> = csvs
        .iter()
        .filter(|c| !c.copied)
        .map(|c| ((c.namespace.clone(), c.name.clone()), c))
        .collect();
    let plan_by_key: HashMap<(String, String), &Arc<InstallPlan>> = plans
        .iter()
        .map(|p| ((p.namespace.clone(), p.name.clone()), p))
        .collect();
    let mut used: HashSet<(String, String)> = HashSet::new();
    let mut out = Vec::new();
    for sub in subscriptions {
        let csv_name = sub.installed_csv.clone();
        let csv = csv_name
            .as_ref()
            .and_then(|n| csv_by_key.get(&(sub.namespace.clone(), n.clone())))
            .map(|c| (*c).clone());
        if let Some(csv) = &csv {
            used.insert((csv.namespace.clone(), csv.name.clone()));
        }
        // The plan the Subscription points at, else the newest one it owns that is still
        // open (OLM leaves `installPlanRef` on the last plan).
        let plan = sub
            .install_plan
            .as_ref()
            .and_then(|n| plan_by_key.get(&(sub.namespace.clone(), n.clone())))
            .map(|p| (*p).clone())
            .or_else(|| {
                plans
                    .iter()
                    .filter(|p| p.namespace == sub.namespace && p.subscriptions.contains(&sub.name))
                    .filter(|p| !p.complete() && !p.failed())
                    .max_by_key(|p| p.created)
                    .cloned()
            });
        // A plan that already installed the current CSV isn't pending.
        let plan = plan.filter(|p| {
            !(p.complete()
                && csv_name
                    .as_ref()
                    .is_some_and(|installed| p.csv_names.contains(installed)))
        });
        let (status, detail, upgrade_to) = status_of(sub, csv.as_deref(), plan.as_deref());
        out.push(Operator {
            key: format!("sub:{}/{}", sub.namespace, sub.name),
            subscription: Some(sub.clone()),
            csv,
            plan: plan.filter(|p| !p.complete()),
            status,
            detail,
            upgrade_to,
        });
    }
    // CSVs nobody subscribes to (installed by hand, or the Subscription was deleted).
    for csv in csvs.iter().filter(|c| !c.copied) {
        let key = (csv.namespace.clone(), csv.name.clone());
        if used.contains(&key) {
            continue;
        }
        // A CSV being replaced by a subscribed one (`replaces`) isn't an operator of its own.
        let replaced = csvs
            .iter()
            .any(|c| c.namespace == csv.namespace && c.replaces.as_deref() == Some(&csv.name));
        if replaced {
            continue;
        }
        let (status, detail) = csv_status(csv);
        out.push(Operator {
            key: format!("csv:{}/{}", csv.namespace, csv.name),
            subscription: None,
            csv: Some(csv.clone()),
            plan: None,
            status,
            detail,
            upgrade_to: None,
        });
    }
    out.sort_by(|a, b| {
        a.display_name()
            .to_lowercase()
            .cmp(&b.display_name().to_lowercase())
            .then_with(|| a.namespace().cmp(b.namespace()))
    });
    out
}

fn csv_status(csv: &Csv) -> (OperatorStatus, Option<String>) {
    let detail = csv.message.clone().or_else(|| csv.reason.clone());
    if csv.deleting {
        return (OperatorStatus::Deleting, None);
    }
    match csv.phase.as_str() {
        "Succeeded" => (OperatorStatus::Succeeded, None),
        "Failed" => (OperatorStatus::Failed, detail),
        "Deleting" => (OperatorStatus::Deleting, None),
        "" => (OperatorStatus::Unknown, None),
        // Pending, InstallReady, Installing, Replacing, Unknown…
        _ => (OperatorStatus::Installing, detail),
    }
}

fn status_of(
    sub: &Subscription,
    csv: Option<&Csv>,
    plan: Option<&InstallPlan>,
) -> (OperatorStatus, Option<String>, Option<String>) {
    let target = plan.and_then(|p| {
        p.csv_names
            .iter()
            .find(|n| Some(n.as_str()) != sub.installed_csv.as_deref())
            .and_then(|n| p.version_of(n).or_else(|| Some(n.clone())))
    });
    if let Some(plan) = plan.filter(|p| p.failed()) {
        return (
            OperatorStatus::Failed,
            plan.failure()
                .or_else(|| Some(format!("Install plan {} failed.", plan.name))),
            target,
        );
    }
    if let Some(csv) = csv
        && csv.failed()
    {
        return (
            OperatorStatus::Failed,
            csv.message.clone().or_else(|| csv.reason.clone()),
            target,
        );
    }
    if let Some(plan) = plan.filter(|p| p.needs_approval()) {
        let detail = Some(format!("Install plan {} waits for approval.", plan.name));
        return match csv {
            Some(_) => (OperatorStatus::UpgradeAvailable, detail, target),
            None => (OperatorStatus::ApprovalRequired, detail, target),
        };
    }
    // A running upgrade: OLM reports `ResolutionFailed` for a moment while the old CSV is being
    // replaced, so the subscription's conditions only count once nothing is in flight.
    let in_flight = plan.is_some_and(|p| matches!(p.phase.as_str(), "Installing" | "Planning"))
        || csv.is_some_and(|c| !c.succeeded() && !c.failed());
    match csv {
        Some(csv) => {
            let (status, detail) = csv_status(csv);
            if in_flight {
                return (OperatorStatus::Installing, detail, target);
            }
            // The operator runs; its subscription can't resolve (the package left the catalog…).
            match sub.failure() {
                Some(failure) => (
                    status,
                    Some(format!("The subscription can't resolve: {failure}")),
                    target,
                ),
                None => (status, detail, target),
            }
        }
        None if !in_flight && sub.failure().is_some() => {
            (OperatorStatus::Failed, sub.failure(), target)
        }
        None if plan.is_some() || sub.current_csv.is_some() => (
            OperatorStatus::Installing,
            sub.current_csv.as_ref().map(|c| format!("Installing {c}.")),
            target,
        ),
        // A new Subscription OLM hasn't resolved yet.
        None => (
            OperatorStatus::Installing,
            Some("OLM is resolving the subscription.".into()),
            target,
        ),
    }
}

/// Counts for the header: installed, upgrades waiting, failing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub installed: usize,
    pub waiting: usize,
    pub failing: usize,
}

impl Counts {
    pub fn of(operators: &[Operator]) -> Self {
        Self {
            installed: operators.len(),
            waiting: operators
                .iter()
                .filter(|o| {
                    matches!(
                        o.status,
                        OperatorStatus::UpgradeAvailable | OperatorStatus::ApprovalRequired
                    )
                })
                .count(),
            failing: operators
                .iter()
                .filter(|o| o.status == OperatorStatus::Failed)
                .count(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sub(name: &str, installed: Option<&str>, plan: Option<&str>) -> Arc<Subscription> {
        Arc::new(
            Subscription::parse(&json!({
                "metadata": {"name": name, "namespace": "ops"},
                "spec": {"name": name, "channel": "stable", "installPlanApproval": "Manual"},
                "status": {"installedCSV": installed, "installPlanRef": plan.map(|p| json!({"name": p}))}
            }))
            .unwrap(),
        )
    }

    fn csv(name: &str, phase: &str) -> Arc<Csv> {
        Arc::new(
            Csv::parse(&json!({
                "metadata": {"name": name, "namespace": "ops"},
                "spec": {"displayName": name, "version": "1.0.0"},
                "status": {"phase": phase}
            }))
            .unwrap(),
        )
    }

    fn plan(name: &str, csvs: &[&str], phase: &str, approved: bool) -> Arc<InstallPlan> {
        Arc::new(
            InstallPlan::parse(&json!({
                "metadata": {"name": name, "namespace": "ops"},
                "spec": {"approval": "Manual", "approved": approved, "clusterServiceVersionNames": csvs},
                "status": {"phase": phase}
            }))
            .unwrap(),
        )
    }

    #[test]
    fn statuses() {
        let subs = [
            sub("a", Some("a.v1.0.0"), None),
            sub("b", Some("b.v1.0.0"), Some("install-b")),
            sub("c", None, Some("install-c")),
            sub("d", Some("d.v1.0.0"), None),
            sub("e", Some("e.v2.0.0"), Some("install-e")),
        ];
        let csvs = [
            csv("a.v1.0.0", "Succeeded"),
            csv("b.v1.0.0", "Succeeded"),
            csv("d.v1.0.0", "Failed"),
            csv("e.v2.0.0", "Succeeded"),
            // Nobody subscribes to it.
            csv("manual.v0.1.0", "Installing"),
        ];
        let plans = [
            plan("install-b", &["b.v1.1.0"], "RequiresApproval", false),
            plan("install-c", &["c.v1.0.0"], "RequiresApproval", false),
            plan("install-e", &["e.v2.0.0"], "Complete", true),
        ];
        let rows = join(&subs, &csvs, &plans);
        let status = |name: &str| {
            rows.iter()
                .find(|r| r.display_name().starts_with(name))
                .map(|r| r.status)
                .unwrap()
        };
        assert_eq!(status("a"), OperatorStatus::Succeeded);
        assert_eq!(status("b"), OperatorStatus::UpgradeAvailable);
        assert_eq!(status("c"), OperatorStatus::ApprovalRequired);
        assert_eq!(status("d"), OperatorStatus::Failed);
        // The plan that installed the current CSV is done: nothing pending.
        assert_eq!(status("e"), OperatorStatus::Succeeded);
        assert!(
            rows.iter()
                .find(|r| r.display_name().starts_with('e'))
                .unwrap()
                .plan
                .is_none()
        );
        assert_eq!(status("manual"), OperatorStatus::Installing);
        let b = rows
            .iter()
            .find(|r| r.display_name().starts_with('b'))
            .unwrap();
        assert_eq!(b.upgrade_to.as_deref(), Some("1.1.0"));
        assert!(b.approvable().is_some());
        assert_eq!(
            Counts::of(&rows),
            Counts {
                installed: 6,
                waiting: 2,
                failing: 1
            }
        );
    }

    /// OLM says `ResolutionFailed` for a moment while a CSV replaces another: that's an
    /// upgrade in progress. It's a failure only when no operator runs.
    #[test]
    fn resolution_failures_count_only_without_a_running_operator() {
        let failing = |name: &str, installed: Option<&str>| {
            Arc::new(
                Subscription::parse(&json!({
                    "metadata": {"name": name, "namespace": "ops"},
                    "spec": {"name": name},
                    "status": {"installedCSV": installed, "conditions": [
                        {"type": "ResolutionFailed", "status": "True", "message": "constraints not satisfiable"}]}
                }))
                .unwrap(),
            )
        };
        let subs = [
            failing("upgrading", Some("upgrading.v2.0.0")),
            failing("running", Some("running.v1.0.0")),
            failing("missing", None),
        ];
        let csvs = [
            csv("upgrading.v2.0.0", "Installing"),
            csv("running.v1.0.0", "Succeeded"),
        ];
        let rows = join(&subs, &csvs, &[]);
        let row = |name: &str| rows.iter().find(|r| r.package() == Some(name)).unwrap();
        assert_eq!(row("upgrading").status, OperatorStatus::Installing);
        assert_eq!(row("running").status, OperatorStatus::Succeeded);
        assert!(
            row("running")
                .detail
                .as_deref()
                .unwrap()
                .contains("can't resolve")
        );
        assert_eq!(row("missing").status, OperatorStatus::Failed);
    }

    #[test]
    fn letter_tiles() {
        assert_eq!(initials("Strimzi"), "ST");
        assert_eq!(initials("cert-manager"), "CM");
        assert_eq!(initials("CloudNativePG"), "CN");
        assert_eq!(initials("Argo CD"), "AC");
        assert_eq!(initials(""), "");
    }
}
