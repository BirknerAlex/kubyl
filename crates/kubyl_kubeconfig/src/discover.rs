//! "Discover cloud clusters" (phase 25, board 23): every cluster of every AWS profile, Azure
//! subscription and Google Cloud project, found with the installed `aws`, `az` and `gcloud`,
//! and added with their own get-credentials commands into one new kubeconfig (opened unsaved
//! in the editor, like the single-cluster import, to test and save as a Kubyl-owned file).
//!
//! No sign-in of Kubyl's own and nothing stored: the lists live while the dialog is open. The
//! work is `kubyl_kubeconfig_core::cloud`; a missing CLI, an expired session or a denied
//! permission shows what to run or which permission is missing, next to the account it is about.

use std::collections::{BTreeSet, HashMap};

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    App, AppContext as _, Context, FocusHandle, Focusable, IntoElement, Render, SharedString, Task,
    Window, div, prelude::*,
};
use gpui_component::Disableable as _;
use gpui_component::checkbox::Checkbox;
use kubyl_core::{Notification, NotificationCenter, spawn_kube};
use kubyl_kubeconfig_core::cloud::{
    self, Account, Added, Cluster, Failure, Provider, Scan, SystemRunner,
};
use kubyl_ui::{ActiveColors, Button, IconName, fonts, h_flex, u, v_flex};

use crate::dialogs;
use crate::files;
use crate::model::Doc;
use crate::state::Draft;
use crate::validate::Severity;
use crate::widgets;

/// Where a cloud's scan is.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    /// Listing profiles, subscriptions or projects.
    Listing,
    /// Scanning accounts: how many are done of how many.
    Scanning {
        done: usize,
        total: usize,
    },
    Done,
    /// The accounts couldn't be listed.
    Failed(Failure),
}

#[derive(Clone, Debug)]
pub struct CloudState {
    pub phase: Phase,
    pub accounts: Vec<Account>,
    pub scans: Vec<Scan>,
}

impl Default for CloudState {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            accounts: Vec::new(),
            scans: Vec::new(),
        }
    }
}

enum Event {
    Accounts(Vec<Account>),
    Scan(Scan),
    Finished(Result<(), Failure>),
}

pub struct DiscoverView {
    provider: Provider,
    clouds: HashMap<Provider, CloudState>,
    /// `Cluster::key`s of the checked clusters (across clouds).
    selected: BTreeSet<String>,
    busy_adding: bool,
    error: Option<String>,
    focus: FocusHandle,
    tasks: HashMap<Provider, Task<()>>,
    _adding: Option<Task<()>>,
}

/// Opens the dialog.
pub fn open(window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| DiscoverView {
        provider: Provider::Aws,
        clouds: HashMap::new(),
        selected: BTreeSet::new(),
        busy_adding: false,
        error: None,
        focus: cx.focus_handle(),
        tasks: HashMap::new(),
        _adding: None,
    });
    let focus = view.read(cx).focus.clone();
    dialogs::open(view, 760.0, Some(focus), window, cx);
}

impl Focusable for DiscoverView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl DiscoverView {
    fn state(&self, provider: Provider) -> CloudState {
        self.clouds.get(&provider).cloned().unwrap_or_default()
    }

    /// A new scan replaces the cloud's results and its checked clusters, not other clouds'.
    fn begin_scan(&mut self, provider: Provider) {
        let keep: BTreeSet<String> = self
            .selected
            .iter()
            .filter(|key| !key.starts_with(&format!("{provider:?}/")))
            .cloned()
            .collect();
        self.selected = keep;
        self.clouds.insert(
            provider,
            CloudState {
                phase: Phase::Listing,
                ..Default::default()
            },
        );
    }

