//! The table's index over a stream's buffer (README "Flow buffer and streaming"): each batch
//! filters only the new flows; a new filter re-filters the buffer in the background ([`scan`]).
//! Rows are sequence numbers, newest first, so a selection survives while old flows fall out.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use jiff::Timestamp;

use crate::buffer::{FlowBuffer, Snapshot};
use crate::filter::{FlowFilter, Op};
use crate::model::Verdict;

/// A filter split for the verdict chips: everything but the verdict terms, and those terms.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SplitFilter {
    pub base: FlowFilter,
    pub verdicts: FlowFilter,
}

impl SplitFilter {
    pub fn new(filter: &FlowFilter) -> Self {
        let (base, verdicts) = filter.split_verdicts();
        Self { base, verdicts }
    }

    /// Whether a flow with `verdict` passes the verdict terms.
    pub fn verdict_ok(&self, verdict: Verdict) -> bool {
        self.verdicts.terms.iter().all(|term| {
            let hit = term.verdicts().is_some_and(|v| v.contains(&verdict));
            if term.op == Op::Ne { !hit } else { hit }
        })
    }
}

/// The oldest time a row may have.
pub fn cutoff(now: Timestamp, window: Duration) -> Timestamp {
    now.checked_sub(jiff::SignedDuration::try_from(window).unwrap_or_default())
        .unwrap_or(Timestamp::UNIX_EPOCH)
}

/// A background scan's result.
pub struct Scan {
    base: Vec<(u64, Verdict, Timestamp)>,
    end: u64,
}

/// Filters a snapshot (off the UI thread).
pub fn scan(snapshot: &Snapshot, filter: &SplitFilter, cutoff: Timestamp) -> Scan {
    Scan {
        base: snapshot
            .iter()
            .filter(|(_, f)| f.time >= cutoff && filter.base.matches(f))
            .map(|(seq, f)| (seq, f.verdict, f.time))
            .collect(),
        end: snapshot.end(),
    }
}

#[derive(Default)]
pub struct Rows {
    filter: SplitFilter,
    /// Flows passing everything but the verdict terms, oldest first.
    base: VecDeque<(u64, Verdict, Timestamp)>,
    counts: HashMap<Verdict, usize>,
    /// Shown rows, newest first.
    pub shown: VecDeque<u64>,
    /// Matching flows that arrived while paused or scrolled away, newest first.
    pub pending: VecDeque<u64>,
    /// The next sequence number to look at.
    next: u64,
}

impl Rows {
    pub fn from_scan(filter: SplitFilter, scan: Scan) -> Self {
        let mut rows = Rows {
            filter,
            next: scan.end,
            ..Rows::default()
        };
        for (seq, verdict, time) in scan.base {
            *rows.counts.entry(verdict).or_default() += 1;
            if rows.filter.verdict_ok(verdict) {
                rows.shown.push_front(seq);
            }
            rows.base.push_back((seq, verdict, time));
        }
        rows
    }

    pub fn filter(&self) -> &SplitFilter {
        &self.filter
    }

    /// Only the verdict terms changed: no rescan needed.
    pub fn set_verdicts(&mut self, filter: SplitFilter) {
        self.filter = filter;
        self.shown = self
            .base
            .iter()
            .rev()
            .filter(|(_, v, _)| self.filter.verdict_ok(*v))
            .map(|(seq, _, _)| *seq)
            .collect();
        self.pending.clear();
    }

    /// Takes new flows and drops old ones. `follow`: new rows show at once, else they wait in
    /// `pending`. Returns how many rows were added.
    pub fn update(
        &mut self,
        buffer: &FlowBuffer,
        now: Timestamp,
        window: Duration,
        follow: bool,
    ) -> usize {
        let start = buffer.range().start;
        let oldest = cutoff(now, window);
        // Gone from the buffer, or out of the window.
        while self
            .base
            .front()
            .is_some_and(|(seq, _, time)| *seq < start || *time < oldest)
        {
            if let Some((_, verdict, _)) = self.base.pop_front()
                && let Some(count) = self.counts.get_mut(&verdict)
            {
                *count = count.saturating_sub(1);
            }
        }
        let first_kept = self.base.front().map_or(u64::MAX, |(seq, _, _)| *seq);
        for list in [&mut self.shown, &mut self.pending] {
            while list.back().is_some_and(|seq| *seq < first_kept) {
                list.pop_back();
            }
        }
        let mut added = 0;
        for (seq, flow) in buffer.since(self.next.max(start)) {
            if flow.time < oldest || !self.filter.base.matches(flow) {
                continue;
            }
            *self.counts.entry(flow.verdict).or_default() += 1;
            self.base.push_back((seq, flow.verdict, flow.time));
            if self.filter.verdict_ok(flow.verdict) {
                if follow {
                    self.shown.push_front(seq);
                } else {
                    self.pending.push_front(seq);
                }
                added += 1;
            }
        }
        self.next = buffer.range().end;
        added
    }

    /// Shows the rows that waited.
    pub fn resume(&mut self) {
        while let Some(seq) = self.pending.pop_back() {
            self.shown.push_front(seq);
        }
    }

    /// Flows passing everything but the verdict terms, with `verdict`.
    pub fn count(&self, verdict: Verdict) -> usize {
        self.counts.get(&verdict).copied().unwrap_or(0)
    }

    /// Flows passing everything but the verdict terms.
    pub fn total(&self) -> usize {
        self.base.len()
    }

