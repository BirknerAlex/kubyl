//! An agent process gets only the allow-listed variables of this process, plus the host's fixed
//! ones. Its own test binary: it sets variables in the process environment, which must not race
//! other tests spawning processes.
#![cfg(unix)]

use kubyl_agent_core::acp::{self, ClientCapabilities, Launch};
use kubyl_kube_core::cli::CliEnv;

fn launch(program: std::path::PathBuf, dir: &std::path::Path, cli_env: CliEnv) -> Launch {
    Launch {
        program,
        args: Vec::new(),
        env: vec![("FROM_SPEC".into(), "spec".into())],
        path: Some("/usr/bin:/bin".into()),
        kubeconfig: dir.join("kubeconfig"),
        cwd: dir.to_path_buf(),
        cli_env,
        capabilities: ClientCapabilities::default(),
        session_meta: None,
    }
}

#[tokio::test]
async fn the_agent_inherits_what_the_policy_allows() {
    for (key, value) in [("HOST_SECRET", "host-only"), ("KUBYL_TEST_KEEP", "kept")] {
        // SAFETY: the only test of this binary, before any process is spawned.
        unsafe { std::env::set_var(key, value) };
    }
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("agent");
    // Dumps its environment, then answers `initialize` with the request's id.
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\nenv > {}/env\nread line\nid=$(echo \"$line\" | sed 's/.*\"id\":\\([0-9]*\\).*/\\1/')\necho \"{{\\\"jsonrpc\\\":\\\"2.0\\\",\\\"id\\\":$id,\\\"result\\\":{{\\\"protocolVersion\\\":1,\\\"agentCapabilities\\\":{{}}}}}}\"\nsleep 5\n",
            dir.path().display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();

    let allow = CliEnv::allow_list(["KUBYL_TEST_KEEP"], Vec::<String>::new())
        .with_var("HOME", "/host/home");
    let (_agent, _calls) = acp::start(launch(program.clone(), dir.path(), allow))
        .await
        .expect("the agent initializes");
    let env = std::fs::read_to_string(dir.path().join("env")).unwrap();
    assert!(!env.contains("HOST_SECRET="), "{env}");
    assert!(env.contains("KUBYL_TEST_KEEP=kept"));
    assert!(env.contains("HOME=/host/home"));
    assert!(env.contains("FROM_SPEC=spec"));
    assert!(env.contains("KUBECONFIG="));
    assert!(env.contains("PATH=/usr/bin:/bin"));

    // The default is what it always was: the whole environment.
    let (_agent, _calls) = acp::start(launch(program, dir.path(), CliEnv::default()))
        .await
        .expect("the agent initializes");
    let env = std::fs::read_to_string(dir.path().join("env")).unwrap();
    assert!(env.contains("HOST_SECRET=host-only"), "{env}");
}
