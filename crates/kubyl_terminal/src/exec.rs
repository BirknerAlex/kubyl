//! Bridges a `kube::Api::exec`/`attach` websocket to byte channels the GPUI view drives, off
//! the UI thread. `AttachedProcess` already aborts its own background task on drop, so this
//! function's only job is proxying bytes and resize requests.

use futures::channel::mpsc;
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use kube::api::AttachParams;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// What to run: a shell/command exec, or an attach to the running process (PID 1).
#[derive(Clone, Debug)]
pub enum Mode {
    Exec { command: Vec<String> },
    Attach,
}

#[derive(Clone, Debug)]
pub struct ExecTarget {
    pub namespace: String,
    pub pod: String,
    pub container: Option<String>,
    pub mode: Mode,
}

/// Runs until `input` closes or the connection drops. Output bytes (both stdout and stderr,
/// interleaved) go to `output`; `resize` carries `(columns, rows)` requests.
pub async fn run(
    client: kube::Client,
    target: ExecTarget,
    mut input: mpsc::UnboundedReceiver<Vec<u8>>,
    output: mpsc::UnboundedSender<Vec<u8>>,
    mut resize: mpsc::UnboundedReceiver<(u16, u16)>,
) -> anyhow::Result<()> {
    use futures::StreamExt as _;

    let api: Api<Pod> = Api::namespaced(client, &target.namespace);
    let mut ap = AttachParams::interactive_tty().stderr(false);
    if let Some(container) = &target.container {
        ap = ap.container(container.clone());
    }

    let mut attached = match &target.mode {
        Mode::Exec { command } => api.exec(&target.pod, command, &ap).await?,
        Mode::Attach => api.attach(&target.pod, &ap).await?,
    };

    let mut stdin = attached.stdin();
    let mut stdout = attached.stdout();
    let mut resize_tx = attached.terminal_size();

    loop {
        let mut buf = [0u8; 4096];
        tokio::select! {
            bytes = input.next() => {
                let Some(bytes) = bytes else { break };
                if let Some(stdin) = stdin.as_mut() {
                    stdin.write_all(&bytes).await.ok();
                }
            }
            size = resize.next() => {
                let Some((columns, rows)) = size else { continue };
                if let Some(tx) = resize_tx.as_mut() {
                    use futures::SinkExt as _;
                    tx.send(kube::api::TerminalSize { width: columns, height: rows })
                        .await
                        .ok();
                }
            }
            read = async {
                match stdout.as_mut() {
                    Some(stdout) => stdout.read(&mut buf).await,
                    None => std::future::pending().await,
                }
            } => {
                match read {
                    Ok(0) => break,
                    Ok(n) => {
                        if output.unbounded_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }
    drop(stdin);
    attached.join().await.ok();
    Ok(())
}
