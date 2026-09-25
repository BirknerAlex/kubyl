//! Columns for the explorer's generic tables of Argo CD kinds (Custom Resources → argoproj.io),
//! so they show sync, health and sources instead of only the CRDs' printer columns.

use gpui::App;
use kubyl_core::{CellValue, ColumnDef, ColumnProvider, ColumnWidth, ResourceColumns, Tone};
use serde_json::Value;

use crate::model::{Application, ApplicationSet, GROUP, Project};
use crate::widgets::{health_tone, sync_tone};

fn text(value: impl Into<String>) -> CellValue {
    CellValue::Text(value.into().into())
}

fn muted(value: impl Into<String>) -> CellValue {
    CellValue::Tinted {
        label: value.into().into(),
        tone: Tone::Neutral,
    }
}

fn name(object: &Value) -> CellValue {
    text(
        object
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
}

fn age(object: &Value) -> CellValue {
    muted(kubyl_resources::format::object_age(
        object,
        jiff::Timestamp::now(),
    ))
}

fn flex(id: &'static str, title: &'static str, min: f32) -> ColumnDef {
    ColumnDef::new(id, title, ColumnWidth::Flex { weight: 1.0, min })
}

fn fixed(id: &'static str, title: &'static str, width: f32) -> ColumnDef {
    ColumnDef::new(id, title, ColumnWidth::Fixed(width))
}

struct Applications;

impl ColumnProvider for Applications {
    fn columns(&self) -> Vec<ColumnDef> {
        vec![
            flex("name", "Name", 160.0).mono(),
            fixed("project", "Project", 96.0),
            fixed("sync", "Sync", 116.0),
            fixed("health", "Health", 110.0),
            fixed("auto", "Auto-sync", 80.0),
            flex("source", "Source", 160.0).mono(),
            fixed("revision", "Revision", 76.0).mono(),
            flex("destination", "Destination", 130.0),
            fixed("age", "Age", 54.0).mono(),
        ]
    }

    fn cell(&self, object: &Value, column: &str) -> CellValue {
        match column {
            "name" => return name(object),
            "age" => return age(object),
            _ => {}
        }
        let Some(app) = Application::parse(object) else {
            return CellValue::Empty;
        };
        match column {
            "project" => muted(app.spec.project.clone()),
            "sync" => match app.activity() {
                Some(activity) => CellValue::Status {
                    label: activity.label().into(),
                    tone: Tone::Info,
                },
                None => CellValue::Status {
                    label: app.sync().label().into(),
                    tone: sync_tone(app.sync()),
                },
            },
            "health" => CellValue::Status {
                label: app.health().label().into(),
                tone: health_tone(app.health()),
            },
            "auto" => {
                let policy = app.policy();
                if policy.auto_sync() {
                    let mut label = "on".to_string();
                    if policy.prune() {
                        label.push_str(" · prune");
                    }
                    if policy.self_heal() {
                        label.push_str(" · heal");
                    }
                    CellValue::Tinted {
                        label: label.into(),
                        tone: Tone::Good,
                    }
                } else {
                    CellValue::Tinted {
                        label: "manual".into(),
                        tone: Tone::Muted,
                    }
                }
            }
            "source" => match app.spec.all_sources().as_slice() {
                [] => CellValue::Empty,
                [one] => text(format!("{} @ {}", one.what(), one.target())),
                many => text(format!("{} sources", many.len())),
            },
            "revision" => text(
                app.synced_revisions()
                    .iter()
                    .map(|r| crate::model::short_revision(r))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            "destination" => muted(app.spec.destination.label()),
            _ => CellValue::Empty,
        }
    }
}

struct ApplicationSets;

impl ColumnProvider for ApplicationSets {
    fn columns(&self) -> Vec<ColumnDef> {
        vec![
            flex("name", "Name", 160.0).mono(),
            flex("generators", "Generators", 200.0),
            fixed("policy", "Policy", 130.0),
            fixed("status", "Status", 150.0),
            fixed("age", "Age", 54.0).mono(),
        ]
    }

    fn cell(&self, object: &Value, column: &str) -> CellValue {
        match column {
            "name" => return name(object),
            "age" => return age(object),
            _ => {}
        }
        let Some(set) = ApplicationSet::parse(object) else {
            return CellValue::Empty;
        };
        match column {
            "generators" => text(set.generators_summary()),
            "policy" => muted(set.policy_summary()),
            "status" => match set.status.conditions.iter().find(|c| c.is_problem()) {
                Some(problem) => CellValue::Status {
                    label: problem.kind.clone().into(),
                    tone: Tone::Bad,
                },
                None => CellValue::Status {
                    label: "OK".into(),
                    tone: Tone::Good,
                },
            },
            _ => CellValue::Empty,
        }
    }
}

struct Projects;

impl ColumnProvider for Projects {
    fn columns(&self) -> Vec<ColumnDef> {
        vec![
            flex("name", "Name", 140.0).mono(),
            flex("description", "Description", 180.0),
            fixed("destinations", "Destinations", 100.0).mono(),
            fixed("windows", "Sync windows", 130.0),
            fixed("age", "Age", 54.0).mono(),
        ]
    }

    fn cell(&self, object: &Value, column: &str) -> CellValue {
        match column {
            "name" => return name(object),
            "age" => return age(object),
            _ => {}
        }
        let Some(project) = Project::parse(object) else {
            return CellValue::Empty;
        };
        match column {
            "description" => muted(project.spec.description.unwrap_or_default()),
            "destinations" => text(project.spec.destinations.len().to_string()),
            "windows" => {
                let now = jiff::Timestamp::now();
                let windows = &project.spec.sync_windows;
                if windows
                    .iter()
                    .any(|w| w.kind == "deny" && crate::windows::is_active(w, now))
                {
                    CellValue::Status {
                        label: "deny now".into(),
                        tone: Tone::Bad,
                    }
                } else if windows.is_empty() {
                    CellValue::Empty
                } else {
                    text(format!("{}", windows.len()))
                }
            }
            _ => CellValue::Empty,
        }
    }
}

pub(crate) fn init(cx: &mut App) {
    ResourceColumns::register(cx, GROUP, "Application", Applications);
    ResourceColumns::register(cx, GROUP, "ApplicationSet", ApplicationSets);
    ResourceColumns::register(cx, GROUP, "AppProject", Projects);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_cells() {
        let object = crate::model::tests::guestbook();
        let columns = Applications;
        assert!(matches!(
            columns.cell(&object, "sync"),
            CellValue::Status {
                tone: Tone::Good,
                ..
            }
        ));
        assert_eq!(
            columns.cell(&object, "source"),
            CellValue::Text("guestbook @ HEAD".into())
        );
        assert_eq!(
            columns.cell(&object, "name"),
            CellValue::Text("guestbook".into())
        );
        assert_eq!(
            columns.cell(&serde_json::json!({}), "sync"),
            CellValue::Status {
                label: "Unknown".into(),
                tone: Tone::Muted
            }
        );
    }
}
