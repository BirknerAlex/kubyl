//! Discovery against the cloud CLIs installed on this machine, with their current sign-in.
//! Ignored by default (it talks to real accounts; read-only: list calls only):
//!
//! ```sh
//! cargo test -p kubyl_kubeconfig_core --test live_cloud -- --ignored --nocapture
//! ```
//!
//! Prints counts, never account or cluster names. A cloud whose CLI isn't installed must say
//! so; a signed-in one must list its accounts and scan each without an unexplained failure.

use kubyl_kubeconfig_core::cloud::{Failure, Provider, SystemRunner, accounts, scan};

#[tokio::test]
#[ignore = "talks to the real cloud CLIs"]
async fn every_cloud_lists_accounts_or_says_what_to_do() {
    let runner = SystemRunner::default();
    for provider in Provider::ALL {
        match accounts(provider, &runner).await {
            Ok(accounts) => {
                let mut clusters = 0;
                let mut benign = 0;
                for account in &accounts {
                    let scan = scan(&runner, account).await;
                    clusters += scan.clusters.len();
                    for problem in &scan.problems {
                        // A project without the API is fine; anything else says what to do.
                        if problem.failure.is_benign() {
                            benign += 1;
                        } else {
                            assert!(
                                !matches!(problem.failure, Failure::Other(ref m) if m.is_empty()),
                                "{provider:?}: an empty failure"
                            );
                            println!(
                                "{provider:?}: a problem: {}",
                                short(&problem.failure.to_string())
                            );
                        }
                    }
                }
                println!(
                    "{provider:?}: {} {}s, {clusters} clusters, {benign} without the API",
                    accounts.len(),
                    provider.account_label()
                );
            }
            Err(failure) => {
                println!("{provider:?}: {}", short(&failure.to_string()));
                assert!(
                    matches!(
                        failure,
                        Failure::NotInstalled(_)
                            | Failure::SignIn { .. }
                            | Failure::Permission { .. }
                            | Failure::Timeout
                            | Failure::Other(_)
                    ),
                    "{failure:?}"
                );
                if let Failure::NotInstalled(p) = &failure {
                    assert!(failure.to_string().contains(p.install_url()));
                }
            }
        }
    }
}

/// The first words of a message (enough to see what kind it is).
fn short(message: &str) -> String {
    message.chars().take(80).collect()
}
