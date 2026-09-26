//! GPUI tests against the kind dev cluster, ignored by default (they talk to the cluster):
//!
//! ```sh
//! script/dev-cluster.sh
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig KUBYL_CREDENTIAL_STORE=memory \
//!   cargo test -p kubyl_kubeconfig --lib live_ui -- --ignored --nocapture --test-threads=1
//! ```
//!
//! One app per test: phase 01's connection manager (with its file watcher), this crate, a
//! temp config dir. Kube work runs on real threads; GPUI timers are virtual, so the clock is
//! advanced while waiting (like `kubyl_argocd`'s live UI test).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, TestAppContext, VisualTestContext};
use kubyl_kube::kubeconfig::ContextInfo;
use kubyl_kube::{ConnectionManager, ConnectionState};

use crate::certs;
use crate::conntest::StepKind;
use crate::dialogs::CaState;
use crate::editor::KubeconfigEditor;
use crate::model::{self, Doc, Kind};
use crate::state::Kubeconfigs;
use crate::wizard::{Auth, CaMode, PemInput, Step, Wizard};

fn dev_kubeconfig() -> PathBuf {
    PathBuf::from(
        std::env::var("KUBYL_TEST_KUBECONFIG")
            .unwrap_or_else(|_| "/tmp/kubyl-dev/kubeconfig".into()),
    )
}

fn dev_context() -> String {
    std::env::var("KUBYL_TEST_CONTEXT").unwrap_or_else(|_| "kind-kubyl-dev".into())
}

