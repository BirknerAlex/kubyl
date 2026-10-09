//! GPUI tests: the install dialog's validation, the preview pane, and that read-only clusters
//! get no Helm write (no dialog, no write hints). No cluster is contacted and no `helm` runs.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext, prelude::*};
use gpui_component::{Root, WindowExt as _};
use kubyl_core::ClusterId;
use kubyl_helm_core::cli::{HelmEnv, HelmInfo, Probe, Version};
use kubyl_helm_core::preview;
use kubyl_helm_core::repo::ChartRef;
use kubyl_helm_core::settings::HelmSettings;
use kubyl_kube::ConnectionManager;
use kubyl_settings::Settings;

use crate::cli::HelmCli;
use crate::dialogs::install::InstallDialog;
use crate::dialogs::preview::{PreviewKind, PreviewPane};
use crate::dialogs::{self, InstallRequest};

const KUBECONFIG: &str = "apiVersion: v1\nkind: Config\ncurrent-context: kind-dev\nclusters:\n- name: kind-dev\n  cluster:\n    server: https://127.0.0.1:6443\ncontexts:\n- name: kind-dev\n  context:\n    cluster: kind-dev\n    user: kind-dev\nusers:\n- name: kind-dev\n  user:\n    token: not-a-real-token\n";

fn helm() -> HelmInfo {
    HelmInfo {
        path: PathBuf::from("/nonexistent/helm"),
        version: Version {
            major: 4,
            minor: 3,
            patch: 0,
        },
        version_text: "v4.3.0".into(),
        env: HelmEnv::default(),
        search_path: None,
        cli_env: Default::default(),
    }
}

/// Kubyl's globals with one context (`read_only` set as asked) and a probed `helm`.
fn setup(cx: &mut TestAppContext, read_only: bool) -> (tempfile::TempDir, ClusterId) {
    let dir = tempfile::tempdir().unwrap();
    let kube = dir.path().join("dev.yaml");
    std::fs::write(&kube, KUBECONFIG).unwrap();
    let settings = serde_json::json!({
        "kubernetes": {
            "load_default_kubeconfig": false,
            "load_kubeconfig_env": false,
            "kubeconfigs": [kube],
        }
    });
    std::fs::write(dir.path().join("settings.json"), settings.to_string()).unwrap();
    let manager = cx.update(|cx| {
        kubyl_core::init(cx);
        kubyl_settings::init_with_dir(cx, dir.path());
        kubyl_ui::init(cx);
        Settings::register::<kubyl_kube::settings::KubeSettings>(cx);
        Settings::register::<HelmSettings>(cx);
        let manager = ConnectionManager::install(dir.path().join("pasted"), false, cx);
        kubyl_resources::init(cx);
        crate::service::Helm::install(false, cx);
        let helm_cli = HelmCli::install(false, cx);
        helm_cli.update(cx, |h, cx| h.set_probe(Probe::Ready(helm()), cx));
        crate::ops::HelmOps::install(cx);
        manager
    });
    cx.run_until_parked();
    let cluster = manager.read_with(cx, |m, _| m.contexts().next().unwrap().id.clone());
    if read_only {
        manager.update(cx, |m, cx| {
            m.update_context_settings(&cluster, cx, |s| s.read_only = true)
        });
        cx.run_until_parked();
    }
    (dir, cluster)
}

fn window(cx: &mut TestAppContext) -> &mut VisualTestContext {
    let (_root, cx) = cx.add_window_view(|window, cx| {
        let empty = cx.new(|_| gpui::Empty);
        Root::new(empty, window, cx)
    });
    cx
}

