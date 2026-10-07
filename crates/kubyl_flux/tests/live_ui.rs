//! The Flux UI follows the CRDs without a restart. Ignored by default: it uninstalls Flux (and
//! its samples) from the kind dev cluster with `script/flux-dev.sh --delete` and installs it
//! again (a few minutes, needs network access to GitHub and ghcr.io):
//!
//! ```sh
//! KUBECONFIG=/tmp/kubyl-dev/kubeconfig script/flux-dev.sh
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig KUBYL_CREDENTIAL_STORE=memory \
//!   cargo test -p kubyl_flux --test live_ui -- --ignored --nocapture --test-threads=1
//! ```
//!
//! One app (connection manager, stores, explorer tree group, Flux state) and one open
//! Kustomizations tab live through the uninstall and the reinstall.

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use gpui::{App, TestAppContext, WindowHandle};
use kubyl_core::{ClusterId, Gvr, ResourceRef};
use kubyl_explorer::catalog;
use kubyl_flux::kinds::Category;
use kubyl_flux::service::Detection;
use kubyl_flux::state::Flux;
use kubyl_flux::views::list::ListView;
use kubyl_kube::ConnectionManager;

fn context() -> String {
    std::env::var("KUBYL_TEST_CONTEXT").unwrap_or("kind-kubyl-dev".into())
}

/// Runs `script/flux-dev.sh` with `args` against the test cluster.
fn script(args: &[&str]) -> Child {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../script/flux-dev.sh");
    Command::new(path)
        .args(args)
        .env(
            "KUBECONFIG",
            std::env::var("KUBYL_TEST_KUBECONFIG").expect("set KUBYL_TEST_KUBECONFIG"),
        )
        .env("CONTEXT", context())
        .spawn()
        .expect("script/flux-dev.sh")
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

/// The script's process, stopped if the test gives up on it.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// Waits for the script to exit, keeping the app running meanwhile.
fn finish(cx: &mut TestAppContext, child: Child, what: &str) {
    let mut child = Running(child);
    let status = wait(cx, what, Duration::from_secs(600), |_| {
        child
            .0
            .try_wait()
            .expect("script status")
            .ok_or_else(|| "running".to_string())
    });
    assert!(status.success(), "{what} failed");
}

/// Whether `script/flux-dev.sh` installed the Flux on the test cluster (`--delete` leaves
/// any other Flux in place, and this test would wait for it to go).
fn installed_by_the_script() -> bool {
    let output = Command::new("kubectl")
        .args([
            "--context",
            &context(),
            "get",
            "namespace",
            "flux-system",
            "-o",
            r"jsonpath={.metadata.labels.kubyl\.dev/flux-dev}",
        ])
        .env(
            "KUBECONFIG",
            std::env::var("KUBYL_TEST_KUBECONFIG").expect("set KUBYL_TEST_KUBECONFIG"),
        )
        .output()
        .expect("kubectl");
    String::from_utf8_lossy(&output.stdout).trim() == "installed"
}

/// What the user sees of Flux on `cluster`.
#[derive(Debug, PartialEq)]
struct Seen {
    any_crd: bool,
    /// Kind rows and view rows of the sidebar's Flux group.
    tree_rows: Vec<String>,
    /// The group's badge: the controllers' version.
    badge: Option<String>,
    /// Controllers found.
    controllers: usize,
    /// Rows of the open Kustomizations tab.
    rows: Vec<String>,
}

fn seen(cluster: &ClusterId, view: &WindowHandle<ListView>, cx: &mut App) -> Seen {
    let manager = ConnectionManager::global(cx);
    let discovery = manager.read(cx).discovery(cluster);
    let group = catalog::contributed_groups("administration", cx)
        .into_iter()
        .find(|g| g.id == "flux")
        .expect("the Flux tree group is registered");
    let (before, after) = catalog::group_views("flux", cluster, cx);
    let mut tree_rows: Vec<String> = before.iter().map(|v| v.label.to_string()).collect();
    tree_rows.extend(
        discovery
            .map(|d| {
                catalog::contributed_kinds(&group, &d)
                    .into_iter()
                    .map(|k| k.label)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );
    tree_rows.extend(after.iter().map(|v| v.label.to_string()));
    let badge = group
        .badge
        .as_ref()
        .and_then(|badge| badge(cluster, cx))
        .map(|b| b.to_string());
    let flux = Flux::global(cx);
    let controllers = flux
        .read(cx)
        .install(cluster)
        .map(|i| i.controllers.len())
        .unwrap_or(0);
    let rows = view
        .read_with(cx, |view, _| view.row_names())
        .unwrap_or_default();
    Seen {
        any_crd: Flux::caps(cluster, cx).any(),
        tree_rows,
        badge,
        controllers,
        rows,
    }
}

const ALL_ROWS: [&str; 6] = [
    "Overview",
    "Kustomizations",
    "HelmReleases",
    "Sources",
    "Image Automation",
    "Notifications",
];

#[gpui::test]
#[ignore = "needs the kind dev cluster with script/flux-dev.sh; uninstalls and reinstalls Flux"]
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
        kubyl_explorer::init(cx);
        kubyl_flux::init(cx);
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
    let target = ResourceRef::list(cluster.clone(), Gvr::new("", "", ""), None);
    let view =
        cx.add_window(|window, cx| ListView::new(Category::Kustomizations, target, window, cx));

    let has_samples = |s: &Seen| {
        s.rows.contains(&"flux-demo/broken".to_string())
            && s.rows.contains(&"flux-demo/podinfo".to_string())
    };
    let before = wait(cx, "Flux in the UI", Duration::from_secs(60), |cx| {
        let s = seen(&cluster, &view, cx);
        let ready = s.any_crd
            && s.tree_rows == ALL_ROWS
            && s.badge.is_some()
            && s.controllers >= 6
            && has_samples(&s);
        if ready { Ok(s) } else { Err(format!("{s:?}")) }
    });
    println!("installed: {before:?}");

    // Uninstalled, CRDs included: everything goes, the tab stays open and empty.
    assert!(
        installed_by_the_script(),
        "Flux on {} wasn't installed by script/flux-dev.sh: this test would remove its samples and wait for a Flux that stays",
        context()
    );
    finish(cx, script(&["--delete"]), "script/flux-dev.sh --delete");
    wait(
        cx,
        "Flux gone from the UI",
        Duration::from_secs(180),
        |cx| {
            let s = seen(&cluster, &view, cx);
            let gone = Seen {
                any_crd: false,
                tree_rows: Vec::new(),
                badge: None,
                controllers: 0,
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
        let flux = Flux::global(cx);
        assert_eq!(flux.read(cx).detection(&cluster), Detection::Idle);
    });
    println!("uninstalled: nothing left");

    // Installed again: it all comes back into the same app and tab.
    finish(cx, script(&[]), "script/flux-dev.sh");
    wait(cx, "Flux back in the UI", Duration::from_secs(300), |cx| {
        let s = seen(&cluster, &view, cx);
        let back = s.any_crd
            && s.tree_rows == ALL_ROWS
            && s.badge == before.badge
            && s.controllers >= 6
            && has_samples(&s);
        if back { Ok(()) } else { Err(format!("{s:?}")) }
    });
    println!(
        "reinstalled: {:?}",
        cx.update(|cx| seen(&cluster, &view, cx))
    );
}