fn wait<T>(
    cx: &mut VisualTestContext,
    what: &str,
    timeout: Duration,
    mut check: impl FnMut(&mut App) -> Result<T, String>,
) -> T {
    let start = Instant::now();
    loop {
        cx.run_until_parked();
        match cx.update(|_, cx| check(cx)) {
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

/// Starts the app with a temp config dir; returns it and the Kubyl-owned folder.
fn start(cx: &mut TestAppContext, sources: Vec<String>) -> (tempfile::TempDir, PathBuf) {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: set before the app starts any thread that reads the environment; the live tests
    // run one at a time (`--test-threads=1`).
    unsafe {
        std::env::set_var("KUBYL_CONFIG_DIR", dir.path());
        std::env::set_var("KUBYL_CREDENTIAL_STORE", "memory");
    }
    let settings = serde_json::json!({
        "kubernetes": {"load_default_kubeconfig": false, "load_kubeconfig_env": false, "kubeconfigs": sources},
    });
    std::fs::write(dir.path().join("settings.json"), settings.to_string()).unwrap();
    cx.update(|cx| {
        kubyl_core::init(cx);
        kubyl_settings::init_with_dir(cx, dir.path());
        kubyl_ui::init(cx);
        gpui_component::init(cx);
        kubyl_kube::init(cx);
        crate::init(cx);
    });
    cx.run_until_parked();
    let owned = dir.path().join("kubeconfigs");
    (dir, owned)
}

/// The dev context's server, CA and client certificate and key (PEM).
fn dev() -> (String, String, String, String) {
    let doc = Doc::parse(&std::fs::read_to_string(dev_kubeconfig()).unwrap()).unwrap();
    let (cluster, user) = doc.context_refs(&dev_context());
    let cluster = doc.body(Kind::Cluster, &cluster.unwrap()).unwrap().clone();
    let user = doc.body(Kind::User, &user.unwrap()).unwrap().clone();
    let pem = |m: &serde_json::Map<String, serde_json::Value>, k: &str| {
        certs::pem_from_data(&model::get_str(m, &[k])).unwrap()
    };
    (
        model::get_str(&cluster, &["server"]),
        pem(&cluster, model::CA_DATA),
        pem(&user, model::CERT_DATA),
        pem(&user, model::KEY_DATA),
    )
}

/// Acceptance: a kubeconfig for kind from scratch in the wizard. The CA is fetched from the
/// server and trusted after its fingerprint matched, the client certificate and key come from
/// files, the test passes (TLS, `kubernetes-admin`, can list pods), and after saving the
/// context is loaded by phase 01 and connects.
#[gpui::test]
#[ignore = "needs the kind dev cluster (script/dev-cluster.sh)"]
fn live_ui_wizard_creates_a_kubeconfig_for_kind_that_connects(cx: &mut TestAppContext) {
    let (dir, owned) = start(cx, Vec::new());
    let (server, ca, cert, key) = dev();
    let files_dir = dir.path().join("certs");
    std::fs::create_dir_all(&files_dir).unwrap();
    std::fs::write(files_dir.join("admin.crt"), &cert).unwrap();
    crate::files::write_atomic(&files_dir.join("admin.key"), key.as_bytes(), Some(0o600)).unwrap();

    // Dialogs close through gpui-component's Root, the window's first layer in the app.
    let slot: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Wizard>>>> = Default::default();
    let (_root, cx) = cx.add_window_view({
        let slot = slot.clone();
        move |window, cx| {
            let wizard = cx.new(|cx| Wizard::new(window, cx));
            *slot.borrow_mut() = Some(wizard.clone());
            gpui_component::Root::new(wizard, window, cx)
        }
    });
    let wizard = slot.borrow().clone().unwrap();
    wizard.update_in(cx, |w, window, cx| {
        w.set("name", "kind-wizard", window, cx);
        w.input_changed("name", "kind-wizard".into(), window, cx);
        w.go(Step::Cluster, window, cx);
        w.set("server", &server, window, cx);
        w.input_changed("server", server.clone(), window, cx);
        w.ca_mode = CaMode::Fetch;
        w.fetch_ca(cx);
    });
    let fingerprint = wait(cx, "the fetched CA", Duration::from_secs(30), |cx| {
        wizard.read_with(cx, |w, _| match &w.ca {
            CaState::Done(f) => f
                .candidates
                .first()
                .map(|c| c.cert.fingerprint())
                .ok_or_else(|| "no candidate".to_string()),
            CaState::Failed(e) if !e.is_empty() => panic!("fetch failed: {e}"),
            _ => Err("fetching".into()),
        })
    });
    // The user compares the fingerprint (here: with the CA the dev kubeconfig holds).
    assert_eq!(
        fingerprint,
        certs::certificates(&ca).unwrap()[0].fingerprint()
    );
    wizard.update_in(cx, |w, window, cx| {
        assert!(w.blocker(cx).is_some(), "not confirmed yet");
        w.ca_confirmed = true;
        assert_eq!(w.blocker(cx), None);
        w.go(Step::Credentials, window, cx);
        w.auth = Auth::ClientCertificate;
        w.cert_input = PemInput::File;
        w.key_input = PemInput::File;
        w.set(
            "cert-file",
            &files_dir.join("admin.crt").display().to_string(),
            window,
            cx,
        );
        w.set(
            "key-file",
            &files_dir.join("admin.key").display().to_string(),
            window,
            cx,
        );
        assert_eq!(w.blocker(cx), None);
        w.go(Step::Context, window, cx);
        w.set("namespace", "payments", window, cx);
        w.go(Step::Test, window, cx);
    });
    let key = wizard.read_with(cx, |w, cx| w.test_key(cx));
    let report = wait(cx, "the test", Duration::from_secs(60), |cx| {
        let report = Kubeconfigs::global(cx).read(cx).report(&key).cloned();
        match report {
            Some(r) if r.done => Ok(r),
            _ => Err("testing".into()),
        }
    });
    for step in &report.steps {
        println!(
            "{:?} {} {:?} {:?}",
            step.status,
            step.kind.title(),
            step.lines,
            step.error
        );
    }
    assert!(report.passed(), "{}", report.summary());
    assert_eq!(report.user.as_deref(), Some("kubernetes-admin"));
    assert!(report.step(StepKind::Tls).lines[0].contains("verified by the kubeconfig's CA"));
    let pods = report
        .step(StepKind::Permissions)
        .checks
        .iter()
        .find(|c| c.label.starts_with("list pods"))
        .unwrap()
        .allowed;
    assert_eq!(pods, Some(true));

    wizard.update_in(cx, |w, window, cx| {
        w.go(Step::Save, window, cx);
        w.connect = true;
        w.save(window, cx);
    });
    let path = owned.join("kind-wizard.yaml");
    wait(cx, "the saved file", Duration::from_secs(10), |_| {
        path.exists()
            .then_some(())
            .ok_or_else(|| "not yet".to_string())
    });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let id = ContextInfo::make_id("kind-wizard", &path);
    wait(cx, "the context connected", Duration::from_secs(60), |cx| {
        let manager = ConnectionManager::global(cx);
        let manager = manager.read(cx);
        if manager.context(&id).is_none() {
            return Err("not loaded".into());
        }
        match manager.state(&id) {
            ConnectionState::Connected { .. } => Ok(()),
            other => Err(format!("{other:?}")),
        }
    });
    cx.update(|_, cx| {
        let manager = ConnectionManager::global(cx);
        assert_eq!(
            manager.read(cx).active(),
            Some(&id),
            "the new context became active"
        );
    });
}

/// After a save, phase 01 reloads the file and rebuilds the client of an edited, connected
/// context (its default namespace comes from the kubeconfig).
#[gpui::test]
#[ignore = "needs the kind dev cluster (script/dev-cluster.sh)"]
fn live_ui_saving_rebuilds_the_edited_contexts_client(cx: &mut TestAppContext) {
    let (dir, _owned) = start(cx, Vec::new());
    // A Kubyl-owned copy of the dev kubeconfig, loaded as a source.
    let owned = dir.path().join("kubeconfigs");
    std::fs::create_dir_all(&owned).unwrap();
    let path = owned.join("dev.yaml");
    std::fs::copy(dev_kubeconfig(), &path).unwrap();
    let context = dev_context();
    let id = ContextInfo::make_id(&context, &path);
    let (editor, cx) = cx.add_window_view({
        let path = path.clone();
        move |window, cx| KubeconfigEditor::new(path.clone(), window, cx)
    });
    cx.update(|_, cx| ConnectionManager::global(cx).update(cx, |m, cx| m.reload(cx)));
    wait(cx, "the context loaded", Duration::from_secs(10), |cx| {
        ConnectionManager::global(cx)
            .read(cx)
            .context(&id)
            .map(|_| ())
            .ok_or_else(|| "not loaded".to_string())
    });
    cx.update(|_, cx| ConnectionManager::global(cx).update(cx, |m, cx| m.connect(&id, cx)));
    let before = wait(cx, "connected", Duration::from_secs(30), |cx| {
        let manager = ConnectionManager::global(cx);
        let manager = manager.read(cx);
        match manager.cluster(&id) {
            Some(c) if c.state.is_connected() => {
                Ok(c.default_namespace().unwrap_or_default().to_string())
            }
            other => Err(format!("{:?}", other.map(|c| c.state.clone()))),
        }
    });
    assert_ne!(before, "kubyl-rebuilt");

    // Edit the namespace in the editor and save.
    let namespace_context = context.clone();
    editor.update_in(cx, |editor, window, cx| {
        editor.edit(window, cx, |doc| {
            model::set_str(
                doc.body_mut(Kind::Context, &namespace_context).unwrap(),
                &["namespace"],
                "kubyl-rebuilt",
            );
        });
    });
    let save = editor.update_in(cx, |editor, window, cx| {
        let text = editor.text_to_save().text;
        editor.save_file(text, false, window, cx)
    });
    wait(cx, "the save", Duration::from_secs(10), |_| {
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("kubyl-rebuilt")
            .then_some(())
            .ok_or_else(|| "not written".to_string())
    });
    drop(save);
    wait(cx, "the rebuilt client", Duration::from_secs(30), |cx| {
        let manager = ConnectionManager::global(cx);
        let manager = manager.read(cx);
        match manager.cluster(&id) {
            Some(c) if c.state.is_connected() && c.default_namespace() == Some("kubyl-rebuilt") => {
                Ok(())
            }
            other => Err(format!(
                "{:?} ns {:?}",
                other.map(|c| c.state.clone()),
                other.and_then(|c| c.default_namespace())
            )),
        }
    });
}
