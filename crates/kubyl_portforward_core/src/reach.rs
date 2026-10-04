//! "Reach this service": a temporary loopback port-forward to a Service, for features that talk
//! to a Service's API (Alertmanager, Prometheus, Hubble Relay, Argo CD…) and don't care how the
//! connection is made.
//!
//! A feature asks a [`Reach`] for a [`ReachRequest`] and gets the local port back once it
//! listens. Dropping the [`Reached`] stops the forward. The forwards themselves live in
//! [`ForwardsCore`](crate::manager::ForwardsCore), which isn't `Send`; [`channel`] connects a
//! `Send` [`Reach`] to the host thread that owns it, and
//! [`ForwardsCore::serve`](crate::manager::ForwardsCore::serve) answers it on any
//! [`Host`](kubyl_base::host::Host).

use std::time::Duration;

use futures::FutureExt as _;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use kubyl_base::ClusterId;

/// The port of a Service to reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReachPort {
    Number(u16),
    /// A named port; the Service says which number it is.
    Name(String),
}

impl ReachPort {
    /// A port as typed in settings or found in a Service: a number, else a name.
    pub fn parse(port: &str) -> Self {
        port.parse()
            .map_or_else(|_| Self::Name(port.into()), Self::Number)
    }
}

/// What to reach.
#[derive(Clone)]
pub struct ReachRequest {
    pub client: kube::Client,
    pub cluster: ClusterId,
    pub namespace: String,
    pub service: String,
    pub port: ReachPort,
    /// The session title of the forward, e.g. `Alertmanager · svc/alertmanager-main`.
    pub title: String,
    /// The Service speaks TLS (for the URL the forward shows).
    pub https: bool,
    /// Give up when the forward doesn't listen within this time.
    pub timeout: Duration,
    /// Give up earlier when the forward is reconnecting (its target doesn't resolve) for this
    /// long, reporting why. `None`: wait for `timeout`.
    pub reconnect_grace: Option<Duration>,
}

/// A forward that listens. Dropping it stops the forward.
pub struct Reached {
    pub local_port: u16,
    stopped: oneshot::Receiver<()>,
    _guard: ReachGuard,
}

impl Reached {
    /// Resolves when the forward ended for any reason other than dropping this value: the user
    /// stopped it, or its cluster disconnected.
    pub async fn stopped(&mut self) {
        let _ = (&mut self.stopped).await;
    }

    /// A future that resolves like [`Self::stopped`], without borrowing `self`.
    pub fn take_stopped(&mut self) -> BoxFuture<'static, ()> {
        let (_, closed) = oneshot::channel();
        let stopped = std::mem::replace(&mut self.stopped, closed);
        async move {
            let _ = stopped.await;
        }
        .boxed()
    }
}

impl Reached {
    /// A forward without a host behind it, for fakes of [`Reach`] in tests of the features that
    /// use one. The [`FakeReached`] plays the host: it ends the forward and sees it released.
    pub fn fake(local_port: u16) -> (Self, FakeReached) {
        let (commands, released) = mpsc::unbounded();
        let (end, stopped) = oneshot::channel();
        let reached = Self {
            local_port,
            stopped,
            _guard: ReachGuard { id: 0, commands },
        };
        (
            reached,
            FakeReached {
                end: Some(end),
                released,
            },
        )
    }
}

/// The host side of a [`Reached::fake`].
pub struct FakeReached {
    end: Option<oneshot::Sender<()>>,
    released: mpsc::UnboundedReceiver<ReachCommand>,
}

impl FakeReached {
    /// Ends the forward as if the user stopped it.
    pub fn end(&mut self) {
        if let Some(end) = self.end.take() {
            end.send(()).ok();
        }
    }

    /// Whether the [`Reached`] was dropped, which stops a real forward.
    pub fn released(&mut self) -> bool {
        matches!(self.released.try_recv(), Ok(ReachCommand::Stop(_)))
    }
}

struct ReachGuard {
    id: u64,
    commands: mpsc::UnboundedSender<ReachCommand>,
}

