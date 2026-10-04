//! Runs `kubyl_base` core services inside GPUI entities.
//!
//! An entity that holds a core service implements [`Hosts`] and calls the service's mutating
//! methods through [`hosted`]:
//!
//! ```ignore
//! impl Hosts<ManagerCore> for ConnectionManager {
//!     fn service(&mut self) -> &mut ManagerCore { &mut self.core }
//!     fn apply(&mut self, effect: KubeEffect, cx: &mut Context<Self>) { /* State, Settings… */ }
//! }
//!
//! pub fn connect(&mut self, id: &ClusterId, cx: &mut Context<Self>) {
//!     hosted(self, cx, |core, host| core.connect(id, host));
//! }
//! ```
//!
//! Spawned work runs on the Tokio runtime ([`spawn_kube`](crate::spawn_kube)) or GPUI's
//! background executor, timers on GPUI's (so tests can advance the clock), and callbacks on the
//! UI thread through `Entity::update`, exactly where `cx.spawn` callbacks ran before. Events,
//! effects, re-renders and toasts are replayed in the order the service asked for them once it
//! returns.

use std::marker::PhantomData;
use std::time::Duration;

use futures::FutureExt as _;
use futures::StreamExt as _;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use gpui::{AsyncApp, Context, EventEmitter, Task, WeakEntity};
use kubyl_base::host::{
    AnyBox, AnyCallback, BatchCallback, Callback, Detach, Executor, Flow, Host, Pace, Service,
    TaskHandle, TickCallback,
};
use kubyl_base::notice::Notice;

use crate::notify::NotificationCenter;
use crate::runtime::spawn_kube;

/// An entity that holds a core service `S`.
pub trait Hosts<S: Service>: EventEmitter<S::Event> + Sized + 'static {
    fn service(&mut self) -> &mut S;

    /// Carries out an effect the service asked for.
    fn apply(&mut self, effect: S::Effect, cx: &mut Context<Self>);
}

/// Calls `f` with the entity's service and a host, then replays what the service asked for.
pub fn hosted<E: Hosts<S>, S: Service, R>(
    this: &mut E,
    cx: &mut Context<E>,
    f: impl FnOnce(&mut S, &mut dyn Host<S>) -> R,
) -> R {
    let mut host = GpuiHost {
        cx,
        outputs: Vec::new(),
        _entity: PhantomData,
    };
    let result = f(this.service(), &mut host);
    let outputs = std::mem::take(&mut host.outputs);
    for output in outputs {
        match output {
            Output::Event(event) => cx.emit(event),
            Output::Effect(effect) => this.apply(effect, cx),
            Output::Notify => cx.notify(),
            Output::Toast(notice) => NotificationCenter::push(cx, notice.into()),
        }
    }
    result
}

enum Output<S: Service> {
    Event(S::Event),
    Effect(S::Effect),
    Notify,
    Toast(Notice),
}

/// The [`Host`] that [`hosted`] passes to a service.
pub struct GpuiHost<'a, 'b, E: Hosts<S>, S: Service> {
    cx: &'a mut Context<'b, E>,
    outputs: Vec<Output<S>>,
    _entity: PhantomData<E>,
}

struct GpuiTask(Task<()>);

impl Detach for GpuiTask {
    fn detach(self: Box<Self>) {
        self.0.detach();
    }
}

/// Runs `then` in the entity behind `this`, unless it was released.
fn call_back<E: Hosts<S>, S: Service, R>(
    this: &WeakEntity<E>,
    cx: &mut AsyncApp,
    then: impl FnOnce(&mut S, &mut dyn Host<S>) -> R,
) -> Option<R> {
    this.update(cx, |this, cx| hosted(this, cx, then)).ok()
}

impl<E: Hosts<S>, S: Service> Host<S> for GpuiHost<'_, '_, E, S> {
    fn spawn_any(
        &mut self,
        executor: Executor,
        work: BoxFuture<'static, AnyBox>,
        then: AnyCallback<S>,
    ) -> TaskHandle {
        let work: Task<AnyBox> = match executor {
            Executor::Kube => spawn_kube(self.cx, work),
            Executor::Background => self.cx.background_executor().spawn(work),
        };
        let task = self
            .cx
            .spawn(async move |this: WeakEntity<E>, cx: &mut AsyncApp| {
                let output = work.await;
                call_back(&this, cx, move |service, host| then(service, output, host));
            });
        TaskHandle::new(GpuiTask(task))
    }

    fn after_any(&mut self, delay: Duration, then: Callback<S>) -> TaskHandle {
        let task = self
            .cx
            .spawn(async move |this: WeakEntity<E>, cx: &mut AsyncApp| {
                cx.background_executor().timer(delay).await;
                call_back(&this, cx, then);
            });
        TaskHandle::new(GpuiTask(task))
    }

