//! Local shells against a real cluster. Ignored by default:
//!
//! ```sh
//! script/dev-cluster.sh
//! kind get kubeconfig --name kubyl-dev > /tmp/kubyl-dev.yaml
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev.yaml \
//!   cargo test -p kubyl_terminal --test live_local -- --ignored --nocapture --test-threads 1
//! ```
//!
//! Needs `kubectl` on the `PATH`. Read-only: the one-context kubeconfig is checked by asking
//! the API server who it is (`kubectl auth whoami`), once with the kubeconfig's own client
//! certificate and once with a token that only reaches the shell through the environment (a
//! token for kube-system's `default` account, which `kubectl create token` mints without
//! creating anything).

use std::path::PathBuf;
use std::process::Command;

use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use kubyl_terminal::exec::Ending;
use kubyl_terminal::local::{self, LaunchPlan};

fn kubeconfig() -> PathBuf {
    PathBuf::from(std::env::var("KUBYL_TEST_KUBECONFIG").expect("KUBYL_TEST_KUBECONFIG"))
}

fn context() -> String {
    std::env::var("KUBYL_TEST_CONTEXT").unwrap_or_else(|_| "kind-kubyl-dev".into())
}

/// Runs `script` in a local shell built from the plan; returns its output and ending.
async fn run_script(mut plan: LaunchPlan, script: &str) -> (String, Ending) {
    plan.program = "/bin/sh".into();
    plan.args = vec!["-c".into(), script.into()];
    let (_input_tx, input_rx) = mpsc::unbounded();
    let (output_tx, mut output_rx) = mpsc::channel(16);
    let (_resize_tx, resize_rx) = mpsc::unbounded();
    let (connected_tx, connected_rx) = oneshot::channel();
    let run = tokio::spawn(local::run(
        plan,
        (200, 40),
        input_rx,
        output_tx,
        resize_rx,
        connected_tx,
    ));
    connected_rx.await.unwrap().unwrap();
    let mut text = Vec::new();
    while let Some(chunk) = output_rx.next().await {
        text.extend(chunk);
    }
    let ending = run.await.unwrap().unwrap();
    (String::from_utf8_lossy(&text).trim().to_string(), ending)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a cluster (script/dev-cluster.sh) and kubectl"]
async fn kubectl_in_a_local_shell_reaches_only_the_context() {
    let text = local::kubeconfig_text(&kubeconfig(), &context(), Some("kube-system"), false)
        .expect("kubeconfig");
    let plan = LaunchPlan::build(&text, &context(), None, None, None).unwrap();
    let file = plan.kubeconfig().to_owned();
    let (out, ending) = run_script(
        plan,
        "kubectl config get-contexts -o name; \
         kubectl auth whoami -o jsonpath='{.status.userInfo.username}'; echo; \
         kubectl config view --minify -o jsonpath='{..namespace}'",
    )
    .await;
    println!("{out}");
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    assert_eq!(lines[0], context(), "only the one context: {out}");
    assert_eq!(lines[1], "kubernetes-admin", "{out}");
    assert_eq!(lines[2], "kube-system", "the tab's namespace: {out}");
    assert_eq!(ending, Ending::Exited { code: Some(0) });
    assert!(!file.exists(), "the kubeconfig outlived the shell");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a cluster (script/dev-cluster.sh) and kubectl"]
async fn a_kubyl_token_reaches_kubectl_through_the_environment_only() {
    let minted = Command::new("kubectl")
        .args(["--kubeconfig"])
        .arg(kubeconfig())
        .args([
            "--context",
            &context(),
            "-n",
            "kube-system",
            "create",
            "token",
            "default",
        ])
        .output()
        .expect("kubectl");
    assert!(
        minted.status.success(),
        "{}",
        String::from_utf8_lossy(&minted.stderr)
    );
    let token = String::from_utf8(minted.stdout).unwrap().trim().to_string();

    let text = local::kubeconfig_text(&kubeconfig(), &context(), None, true).expect("kubeconfig");
    assert!(!text.contains(&token));
    let plan = LaunchPlan::build(&text, &context(), Some(&token), None, None).unwrap();
    // The file on disk, and what the plan says about itself, never hold the token.
    let on_disk = std::fs::read_to_string(plan.kubeconfig()).unwrap();
    assert!(!on_disk.contains(&token) && !format!("{plan:?}").contains(&token));
    assert!(plan.env_value(local::TOKEN_ENV).is_some());

    let (out, ending) = run_script(
        plan,
        "kubectl auth whoami -o jsonpath='{.status.userInfo.username}'",
    )
    .await;
    println!("{out}");
    assert_eq!(out, "system:serviceaccount:kube-system:default");
    assert_eq!(ending, Ending::Exited { code: Some(0) });
}
