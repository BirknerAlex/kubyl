//! With an allow-list, `helm` sees only the allowed variables of this process, the host's
//! fixed ones and what Kubyl sets itself. Its own test binary: it sets variables in the process
//! environment, which must not race other tests spawning processes.
#![cfg(unix)]

use std::path::Path;

use kubyl_helm_core::cli::{self, CliEnv, Invocation, Probe};
use kubyl_kube_core::cli::CliTarget;
use secrecy::SecretString;

fn env_of(dir: &Path, command: &str) -> String {
    std::fs::read_to_string(dir.join(format!("env-{command}"))).unwrap()
}

#[tokio::test]
async fn an_allow_list_decides_what_helm_inherits() {
    for (key, value) in [
        ("HOST_SECRET", "host-only"),
        ("KUBYL_TEST_KEEP", "kept"),
        ("KUBYL_OTHER_PREFIXED", "also-kept"),
        ("HELM_KUBECONTEXT", "prod"),
    ] {
        // SAFETY: the only test of this binary, before any process is spawned.
        unsafe { std::env::set_var(key, value) };
    }
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("helm");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\nenv > {0}/env-$1\nif [ \"$1\" = version ]; then echo v3.19.0; fi\n",
            dir.path().display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let search = std::env::join_paths(["/usr/bin", "/bin"]).unwrap();
    let cli_env = CliEnv::allow_list(["KUBYL_TEST_KEEP"], ["KUBYL_OTHER_", "HELM_"])
        .with_var("HELM_CONFIG_HOME", "/host/config")
        .with_var("HOME", "/host/home");

    // The probe runs `helm` too.
    let probe = cli::probe_with(program.to_str(), Some(search), cli_env.clone()).await;
    let Probe::Ready(info) = probe else {
        panic!("{probe:?}")
    };
    assert_eq!(*info.cli_env, cli_env);
    for command in ["version", "env"] {
        let env = env_of(dir.path(), command);
        assert!(!env.contains("HOST_SECRET="), "{command}: {env}");
        assert!(env.contains("KUBYL_TEST_KEEP=kept"), "{command}");
        assert!(env.contains("KUBYL_OTHER_PREFIXED=also-kept"), "{command}");
        assert!(env.contains("HELM_CONFIG_HOME=/host/config"), "{command}");
        assert!(env.contains("HOME=/host/home"), "{command}");
        assert!(env.contains("PATH=/usr/bin:/bin"), "{command}");
        assert!(env.contains("NO_COLOR=1"), "{command}");
        // Kubyl's scrub still applies to allowed prefixes.
        assert!(!env.contains("HELM_KUBECONTEXT="), "{command}");
    }

    let target = CliTarget::new("/home/me/kube/dev.yaml", "kind-kubyl-dev")
        .with_server(Some("https://127.0.0.1:6443".into()));
    let token = SecretString::from("tok-123".to_string());
    cli::run_with_token(
        &info,
        Some(&target),
        Some(token),
        Invocation::cluster(["status", "web"]),
        None,
        None,
    )
    .await
    .unwrap();
    let env = env_of(dir.path(), "status");
    assert!(!env.contains("HOST_SECRET="), "{env}");
    assert!(env.contains("KUBYL_TEST_KEEP=kept"));
    assert!(env.contains("HELM_KUBETOKEN=tok-123"));
    assert!(env.contains("HELM_KUBEAPISERVER=https://127.0.0.1:6443"));
    assert!(env.contains("HELM_DRIVER=secret"));

    // The default policy is today's behaviour: everything but Kubyl's scrubbed variables.
    let mut plain = info.clone();
    plain.cli_env = Default::default();
    cli::run(&plain, None, Invocation::read(["repo", "list"]), None, None)
        .await
        .unwrap();
    let env = env_of(dir.path(), "repo");
    assert!(env.contains("HOST_SECRET=host-only"));
    assert!(!env.contains("HELM_KUBECONTEXT="));
}
