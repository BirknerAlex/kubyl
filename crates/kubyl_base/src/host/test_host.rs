//! [`TestHost`]: runs a core service's work on a private Tokio runtime, for tests.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use futures::FutureExt as _;
use futures::StreamExt as _;
use futures::future::BoxFuture;
use futures::stream::BoxStream;

use super::{
    AnyBox, AnyCallback, BatchCallback, Callback, Detach, Executor, Flow, Host, Pace, Service,
    TaskHandle, TickCallback,
};
use crate::notice::Notice;

/// How long [`TestHost::run_until_idle`] waits for work before it gives up.
const STALL: Duration = Duration::from_secs(10);

enum Message {
    Done(u64, AnyBox),
    Batch(u64, Vec<AnyBox>),
    Tick(u64),
    Ended(u64),
}

enum Pending<S: Service> {
    Once(AnyCallback<S>),
    Timer(Callback<S>),
    Batches(BatchCallback<S>),
    Ticks(TickCallback<S>),
}

/// A [`Host`] for tests: work runs on its own Tokio runtime, callbacks run when the test calls
/// [`TestHost::run_until_idle`]. Events, effects and notices are collected for assertions.
pub struct TestHost<S: Service> {
    pub events: Vec<S::Event>,
    pub effects: Vec<S::Effect>,
    pub notices: Vec<Notice>,
    /// How often the service asked for a re-render.
    pub notified: usize,
    runtime: tokio::runtime::Runtime,
    tx: mpsc::Sender<Message>,
    rx: mpsc::Receiver<Message>,
    next_id: u64,
    pending: HashMap<u64, (Pending<S>, Arc<AtomicBool>)>,
}

struct Running {
    task: tokio::task::JoinHandle<()>,
    alive: Arc<AtomicBool>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        self.task.abort();
    }
}

impl Detach for Running {
    fn detach(self: Box<Self>) {
        // Neither abort nor forget the callback.
        std::mem::forget(*self);
    }
}

impl<S: Service> Default for TestHost<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Service> TestHost<S> {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            events: Vec::new(),
            effects: Vec::new(),
            notices: Vec::new(),
            notified: 0,
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("test runtime"),
            tx,
            rx,
            next_id: 0,
            pending: HashMap::new(),
        }
    }

    /// Runs callbacks until no work is pending (aborted work doesn't count).
    ///
    /// # Panics
    /// When pending work produces nothing for 10 s.
    pub fn run_until_idle(&mut self, service: &mut S) {
        loop {
            self.pending
                .retain(|_, (_, alive)| alive.load(Ordering::SeqCst));
            if self.pending.is_empty() {
                return;
            }
            let message = self
                .rx
                .recv_timeout(STALL)
                .expect("pending work produced nothing for 10 s");
            self.handle(service, message);
        }
    }

    /// Runs callbacks until `done` returns true, for services with work that never ends (a
    /// stream they serve).
    ///
    /// # Panics
    /// When pending work produces nothing for 10 s.
    pub fn run_until(&mut self, service: &mut S, mut done: impl FnMut(&mut S, &Self) -> bool) {
        while !done(service, self) {
            let message = self
                .rx
                .recv_timeout(STALL)
                .expect("pending work produced nothing for 10 s");
            self.handle(service, message);
        }
    }

    fn handle(&mut self, service: &mut S, message: Message) {
        match message {
            Message::Done(id, output) => match self.pending.remove(&id) {
                Some((Pending::Once(then), alive)) if alive.load(Ordering::SeqCst) => {
                    then(service, output, self)
                }
                Some((Pending::Timer(then), alive)) if alive.load(Ordering::SeqCst) => {
                    then(service, self)
                }
                _ => {}
            },
            Message::Batch(id, batch) => {
                if let Some((Pending::Batches(mut on_batch), alive)) = self.pending.remove(&id)
                    && alive.load(Ordering::SeqCst)
                    && on_batch(service, batch, self) == Flow::Continue
                {
                    self.pending.insert(id, (Pending::Batches(on_batch), alive));
                }
            }
            Message::Tick(id) => {
                if let Some((Pending::Ticks(mut tick), alive)) = self.pending.remove(&id)
                    && alive.load(Ordering::SeqCst)
                    && tick(service, self) == Flow::Continue
                {
                    self.pending.insert(id, (Pending::Ticks(tick), alive));
                }
            }
            Message::Ended(id) => {
                self.pending.remove(&id);
            }
        }
    }

    fn start(
        &mut self,
        pending: Pending<S>,
        work: impl FnOnce(u64, mpsc::Sender<Message>) -> BoxFuture<'static, ()>,
    ) -> TaskHandle {
        self.next_id += 1;
        let id = self.next_id;
        let alive = Arc::new(AtomicBool::new(true));
        self.pending.insert(id, (pending, alive.clone()));
        let task = self.runtime.spawn(work(id, self.tx.clone()));
        TaskHandle::new(Running { task, alive })
    }
}

