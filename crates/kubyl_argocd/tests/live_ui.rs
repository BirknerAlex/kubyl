//! The Argo CD UI follows the CRDs without a restart. Ignored by default: it uninstalls Argo CD
//! from the kind dev cluster and installs it again (the same version; a few minutes, needs
//! network access to GitHub):
//!
//! ```sh
//! script/dev-cluster.sh
//! KUBECONFIG=/tmp/kubyl-dev/kubeconfig script/argocd-dev.sh           # or v3.4.9
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig KUBYL_CREDENTIAL_STORE=memory \
//!   cargo test -p kubyl_argocd --test live_ui -- --ignored --nocapture --test-threads=1
//! ```
//!
//! One app (connection manager, stores, explorer tree group, Argo CD state) and one open
//! Applications tab live through `script/argocd-dev.sh --delete` and a reinstall.

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use gpui::{App, TestAppContext, WindowHandle};
use kubyl_argocd::state::{ArgoCd, Detection};
use kubyl_argocd::views::apps::AppsView;
use kubyl_core::{ClusterId, Gvr, ResourceRef};
use kubyl_explorer::catalog;
use kubyl_kube::ConnectionManager;

fn context() -> String {
    std::env::var("KUBYL_TEST_CONTEXT").unwrap_or("kind-kubyl-dev".into())
}

/// Runs `script/argocd-dev.sh` with `args` against the test cluster.
fn script(args: &[&str]) -> Child {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../script/argocd-dev.sh");
    Command::new(path)
        .args(args)
        .env(
            "KUBECONFIG",
            std::env::var("KUBYL_TEST_KUBECONFIG").expect("set KUBYL_TEST_KUBECONFIG"),
        )
        .env("CONTEXT", context())
        .spawn()
        .expect("script/argocd-dev.sh")
}

/// Pumps the app until `check` returns `Ok`; on timeout, fails with its last `Err`. Kube work
/// runs on real threads; GPUI timers (debounces, retries) are virtual, so the clock is
/// advanced while waiting.
fn wait<T>(
    cx: &mut TestAppContext,
    what: &str,
    timeout: Duration,
    mut check: impl FnMut(&mut App) -> Result<T, String>,
) -> T {
    let start = Instant::now();
    loop {
        cx.run_until_parked();
        match cx.update(&mut check) {
            Ok(done) => return done,
            Err(last) if start.elapsed() > timeout => {
                panic!("timed out waiting for {what}; last: {last}")
            }
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(100));
        cx.executor().advance_clock(Duration::from_millis(500));
    }
}

/// Waits for the script to exit, keeping the app running meanwhile.
fn finish(cx: &mut TestAppContext, mut child: Child, what: &str) {
    let status = wait(cx, what, Duration::from_secs(600), |_| {
        child
            .try_wait()
            .expect("script status")
            .ok_or_else(|| "running".to_string())
    });
    assert!(status.success(), "{what} failed");
}

/// What the user sees of Argo CD on `cluster`.
#[derive(Debug, PartialEq)]
struct Seen {
    applications_crd: bool,
    /// Rows of the sidebar's Argo CD group (the explorer shows only served kinds).
    tree_kinds: Vec<String>,
    /// The group's badge: the install's version.
    badge: Option<String>,
    /// Namespaces of the installs found.
    installs: Vec<String>,
    /// Rows of the open Applications tab.
    rows: Vec<String>,
}

fn seen(cluster: &ClusterId, view: &WindowHandle<AppsView>, cx: &mut App) -> Seen {
    let manager = ConnectionManager::global(cx);
    let discovery = manager.read(cx).discovery(cluster);
    let group = catalog::contributed_groups("administration", cx)
        .into_iter()
        .find(|g| g.id == "argocd")
        .expect("the Argo CD tree group is registered");
    let tree_kinds = discovery
        .map(|d| {
            catalog::contributed_kinds(&group, &d)
                .into_iter()
                .map(|k| k.label)
                .collect()
        })
        .unwrap_or_default();
    let badge = group
        .badge
        .as_ref()
        .and_then(|badge| badge(cluster, cx))
        .map(|b| b.to_string());
    let argo = ArgoCd::global(cx);
    let installs = argo
        .read(cx)
        .installs(cluster)
        .iter()
        .map(|i| i.namespace.clone())
        .collect();
    let rows = view
        .read_with(cx, |view, _| view.row_names())
        .unwrap_or_default();
    Seen {
        applications_crd: ArgoCd::caps(cluster, cx).applications,
        tree_kinds,
        badge,
        installs,
        rows,
    }
}