#[gpui::test]
fn the_install_dialog_validates_name_namespace_and_values(cx: &mut TestAppContext) {
    let (_dir, cluster) = setup(cx, false);
    let cx = window(cx);
    let dialog: Entity<InstallDialog> = cx.update(|window, cx| {
        cx.new(|cx| {
            InstallDialog::new_for_test(
                cluster.clone(),
                InstallRequest {
                    cluster: Some(cluster.clone()),
                    chart: None,
                    version: None,
                    namespace: Some("shop".into()),
                },
                helm(),
                window,
                cx,
            )
        })
    });
    let blocker = |cx: &mut VisualTestContext| dialog.read_with(cx, |d, cx| d.blocker(cx));
    // No chart yet.
    assert!(blocker(cx).unwrap().contains("Pick a chart"));
    dialog.update(cx, |d, _| {
        d.set_chart_for_test(ChartRef::Repo {
            repo: "kubyl-dev".into(),
            name: "kubyl-demo".into(),
        })
    });
    let set = |cx: &mut VisualTestContext, which: &str, text: &str| {
        let (which, text) = (which.to_string(), text.to_string());
        dialog.update_in(cx, |d, window, cx| {
            d.set_field_for_test(&which, &text, window, cx)
        });
        cx.run_until_parked();
    };
    set(cx, "name", "Bad_Name");
    assert!(
        blocker(cx).unwrap().contains("lowercase"),
        "{:?}",
        blocker(cx)
    );
    set(cx, "name", "web");
    // Values that start from the chart's defaults wait for them (another version's defaults
    // in the editor would all be sent as the release's own values).
    assert!(
        blocker(cx).unwrap().contains("default values"),
        "{:?}",
        blocker(cx)
    );
    dialog.update(cx, |d, _| {
        d.set_details_for_test(kubyl_helm_core::repo::ChartDetails {
            values: "replicaCount: 1\nimage: pause\n".into(),
            ..Default::default()
        })
    });
    assert_eq!(blocker(cx), None);
    set(cx, "namespace", "Not Valid");
    assert!(blocker(cx).unwrap().contains("Namespaces"));
    set(cx, "namespace", "shop");
    set(cx, "timeout", "soon");
    assert!(blocker(cx).unwrap().contains("timeout"));
    // Helm wants a unit, and a duration that fits.
    set(cx, "timeout", "90");
    assert!(blocker(cx).unwrap().contains("timeout"));
    set(cx, "timeout", "99999999999999999999h");
    assert!(blocker(cx).unwrap().contains("timeout"));
    set(cx, "timeout", "90s");
    set(cx, "values", "replicaCount: [\n");
    assert!(blocker(cx).unwrap().contains("values"), "{:?}", blocker(cx));
    set(cx, "values", "replicaCount: 2\nimage: pause\n");
    assert_eq!(blocker(cx), None);
    // Only what differs from the chart's defaults is sent.
    let payload = dialog
        .read_with(cx, |d, cx| d.payload_for_test(cx))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&payload).unwrap(),
        serde_json::json!({"replicaCount": 2})
    );
    // A namespace that doesn't exist yet gets created (the list is empty here: unknown).
    dialog.read_with(cx, |d, _| assert!(!d.creates_namespace_for_test()));
}

