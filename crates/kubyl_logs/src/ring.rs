//! A fixed-capacity ring buffer of [`LogLine`]s. Default capacity 100k lines (configurable via
//! `LogsSettings::ring_buffer_lines`).

use std::collections::VecDeque;

use crate::line::LogLine;

/// Default ring buffer capacity: 100k lines.
pub const DEFAULT_CAPACITY: usize = 100_000;

/// Holds up to `capacity` lines. Pushing past capacity evicts the oldest line. Tracks how many
/// lines were evicted so the UI can show "N lines dropped".
pub struct LogRingBuffer {
    capacity: usize,
    lines: VecDeque<LogLine>,
    next_seq: u64,
    evicted: u64,
}

impl LogRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            lines: VecDeque::with_capacity(capacity.min(4096)),
            next_seq: 0,
            evicted: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Changes the capacity, evicting from the front if the buffer is now over it.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        while self.lines.len() > self.capacity {
            self.lines.pop_front();
            self.evicted += 1;
        }
    }

    /// Appends a line built by `make` with the next sequence number. Returns the evicted line,
    /// if pushing past capacity evicted one (so callers can maintain incremental derived state,
    /// e.g. per-level counts, without rescanning the whole ring).
    pub fn push_with(&mut self, make: impl FnOnce(u64) -> LogLine) -> (u64, Option<LogLine>) {
        let seq = self.next_seq;
        self.next_seq += 1;
        let evicted = self.push_line(make(seq));
        (seq, evicted)
    }

    /// Appends a raw line (no timestamp), assigning it the next sequence number.
    pub fn push(&mut self, pod: String, container: String, text: String) -> (u64, Option<LogLine>) {
        self.push_with(|seq| LogLine::new(seq, pod.into(), container.into(), None, text))
    }

    /// Appends a marker line (reconnect gap, pod joined or left).
    pub fn push_marker(
        &mut self,
        pod: String,
        container: String,
        message: String,
    ) -> (u64, Option<LogLine>) {
        self.push_with(|seq| LogLine::marker(seq, pod.into(), container.into(), message))
    }

    fn push_line(&mut self, line: LogLine) -> Option<LogLine> {
        let evicted = if self.lines.len() >= self.capacity {
            self.evicted += 1;
            self.lines.pop_front()
        } else {
            None
        };
        self.lines.push_back(line);
        evicted
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn evicted(&self) -> u64 {
        self.evicted
    }

    pub fn iter(&self) -> impl Iterator<Item = &LogLine> {
        self.lines.iter()
    }

    pub fn get(&self, index: usize) -> Option<&LogLine> {
        self.lines.get(index)
    }

    /// Looks up a line by its stable `seq`, in O(1) (lines are stored in increasing-seq order,
    /// so the front line's seq gives the offset of every other line).
    pub fn get_by_seq(&self, seq: u64) -> Option<&LogLine> {
        let front_seq = self.lines.front()?.seq;
        let index = seq.checked_sub(front_seq)?;
        self.lines
            .get(index as usize)
            .filter(|line| line.seq == seq)
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.evicted = 0;
    }
}

impl Default for LogRingBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_oldest_past_capacity() {
        let mut ring = LogRingBuffer::new(3);
        for i in 0..5 {
            ring.push("pod".into(), "c".into(), format!("line {i}"));
        }
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.evicted(), 2);
        let texts: Vec<_> = ring.iter().map(|l| l.text.clone()).collect();
        assert_eq!(texts, ["line 2", "line 3", "line 4"]);
    }

    #[test]
    fn push_reports_the_evicted_line_once_past_capacity() {
        let mut ring = LogRingBuffer::new(2);
        let (_, evicted) = ring.push("pod".into(), "c".into(), "a".into());
        assert!(evicted.is_none());
        let (_, evicted) = ring.push("pod".into(), "c".into(), "b".into());
        assert!(evicted.is_none());
        let (_, evicted) = ring.push("pod".into(), "c".into(), "c".into());
        assert_eq!(evicted.map(|l| l.text), Some("a".to_string()));
    }

    #[test]
    fn get_by_seq_finds_lines_and_rejects_evicted_ones() {
        let mut ring = LogRingBuffer::new(2);
        for i in 0..4 {
            ring.push("pod".into(), "c".into(), format!("line {i}"));
        }
        assert!(ring.get_by_seq(0).is_none());
        assert!(ring.get_by_seq(1).is_none());
        assert_eq!(
            ring.get_by_seq(2).map(|l| l.text.clone()),
            Some("line 2".to_string())
        );
        assert_eq!(
            ring.get_by_seq(3).map(|l| l.text.clone()),
            Some("line 3".to_string())
        );
        assert!(ring.get_by_seq(4).is_none());
    }

    #[test]
    fn sequence_numbers_are_monotonic_across_eviction() {
        let mut ring = LogRingBuffer::new(2);
        for i in 0..4 {
            ring.push("pod".into(), "c".into(), format!("line {i}"));
        }
        let seqs: Vec<_> = ring.iter().map(|l| l.seq).collect();
        assert_eq!(seqs, [2, 3]);
    }

    #[test]
    fn shrinking_capacity_evicts_from_the_front() {
        let mut ring = LogRingBuffer::new(10);
        for i in 0..5 {
            ring.push("pod".into(), "c".into(), format!("line {i}"));
        }
        ring.set_capacity(2);
        assert_eq!(ring.len(), 2);
        let texts: Vec<_> = ring.iter().map(|l| l.text.clone()).collect();
        assert_eq!(texts, ["line 3", "line 4"]);
    }
}