    fn batches_any(
        &mut self,
        mut items: BoxStream<'static, AnyBox>,
        pace: Pace,
        mut on_batch: BatchCallback<S>,
    ) -> TaskHandle {
        let task = self
            .cx
            .spawn(async move |this: WeakEntity<E>, cx: &mut AsyncApp| {
                let mut index = 0;
                while let Some(first) = items.next().await {
                    let delay = pace.delay_before(index);
                    if !delay.is_zero() {
                        cx.background_executor().timer(delay).await;
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
                    let flow = call_back(&this, cx, |service, host| on_batch(service, batch, host));
                    if flow != Some(Flow::Continue) || ended {
                        break;
                    }
                    if !pace.gap.is_zero() {
                        cx.background_executor().timer(pace.gap).await;
                    }
                }
            });
        TaskHandle::new(GpuiTask(task))
    }

    fn every_any(&mut self, interval: Duration, mut tick: TickCallback<S>) -> TaskHandle {
        let task = self
            .cx
            .spawn(async move |this: WeakEntity<E>, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor().timer(interval).await;
                    if call_back(&this, cx, |service, host| tick(service, host))
                        != Some(Flow::Continue)
                    {
                        break;
                    }
                }
            });
        TaskHandle::new(GpuiTask(task))
    }

    fn emit(&mut self, event: S::Event) {
        self.outputs.push(Output::Event(event));
    }

    fn effect(&mut self, effect: S::Effect) {
        self.outputs.push(Output::Effect(effect));
    }

    fn notify(&mut self) {
        self.outputs.push(Output::Notify);
    }

    fn toast(&mut self, notice: Notice) {
        self.outputs.push(Output::Toast(notice));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, Entity, TestAppContext};
    use kubyl_base::host::HostExt as _;

    #[derive(Default)]
    struct Counter {
        total: u32,
        running: Vec<TaskHandle>,
    }

    impl Service for Counter {
        type Event = u32;
        type Effect = &'static str;
    }

    impl Counter {
        fn start(&mut self, host: &mut dyn Host<Self>) {
            host.effect("started");
            host.emit(0);
            self.running
                .push(host.after(Duration::from_secs(1), |this, host| {
                    this.total += 1;
                    host.emit(this.total);
                    host.effect("tick");
                }));
            self.running
                .push(host.background(async { 41 }, |this, n, host| {
                    this.total += n;
                    host.notify();
                }));
        }
    }

    struct Holder {
        core: Counter,
        effects: Vec<&'static str>,
    }

    impl EventEmitter<u32> for Holder {}

    impl Hosts<Counter> for Holder {
        fn service(&mut self) -> &mut Counter {
            &mut self.core
        }

        fn apply(&mut self, effect: &'static str, _cx: &mut Context<Self>) {
            self.effects.push(effect);
        }
    }

    fn holder(
        cx: &mut TestAppContext,
    ) -> (Entity<Holder>, std::rc::Rc<std::cell::RefCell<Vec<u32>>>) {
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let holder = cx.new(|_| Holder {
            core: Counter::default(),
            effects: Vec::new(),
        });
        let seen = events.clone();
        cx.update(|cx| {
            cx.subscribe(&holder, move |_, event: &u32, _| {
                seen.borrow_mut().push(*event)
            })
            .detach()
        });
        (holder, events)
    }

    #[gpui::test]
    fn callbacks_run_in_the_entity_and_replay_outputs(cx: &mut TestAppContext) {
        let (holder, events) = holder(cx);
        holder.update(cx, |this, cx| {
            hosted(this, cx, |core, host| core.start(host))
        });
        cx.run_until_parked();
        holder.read_with(cx, |this, _| {
            assert_eq!(this.core.total, 41);
            assert_eq!(this.effects, ["started"]);
        });
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        holder.read_with(cx, |this, _| {
            assert_eq!(this.core.total, 42);
            assert_eq!(this.effects, ["started", "tick"]);
        });
        assert_eq!(*events.borrow(), [0, 42]);
    }

    #[gpui::test]
    fn dropped_handles_cancel_their_callbacks(cx: &mut TestAppContext) {
        let (holder, events) = holder(cx);
        holder.update(cx, |this, cx| {
            hosted(this, cx, |core, host| core.start(host));
            this.core.running.clear();
        });
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        holder.read_with(cx, |this, _| assert_eq!(this.core.total, 0));
        assert_eq!(*events.borrow(), [0]);
    }

    #[gpui::test]
    fn batches_arrive_together(cx: &mut TestAppContext) {
        let (holder, _) = holder(cx);
        let (tx, rx) = futures::channel::mpsc::unbounded::<u32>();
        let handle = holder.update(cx, |this, cx| {
            hosted(this, cx, |_, host| {
                host.batches(
                    rx,
                    Pace::debounce(Duration::from_millis(100)),
                    |this, batch, host| {
                        this.total += batch.len() as u32;
                        host.emit(this.total);
                        Flow::Continue
                    },
                )
            })
        });
        for n in 0..3 {
            tx.unbounded_send(n).unwrap();
        }
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        holder.read_with(cx, |this, _| assert_eq!(this.core.total, 3));
        drop(handle);
    }
}
