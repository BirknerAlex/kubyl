//! The Applications table as text: its columns, filter and CSV records.

use jiff::Timestamp;
use kubyl_resources_core::format;

use crate::model::App;

/// Column ids and titles, in the order of the table and of the CSV.
pub const COLUMNS: [(&str, &str); 6] = [
    ("instance", "Instance"),
    ("namespace", "Namespace"),
    ("managed_by", "Managed by"),
    ("version", "Version"),
    ("age", "Age"),
    ("status", "Status"),
];

/// How old the application is: its oldest object's age.
pub fn age(app: &App, now: Timestamp) -> String {
    app.created
        .map(|t| format::human_duration(format::seconds_since(t, now)))
        .unwrap_or_default()
}

/// The text of one cell.
pub fn cell(app: &App, column: &str, now: Timestamp) -> String {
    match column {
        "instance" => app.instance.clone(),
        "namespace" => app.namespace.clone(),
        "managed_by" => app.manager.label(),
        "version" => app.version.clone(),
        "age" => age(app, now),
        "status" => app.health.label().to_string(),
        _ => String::new(),
    }
}

/// The header of the CSV export.
pub fn header() -> Vec<String> {
    COLUMNS.iter().map(|(_, title)| title.to_string()).collect()
}

/// One CSV record per application, in the order given (the table's sort and filter).
pub fn records<'a>(apps: impl IntoIterator<Item = &'a App>, now: Timestamp) -> Vec<Vec<String>> {
    apps.into_iter()
        .map(|app| COLUMNS.iter().map(|(id, _)| cell(app, id, now)).collect())
        .collect()
}

/// Whether `app` matches the filter box: every word must appear in the instance, namespace,
/// manager, version, status or the `name` labels (case-insensitive).
pub fn matches(app: &App, filter: &str) -> bool {
    let haystack = format!(
        "{} {} {} {} {} {}",
        app.instance,
        app.namespace,
        app.manager.label(),
        app.version,
        app.health.label(),
        app.names.iter().cloned().collect::<Vec<_>>().join(" "),
    )
    .to_lowercase();
    filter
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::health::Health;
    use crate::model::Manager;

    fn app(instance: &str, ns: &str, manager: Manager, health: Health) -> App {
        App {
            namespace: ns.into(),
            instance: instance.into(),
            names: BTreeSet::from(["web".to_string()]),
            part_of: None,
            manager,
            version: "1.4.2".into(),
            created: "2026-10-01T10:00:00Z".parse().ok(),
            health,
            members: Vec::new(),
        }
    }

    #[test]
    fn records_follow_the_columns() {
        let now: Timestamp = "2026-10-04T10:00:00Z".parse().unwrap();
        let apps = [
            app(
                "shop",
                "prod",
                Manager::Helm {
                    namespace: "prod".into(),
                    release: "shop".into(),
                },
                Health::Healthy,
            ),
            app(
                "=cmd|' /C calc'!A0",
                "prod",
                Manager::None,
                Health::Degraded,
            ),
        ];
        let records = records(&apps, now);
        assert_eq!(
            header(),
            [
                "Instance",
                "Namespace",
                "Managed by",
                "Version",
                "Age",
                "Status"
            ]
        );
        assert_eq!(
            records[0],
            ["shop", "prod", "Helm · shop", "1.4.2", "3d", "Healthy"]
        );
        assert_eq!(records[1][2], "");
        // The writer guards what the cluster controls (an instance label can hold anything).
        let csv = kubyl_base::csv::write(&header(), &records);
        assert!(csv.contains("\r\n'=cmd|' /C calc'!A0,prod,"), "{csv}");
    }

    #[test]
    fn filters_by_every_word() {
        let a = app(
            "shop",
            "prod",
            Manager::Label("kustomize".into()),
            Health::Progressing,
        );
        assert!(matches(&a, ""));
        assert!(matches(&a, "SHOP prod"));
        assert!(matches(&a, "kustomize progressing"));
        assert!(matches(&a, "web"));
        assert!(!matches(&a, "shop staging"));
    }
}
