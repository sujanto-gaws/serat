//! The bounded, path-deduplicating work queue.
//!
//! Architecture §3.1 step 2, and the place backpressure lives. A git checkout
//! touching 4,000 files enqueues 4,000 entries, not 4,000 parses — and if the
//! consumer cannot keep up, this is where that becomes visible instead of becoming
//! unbounded memory.

use std::collections::{HashSet, VecDeque};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use serat_core::VaultPath;

/// Default capacity, in distinct paths.
///
/// Sized so that ordinary bulk operations — a branch switch, a folder move, a sync
/// tool catching up — fit without overflowing, while a runaway producer is capped
/// well below the point where the queue itself is the memory problem. A path is a
/// short string; 8,192 of them is well under a megabyte.
pub const DEFAULT_CAPACITY: usize = 8_192;

/// What the queue has been doing. Diagnostics, and the input to any decision about
/// whether the pipeline is keeping up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueueStats {
    /// Paths currently waiting.
    pub depth: usize,
    /// Pushes that found the path already queued and did nothing. This is the number
    /// that justifies the design: on a bulk edit it should dwarf `accepted`.
    pub deduplicated: u64,
    /// Paths accepted onto the queue.
    pub accepted: u64,
    /// Pushes refused because the queue was full.
    pub overflowed: u64,
}

/// A bounded queue of paths waiting to be indexed, holding each path at most once.
///
/// # Why dropping on overflow is correct
///
/// When the queue is full a push is refused and [`WorkQueue::overflowed`] latches.
/// The consumer's contract is then to do a full walk and reconcile, which finds
/// everything the drops lost — so overflow costs a rescan, never a missed change.
///
/// The alternative, blocking the producer, is worse in exactly the case that matters:
/// the producer is the watcher's coalescer, and blocking it means the OS event buffer
/// overflows instead. That loses events with no record that anything was lost, which
/// is the one outcome C-02 cannot tolerate — a vault whose index quietly disagrees
/// with its files.
#[derive(Debug)]
pub struct WorkQueue {
    inner: Mutex<Inner>,
    ready: Condvar,
    capacity: usize,
}

#[derive(Debug)]
struct Inner {
    order: VecDeque<VaultPath>,
    queued: HashSet<VaultPath>,
    stats: QueueStats,
    overflowed: bool,
    closed: bool,
}

