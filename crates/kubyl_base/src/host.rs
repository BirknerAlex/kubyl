//! How core services run without knowing the UI framework.
//!
//! A core service is a plain state struct. It never spawns, sleeps or touches app globals
//! itself; its mutating methods take a `&mut dyn Host<Self>` and ask the host to:
//!
//! - run work on the Tokio runtime ([`HostExt::spawn`]) or a background thread
//!   ([`HostExt::background`]) and call back with the result,
//! - call back after a delay ([`HostExt::after`]) or for batches of a stream
//!   ([`HostExt::batches`]),
//! - publish an event ([`Host::emit`]), carry out an app-level effect ([`Host::effect`]), request
//!   a re-render ([`Host::notify`]) or show a [`Notice`] ([`Host::toast`]).
//!
//! Callbacks run on the host's thread with the service borrowed mutably, one at a time, like
//! `Entity::update` callbacks in GPUI. Every spawn returns a [`TaskHandle`]: dropping it aborts
//! the work and its callback never runs.
//!
//! The GPUI implementation is `kubyl_core::host::GpuiHost`. [`TestHost`] runs services on a
//! private Tokio runtime for tests.

use std::any::Any;
use std::future::Future;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::{BoxStream, Stream, StreamExt as _};

use crate::notice::Notice;

/// A value of any type that crosses from a worker thread to the host.
pub type AnyBox = Box<dyn Any + Send>;

/// A state struct driven through a [`Host`].
pub trait Service: 'static {
    /// What the service tells its subscribers (GPUI adapters re-emit these with `cx.emit`).
    type Event: 'static;
    /// App-level side effects the host carries out for the service (saving state, changing
    /// settings, updating app globals).
    type Effect: 'static;
}

/// Where spawned work runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Executor {
    /// The shared Tokio runtime (network, kube-rs).
    Kube,
    /// A background thread of the host (file system, CPU work).
    Background,
}

/// When [`HostExt::batches`] delivers.
///
/// After an item arrives the host waits `first_delay` (for the first batch) or `delay` (for
/// later ones), then delivers everything that arrived by then as one batch, then waits `gap`
/// before it looks at the stream again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pace {
    pub first_delay: Duration,
    pub delay: Duration,
    pub gap: Duration,
}

impl Pace {
    /// Each item as soon as it arrives (with whatever else is already waiting).
    pub const IMMEDIATE: Pace = Pace {
        first_delay: Duration::ZERO,
        delay: Duration::ZERO,
        gap: Duration::ZERO,
    };

    /// Waits `delay` after an item so a burst arrives as one batch.
    pub const fn debounce(delay: Duration) -> Self {
        Pace {
            first_delay: delay,
            delay,
            gap: Duration::ZERO,
        }
    }

    /// Like [`Pace::debounce`], but the first batch comes right away.
    pub const fn debounce_after_first(delay: Duration) -> Self {
        Pace {
            first_delay: Duration::ZERO,
            delay,
            gap: Duration::ZERO,
        }
    }

    /// Delivers right away, then at most once per `gap`.
    pub const fn throttle(gap: Duration) -> Self {
        Pace {
            first_delay: Duration::ZERO,
            delay: Duration::ZERO,
            gap,
        }
    }

    /// How long to wait before delivering batch number `index` (0-based).
    pub fn delay_before(&self, index: usize) -> Duration {
        if index == 0 {
            self.first_delay
        } else {
            self.delay
        }
    }
}

/// Whether a batch callback wants more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Stop,
}

pub type Callback<S> = Box<dyn FnOnce(&mut S, &mut dyn Host<S>)>;
pub type AnyCallback<S> = Box<dyn FnOnce(&mut S, AnyBox, &mut dyn Host<S>)>;
pub type BatchCallback<S> = Box<dyn FnMut(&mut S, Vec<AnyBox>, &mut dyn Host<S>) -> Flow>;
pub type TickCallback<S> = Box<dyn FnMut(&mut S, &mut dyn Host<S>) -> Flow>;

/// Something a host gave back for running work, kept alive by a [`TaskHandle`].
pub trait Detach: Any {
    /// Lets the work run to completion without a handle.
    fn detach(self: Box<Self>);
}

/// Running work started through a [`Host`]. Dropping it aborts the work.
#[must_use = "dropping a TaskHandle aborts its work"]
pub struct TaskHandle(Option<Box<dyn Detach>>);