#[gpui::test]
#[ignore = "needs the kind dev cluster with Argo CD; uninstalls and reinstalls it"]
fn the_ui_follows_the_crds_without_a_restart(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let kubeconfig = std::env::var("KUBYL_TEST_KUBECONFIG").expect("set KUBYL_TEST_KUBECONFIG");
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: set before the app starts any thread that reads the environment.
    unsafe {
        std::env::set_var("KUBYL_CONFIG_DIR", dir.path());
        std::env::set_var("KUBYL_CREDENTIAL_STORE", "memory");
    }
    let settings = serde_json::json!({
        "kubernetes": {
            "load_default_kubeconfig": false,
            "load_kubeconfig_env": false,
            "kubeconfigs": [kubeconfig],
        }
    });
    std::fs::write(dir.path().join("settings.json"), settings.to_string()).unwrap();
    cx.update(|cx| {
        kubyl_core::init(cx);
        kubyl_settings::init_with_dir(cx, dir.path());
        kubyl_ui::init(cx);
        kubyl_kube::init(cx);
        kubyl_resources::init(cx);
        kubyl_portforward::init(cx);
        kubyl_explorer::init(cx);
        kubyl_argocd::init(cx);
    });
    cx.run_until_parked();

    let cluster = cx.update(|cx| {
        let manager = ConnectionManager::global(cx);
        let id = manager
            .read(cx)
            .contexts()
            .find(|c| c.name == context())
            .map(|c| c.id.clone())
            .expect("the test context");
        manager.update(cx, |m, cx| m.activate(&id, cx));
        id
    });
    let target = ResourceRef::list(
        cluster.clone(),
        Gvr::new("argoproj.io", "v1alpha1", "applications"),
        None,
    );
    let view = cx.add_window(|window, cx| AppsView::new(target, window, cx));

    // Installed: the tree group, its version badge, detection and the open tab.
    let before = wait(cx, "Argo CD in the UI", Duration::from_secs(60), |cx| {
        let s = seen(&cluster, &view, cx);
        let ready = s.applications_crd
            && s.tree_kinds.len() == 3
            && s.badge.is_some()
            && s.installs.contains(&"argocd".to_string())
            && s.rows.contains(&"argocd/guestbook".to_string());
        if ready { Ok(s) } else { Err(format!("{s:?}")) }
    });
    println!("installed: {before:?}");
    assert_eq!(
        before.tree_kinds,
        ["Applications", "ApplicationSets", "Projects"]
    );
    let version = before.badge.clone().unwrap();
    assert!(version.starts_with('v'), "badge {version}");

    // Uninstalled, CRDs included: everything goes, the tab stays open and empty.
    finish(cx, script(&["--delete"]), "script/argocd-dev.sh --delete");
    wait(
        cx,
        "Argo CD gone from the UI",
        Duration::from_secs(120),
        |cx| {
            let s = seen(&cluster, &view, cx);
            let gone = Seen {
                applications_crd: false,
                tree_kinds: Vec::new(),
                badge: None,
                installs: Vec::new(),
                rows: Vec::new(),
            };
            if s == gone {
                Ok(())
            } else {
                Err(format!("{s:?}"))
            }
        },
    );
    cx.update(|cx| {
        let argo = ArgoCd::global(cx);
        assert_eq!(argo.read(cx).detection(&cluster), Detection::Idle);
    });
    println!("uninstalled: nothing left");

    // Installed again (same version): it all comes back into the same app and tab.
    finish(cx, script(&[&version]), "script/argocd-dev.sh");
    wait(
        cx,
        "Argo CD back in the UI",
        Duration::from_secs(180),
        |cx| {
            let s = seen(&cluster, &view, cx);
            let back = s.applications_crd
                && s.tree_kinds == before.tree_kinds
                && s.badge.as_deref() == Some(version.as_str())
                && s.installs.contains(&"argocd".to_string())
                && s.rows.contains(&"argocd/guestbook".to_string());
            if back { Ok(()) } else { Err(format!("{s:?}")) }
        },
    );
    println!(
        "reinstalled: {:?}",
        cx.update(|cx| seen(&cluster, &view, cx))
    );
}
