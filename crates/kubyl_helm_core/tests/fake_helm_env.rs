//! The user's `HELM_KUBE*`, `HELM_NAMESPACE` and `HELM_DRIVER` never reach the `helm` Kubyl
//! runs. Its own test binary: it sets variables in the process environment, which must not race
//! other tests spawning processes.
#![cfg(unix)]

use std::path::Path;

use kubyl_helm_core::cli::{self, Invocation};
use kubyl_helm_core::decode::Driver;
use kubyl_kube_core::cli::CliTarget;

#[tokio::test]
async fn the_users_helm_variables_are_scrubbed() {
    let user_env = [
        ("HELM_KUBETOKEN", "users-own-token"),
        ("HELM_KUBEAPISERVER", "https://elsewhere.example.com"),
        ("HELM_KUBECONTEXT", "prod"),
        ("HELM_KUBEINSECURE_SKIP_TLS_VERIFY", "true"),
        ("HELM_KUBECAFILE", "/tmp/evil-ca.pem"),
        ("HELM_KUBETLS_SERVER_NAME", "evil.example.com"),
        ("HELM_KUBEASUSER", "admin"),
        ("HELM_NAMESPACE", "kube-system"),
        ("HELM_DRIVER", "configmap"),
    ];
    for (key, value) in user_env {
        // SAFETY: the only test of this binary, before any process is spawned.
        unsafe { std::env::set_var(key, value) };
    }
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("helm");
    std::fs::write(
        &program,
        format!("#!/bin/sh\nenv > {}/env\n", dir.path().display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let info = cli::HelmInfo {
        path: program,
        version: cli::Version {
            major: 3,
            minor: 19,
            patch: 0,
        },
        version_text: "v3.19.0".into(),
        env: Default::default(),
        search_path: Some(
            std::env::join_paths([Path::new("/usr/bin"), Path::new("/bin")]).unwrap(),
        ),
    };
    let target = CliTarget::new("/home/me/kube/dev.yaml", "kind-kubyl-dev");
    let read_env = || std::fs::read_to_string(dir.path().join("env")).unwrap();
    for (invocation, driver) in [
        (Invocation::cluster(["status", "web"]), "secret"),
        (
            Invocation::cluster(["rollback", "web", "2"]).driver(Driver::ConfigMap),
            "configmap",
        ),
        (Invocation::read(["repo", "list"]), "secret"),
    ] {
        cli::run(&info, Some(&target), invocation, None, None)
            .await
            .unwrap();
        let env = read_env();
        for (key, _) in user_env {
            if key == "HELM_DRIVER" {
                continue;
            }
            assert!(!env.contains(&format!("{key}=")), "{key} reached helm");
        }
        assert!(
            env.lines().any(|l| l == format!("HELM_DRIVER={driver}")),
            "{driver}"
        );
    }
}
