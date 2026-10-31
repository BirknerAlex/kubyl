//! Changes an agent proposes, and the user's decision.
//!
//! Write tools never touch a cluster by themselves: they build a [`Proposal`] (what changes,
//! as a summary and a diff of masked YAML), hand it to an [`Approver`] and act only on
//! [`Decision::Approved`]. The approver is whoever can ask the user: the agent panel for a
//! thread, the Kubyl window for an outside MCP client. Nothing waits forever: a request that
//! isn't answered in [`ANSWER_TIMEOUT`] counts as rejected.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use kubyl_base::ClusterId;
use tokio::sync::{mpsc, oneshot};

/// How long the user has to answer before the change is dropped.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// One object a change touches, as masked YAML.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeDoc {
    /// `deployment shop/web`.
    pub label: String,
    /// The live object; `None` when the change creates it.
    pub old: Option<String>,
    /// What it becomes; empty when the change deletes it.
    pub new: String,
}

/// A change waiting for the user.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub cluster: ClusterId,
    pub cluster_name: String,
    /// The cluster is flagged PROD in Kubyl.
    pub production: bool,
    /// One line: `Scale deployment shop/web from 2 to 5`.
    pub title: String,
    /// The objects and what happens to them.
    pub docs: Vec<ChangeDoc>,
    /// Things the user should weigh (a namespace delete removes everything in it).
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Approved,
    Rejected,
}

/// Asks the user whether a change may happen.
pub trait Approver: Send + Sync {
    fn approve(&self, proposal: Proposal) -> BoxFuture<'static, Decision>;
}

/// A [`Proposal`] on its way to the user, with the way back.
#[derive(Debug)]
pub struct ApprovalRequest {
    pub proposal: Proposal,
    pub reply: oneshot::Sender<Decision>,
}

/// An [`Approver`] that sends requests down a channel for whoever owns the UI to answer.
#[derive(Debug)]
pub struct ChannelApprover {
    requests: mpsc::UnboundedSender<ApprovalRequest>,
}

impl ChannelApprover {
    pub fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<ApprovalRequest>) {
        let (requests, receiver) = mpsc::unbounded_channel();
        (Arc::new(Self { requests }), receiver)
    }
}

impl Approver for ChannelApprover {
    fn approve(&self, proposal: Proposal) -> BoxFuture<'static, Decision> {
        let (reply, answer) = oneshot::channel();
        let sent = self
            .requests
            .send(ApprovalRequest { proposal, reply })
            .is_ok();
        Box::pin(async move {
            if !sent {
                return Decision::Rejected;
            }
            match tokio::time::timeout(ANSWER_TIMEOUT, answer).await {
                Ok(Ok(decision)) => decision,
                // Closed without an answer (the thread ended), or too slow.
                _ => Decision::Rejected,
            }
        })
    }
}

/// Test helper: approves or rejects everything.
#[derive(Clone, Copy, Debug)]
pub struct Fixed(pub Decision);

impl Approver for Fixed {
    fn approve(&self, _: Proposal) -> BoxFuture<'static, Decision> {
        let decision = self.0;
        Box::pin(async move { decision })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal() -> Proposal {
        Proposal {
            cluster: ClusterId::new("kind"),
            cluster_name: "kind".into(),
            production: false,
            title: "Scale deployment shop/web from 2 to 3".into(),
            docs: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[tokio::test]
    async fn the_answer_comes_back_and_silence_rejects() {
        let (approver, mut requests) = ChannelApprover::new();
        let waiting = tokio::spawn(approver.approve(proposal()));
        let request = requests.recv().await.unwrap();
        assert_eq!(request.proposal.title, proposal().title);
        request.reply.send(Decision::Approved).unwrap();
        assert_eq!(waiting.await.unwrap(), Decision::Approved);

        // The owner drops the request without answering.
        let waiting = tokio::spawn(approver.approve(proposal()));
        drop(requests.recv().await.unwrap());
        assert_eq!(waiting.await.unwrap(), Decision::Rejected);

        // Nobody listens any more.
        drop(requests);
        assert_eq!(approver.approve(proposal()).await, Decision::Rejected);
    }
}