#[gpui::test]
fn previews_render_objects_values_and_masked_secrets(cx: &mut TestAppContext) {
    let (_dir, _cluster) = setup(cx, false);
    let cx = window(cx);
    let manifest = "---\napiVersion: v1\nkind: Secret\nmetadata:\n  name: web-auth\nstringData:\n  password: \"hunter2\"\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\nspec:\n  replicas: 1\n";
    let json = serde_json::json!({
        "name": "web", "namespace": "shop", "version": 2,
        "info": {"status": "pending-upgrade"},
        "chart": {"metadata": {"name": "demo", "version": "0.2.0"}},
        "config": {"replicaCount": 3},
        "manifest": manifest.replace("replicas: 1", "replicas: 3").replace("hunter2", "s3cret"),
        "hooks": [{"name": "web-migrate", "kind": "Job", "path": "", "events": ["post-upgrade"], "manifest": ""}]
    })
    .to_string();
    let current = kubyl_helm_core::decode::Release {
        summary: Default::default(),
        values: serde_json::json!({"replicaCount": 1}),
        chart_values: serde_json::json!({}),
        manifest: manifest.into(),
        notes: None,
        hooks: Vec::new(),
        crds: Vec::new(),
    };
    let upgrade = preview::upgrade_preview(&json, &current, false).unwrap();
    assert!(!format!("{:?}", upgrade.changes).contains("s3cret"));
    let install = preview::install_preview(&json, true).unwrap();
    let panes: Vec<Entity<PreviewPane>> = cx.update(|_, cx| {
        vec![
            cx.new(|_| PreviewPane::new(PreviewKind::Upgrade, Arc::new(upgrade))),
            cx.new(|_| PreviewPane::new(PreviewKind::Install, Arc::new(install))),
        ]
    });
    for pane in &panes {
        let pane = pane.clone();
        cx.update(|window, cx| {
            window.open_dialog(cx, move |dialog, _, _| {
                let pane = pane.clone();
                dialog.content(move |content, _, _| content.child(pane.clone()))
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.close_dialog(cx));
    }
    panes[0].read_with(cx, |pane, _| {
        assert!(pane.changes_anything());
        let counts = pane.counts();
        assert_eq!((counts.changed, counts.unchanged), (2, 0));
        assert!(
            pane.preview()
                .values
                .as_ref()
                .is_some_and(|v| !v.is_empty())
        );
    });
    panes[1].read_with(cx, |pane, _| {
        assert!(pane.preview().client_side);
        assert_eq!(pane.preview().changes.len(), 2);
    });
}

#[gpui::test]
fn read_only_clusters_get_no_helm_writes(cx: &mut TestAppContext) {
    let (_dir, cluster) = setup(cx, true);
    let cx = window(cx);
    assert!(cx.update(|_, cx| crate::cli::read_only(&cluster, cx)));
    cx.update(|window, cx| {
        dialogs::open_install(
            InstallRequest {
                cluster: Some(cluster.clone()),
                ..Default::default()
            },
            window,
            cx,
        )
    });
    cx.run_until_parked();
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    let hints = vec![
        ("u".into(), "Upgrade…".into()),
        ("1".into(), "Values".into()),
        ("ctrl-d".into(), "Uninstall…".into()),
        // The Charts tab's.
        ("i".into(), "Install…".into()),
    ];
    let shown = crate::visible_hints(hints.clone(), false);
    assert_eq!(shown.len(), 1);
    assert_eq!(crate::visible_hints(hints, true).len(), 4);
}

#[gpui::test]
fn without_helm_writes_explain_how_to_install_it(cx: &mut TestAppContext) {
    let (_dir, cluster) = setup(cx, false);
    cx.update(|cx| {
        let helm = HelmCli::global(cx).unwrap();
        helm.update(cx, |h, cx| {
            h.set_probe(Probe::Missing { configured: None }, cx)
        });
    });
    let cx = window(cx);
    assert!(cx.update(|_, cx| HelmCli::info(cx).is_none()));
    cx.update(|window, cx| {
        dialogs::open_install(
            InstallRequest {
                cluster: Some(cluster.clone()),
                ..Default::default()
            },
            window,
            cx,
        )
    });
    cx.run_until_parked();
    // The "helm isn't installed" note opens instead of the install dialog.
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
}

#[test]
fn version_menus_name_the_latest_release_and_pre_releases() {
    let versions: Vec<(String, Option<String>)> = ["0.4.0-rc.1", "0.3.0", "0.2.0"]
        .into_iter()
        .map(|v| (v.to_string(), None))
        .collect();
    // `helm install` without --version picks 0.3.0, not the newer pre-release.
    assert_eq!(dialogs::version_label(&versions, "0.3.0"), "0.3.0 (latest)");
    assert_eq!(
        dialogs::version_label(&versions, "0.4.0-rc.1"),
        "0.4.0-rc.1 (pre-release)"
    );
    assert_eq!(dialogs::version_label(&versions, "0.2.0"), "0.2.0");
}