impl WorkQueue {
    /// A queue with [`DEFAULT_CAPACITY`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// A queue holding at most `capacity` distinct paths.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero, which would be a queue that accepts nothing.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "a work queue with no capacity accepts nothing"
        );
        Self {
            inner: Mutex::new(Inner {
                order: VecDeque::new(),
                queued: HashSet::new(),
                stats: QueueStats::default(),
                overflowed: false,
                closed: false,
            }),
            ready: Condvar::new(),
            capacity,
        }
    }

    /// Offer a path.
    ///
    /// Returns whether it was newly queued. A path already waiting is deduplicated —
    /// the file will be read once, when its turn comes, in whatever state it is in
    /// then, which is the state that matters.
    pub fn push(&self, path: VaultPath) -> bool {
        let mut inner = self.lock();
        if inner.closed {
            return false;
        }
        if inner.queued.contains(&path) {
            inner.stats.deduplicated += 1;
            return false;
        }
        if inner.order.len() >= self.capacity {
            inner.stats.overflowed += 1;
            inner.overflowed = true;
            return false;
        }
        inner.queued.insert(path.clone());
        inner.order.push_back(path);
        inner.stats.accepted += 1;
        drop(inner);
        self.ready.notify_one();
        true
    }

    /// Take the next path, blocking until one arrives or the queue closes.
    ///
    /// `None` means closed and drained, which is the consumer's signal to stop.
    pub fn pop(&self) -> Option<VaultPath> {
        let mut inner = self.lock();
        loop {
            if let Some(path) = inner.order.pop_front() {
                inner.queued.remove(&path);
                return Some(path);
            }
            if inner.closed {
                return None;
            }
            inner = self
                .ready
                .wait(inner)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Take the next path, waiting at most `timeout`.
    pub fn pop_timeout(&self, timeout: Duration) -> Option<VaultPath> {
        let deadline = std::time::Instant::now() + timeout;
        let mut inner = self.lock();
        loop {
            if let Some(path) = inner.order.pop_front() {
                inner.queued.remove(&path);
                return Some(path);
            }
            if inner.closed {
                return None;
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let (guard, _) = self
                .ready
                .wait_timeout(inner, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner = guard;
        }
    }

    /// Take everything waiting, in order, without blocking.
    ///
    /// How the index writer should drain: one batch, one transaction (§3.1 step 5).
    pub fn drain(&self) -> Vec<VaultPath> {
        let mut inner = self.lock();
        inner.queued.clear();
        inner.order.drain(..).collect()
    }

    /// Whether the queue has overflowed since this was last checked, clearing the
    /// flag.
    ///
    /// A `true` means: paths were dropped, so walk the vault and reconcile. It is
    /// latched rather than counted because the answer to one drop and to four
    /// thousand is the same.
    pub fn take_overflowed(&self) -> bool {
        let mut inner = self.lock();
        std::mem::replace(&mut inner.overflowed, false)
    }

    /// Counters since construction.
    #[must_use]
    pub fn stats(&self) -> QueueStats {
        let mut inner = self.lock();
        inner.stats.depth = inner.order.len();
        inner.stats
    }

    /// Number of paths waiting.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.lock().order.len()
    }

    /// Refuse further pushes and wake every consumer.
    pub fn close(&self) {
        let mut inner = self.lock();
        inner.closed = true;
        drop(inner);
        self.ready.notify_all();
    }

    /// Whether the queue is closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for WorkQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn path(text: &str) -> VaultPath {
        VaultPath::new(text).expect("path")
    }

    #[test]
    fn a_path_offered_twice_is_queued_once() {
        let queue = WorkQueue::new();
        assert!(queue.push(path("note.md")));
        assert!(!queue.push(path("note.md")));
        assert!(!queue.push(path("note.md")));

        assert_eq!(queue.depth(), 1);
        assert_eq!(queue.pop(), Some(path("note.md")));
        assert_eq!(queue.depth(), 0);

        let stats = queue.stats();
        assert_eq!(stats.accepted, 1);
        assert_eq!(stats.deduplicated, 2);
    }

    #[test]
    fn a_path_can_be_queued_again_after_it_has_been_taken() {
        // The case that makes dedup safe: an edit arriving while the previous version
        // is being parsed must not be lost.
        let queue = WorkQueue::new();
        queue.push(path("note.md"));
        assert_eq!(queue.pop(), Some(path("note.md")));
        assert!(queue.push(path("note.md")));
        assert_eq!(queue.depth(), 1);
    }

    #[test]
    fn four_thousand_files_become_four_thousand_entries_not_more() {
        // Architecture §3.1 step 2, stated as a number: a git checkout emits several
        // events per file, and the queue must hold one entry per path.
        let queue = WorkQueue::with_capacity(8_192);
        for round in 0..3 {
            for i in 0..4_000 {
                queue.push(path(&format!("10-projects/note-{i}.md")));
            }
            assert_eq!(queue.depth(), 4_000, "after round {round}");
        }
        let stats = queue.stats();
        assert_eq!(stats.accepted, 4_000);
        assert_eq!(stats.deduplicated, 8_000);
        assert_eq!(stats.overflowed, 0);
    }

    #[test]
    fn overflow_is_latched_so_the_consumer_knows_to_rescan() {
        let queue = WorkQueue::with_capacity(2);
        assert!(queue.push(path("a.md")));
        assert!(queue.push(path("b.md")));
        assert!(!queue.push(path("c.md")), "accepted past capacity");

        assert!(queue.take_overflowed(), "overflow was not reported");
        assert!(!queue.take_overflowed(), "overflow did not clear");
        assert_eq!(queue.stats().overflowed, 1);

        // The queue keeps working; it is the dropped path that needs the rescan.
        assert_eq!(queue.pop(), Some(path("a.md")));
        assert!(queue.push(path("c.md")));
    }

    #[test]
    fn order_is_preserved_for_the_paths_that_are_accepted() {
        let queue = WorkQueue::new();
        for name in ["c.md", "a.md", "b.md"] {
            queue.push(path(name));
        }
        assert_eq!(
            queue.drain(),
            vec![path("c.md"), path("a.md"), path("b.md")]
        );
        assert_eq!(queue.depth(), 0);
    }

    #[test]
    fn draining_clears_the_dedup_set_too() {
        // If it did not, a path drained but not yet indexed could never be requeued,
        // and an edit made during that window would be lost forever.
        let queue = WorkQueue::new();
        queue.push(path("note.md"));
        assert_eq!(queue.drain(), vec![path("note.md")]);
        assert!(queue.push(path("note.md")));
    }

    #[test]
    fn a_consumer_blocked_on_pop_wakes_when_work_arrives() {
        let queue = Arc::new(WorkQueue::new());
        let consumer = Arc::clone(&queue);
        let handle = std::thread::spawn(move || consumer.pop());

        std::thread::sleep(Duration::from_millis(20));
        queue.push(path("late.md"));

        assert_eq!(handle.join().expect("consumer"), Some(path("late.md")));
    }

    #[test]
    fn closing_wakes_every_consumer_and_stops_accepting() {
        let queue = Arc::new(WorkQueue::new());
        let handles: Vec<_> = (0..3)
            .map(|_| {
                let consumer = Arc::clone(&queue);
                std::thread::spawn(move || consumer.pop())
            })
            .collect();

        std::thread::sleep(Duration::from_millis(20));
        queue.close();

        for handle in handles {
            assert_eq!(handle.join().expect("consumer"), None);
        }
        assert!(!queue.push(path("too-late.md")));
        assert!(queue.is_closed());
    }

    #[test]
    fn pop_timeout_returns_rather_than_hanging() {
        let queue = WorkQueue::new();
        let started = std::time::Instant::now();
        assert_eq!(queue.pop_timeout(Duration::from_millis(30)), None);
        assert!(started.elapsed() >= Duration::from_millis(25));
    }

    #[test]
    fn many_producers_and_one_consumer_lose_nothing() {
        let queue = Arc::new(WorkQueue::with_capacity(4_096));
        let producers: Vec<_> = (0..4)
            .map(|worker| {
                let queue = Arc::clone(&queue);
                std::thread::spawn(move || {
                    for i in 0..250 {
                        queue.push(path(&format!("w{worker}/note-{i}.md")));
                    }
                })
            })
            .collect();
        for producer in producers {
            producer.join().expect("producer");
        }

        assert_eq!(queue.stats().accepted, 1_000);
        assert_eq!(queue.stats().overflowed, 0);
        assert_eq!(queue.drain().len(), 1_000);
    }
}