    /// Every matching row, newest first (shown and pending).
    pub fn all(&self) -> impl Iterator<Item = u64> + '_ {
        self.pending.iter().chain(self.shown.iter()).copied()
    }

    /// The position of `seq` among the shown rows (newest first: descending).
    pub fn position(&self, seq: u64) -> Option<usize> {
        self.shown.binary_search_by(|probe| seq.cmp(probe)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Endpoint, EndpointKind, Flow};

    fn flow(second: i64, ns: &str, verdict: Verdict) -> Flow {
        let mut flow = Flow::new(Timestamp::from_second(second).unwrap());
        flow.source = Endpoint {
            kind: EndpointKind::Pod,
            namespace: Some(ns.into()),
            ..Endpoint::default()
        };
        flow.verdict = verdict;
        flow
    }

    fn split(text: &str) -> SplitFilter {
        SplitFilter::new(&FlowFilter::parse(text).unwrap())
    }

    const WINDOW: Duration = Duration::from_secs(3_600);

    #[test]
    fn incremental_rows_follow_the_buffer() {
        let mut buffer = FlowBuffer::new(100, Duration::from_secs(3_600));
        buffer.push(vec![
            flow(1_000, "a", Verdict::Forwarded),
            flow(1_001, "b", Verdict::Dropped),
            flow(1_002, "a", Verdict::Dropped),
        ]);
        let now = Timestamp::from_second(1_010).unwrap();
        let filter = split("ns=a verdict=dropped");
        let scanned = scan(&buffer.snapshot(), &filter, cutoff(now, WINDOW));
        let mut rows = Rows::from_scan(filter, scanned);
        assert_eq!(rows.shown, [2]);
        assert_eq!(
            (
                rows.total(),
                rows.count(Verdict::Forwarded),
                rows.count(Verdict::Dropped)
            ),
            (2, 1, 1)
        );
        // New flows: only the new ones are looked at; newest first.
        buffer.push(vec![
            flow(1_003, "a", Verdict::Dropped),
            flow(1_004, "a", Verdict::Forwarded),
        ]);
        assert_eq!(rows.update(&buffer, now, WINDOW, true), 1);
        assert_eq!(rows.shown, [3, 2]);
        assert_eq!(rows.position(2), Some(1));
        // While scrolled away they wait.
        buffer.push(vec![flow(1_005, "a", Verdict::Dropped)]);
        rows.update(&buffer, now, WINDOW, false);
        assert_eq!((rows.shown.len(), rows.pending.len()), (2, 1));
        rows.resume();
        assert_eq!(rows.shown, [5, 3, 2]);
        // The verdict chips switch without a rescan.
        rows.set_verdicts(split("ns=a verdict=forwarded"));
        assert_eq!(rows.shown, [4, 0]);
        assert_eq!(rows.count(Verdict::Dropped), 3);
    }

    #[test]
    fn old_rows_fall_out() {
        let mut buffer = FlowBuffer::new(100, Duration::from_secs(3_600));
        buffer.push(
            (0..150)
                .map(|i| flow(1_000 + i, "a", Verdict::Forwarded))
                .collect(),
        );
        let now = Timestamp::from_second(1_150).unwrap();
        let filter = split("");
        let mut rows = Rows::from_scan(
            filter.clone(),
            scan(&buffer.snapshot(), &filter, cutoff(now, WINDOW)),
        );
        // The buffer holds the last 100.
        assert_eq!(rows.shown.len(), 100);
        assert_eq!(rows.shown.back(), Some(&50));
        // Ten more push ten out.
        buffer.push(
            (0..10)
                .map(|i| flow(1_150 + i, "a", Verdict::Forwarded))
                .collect(),
        );
        rows.update(&buffer, now, WINDOW, true);
        assert_eq!(rows.shown.len(), 100);
        assert_eq!(
            (rows.shown.front(), rows.shown.back()),
            (Some(&159), Some(&60))
        );
        // A 60 s window at 1,200 keeps flows from 1,140 on.
        rows.update(
            &buffer,
            Timestamp::from_second(1_200).unwrap(),
            Duration::from_secs(60),
            true,
        );
        assert_eq!(rows.shown.len(), 20);
        assert_eq!(rows.total(), 20);
    }

    /// The acceptance bar: 5,000+ rows under a live stream stay cheap per batch.
    #[test]
    fn twenty_thousand_flows_filter_quickly() {
        let mut buffer = FlowBuffer::new(20_000, Duration::from_secs(3_600));
        let verdict = |i: i64| {
            if i % 7 == 0 {
                Verdict::Dropped
            } else {
                Verdict::Forwarded
            }
        };
        buffer.push(
            (0..20_000)
                .map(|i| {
                    flow(
                        10_000 + i / 10,
                        if i % 3 == 0 { "a" } else { "b" },
                        verdict(i),
                    )
                })
                .collect(),
        );
        let now = Timestamp::from_second(12_000).unwrap();
        let filter = split("ns=a");
        let started = std::time::Instant::now();
        let mut rows = Rows::from_scan(
            filter.clone(),
            scan(&buffer.snapshot(), &filter, cutoff(now, WINDOW)),
        );
        let full = started.elapsed();
        assert!(rows.shown.len() > 6_000);
        let started = std::time::Instant::now();
        for batch in 0..60 {
            buffer.push(
                (0..10)
                    .map(|i| flow(12_000 + batch, "a", verdict(i)))
                    .collect(),
            );
            rows.update(&buffer, now, WINDOW, true);
        }
        let per_batch = started.elapsed() / 60;
        assert!(full < std::time::Duration::from_secs(2), "{full:?}");
        assert!(
            per_batch < std::time::Duration::from_millis(8),
            "{per_batch:?}"
        );
    }
}