impl TaskHandle {
    pub fn new(inner: impl Detach) -> Self {
        Self(Some(Box::new(inner)))
    }

    /// A handle for work that was never started.
    pub fn none() -> Self {
        Self(None)
    }

    /// Lets the work run to completion.
    pub fn detach(mut self) {
        if let Some(inner) = self.0.take() {
            inner.detach();
        }
    }
}

impl std::fmt::Debug for TaskHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TaskHandle")
    }
}

/// What a core service needs from the app it runs in. See the module docs.
pub trait Host<S: Service> {
    fn spawn_any(
        &mut self,
        executor: Executor,
        work: BoxFuture<'static, AnyBox>,
        then: AnyCallback<S>,
    ) -> TaskHandle;

    fn after_any(&mut self, delay: Duration, then: Callback<S>) -> TaskHandle;

    /// Delivers the stream's items in batches paced by `pace`. Ends with the stream or when the
    /// callback returns [`Flow::Stop`].
    fn batches_any(
        &mut self,
        items: BoxStream<'static, AnyBox>,
        pace: Pace,
        on_batch: BatchCallback<S>,
    ) -> TaskHandle;

    /// Calls `tick` every `interval` (the first time after one interval) until it returns
    /// [`Flow::Stop`].
    fn every_any(&mut self, interval: Duration, tick: TickCallback<S>) -> TaskHandle;

    fn emit(&mut self, event: S::Event);

    fn effect(&mut self, effect: S::Effect);

    /// The service's state changed in a way views show.
    fn notify(&mut self);

    fn toast(&mut self, notice: Notice);
}

/// Typed helpers over [`Host`].
pub trait HostExt<S: Service>: Host<S> {
    /// Runs `work` on the Tokio runtime, then `then` with its output.
    fn spawn<T: Send + 'static>(
        &mut self,
        work: impl Future<Output = T> + Send + 'static,
        then: impl FnOnce(&mut S, T, &mut dyn Host<S>) + 'static,
    ) -> TaskHandle {
        spawn_on(self, Executor::Kube, work, then)
    }

    /// Runs `work` on a background thread, then `then` with its output.
    fn background<T: Send + 'static>(
        &mut self,
        work: impl Future<Output = T> + Send + 'static,
        then: impl FnOnce(&mut S, T, &mut dyn Host<S>) + 'static,
    ) -> TaskHandle {
        spawn_on(self, Executor::Background, work, then)
    }

    fn after(
        &mut self,
        delay: Duration,
        then: impl FnOnce(&mut S, &mut dyn Host<S>) + 'static,
    ) -> TaskHandle {
        self.after_any(delay, Box::new(then))
    }

    /// See [`Host::every_any`].
    fn every(
        &mut self,
        interval: Duration,
        tick: impl FnMut(&mut S, &mut dyn Host<S>) -> Flow + 'static,
    ) -> TaskHandle {
        self.every_any(interval, Box::new(tick))
    }

    /// See [`Host::batches_any`].
    fn batches<T: Send + 'static>(
        &mut self,
        items: impl Stream<Item = T> + Send + 'static,
        pace: Pace,
        mut on_batch: impl FnMut(&mut S, Vec<T>, &mut dyn Host<S>) -> Flow + 'static,
    ) -> TaskHandle {
        self.batches_any(
            items.map(|item| Box::new(item) as AnyBox).boxed(),
            pace,
            Box::new(move |service, batch, host| {
                let batch = batch.into_iter().map(|item| *downcast(item)).collect();
                on_batch(service, batch, host)
            }),
        )
    }
}

impl<S: Service, H: Host<S> + ?Sized> HostExt<S> for H {}

fn spawn_on<S: Service, H: Host<S> + ?Sized, T: Send + 'static>(
    host: &mut H,
    executor: Executor,
    work: impl Future<Output = T> + Send + 'static,
    then: impl FnOnce(&mut S, T, &mut dyn Host<S>) + 'static,
) -> TaskHandle {
    host.spawn_any(
        executor,
        Box::pin(async move { Box::new(work.await) as AnyBox }),
        Box::new(move |service, output, host| then(service, *downcast(output), host)),
    )
}

fn downcast<T: 'static>(value: AnyBox) -> Box<T> {
    value
        .downcast()
        .unwrap_or_else(|_| unreachable!("host returned a value of another type"))
}

mod test_host;
pub use test_host::TestHost;
