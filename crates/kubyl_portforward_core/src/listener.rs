//! Binds a local TCP port and bridges each accepted connection to a fresh kube portforward
//! stream, off the UI thread. A fresh stream per local connection means a Service forward
//! automatically re-resolves to a live pod (see [`crate::resolve`]) if the previous one died
//! between connections.

use std::time::Duration;

use futures::channel::mpsc;
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

use crate::resolve::{self, ForwardKind, RemotePort};

/// Initial delay before retrying `accept()` after an error (e.g. EMFILE), doubled each
/// consecutive failure up to [`MAX_ACCEPT_BACKOFF`].
const INITIAL_ACCEPT_BACKOFF: Duration = Duration::from_millis(50);
/// Cap on the accept-error backoff, so a listener recovers reasonably quickly once the
/// underlying condition (e.g. too many open files) clears.
const MAX_ACCEPT_BACKOFF: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub enum ForwardEvent {
    Listening {
        local_port: u16,
    },
    /// A local connection reached `pod`.
    ConnectionOpened {
        pod: String,
    },
    ConnectionClosed,
    BytesTransferred {
        sent: u64,
        received: u64,
    },
    Error(String),
}

/// Binds `bind_address:local_port`; with `fallback_any`, a taken port falls back to a free one
/// (no probe-then-bind race).
async fn bind(
    bind_address: &str,
    local_port: u16,
    fallback_any: bool,
) -> std::io::Result<TcpListener> {
    match TcpListener::bind((bind_address, local_port)).await {
        Err(_) if fallback_any && local_port != 0 => TcpListener::bind((bind_address, 0)).await,
        result => result,
    }
}

/// Runs the forward until dropped. Binds `bind_address:local_port` (`local_port == 0` picks a
/// free port; the actual port is reported via [`ForwardEvent::Listening`]).
#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: kube::Client,
    namespace: String,
    kind: ForwardKind,
    port: RemotePort,
    bind_address: String,
    local_port: u16,
    fallback_any: bool,
    events: mpsc::UnboundedSender<ForwardEvent>,
) -> anyhow::Result<()> {
    let listener = bind(&bind_address, local_port, fallback_any).await?;
    let actual_port = listener.local_addr()?.port();
    events
        .unbounded_send(ForwardEvent::Listening {
            local_port: actual_port,
        })
        .ok();

    // Connections in flight, so stopping the forward (dropping/cancelling this task) aborts
    // them too instead of leaving `copy_bidirectional` running in the background.
    let mut connections = JoinSet::new();
    let mut accept_backoff = INITIAL_ACCEPT_BACKOFF;
    let mut last_accept_error: Option<String> = None;

    loop {
        // Reap finished connections so the set doesn't grow unbounded.
        while connections.try_join_next().is_some() {}

        let (stream, _) = match listener.accept().await {
            Ok(pair) => {
                accept_backoff = INITIAL_ACCEPT_BACKOFF;
                last_accept_error = None;
                pair
            }
            Err(err) => {
                let message = err.to_string();
                if last_accept_error.as_deref() != Some(message.as_str()) {
                    events
                        .unbounded_send(ForwardEvent::Error(message.clone()))
                        .ok();
                    last_accept_error = Some(message);
                }
                tokio::time::sleep(accept_backoff).await;
                accept_backoff = (accept_backoff * 2).min(MAX_ACCEPT_BACKOFF);
                continue;
            }
        };
        let client = client.clone();
        let namespace = namespace.clone();
        let kind = kind.clone();
        let port = port.clone();
        let events = events.clone();
        connections.spawn(async move {
            if let Err(err) =
                handle_connection(client, &namespace, &kind, port, stream, &events).await
            {
                events
                    .unbounded_send(ForwardEvent::Error(err.to_string()))
                    .ok();
            }
        });
    }
}

async fn handle_connection(
    client: kube::Client,
    namespace: &str,
    kind: &ForwardKind,
    port: RemotePort,
    local: TcpStream,
    events: &mpsc::UnboundedSender<ForwardEvent>,
) -> anyhow::Result<()> {
    let resolved = resolve::resolve(client.clone(), namespace, kind, port).await?;
    let api: Api<Pod> = Api::namespaced(client, namespace);
    let mut forwarder = api.portforward(&resolved.pod, &[resolved.port]).await?;
    let remote = forwarder
        .take_stream(resolved.port)
        .ok_or_else(|| anyhow::anyhow!("no stream for port {}", resolved.port))?;

    events
        .unbounded_send(ForwardEvent::ConnectionOpened {
            pod: resolved.pod.clone(),
        })
        .ok();
    let mut local = local;
    let mut remote = remote;
    let copy = tokio::io::copy_bidirectional(&mut local, &mut remote).await;
    events.unbounded_send(ForwardEvent::ConnectionClosed).ok();
    match copy {
        Ok((sent, received)) => {
            events
                .unbounded_send(ForwardEvent::BytesTransferred { sent, received })
                .ok();
        }
        Err(err) => {
            events
                .unbounded_send(ForwardEvent::Error(err.to_string()))
                .ok();
        }
    }
    forwarder.join().await.ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taken_port_falls_back_only_when_asked() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let held = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let port = held.local_addr().unwrap().port();
            assert!(bind("127.0.0.1", port, false).await.is_err());
            let other = bind("127.0.0.1", port, true).await.unwrap();
            assert_ne!(other.local_addr().unwrap().port(), port);
        });
    }
}