    /// Lists the accounts of a cloud and scans each, streaming results into the view.
    pub fn scan(&mut self, provider: Provider, cx: &mut Context<Self>) {
        self.begin_scan(provider);
        let (tx, mut rx) = mpsc::unbounded();
        let work = spawn_kube(cx, async move {
            let runner = SystemRunner::default();
            let (accounts_tx, scans_tx) = (tx.clone(), tx.clone());
            let result = cloud::discover(
                provider,
                &runner,
                move |accounts| {
                    accounts_tx
                        .unbounded_send(Event::Accounts(accounts.to_vec()))
                        .ok();
                },
                move |scan| {
                    scans_tx.unbounded_send(Event::Scan(scan)).ok();
                },
            )
            .await;
            tx.unbounded_send(Event::Finished(result)).ok();
        });
        let task = cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let alive = this
                    .update(cx, |this, cx| this.apply(provider, event, cx))
                    .is_ok();
                if !alive {
                    break;
                }
            }
            // The work ends with the channel; dropping the task before would cancel it.
            drop(work);
        });
        self.tasks.insert(provider, task);
        cx.notify();
    }

    fn apply(&mut self, provider: Provider, event: Event, cx: &mut Context<Self>) {
        let state = self.clouds.entry(provider).or_default();
        match event {
            Event::Accounts(accounts) => {
                state.phase = Phase::Scanning {
                    done: 0,
                    total: accounts.len(),
                };
                state.accounts = accounts;
            }
            Event::Scan(scan) => {
                state.scans.push(scan);
                state
                    .scans
                    .sort_by(|a, b| a.account.label.cmp(&b.account.label));
                if let Phase::Scanning { total, .. } = state.phase {
                    state.phase = Phase::Scanning {
                        done: state.scans.len(),
                        total,
                    };
                }
            }
            Event::Finished(Ok(())) => state.phase = Phase::Done,
            Event::Finished(Err(failure)) => state.phase = Phase::Failed(failure),
        }
        cx.notify();
    }

    /// The checked clusters, in the order of the lists.
    pub fn chosen(&self) -> Vec<Cluster> {
        Provider::ALL
            .iter()
            .filter_map(|p| self.clouds.get(p))
            .flat_map(|state| state.scans.iter())
            .flat_map(|scan| scan.clusters.iter())
            .filter(|c| self.selected.contains(&c.key()))
            .cloned()
            .collect()
    }

    fn toggle(&mut self, key: String, on: bool, cx: &mut Context<Self>) {
        if on {
            self.selected.insert(key);
        } else {
            self.selected.remove(&key);
        }
        cx.notify();
    }

    fn set_all(&mut self, scan: &Scan, on: bool, cx: &mut Context<Self>) {
        for cluster in &scan.clusters {
            if on {
                self.selected.insert(cluster.key());
            } else {
                self.selected.remove(&cluster.key());
            }
        }
        cx.notify();
    }

    /// The name of the draft the clusters are added as.
    fn draft_name(chosen: &[Cluster]) -> String {
        match chosen {
            [one] => format!("{}-{}", one.provider.cli(), one.name),
            many if many.iter().all(|c| c.provider == many[0].provider) => {
                format!("{}-clusters", many[0].provider.cli())
            }
            _ => "cloud-clusters".into(),
        }
    }

    /// Runs get-credentials for the checked clusters into one private temp kubeconfig and
    /// opens the result as an unsaved kubeconfig.
    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = self.chosen();
        if chosen.is_empty() || self.busy_adding {
            return;
        }
        self.busy_adding = true;
        self.error = None;
        let work = chosen.clone();
        let run = spawn_kube(cx, async move {
            let dir = tempfile::Builder::new()
                .prefix("kubyl-cloud-")
                .tempdir()
                .map_err(|err| format!("Couldn't create a temp folder: {err}"))?;
            let file = dir.path().join("config");
            let added = cloud::add_clusters(&SystemRunner::default(), &work, &file).await;
            // The temp folder (and what the CLIs wrote there) goes away here.
            drop(dir);
            Ok::<_, String>(added)
        });
        let name = Self::draft_name(&chosen);
        self._adding = Some(cx.spawn_in(window, async move |this, cx| {
            let added = run.await;
            this.update_in(cx, |this, window, cx| {
                this.busy_adding = false;
                match added {
                    Ok(added) => this.finish(added, &name, window, cx),
                    Err(why) => {
                        this.error = Some(why);
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn finish(&mut self, added: Added, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Added {
            kubeconfig,
            added,
            failed,
        } = added;
        let failures: Vec<String> = failed
            .iter()
            .map(|(cluster, failure)| format!("{}: {failure}", cluster.name))
            .collect();
        let Some(text) = kubeconfig else {
            self.error = Some(if !added.is_empty() {
                "The CLIs added the clusters, but their kubeconfig couldn't be read back.".into()
            } else if failures.is_empty() {
                "No cluster was added.".into()
            } else {
                failures.join("\n")
            });
            cx.notify();
            return;
        };
        match Doc::parse(&text) {
            Ok(doc) => {
                crate::actions::open_draft(
                    Draft {
                        path: files::new_owned_path(
                            &crate::state::Kubeconfigs::dirs(cx).owned,
                            name,
                        ),
                        title: name.to_string(),
                        doc,
                    },
                    cx,
                );
                if !failures.is_empty() {
                    NotificationCenter::push(
                        cx,
                        Notification::warning(format!(
                            "Added {} of {} clusters. {}",
                            added.len(),
                            added.len() + failed.len(),
                            failures.join(" ")
                        )),
                    );
                }
                use gpui_component::WindowExt as _;
                window.close_dialog(cx);
            }
            Err(err) => {
                self.error = Some(format!(
                    "The CLIs wrote a kubeconfig Kubyl can't read: {err}"
                ));
                cx.notify();
            }
        }
    }
}

fn failure_severity(failure: &Failure) -> Severity {
    if failure.is_benign() {
        Severity::Info
    } else {
        Severity::Warning
    }
}

impl Render for DiscoverView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let weak = cx.entity().downgrade();
        let provider = self.provider;
        let state = self.state(provider);
        let seg = {
            let weak = weak.clone();
            widgets::segmented(
                "discover-cloud",
                Provider::ALL
                    .iter()
                    .map(|p| (*p, SharedString::from(p.label()), Some(IconName::Cloud)))
                    .collect(),
                provider,
                &colors,
                move |p, _, cx| {
                    let p = *p;
                    weak.update(cx, |this, cx| {
                        this.provider = p;
                        this.error = None;
                        cx.notify();
                    })
                    .ok();
                },
            )
        };
        let scanning = matches!(state.phase, Phase::Listing | Phase::Scanning { .. });
        let status = match &state.phase {
            Phase::Idle => format!(
                "Scans every {} the `{}` CLI can see.",
                provider.account_label(),
                provider.cli()
            ),
            Phase::Listing => format!("Listing {}s…", provider.account_label()),
            Phase::Scanning { done, total } => {
                format!("Scanned {done} of {total} {}s…", provider.account_label())
            }
            Phase::Done => {
                let clusters: usize = state.scans.iter().map(|s| s.clusters.len()).sum();
                format!(
                    "{clusters} cluster{} in {} {}{}.",
                    if clusters == 1 { "" } else { "s" },
                    state.scans.len(),
                    provider.account_label(),
                    if state.scans.len() == 1 { "" } else { "s" }
                )
            }
            Phase::Failed(_) => String::new(),
        };
        let scan_weak = weak.clone();
        let controls = h_flex()
            .gap(u(10.0))
            .child(seg)
            .child(
                Button::new("discover-scan")
                    .primary()
                    .icon(IconName::RefreshCw)
                    .label(if state.phase == Phase::Idle {
                        "Scan"
                    } else {
                        "Scan again"
                    })
                    .disabled(scanning)
                    .on_click(move |_, _, cx| {
                        scan_weak
                            .update(cx, |this, cx| this.scan(provider, cx))
                            .ok();
                    }),
            )
            .child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(status),
            );

        let mut list = v_flex().gap(u(10.0));
        if let Phase::Failed(failure) = &state.phase {
            list = list.child(widgets::notice(
                Severity::Warning,
                failure.to_string(),
                &colors,
            ));
        }
        for (si, scan) in state.scans.iter().enumerate() {
            let account = &scan.account;
            let all_on = !scan.clusters.is_empty()
                && scan
                    .clusters
                    .iter()
                    .all(|c| self.selected.contains(&c.key()));
            let scan_for_all = scan.clone();
            let toggle_weak = weak.clone();
            let mut card = v_flex()
                .gap(u(4.0))
                .p(u(10.0))
                .rounded(u(8.0))
                .border_1()
                .border_color(colors.border)
                .bg(colors.panel)
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .child(
                            Checkbox::new(("discover-all", si))
                                .checked(all_on)
                                .disabled(scan.clusters.is_empty())
                                .on_click(move |checked: &bool, _, cx| {
                                    let on = *checked;
                                    let scan = scan_for_all.clone();
                                    toggle_weak
                                        .update(cx, |this, cx| this.set_all(&scan, on, cx))
                                        .ok();
                                }),
                        )
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(account.label.clone()),
                        )
                        .child(div().text_size(u(11.5)).text_color(colors.text_dim).child(
                            match (account.id == account.label, account.detail.is_empty()) {
                                (false, _) => account.id.clone(),
                                (true, false) => account.detail.clone(),
                                (true, true) => String::new(),
                            },
                        ))
                        .child(div().flex_1())
                        .child(div().text_size(u(11.5)).text_color(colors.text_dim).child(
                            format!(
                                "{} cluster{}",
                                scan.clusters.len(),
                                if scan.clusters.len() == 1 { "" } else { "s" }
                            ),
                        )),
                );
            for (ci, cluster) in scan.clusters.iter().enumerate() {
                let key = cluster.key();
                let on = self.selected.contains(&key);
                let toggle_weak = weak.clone();
                card = card.child(
                    h_flex()
                        .id(("discover-cluster", si * 1000 + ci))
                        .gap(u(10.0))
                        .pl(u(24.0))
                        .h(u(26.0))
                        .child(
                            Checkbox::new(("discover-pick", si * 1000 + ci))
                                .checked(on)
                                .on_click(move |checked: &bool, _, cx| {
                                    let (key, on) = (key.clone(), *checked);
                                    toggle_weak
                                        .update(cx, |this, cx| this.toggle(key, on, cx))
                                        .ok();
                                }),
                        )
                        .child(
                            div()
                                .w(u(240.0))
                                .truncate()
                                .font_family(fonts::MONO)
                                .text_size(u(12.0))
                                .child(cluster.name.clone()),
                        )
                        .child(
                            div()
                                .w(u(130.0))
                                .truncate()
                                .text_size(u(12.0))
                                .text_color(colors.text_muted)
                                .child(match &cluster.group {
                                    Some(group) => format!("{} · {group}", cluster.region),
                                    None => cluster.region.clone(),
                                }),
                        )
                        .child(
                            div()
                                .w(u(110.0))
                                .font_family(fonts::MONO)
                                .text_size(u(11.5))
                                .text_color(colors.text_dim)
                                .child(cluster.version.clone()),
                        )
                        .child(
                            div()
                                .text_size(u(11.5))
                                .text_color(status_color(&cluster.status, &colors))
                                .child(cluster.status.clone()),
                        ),
                );
            }
            for problem in &scan.problems {
                let text = if problem.scope.is_empty() {
                    problem.failure.to_string()
                } else {
                    format!("{}: {}", problem.scope, problem.failure)
                };
                card = card.child(div().pl(u(24.0)).pt(u(2.0)).child(widgets::notice(
                    failure_severity(&problem.failure),
                    text,
                    &colors,
                )));
            }
            list = list.child(card);
        }
        if state.phase == Phase::Done && state.scans.is_empty() {
            list = list.child(widgets::hint(
                format!(
                    "No {} found. Is the CLI signed in to the right account?",
                    provider.account_label()
                ),
                &colors,
            ));
        }
        let chosen = self.chosen();
        let add_weak = weak.clone();
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .child(dialogs::header(
                IconName::Cloud,
                "Discover cloud clusters",
                None,
                &colors,
            ))
            .child(
                v_flex()
                    .gap(u(12.0))
                    .p(u(16.0))
                    .child(controls)
                    .child(widgets::hint(
                        "Uses your installed aws, az and gcloud with their current sign-in (your login shell's PATH): nothing is stored, and Kubyl asks nothing of the clouds but the lists and each cluster's get-credentials.",
                        &colors,
                    ))
                    .child(
                        div()
                            .id("discover-list")
                            .debug_selector(|| "discover-list".into())
                            .max_h(u(380.0))
                            .overflow_y_scroll()
                            .child(list),
                    )
                    .children(self.error.clone().map(|e| widgets::notice(Severity::Error, e, &colors))),
            )
            .child(dialogs::footer(
                Some(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child(format!("{} selected", chosen.len()))
                        .into_any_element(),
                ),
                vec![
                    dialogs::cancel_button("discover-cancel"),
                    Button::new("discover-add")
                        .primary()
                        .icon(IconName::Plus)
                        .label(if self.busy_adding {
                            "Getting credentials…".to_string()
                        } else {
                            format!("Add {} cluster{}", chosen.len(), if chosen.len() == 1 { "" } else { "s" })
                        })
                        .disabled(chosen.is_empty() || self.busy_adding)
                        .on_click(move |_, window, cx| {
                            add_weak.update(cx, |this, cx| this.add(window, cx)).ok();
                        })
                        .into_any_element(),
                ],
                &colors,
            ))
    }
}

fn status_color(status: &str, colors: &kubyl_ui::Colors) -> gpui::Hsla {
    match status.to_ascii_uppercase().as_str() {
        "ACTIVE" | "RUNNING" | "SUCCEEDED" => colors.green,
        "STOPPED" | "FAILED" | "ERROR" | "DEGRADED" => colors.red,
        "" => colors.text_dim,
        _ => colors.yellow,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{Entity, TestAppContext, VisualTestContext};
    use kubyl_core::NotificationCenter;
    use kubyl_kubeconfig_core::cloud::Problem;

    use super::*;
    use crate::state::{Dirs, Kubeconfigs};

    const KUBECONFIG: &str = "apiVersion: v1\nkind: Config\nclusters:\n- name: arn:aws:eks:eu-west-1:1:cluster/prod-eu\n  cluster:\n    server: https://x.eks.amazonaws.com\ncontexts:\n- name: prod-eu\n  context:\n    cluster: arn:aws:eks:eu-west-1:1:cluster/prod-eu\n    user: prod-eu\nusers:\n- name: prod-eu\n  user:\n    exec:\n      apiVersion: client.authentication.k8s.io/v1beta1\n      command: aws\n      args: [eks, get-token, --cluster-name, prod-eu]\ncurrent-context: prod-eu\n";

    fn cluster(provider: Provider, account: &str, name: &str, region: &str) -> Cluster {
        Cluster {
            provider,
            account: account.into(),
            name: name.into(),
            region: region.into(),
            group: None,
            version: "1.31".into(),
            status: "ACTIVE".into(),
        }
    }

    fn account(provider: Provider, id: &str) -> Account {
        Account {
            provider,
            id: id.into(),
            label: id.into(),
            detail: String::new(),
        }
    }

    fn open_view(
        cx: &mut TestAppContext,
    ) -> (
        Entity<DiscoverView>,
        &mut VisualTestContext,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            owned: dir.path().join("config/kubeconfigs"),
            backups: dir.path().join("config/backups"),
        };
        std::fs::create_dir_all(&dirs.owned).unwrap();
        let config = dir.path().join("config");
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, &config);
            kubyl_ui::init(cx);
            gpui_component::init(cx);
            Kubeconfigs::install_with(dirs, cx);
        });
        let slot: Rc<RefCell<Option<Entity<DiscoverView>>>> = Rc::default();
        let (_root, cx) = cx.add_window_view({
            let slot = slot.clone();
            move |window, cx| {
                let view = cx.new(|cx| DiscoverView {
                    provider: Provider::Aws,
                    clouds: HashMap::new(),
                    selected: BTreeSet::new(),
                    busy_adding: false,
                    error: None,
                    focus: cx.focus_handle(),
                    tasks: HashMap::new(),
                    _adding: None,
                });
                *slot.borrow_mut() = Some(view.clone());
                gpui_component::Root::new(view, window, cx)
            }
        });
        cx.run_until_parked();
        let view = slot.borrow().clone().unwrap();
        (view, cx, dir)
    }

    #[gpui::test]
    fn results_stream_in_and_selected_clusters_are_listed_in_order(cx: &mut TestAppContext) {
        let (view, cx, _dir) = open_view(cx);
        let prod = account(Provider::Aws, "prod");
        let broken = account(Provider::Aws, "broken");
        view.update(cx, |view, cx| {
            view.apply(
                Provider::Aws,
                Event::Accounts(vec![prod.clone(), broken.clone()]),
                cx,
            );
            assert_eq!(
                view.state(Provider::Aws).phase,
                Phase::Scanning { done: 0, total: 2 }
            );
            view.apply(
                Provider::Aws,
                Event::Scan(Scan {
                    account: broken.clone(),
                    clusters: vec![],
                    problems: vec![Problem {
                        scope: String::new(),
                        failure: Failure::SignIn {
                            command: "aws sso login --profile broken".into(),
                            why: "The SSO session expired.".into(),
                        },
                    }],
                }),
                cx,
            );
            view.apply(
                Provider::Aws,
                Event::Scan(Scan {
                    account: prod.clone(),
                    clusters: vec![
                        cluster(Provider::Aws, "prod", "prod-eu", "eu-west-1"),
                        cluster(Provider::Aws, "prod", "staging-eu", "eu-west-1"),
                    ],
                    problems: vec![Problem {
                        scope: "eu-central-1".into(),
                        failure: Failure::Permission {
                            permission: "eks:ListClusters".into(),
                        },
                    }],
                }),
                cx,
            );
            assert_eq!(
                view.state(Provider::Aws).phase,
                Phase::Scanning { done: 2, total: 2 }
            );
            view.apply(Provider::Aws, Event::Finished(Ok(())), cx);
            assert_eq!(view.state(Provider::Aws).phase, Phase::Done);
            // Accounts sort by label.
            let labels: Vec<String> = view
                .state(Provider::Aws)
                .scans
                .iter()
                .map(|s| s.account.label.clone())
                .collect();
            assert_eq!(labels, ["broken", "prod"]);
            // Checking a cluster, and all of an account's.
            assert!(view.chosen().is_empty());
            let first = view.state(Provider::Aws).scans[1].clusters[0].key();
            view.toggle(first, true, cx);
            assert_eq!(view.chosen().len(), 1);
            let scan = view.state(Provider::Aws).scans[1].clone();
            view.set_all(&scan, true, cx);
            let names: Vec<String> = view.chosen().iter().map(|c| c.name.clone()).collect();
            assert_eq!(names, ["prod-eu", "staging-eu"]);
            view.set_all(&scan, false, cx);
            assert!(view.chosen().is_empty());
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("discover-list").is_some());
    }

    #[gpui::test]
    fn a_failed_listing_and_a_new_scan_keep_other_clouds_choices(cx: &mut TestAppContext) {
        let (view, cx, _dir) = open_view(cx);
        view.update(cx, |view, cx| {
            let gke = cluster(Provider::Google, "acme", "analytics", "europe-west4");
            view.apply(
                Provider::Google,
                Event::Accounts(vec![account(Provider::Google, "acme")]),
                cx,
            );
            view.apply(
                Provider::Google,
                Event::Scan(Scan {
                    account: account(Provider::Google, "acme"),
                    clusters: vec![gke.clone()],
                    problems: vec![],
                }),
                cx,
            );
            view.toggle(gke.key(), true, cx);
            // AWS can't list profiles: the cloud shows the failure, the choice in Google stays.
            view.apply(
                Provider::Aws,
                Event::Finished(Err(Failure::NotInstalled(Provider::Aws))),
                cx,
            );
            assert!(matches!(
                view.state(Provider::Aws).phase,
                Phase::Failed(Failure::NotInstalled(_))
            ));
            assert_eq!(view.chosen().len(), 1);
            // Scanning Google again forgets Google's choices only.
            view.selected.insert("Aws/p/r/other".into());
            view.begin_scan(Provider::Google);
            assert_eq!(view.state(Provider::Google).phase, Phase::Listing);
            assert!(view.state(Provider::Google).scans.is_empty());
            assert!(view.selected.iter().all(|k| !k.starts_with("Google/")));
            assert!(view.selected.contains("Aws/p/r/other"));
        });
    }

    #[test]
    fn drafts_are_named_after_what_was_added() {
        let eks = cluster(Provider::Aws, "p", "prod-eu", "eu-west-1");
        let eks2 = cluster(Provider::Aws, "p", "staging-eu", "eu-west-1");
        let gke = cluster(Provider::Google, "p", "a", "r");
        assert_eq!(
            DiscoverView::draft_name(std::slice::from_ref(&eks)),
            "aws-prod-eu"
        );
        assert_eq!(
            DiscoverView::draft_name(&[eks.clone(), eks2.clone()]),
            "aws-clusters"
        );
        assert_eq!(DiscoverView::draft_name(&[eks, gke]), "cloud-clusters");
    }

    #[gpui::test]
    fn added_clusters_open_as_an_unsaved_kubeconfig_and_failures_are_reported(
        cx: &mut TestAppContext,
    ) {
        let (view, cx, _dir) = open_view(cx);
        let before = cx.update(|_, cx| NotificationCenter::global(cx).latest_id());
        let prod = cluster(Provider::Aws, "prod", "prod-eu", "eu-west-1");
        let staging = cluster(Provider::Aws, "prod", "staging-eu", "eu-west-1");
        view.update_in(cx, |view, window, cx| {
            view.finish(
                Added {
                    kubeconfig: Some(KUBECONFIG.into()),
                    added: vec![prod.clone()],
                    failed: vec![(
                        staging.clone(),
                        Failure::Permission {
                            permission: "eks:DescribeCluster".into(),
                        },
                    )],
                },
                "aws-clusters",
                window,
                cx,
            );
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            // Opened as a draft, not written anywhere.
            let global = Kubeconfigs::global(cx);
            let draft = (1..6).find_map(|id| global.update(cx, |g, _| g.take_draft(id)));
            let draft = draft.expect("the clusters are an unsaved draft");
            assert_eq!(draft.title, "aws-clusters");
            assert!(draft.path.starts_with(Kubeconfigs::dirs(cx).owned));
            assert!(!draft.path.exists());
            // The one the profile may not describe is told about.
            let toasts: Vec<String> = NotificationCenter::global(cx)
                .since(before)
                .map(|(_, n)| n.message.to_string())
                .collect();
            assert!(
                toasts.iter().any(|t| t.contains("Added 1 of 2")
                    && t.contains("staging-eu")
                    && t.contains("eks:DescribeCluster")),
                "{toasts:?}"
            );
        });
        // Nothing added: an error in the dialog, no draft.
        view.update_in(cx, |view, window, cx| {
            view.finish(
                Added {
                    kubeconfig: None,
                    added: vec![],
                    failed: vec![(staging.clone(), Failure::Timeout)],
                },
                "x",
                window,
                cx,
            );
            assert!(view.error.as_deref().unwrap().contains("staging-eu"));
        });
    }
}
