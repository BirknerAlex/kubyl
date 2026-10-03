//! The Summary of a CronJob: its schedule and the Job history, each Job with its pods. The
//! Jobs and pods come from namespace stores, filtered by owner (Jobs: the CronJob's uid; pods:
//! the Job's uid or its `job-name` label).

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AnyElement, Context, IntoElement, SharedString, div, prelude::*};
use jiff::Timestamp;
use kubyl_core::{Gvr, ResourceRef};
use kubyl_resources::StoreStatus;
use kubyl_resources::columns::{
    job_completions, job_run_time, job_status, job_tone, pod_status, status_tone,
};
use kubyl_resources::format::{
    array_at, creation, human_duration, seconds_since, str_at, timestamp,
};
use kubyl_ui::{Colors, Selectable, StatusDot, fonts, h_flex, tone_color, u, v_flex};
use serde_json::Value;

use super::{DetailsContent, Target, kv, link, section};

/// Jobs shown in the history.
const SHOWN: usize = 10;

/// When a Job started: `status.startTime`, else its creation.
fn started(job: &Value) -> Option<Timestamp> {
    timestamp(str_at(job, "/status/startTime")).or_else(|| creation(job))
}

/// The Jobs owned by the CronJob `uid`, newest first.
pub(super) fn owned_jobs<'a>(
    uid: &str,
    jobs: impl IntoIterator<Item = &'a Arc<Value>>,
) -> Vec<&'a Arc<Value>> {
    let mut owned: Vec<&Arc<Value>> = jobs
        .into_iter()
        .filter(|job| {
            !uid.is_empty()
                && array_at(job, "/metadata/ownerReferences")
                    .iter()
                    .any(|o| str_at(o, "/uid") == uid)
        })
        .collect();
    owned.sort_by(|a, b| {
        (started(b), str_at(b, "/metadata/name")).cmp(&(started(a), str_at(a, "/metadata/name")))
    });
    owned
}

/// The pods of each Job (by Job name), oldest first so retries read in order. A pod with a Job
/// owner belongs to the one with that uid (a stale owner matches none); only a pod without
/// one is matched by its `job-name` label.
pub(super) fn group_pods<'a>(
    jobs: &[&Arc<Value>],
    pods: impl IntoIterator<Item = &'a Arc<Value>>,
) -> HashMap<String, Vec<&'a Arc<Value>>> {
    let by_uid: HashMap<&str, &str> = jobs
        .iter()
        .map(|job| (str_at(job, "/metadata/uid"), str_at(job, "/metadata/name")))
        .collect();
    let mut grouped: HashMap<String, Vec<&Arc<Value>>> = HashMap::new();
    for pod in pods {
        let owners: Vec<&Value> = array_at(pod, "/metadata/ownerReferences")
            .iter()
            .filter(|o| str_at(o, "/kind") == "Job")
            .collect();
        let name = if owners.is_empty() {
            let labels = &pod["metadata"]["labels"];
            ["batch.kubernetes.io/job-name", "job-name"]
                .iter()
                .find_map(|key| labels[*key].as_str())
        } else {
            owners
                .iter()
                .find_map(|o| by_uid.get(str_at(o, "/uid")).copied())
        };
        if let Some(name) = name {
            grouped.entry(name.to_string()).or_default().push(pod);
        }
    }
    for pods in grouped.values_mut() {
        pods.sort_by(|a, b| {
            (creation(a), str_at(a, "/metadata/name"))
                .cmp(&(creation(b), str_at(b, "/metadata/name")))
        });
    }
    grouped
}

/// `1/1 · 5m ago · took 12s`
fn meta(job: &Value, now: Timestamp) -> String {
    let mut parts = vec![job_completions(job)];
    if let Some(at) = started(job) {
        parts.push(format!("{} ago", age(at, now)));
    }
    if let Some(seconds) = job_run_time(job) {
        parts.push(format!("took {}", human_duration(seconds)));
    }
    parts.join(" · ")
}

/// `*/5 * * * *`, with its time zone when set.
fn schedule(cronjob: &Value) -> String {
    match str_at(cronjob, "/spec/timeZone") {
        "" => str_at(cronjob, "/spec/schedule").to_string(),
        zone => format!("{} ({zone})", str_at(cronjob, "/spec/schedule")),
    }
}

