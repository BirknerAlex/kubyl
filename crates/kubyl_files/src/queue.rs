//! The transfer queue: runs [`crate::transfer`] jobs with a per-pod concurrency limit, tracks
//! progress, speed and ETA, and keeps finished transfers for the History tab. Cancelling drops
//! a job's task (aborting its exec streams); retrying a chunked download resumes it.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use futures::channel::mpsc;
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, Task};

use crate::settings::FilesSettings;
use crate::transfer::{self, Direction, TransferJob, Verification};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferState {
    Queued,
    Running,
    Done(Verification),
    Failed(String),
    Cancelled,
}

impl TransferState {
    pub fn is_finished(&self) -> bool {
        !matches!(self, TransferState::Queued | TransferState::Running)
    }
}

pub struct Transfer {
    pub id: u64,
    pub job: TransferJob,
    pub state: TransferState,
    pub done_bytes: u64,
    pub total_bytes: u64,
    /// Bytes per second (smoothed).
    pub speed: f64,
    pub started: Option<Instant>,
    pub finished: Option<jiff::Timestamp>,
    /// Hidden from the Transfers tab (still in History).
    pub dismissed: bool,
    last_sample: Option<(Instant, u64)>,
    task: Option<Task<()>>,
}

impl Transfer {
    /// Transfers of the same pod share its concurrency limit.
    fn pod_key(&self) -> (kubyl_core::ClusterId, String, String) {
        (
            self.job.target.cluster.clone(),
            self.job.target.namespace.clone(),
            self.job.target.pod.clone(),
        )
    }

    pub fn percent(&self) -> Option<f32> {
        (self.total_bytes > 0)
            .then(|| (self.done_bytes as f32 / self.total_bytes as f32 * 100.0).clamp(0.0, 100.0))
    }

    /// Seconds left at the current speed.
    pub fn eta(&self) -> Option<Duration> {
        if self.speed <= 0.0 || self.total_bytes == 0 || self.done_bytes >= self.total_bytes {
            return None;
        }
        Some(Duration::from_secs_f64(
            (self.total_bytes - self.done_bytes) as f64 / self.speed,
        ))
    }

    fn sample(&mut self, now: Instant) {
        let Some((at, bytes)) = self.last_sample else {
            self.last_sample = Some((now, self.done_bytes));
            return;
        };
        let elapsed = now.duration_since(at).as_secs_f64();
        if elapsed < 0.5 {
            return;
        }
        let rate = (self.done_bytes.saturating_sub(bytes)) as f64 / elapsed;
        self.speed = if self.speed == 0.0 {
            rate
        } else {
            self.speed * 0.6 + rate * 0.4
        };
        self.last_sample = Some((now, self.done_bytes));
    }
}

/// Emitted when a transfer finishes, so views refresh the destination.
#[derive(Clone, Debug)]
pub struct TransferFinished {
    pub id: u64,
    pub direction: Direction,
    /// Upload: the remote directory. Download: the local directory.
    pub remote_dir: String,
    pub local_dir: std::path::PathBuf,
    pub pod: String,
}

pub struct TransferQueue {
    transfers: Vec<Transfer>,
    next_id: u64,
}

impl EventEmitter<TransferFinished> for TransferQueue {}

struct GlobalQueue(Entity<TransferQueue>);

impl Global for GlobalQueue {}

/// Finished transfers kept for History.
const HISTORY: usize = 200;