impl<S: Service> Host<S> for TestHost<S> {
    fn spawn_any(
        &mut self,
        _executor: Executor,
        work: BoxFuture<'static, AnyBox>,
        then: AnyCallback<S>,
    ) -> TaskHandle {
        self.start(Pending::Once(then), |id, tx| {
            async move {
                tx.send(Message::Done(id, work.await)).ok();
            }
            .boxed()
        })
    }

    fn after_any(&mut self, delay: Duration, then: Callback<S>) -> TaskHandle {
        self.start(Pending::Timer(then), move |id, tx| {
            async move {
                tokio::time::sleep(delay).await;
                tx.send(Message::Done(id, Box::new(()))).ok();
            }
            .boxed()
        })
    }

    fn batches_any(
        &mut self,
        mut items: BoxStream<'static, AnyBox>,
        pace: Pace,
        on_batch: BatchCallback<S>,
    ) -> TaskHandle {
        self.start(Pending::Batches(on_batch), move |id, tx| {
            async move {
                let mut index = 0;
                while let Some(first) = items.next().await {
                    let delay = pace.delay_before(index);
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    index += 1;
                    let mut batch = vec![first];
                    let mut ended = false;
                    loop {
                        match items.next().now_or_never() {
                            Some(Some(item)) => batch.push(item),
                            Some(None) => {
                                ended = true;
                                break;
                            }
                            None => break,
                        }
                    }
                    if tx.send(Message::Batch(id, batch)).is_err() || ended {
                        break;
                    }
                    if !pace.gap.is_zero() {
                        tokio::time::sleep(pace.gap).await;
                    }
                }
                tx.send(Message::Ended(id)).ok();
            }
            .boxed()
        })
    }

    fn every_any(&mut self, interval: Duration, tick: TickCallback<S>) -> TaskHandle {
        self.start(Pending::Ticks(tick), move |id, tx| {
            async move {
                loop {
                    tokio::time::sleep(interval).await;
                    if tx.send(Message::Tick(id)).is_err() {
                        break;
                    }
                }
            }
            .boxed()
        })
    }

    fn emit(&mut self, event: S::Event) {
        self.events.push(event);
    }

    fn effect(&mut self, effect: S::Effect) {
        self.effects.push(effect);
    }

    fn notify(&mut self) {
        self.notified += 1;
    }

    fn toast(&mut self, notice: Notice) {
        self.notices.push(notice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::HostExt as _;

    #[derive(Default)]
    struct Counter {
        total: u32,
        running: Option<TaskHandle>,
    }

    impl Service for Counter {
        type Event = u32;
        type Effect = ();
    }

    impl Counter {
        fn add_later(&mut self, n: u32, host: &mut dyn Host<Self>) {
            self.running = Some(host.spawn(async move { n * 2 }, |this, doubled, host| {
                this.total += doubled;
                host.emit(this.total);
            }));
        }
    }

    #[test]
    fn spawned_work_calls_back_with_the_service() {
        let mut host = TestHost::new();
        let mut counter = Counter::default();
        counter.add_later(3, &mut host);
        host.run_until_idle(&mut counter);
        assert_eq!(counter.total, 6);
        assert_eq!(host.events, [6]);
    }

    #[test]
    fn dropping_the_handle_aborts_the_callback() {
        let mut host = TestHost::new();
        let mut counter = Counter::default();
        counter.add_later(3, &mut host);
        counter.running = None;
        host.run_until_idle(&mut counter);
        assert_eq!(counter.total, 0);
    }

    #[test]
    fn batches_collect_what_arrived_in_the_window() {
        let mut host: TestHost<Counter> = TestHost::new();
        let mut counter = Counter::default();
        let (tx, rx) = futures::channel::mpsc::unbounded::<u32>();
        for n in [1, 2, 3] {
            tx.unbounded_send(n).unwrap();
        }
        drop(tx);
        let pace = Pace::debounce(Duration::from_millis(10));
        let handle = host.batches(rx, pace, |this, batch, host| {
            this.total += batch.iter().sum::<u32>();
            host.emit(batch.len() as u32);
            Flow::Continue
        });
        host.run_until_idle(&mut counter);
        drop(handle);
        assert_eq!(counter.total, 6);
        assert_eq!(host.events, [3]);
    }

    #[test]
    fn ticks_repeat_until_stopped() {
        let mut host: TestHost<Counter> = TestHost::new();
        let mut counter = Counter::default();
        let ticks = host.every(Duration::from_millis(5), |this, _| {
            this.total += 1;
            if this.total == 3 {
                Flow::Stop
            } else {
                Flow::Continue
            }
        });
        host.run_until_idle(&mut counter);
        drop(ticks);
        assert_eq!(counter.total, 3);
    }

    #[test]
    fn timers_fire_after_their_delay() {
        let mut host: TestHost<Counter> = TestHost::new();
        let mut counter = Counter::default();
        let started = std::time::Instant::now();
        host.after(Duration::from_millis(20), |this, _| this.total = 1)
            .detach();
        host.run_until_idle(&mut counter);
        assert_eq!(counter.total, 1);
        assert!(started.elapsed() >= Duration::from_millis(20));
    }
}