impl Drop for ReachGuard {
    fn drop(&mut self) {
        self.commands
            .unbounded_send(ReachCommand::Stop(self.id))
            .ok();
    }
}

/// Opens forwards to Services. Implemented by [`ChannelReach`]; a feature that needs one takes
/// an `Arc<dyn Reach>`.
pub trait Reach: Send + Sync + 'static {
    fn reach(&self, request: ReachRequest) -> BoxFuture<'static, Result<Reached, String>>;
}

/// What a [`ChannelReach`] sends to the host that owns the forwards.
pub enum ReachCommand {
    Open {
        request: Box<ReachRequest>,
        reply: oneshot::Sender<Result<Opened, String>>,
    },
    Stop(u64),
}

/// A forward the host opened.
pub struct Opened {
    pub(crate) id: u64,
    pub(crate) local_port: u16,
    pub(crate) stopped: oneshot::Receiver<()>,
}

impl Opened {
    /// A forward that listens on `local_port`. `stopped` resolves when the forward ends on its
    /// own. For hosts that open forwards without [`ForwardsCore`](crate::manager::ForwardsCore),
    /// such as the fakes in tests.
    pub fn new(id: u64, local_port: u16, stopped: oneshot::Receiver<()>) -> Self {
        Self {
            id,
            local_port,
            stopped,
        }
    }
}

/// A [`Reach`] that sends its requests to the host thread owning the forwards.
#[derive(Clone)]
pub struct ChannelReach {
    commands: mpsc::UnboundedSender<ReachCommand>,
}

/// The requests of a [`ChannelReach`], for
/// [`ForwardsCore::serve`](crate::manager::ForwardsCore::serve).
pub struct ReachInbox(pub(crate) mpsc::UnboundedReceiver<ReachCommand>);

impl futures::Stream for ReachInbox {
    type Item = ReachCommand;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<ReachCommand>> {
        std::pin::Pin::new(&mut self.0).poll_next(cx)
    }
}

pub fn channel() -> (ChannelReach, ReachInbox) {
    let (commands, inbox) = mpsc::unbounded();
    (ChannelReach { commands }, ReachInbox(inbox))
}

impl Reach for ChannelReach {
    fn reach(&self, mut request: ReachRequest) -> BoxFuture<'static, Result<Reached, String>> {
        let commands = self.commands.clone();
        async move {
            if let ReachPort::Name(name) = &request.port {
                // Forwards take the Service's port number: look a named one up.
                let number =
                    service_port(&request.client, &request.namespace, &request.service, name)
                        .await
                        .ok_or_else(|| format!("svc/{} has no port {name}", request.service))?;
                request.port = ReachPort::Number(number);
            }
            let (reply, answer) = oneshot::channel();
            commands
                .unbounded_send(ReachCommand::Open {
                    request: Box::new(request),
                    reply,
                })
                .map_err(|_| "port-forwards aren't available".to_string())?;
            let opened = answer
                .await
                .map_err(|_| "the port-forward stopped".to_string())??;
            Ok(Reached {
                local_port: opened.local_port,
                stopped: opened.stopped,
                _guard: ReachGuard {
                    id: opened.id,
                    commands,
                },
            })
        }
        .boxed()
    }
}

/// The number of a Service's named port.
pub async fn service_port(
    client: &kube::Client,
    namespace: &str,
    service: &str,
    name: &str,
) -> Option<u16> {
    let request = http::Request::get(format!("/api/v1/namespaces/{namespace}/services/{service}"))
        .body(Vec::new())
        .ok()?;
    let svc: serde_json::Value = client.request(request).await.ok()?;
    svc.pointer("/spec/ports")?
        .as_array()?
        .iter()
        .find(|p| p["name"].as_str() == Some(name))
        .and_then(|p| p["port"].as_u64())
        .and_then(|p| u16::try_from(p).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fake_reached_stops_like_a_forward_and_reports_its_release() {
        let (mut reached, mut end) = Reached::fake(4242);
        assert_eq!(reached.local_port, 4242);
        let stopped = reached.take_stopped();
        end.end();
        futures::executor::block_on(stopped);

        assert!(!end.released());
        drop(reached);
        assert!(end.released());
    }
}
