//! ACP protocol checks against the real agent adapters installed on this machine, without a
//! single prompt: no model is called, so no tokens are spent. Ignored by default and never run
//! in CI (the adapters aren't installed there):
//!
//! ```sh
//! cargo test -p kubyl_agent_core --test adapters -- --ignored --nocapture
//! ```
//!
//! For every built-in agent found through the login shell's `PATH` (`claude-agent-acp`,
//! `codex-acp`, `gemini`, `copilot`, `goose`, `opencode`): `initialize` with Kubyl's
//! capabilities, `session/new` with Kubyl's MCP server, the session settings it offers, a
//! `session/set_config_option` that keeps the current value, and `session/cancel`. An agent that
//! wants a sign-in first (`auth_required`) passes with a note. `KUBYL_TEST_AGENTS=claude,codex`
//! limits the run.

use std::path::PathBuf;

use kubyl_agent_core::acp::{self, SessionScope};
use kubyl_agent_core::agents;
use kubyl_agent_core::jsonrpc::RpcError;
use kubyl_agent_core::mcp;
use kubyl_agent_core::thread::{ConfigValue, config_options};
use kubyl_agent_core::tools::ToolContext;
use tokio::sync::watch;

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn installed_adapters_speak_acp_without_a_prompt() {
    let only: Option<Vec<String>> = std::env::var("KUBYL_TEST_AGENTS")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect());
    let dir = tempfile::tempdir().unwrap();
    let kubeconfig = dir.path().join("kubeconfig");
    std::fs::write(&kubeconfig, kubyl_agent_core::kubeconfig::EMPTY).unwrap();
    let shell_path = tokio::task::spawn_blocking(kubyl_kube_core::auth::shell_env::path)
        .await
        .unwrap();

    let mut checked = 0;
    for spec in agents::builtins() {
        if only.as_ref().is_some_and(|o| !o.contains(&spec.id)) {
            continue;
        }
        let Some(program) = agents::resolve(&spec.command, shell_path.as_deref()) else {
            println!("{}: not installed, skipped", spec.name);
            continue;
        };
        println!("{}: {}", spec.name, program.display());
        check(
            &spec.name,
            program,
            &spec.args,
            &kubeconfig,
            shell_path.clone(),
            dir.path().to_path_buf(),
        )
        .await;
        checked += 1;
    }
    println!("{checked} adapter(s) checked");
}

async fn check(
    name: &str,
    program: PathBuf,
    args: &[String],
    kubeconfig: &std::path::Path,
    path: Option<std::ffi::OsString>,
    cwd: PathBuf,
) {
    let (agent, _calls) = acp::start(acp::Launch {
        program,
        args: args.to_vec(),
        env: Vec::new(),
        path: path.clone(),
        kubeconfig: kubeconfig.to_path_buf(),
        cwd: cwd.clone(),
        cli_env: Default::default(),
        capabilities: Default::default(),
        session_meta: None,
    })
    .await
    .unwrap_or_else(|err| panic!("{name}: initialize failed: {err}"));
    let init = &agent.init;
    println!(
        "  initialize: ACP v1, loadSession {}, http MCP {}, embedded context {}, {} auth method(s)",
        init.load_session(),
        init.http_mcp(),
        init.embedded_context(),
        init.auth_methods.len()
    );

    // Kubyl's MCP server as the agent would get it, over HTTP or the stdio bridge.
    let (_tx, rx) = watch::channel(ToolContext::default());
    let server = mcp::serve(rx, None).await.unwrap();
    // No Kubyl binary in this crate's tests: agents without HTTP MCP get a bridge that doesn't
    // exist, and must still start the session (they report the server as failed).
    let bridge = PathBuf::from("/nonexistent/kubyl");
    let entry = server.acp_entry(init.http_mcp(), &bridge);
    let scope = SessionScope {
        thread: 1,
        root: cwd,
        kubeconfig: kubeconfig.to_path_buf(),
        path,
    };
    let session = match agent.new_session(scope, entry).await {
        Ok(session) => session,
        Err(err) if err.code == RpcError::AUTH_REQUIRED => {
            println!(
                "  session/new: needs a sign-in first ({}), the rest skipped",
                err.message
            );
            return;
        }
        Err(err) => panic!("{name}: session/new failed: {err}"),
    };
    let id = session.session_id.0.to_string();
    assert!(!id.is_empty(), "{name}: empty session id");
    let options = config_options(session.config_options.as_deref().unwrap_or_default());
    for option in &options {
        println!(
            "  setting {} ({:?}): {}",
            option.name,
            option.category,
            option.current_label()
        );
    }
    if let Some(modes) = &session.modes {
        println!(
            "  modes: {} (current {})",
            modes.available_modes.len(),
            modes.current_mode_id.0
        );
    }

    // Set a select setting to the value it already has: the protocol round trip without
    // changing anything.
    if let Some(option) = options
        .iter()
        .find(|o| matches!(o.value, ConfigValue::Select { .. }))
    {
        let answer = agent
            .set_config_option(&id, &option.id, &option.value)
            .await
            .unwrap_or_else(|err| panic!("{name}: set_config_option failed: {err}"));
        assert!(
            answer.get("configOptions").is_some_and(|o| o.is_array()),
            "{name}: set_config_option answered without configOptions: {answer}"
        );
        println!("  set_config_option {}: ok", option.id);
    }

    // Cancelling with nothing running must be harmless.
    agent.cancel(&id);
    assert!(
        agent.is_open(),
        "{name}: the agent closed after session/cancel"
    );
    agent.close_session(&id);
}
