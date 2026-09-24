//! The Tokio runtime and the bridge from GPUI into it.
//!
//! GPUI's executors drive the UI. Everything that talks to a Kubernetes API server (kube-rs,
//! hyper, rustls) needs Tokio, so it runs on one shared multi-thread runtime on background
//! threads. Views never await network calls on the UI thread; they call [`spawn_kube`] and
//! await the returned [`gpui::Task`] from `cx.spawn`:
//!
//! ```ignore
//! let task = kubyl_core::spawn_kube(cx, async move { client.list_pods().await });
//! cx.spawn(async move |this, cx| {
//!     let pods = task.await?;
//!     this.update(cx, |this, cx| { this.pods = pods; cx.notify(); })
//! })
//! .detach();
//! ```
//!
//! For streams (watches, logs), spawn a Tokio task that sends into a
//! `futures::channel::mpsc` channel and consume the receiver in `cx.spawn`. Batch updates
//! before calling `cx.notify()` (≤ 60 Hz) so large watch streams don't re-render per event.

use std::future::Future;
use std::sync::OnceLock;

use gpui::{App, Task};
use tokio::runtime::{Handle, Runtime};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// The shared Tokio runtime. Created on first use.
pub fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .thread_name("kubyl-tokio")
            .enable_all()
            .build()
            .expect("failed to start the Tokio runtime")
    })
}

/// A handle to the shared runtime, for code that must spawn Tokio tasks itself.
pub fn handle() -> Handle {
    runtime().handle().clone()
}

/// Runs `future` on the Tokio runtime and returns a GPUI task that resolves to its output.
///
/// Dropping the returned task aborts the Tokio task, so a view that goes away cancels its
/// in-flight requests. Call `.detach()` to let it run to completion instead.
///
/// A panic inside `future` is re-raised when the task is awaited.
pub fn spawn_kube<F, T>(cx: &App, future: F) -> Task<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let mut guard = AbortOnDrop(runtime().spawn(future));
    cx.background_executor().spawn(async move {
        match (&mut guard.0).await {
            Ok(output) => output,
            Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
            // Only reachable while the runtime shuts down at process exit.
            Err(_) => std::future::pending().await,
        }
    })
}

struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[gpui::test]
    async fn spawn_kube_resolves_on_tokio(cx: &mut gpui::TestAppContext) {
        cx.executor().allow_parking();
        let task = cx.update(|cx| {
            spawn_kube(cx, async {
                // Only works inside a Tokio runtime.
                tokio::time::sleep(Duration::from_millis(1)).await;
                Handle::try_current().is_ok()
            })
        });
        assert!(task.await);
    }

    #[gpui::test]
    async fn dropping_the_task_aborts_the_tokio_task(cx: &mut gpui::TestAppContext) {
        cx.executor().allow_parking();
        let finished = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let task = cx.update(|cx| {
            let finished = finished.clone();
            spawn_kube(cx, async move {
                started_tx.send(()).ok();
                tokio::time::sleep(Duration::from_millis(200)).await;
                finished.store(true, Ordering::SeqCst);
            })
        });
        started_rx.recv().unwrap();
        drop(task);
        // The test executor drops a cancelled future the next time it runs its queue.
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(400));
        assert!(!finished.load(Ordering::SeqCst));
    }
}