impl TransferQueue {
    pub fn install(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            transfers: Vec::new(),
            next_id: 0,
        });
        cx.set_global(GlobalQueue(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalQueue>().0.clone()
    }

    pub fn transfers(&self) -> &[Transfer] {
        &self.transfers
    }

    pub fn active_count(&self) -> usize {
        self.transfers
            .iter()
            .filter(|t| !t.state.is_finished())
            .count()
    }

    /// Adds a job and starts it when its pod has a free slot.
    pub fn enqueue(&mut self, job: TransferJob, cx: &mut Context<Self>) -> u64 {
        let id = self.push(job, TransferState::Queued);
        self.pump(cx);
        cx.notify();
        id
    }

    /// Records a transfer that was refused before it started (read-only mount…).
    pub fn reject(&mut self, job: TransferJob, reason: String, cx: &mut Context<Self>) {
        let id = self.push(job, TransferState::Failed(reason));
        if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
            t.finished = Some(jiff::Timestamp::now());
        }
        cx.notify();
    }

    fn push(&mut self, job: TransferJob, state: TransferState) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let total = job.size;
        self.transfers.push(Transfer {
            id,
            job,
            state,
            done_bytes: 0,
            total_bytes: total,
            speed: 0.0,
            started: None,
            finished: None,
            dismissed: false,
            last_sample: None,
            task: None,
        });
        let finished = self
            .transfers
            .iter()
            .filter(|t| t.state.is_finished())
            .count();
        if finished > HISTORY
            && let Some(ix) = self.transfers.iter().position(|t| t.state.is_finished())
        {
            self.transfers.remove(ix);
        }
        id
    }

    pub fn cancel(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id)
            && !t.state.is_finished()
        {
            t.task = None;
            t.state = TransferState::Cancelled;
            t.finished = Some(jiff::Timestamp::now());
        }
        self.pump(cx);
        cx.notify();
    }

    /// Starts a failed or cancelled transfer again (chunked downloads resume).
    pub fn retry(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id)
            && matches!(t.state, TransferState::Failed(_) | TransferState::Cancelled)
        {
            t.state = TransferState::Queued;
            t.done_bytes = 0;
            t.speed = 0.0;
            t.finished = None;
            t.dismissed = false;
            t.last_sample = None;
        }
        self.pump(cx);
        cx.notify();
    }

    /// Hides a finished transfer from the Transfers tab.
    pub fn dismiss(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
            t.dismissed = true;
        }
        cx.notify();
    }

    pub fn clear_history(&mut self, cx: &mut Context<Self>) {
        self.transfers.retain(|t| !t.state.is_finished());
        cx.notify();
    }

    /// Starts queued transfers while their pods have free slots.
    fn pump(&mut self, cx: &mut Context<Self>) {
        let limit = kubyl_settings::Settings::get::<FilesSettings>(cx)
            .concurrency_per_pod
            .max(1);
        let queued: Vec<u64> = self
            .transfers
            .iter()
            .filter(|t| t.state == TransferState::Queued)
            .map(|t| t.id)
            .collect();
        for id in queued {
            let Some(pod) = self
                .transfers
                .iter()
                .find(|t| t.id == id)
                .map(Transfer::pod_key)
            else {
                continue;
            };
            let running = self
                .transfers
                .iter()
                .filter(|t| t.state == TransferState::Running && t.pod_key() == pod)
                .count();
            if running >= limit {
                continue;
            }
            self.start(id, cx);
        }
    }

    fn start(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(transfer) = self.transfers.iter_mut().find(|t| t.id == id) else {
            return;
        };
        transfer.state = TransferState::Running;
        transfer.started = Some(Instant::now());
        let job = transfer.job.clone();
        let (tx, mut rx) = mpsc::unbounded::<u64>();
        let run = kubyl_core::spawn_kube(cx, transfer::run(job, tx));
        transfer.task = Some(cx.spawn(async move |this, cx| {
            // Progress is drained ~8 times a second while the transfer runs; the channel
            // closes when the transfer returns.
            let progress = cx.spawn({
                let this = this.clone();
                async move |cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(125))
                            .await;
                        let mut delta = 0;
                        let mut closed = false;
                        loop {
                            match rx.try_recv() {
                                Ok(n) => delta += n,
                                Err(mpsc::TryRecvError::Closed) => {
                                    closed = true;
                                    break;
                                }
                                Err(mpsc::TryRecvError::Empty) => break,
                            }
                        }
                        if delta > 0
                            && this
                                .update(cx, |this, cx| this.progress(id, delta, cx))
                                .is_err()
                        {
                            return;
                        }
                        if closed {
                            return;
                        }
                    }
                }
            });
            let result = run.await;
            progress.await;
            this.update(cx, |this, cx| this.finish(id, result, cx)).ok();
        }));
    }

    fn progress(&mut self, id: u64, delta: u64, cx: &mut Context<Self>) {
        if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
            t.done_bytes += delta;
            if t.total_bytes > 0 && t.done_bytes > t.total_bytes {
                // `du` rounds; tar adds headers.
                t.total_bytes = t.done_bytes;
            }
            t.sample(Instant::now());
            cx.notify();
        }
    }

    fn finish(&mut self, id: u64, result: anyhow::Result<Verification>, cx: &mut Context<Self>) {
        let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) else {
            return;
        };
        t.finished = Some(jiff::Timestamp::now());
        match result {
            Ok(verification) => {
                t.state = TransferState::Done(verification);
                if t.total_bytes < t.done_bytes || t.total_bytes == 0 {
                    t.total_bytes = t.done_bytes;
                }
                t.done_bytes = t.total_bytes.max(t.done_bytes);
            }
            Err(err) => t.state = TransferState::Failed(format!("{err:#}")),
        }
        let event = TransferFinished {
            id,
            direction: t.job.direction,
            remote_dir: match t.job.direction {
                Direction::Upload => t.job.remote.clone(),
                Direction::Download => crate::entry::parent(&t.job.remote),
            },
            local_dir: match t.job.direction {
                Direction::Download => t.job.local.clone(),
                Direction::Upload => t
                    .job
                    .local
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default(),
            },
            pod: t.job.target.pod.clone(),
        };
        t.task = None;
        cx.emit(event);
        self.pump(cx);
        cx.notify();
    }
}

/// `38 MB/s`.
pub fn human_speed(bytes_per_second: f64) -> String {
    format!("{}/s", crate::entry::human_size(bytes_per_second as u64))
}

/// `4s`, `2m 10s`, `1h 5m`.
pub fn human_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
    }
}

/// History entries (finished transfers), newest first.
pub fn history(transfers: &[Transfer]) -> Vec<&Transfer> {
    let mut finished: VecDeque<&Transfer> = VecDeque::new();
    for t in transfers.iter().filter(|t| t.state.is_finished()) {
        finished.push_front(t);
    }
    finished.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_speed_and_duration() {
        assert_eq!(human_speed(38.0 * 1024.0 * 1024.0), "38.0 MB/s");
        assert_eq!(human_duration(Duration::from_secs(4)), "4s");
        assert_eq!(human_duration(Duration::from_secs(130)), "2m 10s");
        assert_eq!(human_duration(Duration::from_secs(3900)), "1h 5m");
    }
}
