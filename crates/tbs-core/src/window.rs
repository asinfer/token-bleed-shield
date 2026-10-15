//! Sliding window + seen-shingle set (docs/architecture/02 §2.3).
//!
//! Holds only the last N bytes of decoded text (default 32 KiB) and a bounded
//! hash set of shingles; both are memory-only and dropped at stream end.

use std::collections::HashSet;

use crate::hash::shingle_hashes;

pub const DEFAULT_WINDOW_BYTES: usize = 32 * 1024;
pub const DEFAULT_SHINGLE_CAP: usize = 65_536;

pub struct SlidingWindow {
    buf: String,
    cap: usize,
}

impl SlidingWindow {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: String::new(),
            cap,
        }
    }

    pub fn push(&mut self, text: &str) {
        self.buf.push_str(text);
        if self.buf.len() > self.cap {
            let mut cut = self.buf.len() - self.cap;
            while !self.buf.is_char_boundary(cut) {
                cut += 1;
            }
            self.buf.drain(..cut);
        }
    }

    pub fn text(&self) -> &str {
        &self.buf
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

pub struct SeenShingles {
    set: HashSet<u64>,
    cap: usize,
}

impl SeenShingles {
    pub fn new(cap: usize) -> Self {
        Self {
            set: HashSet::new(),
            cap,
        }
    }

    /// Observe a batch of shingle hashes, returning (total, repeats) for the batch.
    pub fn observe(&mut self, shingles: &[u64]) -> (u64, u64) {
        let mut repeats = 0u64;
        for s in shingles {
            if self.set.contains(s) {
                repeats += 1;
            } else if self.set.len() < self.cap {
                self.set.insert(*s);
            }
        }
        (shingles.len() as u64, repeats)
    }
}

/// (total, repeats) over 8-gram shingles of `text`, fed through a fresh set.
pub fn repetition_stats(text: &str) -> (u64, u64) {
    let shingles = shingle_hashes(text, 8);
    let mut set = SeenShingles::new(DEFAULT_SHINGLE_CAP);
    set.observe(&shingles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_evicts_oldest_bytes() {
        let mut w = SlidingWindow::new(16);
        w.push("0123456789abcdef");
        w.push("XYZ");
        assert!(w.text().ends_with("XYZ"));
        assert!(w.len() <= 16);
        assert!(!w.text().contains("01"));
    }

    #[test]
    fn window_never_splits_utf8() {
        let mut w = SlidingWindow::new(5);
        w.push("ééééé");
        assert!(w.text().chars().count() < 5);
    }

    #[test]
    fn repetition_detects_loops() {
        let looped = "please continue checking the file please continue checking the file \
                      please continue checking the file please continue checking the file";
        let (total, repeats) = repetition_stats(looped);
        assert!(total > 0);
        assert!(repeats as f64 / total as f64 > 0.5);
    }

    #[test]
    fn repetition_ignores_distinct_text() {
        let text = "the quick brown fox jumps over the lazy dog while a second fox \
                    watches from behind the fence and a third fox sleeps under the tree";
        let (total, repeats) = repetition_stats(text);
        assert!(total > 0);
        assert_eq!(repeats, 0);
    }

    #[test]
    fn shingle_set_is_bounded() {
        let mut set = SeenShingles::new(4);
        let (_, reps) = set.observe(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(reps, 0);
        // Capacity reached: unseen 6,7 were dropped, seen 1,2 still hit.
        let (_, reps) = set.observe(&[6, 7]);
        assert_eq!(reps, 0);
        let (_, reps) = set.observe(&[1, 2]);
        assert_eq!(reps, 2);
    }
}
