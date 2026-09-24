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

    /// Appends a raw line, assigning it the next sequence number.
    pub fn push(&mut self, pod: String, container: String, text: String) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.push_line(LogLine::new(seq, pod, container, text));
        seq
    }

    pub fn push_gap(&mut self, pod: String, container: String, message: String) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.push_line(LogLine::gap(seq, pod, container, message));
        seq
    }

    fn push_line(&mut self, line: LogLine) {
        if self.lines.len() >= self.capacity {
            self.lines.pop_front();
            self.evicted += 1;
        }
        self.lines.push_back(line);
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