/// `5m`; a time in the future (clock skew) is `0s`.
fn age(at: Timestamp, now: Timestamp) -> String {
    human_duration(seconds_since(at, now).max(0))
}

fn ago(value: &str, now: Timestamp) -> Option<String> {
    timestamp(value).map(|at| format!("{} ago", age(at, now)))
}

impl DetailsContent {
    pub(super) fn render_cronjob(
        &self,
        cronjob: &Value,
        target: &Target,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let now = Timestamp::now();
        let mut rows: Vec<(&'static str, String)> = vec![("Schedule", schedule(cronjob))];
        if cronjob.pointer("/spec/suspend").and_then(Value::as_bool) == Some(true) {
            rows.push(("Suspended", "yes".into()));
        }
        if let Some(text) = ago(str_at(cronjob, "/status/lastScheduleTime"), now) {
            rows.push(("Last schedule", text));
        }
        if let Some(text) = ago(str_at(cronjob, "/status/lastSuccessfulTime"), now) {
            rows.push(("Last success", text));
        }
        rows.push((
            "Active",
            array_at(cronjob, "/status/active").len().to_string(),
        ));
        let mut out = vec![
            section("CronJob", colors)
                .child(kv(rows, colors))
                .into_any_element(),
        ];

        let Some(jobs) = &self.related.jobs else {
            return out;
        };
        let uid = str_at(cronjob, "/metadata/uid");
        let store = jobs.read(cx);
        let history = owned_jobs(uid, store.objects().values());
        if history.is_empty() {
            // Nothing to show while the list loads.
            if matches!(store.status(), StoreStatus::Ready) {
                out.push(
                    section("Job history", colors)
                        .child(
                            div()
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child(Selectable::new("jobs-none", "No Jobs.")),
                        )
                        .into_any_element(),
                );
            }
            return out;
        }
        let mut pods = self
            .related
            .pods
            .as_ref()
            .map(|p| group_pods(&history, p.read(cx).objects().values()))
            .unwrap_or_default();
        let mut list = v_flex().gap(u(8.0)).text_size(u(12.0));
        for job in history.iter().take(SHOWN) {
            let status = job_status(job);
            let color = tone_color(job_tone(status), colors);
            let name = str_at(job, "/metadata/name").to_string();
            let key = name.clone();
            let reference = ResourceRef::object(
                target.cluster.clone(),
                Gvr::new("batch", "v1", "jobs"),
                target.namespace.clone(),
                name.clone(),
            );
            let mut item = v_flex().gap(u(2.0)).child(
                h_flex()
                    .gap(u(8.0))
                    .child(StatusDot::new(color))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .child(link(
                                SharedString::from(format!("job-{key}")),
                                name,
                                reference,
                                colors,
                            )),
                    )
                    .child(div().text_color(color).child(Selectable::new(
                        SharedString::from(format!("job-{key}-status")),
                        status,
                    )))
                    .child(div().flex_1())
                    .child(div().text_color(colors.text_dim).text_size(u(11.5)).child(
                        Selectable::new(
                            SharedString::from(format!("job-{key}-meta")),
                            meta(job, now),
                        ),
                    )),
            );
            let job_pods = pods.remove(&key).unwrap_or_default();
            for pod in job_pods {
                let state = pod_status(pod);
                let pod_color = tone_color(status_tone(&state.reason), colors);
                let pod_name = str_at(pod, "/metadata/name").to_string();
                let pod_key = pod_name.clone();
                let reference = ResourceRef::object(
                    target.cluster.clone(),
                    Gvr::new("", "v1", "pods"),
                    target.namespace.clone(),
                    pod_name.clone(),
                );
                item = item.child(
                    h_flex()
                        .pl(u(15.0))
                        .gap(u(8.0))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_family(fonts::MONO)
                                .text_size(u(11.0))
                                .child(link(
                                    SharedString::from(format!("job-{key}-pod-{pod_key}")),
                                    pod_name,
                                    reference,
                                    colors,
                                )),
                        )
                        .child(div().text_size(u(11.0)).text_color(pod_color).child(
                            Selectable::new(
                                SharedString::from(format!("job-{key}-pod-{pod_key}-status")),
                                state.reason,
                            ),
                        )),
                );
            }
            list = list.child(item);
        }
        if history.len() > SHOWN {
            list = list.child(div().text_color(colors.text_dim).child(Selectable::new(
                "jobs-more",
                format!("… {} more", history.len() - SHOWN),
            )));
        }
        out.push(
            section(format!("Job history · {}", history.len()), colors)
                .child(list)
                .into_any_element(),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job(name: &str, owner: &str, start: &str, status: Value) -> Arc<Value> {
        Arc::new(json!({
            "metadata": {"name": name, "uid": format!("uid-{name}"),
                "ownerReferences": [{"kind": "CronJob", "uid": owner}]},
            "status": {"startTime": start, "conditions": status},
        }))
    }

    fn names(jobs: &[&Arc<Value>]) -> Vec<String> {
        jobs.iter()
            .map(|j| str_at(j, "/metadata/name").to_string())
            .collect()
    }

    #[test]
    fn jobs_are_owned_and_newest_first() {
        let all = [
            job("a", "cron", "2026-01-01T10:00:00Z", json!([])),
            job("b", "other", "2026-01-01T12:00:00Z", json!([])),
            job("c", "cron", "2026-01-01T11:00:00Z", json!([])),
        ];
        assert_eq!(names(&owned_jobs("cron", all.iter())), ["c", "a"]);
        assert!(owned_jobs("", all.iter()).is_empty());
    }

    #[test]
    fn the_schedule_shows_its_time_zone() {
        let plain = json!({"spec": {"schedule": "*/5 * * * *"}});
        assert_eq!(schedule(&plain), "*/5 * * * *");
        let zoned = json!({"spec": {"schedule": "0 3 * * *", "timeZone": "Europe/Berlin"}});
        assert_eq!(schedule(&zoned), "0 3 * * * (Europe/Berlin)");
    }

    #[test]
    fn a_job_without_start_time_sorts_by_creation() {
        let created = Arc::new(
            json!({"metadata": {"name": "b", "creationTimestamp": "2026-01-01T11:30:00Z",
            "ownerReferences": [{"uid": "cron"}]}}),
        );
        let all = [
            job("a", "cron", "2026-01-01T11:00:00Z", json!([])),
            created,
            job("c", "cron", "2026-01-01T12:00:00Z", json!([])),
        ];
        assert_eq!(names(&owned_jobs("cron", all.iter())), ["c", "b", "a"]);
    }

    #[test]
    fn meta_reads_age_and_run_time_and_clamps_skew() {
        let running = json!({"status": {"startTime": "2026-01-01T10:00:00Z", "active": 1}});
        let now: Timestamp = "2026-01-01T10:05:00Z".parse().unwrap();
        assert_eq!(meta(&running, now), "0/1 · 5m ago");
        let early: Timestamp = "2026-01-01T09:59:00Z".parse().unwrap();
        assert_eq!(meta(&running, early), "0/1 · 0s ago");
        let done = json!({"status": {"startTime": "2026-01-01T10:00:00Z",
            "completionTime": "2026-01-01T10:00:12Z", "succeeded": 1}});
        assert_eq!(meta(&done, now), "1/1 · 5m ago · took 12s");
    }

    #[test]
    fn pods_group_by_owner_uid_and_only_ownerless_pods_by_label() {
        let jobs = [job("job-1", "cron", "2026-01-01T10:00:00Z", json!([]))];
        let jobs: Vec<&Arc<Value>> = jobs.iter().collect();
        let pod = |name: &str, created: &str, owners: Value, label: &str| {
            Arc::new(
                json!({"metadata": {"name": name, "creationTimestamp": created,
                "labels": {"batch.kubernetes.io/job-name": label}, "ownerReferences": owners}}),
            )
        };
        let owned = |uid: &str| json!([{"kind": "Job", "uid": uid}]);
        let pods = [
            pod("retry", "2026-01-01T10:02:00Z", owned("uid-job-1"), "job-1"),
            pod("first", "2026-01-01T10:00:00Z", owned("uid-job-1"), "job-1"),
            // A pod of a deleted Job that was recreated under the same name.
            pod("stale", "2026-01-01T09:00:00Z", owned("old-uid"), "job-1"),
            pod("labelled", "2026-01-01T10:03:00Z", json!([]), "job-1"),
            pod("other", "2026-01-01T10:00:00Z", owned("uid-job-2"), "job-2"),
        ];
        let grouped = group_pods(&jobs, pods.iter());
        assert_eq!(names(&grouped["job-1"]), ["first", "retry", "labelled"]);
        assert_eq!(grouped.len(), 1);
    }
}
