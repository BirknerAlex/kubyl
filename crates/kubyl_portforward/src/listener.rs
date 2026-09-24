//! Binds a local TCP port and bridges each accepted connection to a fresh kube portforward
//! stream, off the UI thread. A fresh stream per local connection means a Service forward
//! automatically re-resolves to a live pod (see [`crate::resolve`]) if the previous one died
//! between connections.

use futures::channel::mpsc;
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use tokio::net::{TcpListener, TcpStream};

use crate::resolve::{self, ForwardKind, RemotePort};

#[derive(Clone, Debug)]
pub enum ForwardEvent {
    Listening { local_port: u16 },
    ConnectionOpened,
    ConnectionClosed,
    BytesTransferred { sent: u64, received: u64 },
    Error(String),
}

/// Runs the forward until dropped. Binds `bind_address:local_port` (`local_port == 0` picks a
/// free port; the actual port is reported via [`ForwardEvent::Listening`]).
pub async fn run(
    client: kube::Client,
    namespace: String,
    kind: ForwardKind,
    port: RemotePort,
    bind_address: String,
    local_port: u16,
    events: mpsc::UnboundedSender<ForwardEvent>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind((bind_address.as_str(), local_port)).await?;
    let actual_port = listener.local_addr()?.port();
    events
        .unbounded_send(ForwardEvent::Listening {
            local_port: actual_port,
        })
        .ok();

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(pair) => pair,
            Err(err) => {
                events
                    .unbounded_send(ForwardEvent::Error(err.to_string()))
                    .ok();
                continue;
            }
        };
        let client = client.clone();
        let namespace = namespace.clone();
        let kind = kind.clone();
        let port = port.clone();
        let events = events.clone();
        tokio::spawn(async move {
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

    events.unbounded_send(ForwardEvent::ConnectionOpened).ok();
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
