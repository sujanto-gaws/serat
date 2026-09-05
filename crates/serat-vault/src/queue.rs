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

/// Evidence that a rescan was owed, handed back when it completes.
///
/// Carries the overflow generation it covers, so a rescan that ran while more paths
/// were being dropped does not clear a latch it did not earn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RescanTicket(u64);

/// A bounded queue of paths waiting to be indexed, holding each path at most once.
///
/// # Which producer may be dropped
///
/// This is the decision, and it is not "may we drop" — it is "who".
///
/// **The watcher may be dropped.** [`WorkQueue::push`] refuses when full and latches
/// a rescan. Blocking the watcher's coalescer instead would stop us draining the OS
/// event buffer, so the platform would drop notifications and tell nobody — losing
/// changes with no record that anything was lost, which is the one outcome C-02
/// cannot tolerate. A dropped watcher event is recoverable because a walk finds it.
///
/// **A walk may not be dropped.** [`WorkQueue::push_blocking`] waits for space. The
/// walk is not the coalescer, so the argument above does not apply to it, and it is
/// the recovery mechanism: routing it through a queue that drops means the recovery
/// re-enters the failure.
///
/// That second half was a defect, not a design. The queue holds 8,192 paths and the
/// P0 vault has 20,200 files; `walk()` returns them sorted and this queue is FIFO, so
/// every rescan pushed the same sorted prefix, the same 8,192 won the slots, and
/// 12,008 files had never once reached the consumer after five full rescans. A stable
/// loop, not a slow one. `overflow_recovery_converges_on_a_vault_larger_than_the_queue`
/// is the test that would have caught it.
#[derive(Debug)]
pub struct WorkQueue {
    inner: Mutex<Inner>,
    /// Signalled when work arrives, for consumers waiting in `pop`.
    ready: Condvar,
    /// Signalled when work is taken, for bulk producers waiting in `push_blocking`.
    space: Condvar,
    capacity: usize,
}

#[derive(Debug)]
struct Inner {
    order: VecDeque<VaultPath>,
    queued: HashSet<VaultPath>,
    stats: QueueStats,
    /// Bumped on every overflow. A rescan clears the latch only if no further
    /// overflow happened while it ran, which is what makes the clear happen on
    /// completion rather than on read.
    overflow_generation: u64,
    /// The generation a rescan has successfully covered.
    rescanned_generation: u64,
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
                overflow_generation: 0,
                rescanned_generation: 0,
                closed: false,
            }),
            ready: Condvar::new(),
            space: Condvar::new(),
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
            inner.overflow_generation += 1;
            return false;
        }
        inner.queued.insert(path.clone());
        inner.order.push_back(path);
        inner.stats.accepted += 1;
        drop(inner);
        self.ready.notify_one();
        true
    }

    /// Offer a path, waiting for space rather than dropping it.
    ///
    /// For bulk producers: the initial walk and the rescan that recovers from an
    /// overflow. Nothing will mention these paths again, so dropping one means the
    /// index silently disagrees with the disk — and when the producer *is* the
    /// recovery mechanism, dropping means the recovery re-enters the failure it was
    /// sent to repair.
    ///
    /// Safe to block precisely because this producer is not the watcher's coalescer:
    /// waiting here cannot stop the OS event buffer being drained.
    ///
    /// Returns whether the path was newly queued; `false` means it was already
    /// waiting, or the queue closed.
    pub fn push_blocking(&self, path: VaultPath) -> bool {
        let mut inner = self.lock();
        loop {
            if inner.closed {
                return false;
            }
            if inner.queued.contains(&path) {
                inner.stats.deduplicated += 1;
                return false;
            }
            if inner.order.len() < self.capacity {
                inner.queued.insert(path.clone());
                inner.order.push_back(path);
                inner.stats.accepted += 1;
                drop(inner);
                self.ready.notify_one();
                return true;
            }
            inner = self
                .space
                .wait(inner)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Take the next path, blocking until one arrives or the queue closes.
    ///
    /// `None` means closed and drained, which is the consumer's signal to stop.
    pub fn pop(&self) -> Option<VaultPath> {
        let mut inner = self.lock();
        loop {
            if let Some(path) = inner.order.pop_front() {
                inner.queued.remove(&path);
                drop(inner);
                self.space.notify_one();
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
                drop(inner);
                self.space.notify_one();
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
        let taken: Vec<VaultPath> = inner.order.drain(..).collect();
        drop(inner);
        self.space.notify_all();
        taken
    }

    /// Whether paths have been dropped and not yet reconciled.
    ///
    /// Returns the ticket a completed rescan hands back. Peeking does **not** clear:
    /// clear-on-read loses the signal if the rescan then fails part-way or the process
    /// exits, and "the index may disagree with the disk" is exactly the state that
    /// must survive a crash.
    ///
    /// The queue owns this latch. The watcher may peek to raise
    /// `VaultEvent::RescanRequired`, but only the consumer that actually completes the
    /// walk calls [`WorkQueue::rescan_completed`].
    #[must_use]
    pub fn rescan_needed(&self) -> Option<RescanTicket> {
        let inner = self.lock();
        (inner.overflow_generation > inner.rescanned_generation)
            .then_some(RescanTicket(inner.overflow_generation))
    }

    /// Record that a rescan covering `ticket` finished.
    ///
    /// Clears the latch only if nothing overflowed while the rescan ran; otherwise the
    /// newer overflow is still outstanding and another rescan is owed. Returns whether
    /// the latch is now clear.
    pub fn rescan_completed(&self, ticket: RescanTicket) -> bool {
        let mut inner = self.lock();
        inner.rescanned_generation = inner.rescanned_generation.max(ticket.0);
        inner.overflow_generation <= inner.rescanned_generation
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

    /// Refuse further pushes and wake every consumer and every blocked producer.
    ///
    /// Both condvars, not just `ready` (F-11 review follow-up, code-reviewer). A
    /// producer parked in [`WorkQueue::push_blocking`] waits on `space`, and closing
    /// without waking it left that producer blocked indefinitely — measured at 200 ms
    /// still blocked in 7 of 8 runs, with a no-close control confirming the wake was
    /// not coming from anywhere else. The window is not a corner case: ADR-0011's
    /// rescan recovery drives its walk through `push_blocking` precisely so it cannot
    /// drop a path, which means it spends most of a large rescan blocked in exactly
    /// this state, and a shutdown during a rescan hits it with high probability rather
    /// than low. Same family as F-21, where a channel's lifetime stood in for a signal
    /// and a caller waiting on the wrong thing hung for 600 s before anyone noticed.
    pub fn close(&self) {
        let mut inner = self.lock();
        inner.closed = true;
        drop(inner);
        self.ready.notify_all();
        self.space.notify_all();
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

        let ticket = queue.rescan_needed().expect("overflow was not reported");
        assert!(
            queue.rescan_needed().is_some(),
            "peeking cleared the latch; it must survive until a rescan completes"
        );
        assert!(queue.rescan_completed(ticket), "the latch did not clear");
        assert!(queue.rescan_needed().is_none());
        assert_eq!(queue.stats().overflowed, 1);

        // The queue keeps working; it is the dropped path that needs the rescan.
        assert_eq!(queue.pop(), Some(path("a.md")));
        assert!(queue.push(path("c.md")));
    }

    #[test]
    fn a_rescan_does_not_clear_a_latch_it_did_not_earn() {
        // S7. If paths were dropped while the rescan was running, that rescan cannot
        // have seen them, so the latch stays set and another is owed. Clearing on read
        // would have lost the second overflow entirely.
        let queue = WorkQueue::with_capacity(1);
        queue.push(path("a.md"));
        assert!(!queue.push(path("b.md")));
        let ticket = queue.rescan_needed().expect("first overflow");

        // ... rescan starts ... and another overflow happens while it runs.
        assert!(!queue.push(path("c.md")));

        assert!(
            !queue.rescan_completed(ticket),
            "a rescan cleared a latch raised after it started"
        );
        assert!(
            queue.rescan_needed().is_some(),
            "the second overflow was lost"
        );
    }

    #[test]
    fn a_watcher_drop_while_the_queue_is_full_is_recovered_by_the_rescan_that_follows() {
        // F-225. "A dropped watcher event is recovered by the rescan latch" is
        // load-bearing for the entire backpressure design, and before this test it was
        // asserted in three comments (this module's own doc, `watch.rs`'s
        // `VaultEvent::RescanRequired`, and I5's `incremental.rs` before F-224) and
        // tested in none. `overflow_recovery_converges_on_a_vault_larger_than_the_queue`
        // below proves a bulk producer using `push_blocking` alone never drops;
        // `a_rescan_does_not_clear_a_latch_it_did_not_earn` above proves the generation
        // bookkeeping abstractly, with plain pushes standing in for whatever produced
        // them. Neither traces one concrete path from "the watcher's push was refused"
        // through to "the consumer actually received it, exactly once" — which is the
        // sentence itself, not a restatement of it.
        //
        // "Both queues full" (F-216, F-225): the writer being behind is usually why the
        // watcher's own queue is full too, and the chain a fix must terminate at is not
        // "the watcher enqueues it" but "the watcher enqueues it, or drops it and
        // latches a generation a rescan covers." There is one `WorkQueue` type in this
        // crate, so what is reproduced here is that second clause end to end: the
        // queue is filled to capacity with no consumer draining it — standing in for a
        // writer that cannot keep up — a live watcher event finds no room and is
        // dropped, and only then does a consumer start and a rescan (the recovery
        // mechanism the latch exists to trigger) run, through the door that waits
        // rather than refuses.
        //
        // The queue is saturated synchronously, before anything is spawned, so the
        // drop below is forced rather than raced — a setup that only sometimes reaches
        // the drop would prove nothing about the recovery (F-58).
        let queue = Arc::new(WorkQueue::with_capacity(2));
        assert!(queue.push(path("backlog-1.md")));
        assert!(queue.push(path("backlog-2.md")));

        let dropped = path("watcher/live-edit.md");
        assert!(
            !queue.push(dropped.clone()),
            "the queue had room; this test proves nothing about a drop"
        );
        assert_eq!(queue.stats().overflowed, 1);
        let ticket = queue
            .rescan_needed()
            .expect("the refused push did not raise the latch");

        // The consumer starts only now, and drains one at a time with a real timeout,
        // so the rescan's push_blocking below genuinely waits for space at least once
        // rather than finding it already free.
        let consumer_queue = Arc::clone(&queue);
        let consumer = std::thread::spawn(move || {
            let mut seen = Vec::new();
            loop {
                match consumer_queue.pop_timeout(Duration::from_millis(20)) {
                    Some(path) => seen.push(path),
                    None if consumer_queue.is_closed() && consumer_queue.depth() == 0 => {
                        return seen;
                    }
                    None => {}
                }
            }
        });

        // The rescan: walk the vault and offer every path again, including the one
        // that was dropped. The two already-queued paths dedup harmlessly; the
        // dropped one is what this test is about.
        for candidate in [path("backlog-1.md"), path("backlog-2.md"), dropped.clone()] {
            queue.push_blocking(candidate);
        }
        assert!(
            queue.rescan_completed(ticket),
            "the rescan did not clear the latch it was sent to cover"
        );
        assert!(queue.rescan_needed().is_none());
        queue.close();

        let seen = consumer.join().expect("consumer");
        assert!(
            seen.contains(&dropped),
            "the watcher's dropped path was never recovered by the rescan that followed it"
        );
        assert_eq!(
            seen.iter().filter(|candidate| **candidate == dropped).count(),
            1,
            "the dropped path was recovered more than once"
        );
    }

    #[test]
    fn overflow_recovery_converges_on_a_vault_larger_than_the_queue() {
        // BLOCKING 1 from the I2 review, as a test. The queue holds 8,192 and the P0
        // vault has 20,200 files; walk() returns them sorted and this queue is FIFO,
        // so a rescan that pushed with the dropping producer re-pushed the same sorted
        // prefix, the same paths won the slots, and 12,008 files never once reached
        // the consumer. The recovery re-entered the failure.
        //
        // With the bulk producer waiting for space instead, every path arrives.
        let queue = Arc::new(WorkQueue::with_capacity(8_192));
        let paths: Vec<VaultPath> = (0..20_200)
            .map(|i| path(&format!("10-projects/note-{i:05}.md")))
            .collect();

        // The consumer starts late and drains in bursts, so the queue genuinely fills
        // and push_blocking genuinely waits. Without this the producer never reaches
        // capacity, and a version that drops on full behaves identically — which is
        // what the first version of this test asserted by accident.
        let consumer = {
            let queue = Arc::clone(&queue);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                let mut seen = Vec::new();
                loop {
                    let mut drained = 0;
                    while drained < 500 {
                        match queue.pop_timeout(Duration::from_millis(50)) {
                            Some(path) => {
                                seen.push(path);
                                drained += 1;
                            }
                            None => {
                                if queue.is_closed() && queue.depth() == 0 {
                                    return seen;
                                }
                                break;
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
        };

        for path in &paths {
            queue.push_blocking(path.clone());
        }
        queue.close();

        let seen = consumer.join().expect("consumer");
        assert_eq!(seen.len(), paths.len(), "the walk lost paths");
        assert_eq!(seen, paths, "and it lost their order");
        assert_eq!(
            queue.stats().overflowed,
            0,
            "a blocking producer overflowed"
        );
        assert!(queue.rescan_needed().is_none());
    }

    #[test]
    fn push_blocking_waits_for_space_rather_than_refusing() {
        // The mirror of the_watcher_still_drops_rather_than_blocking, and the guard
        // the convergence test was standing in for without actually providing: that
        // one had its consumer draining from the start, so the queue never filled and
        // a push_blocking that dropped on full behaved identically.
        //
        // Here the queue is full before the third push, so the producer must be
        // observed still waiting — and then completing once room is made.
        let queue = Arc::new(WorkQueue::with_capacity(2));
        assert!(queue.push_blocking(path("a.md")));
        assert!(queue.push_blocking(path("b.md")));

        let waiting = {
            let queue = Arc::clone(&queue);
            std::thread::spawn(move || queue.push_blocking(path("c.md")))
        };

        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !waiting.is_finished(),
            "push_blocking refused instead of waiting; the walk would lose this path              and nothing would ever mention it again"
        );

        assert_eq!(queue.pop(), Some(path("a.md")));
        assert!(
            waiting.join().expect("producer"),
            "push_blocking did not queue the path once there was room"
        );
        assert_eq!(queue.stats().overflowed, 0);
        assert!(queue.rescan_needed().is_none());
    }

    #[test]
    fn the_watcher_still_drops_rather_than_blocking() {
        // The other half of the ruling: push (the watcher's door) must never wait, or
        // the coalescer stops draining the OS event buffer and the platform loses
        // events without telling anyone.
        let queue = WorkQueue::with_capacity(2);
        queue.push(path("a.md"));
        queue.push(path("b.md"));

        let started = std::time::Instant::now();
        assert!(!queue.push(path("c.md")), "push waited for space");
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "push blocked; it must refuse instead"
        );
        assert!(queue.rescan_needed().is_some());
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
    fn closing_wakes_a_producer_blocked_in_push_blocking() {
        // Task #12 / F-11 review follow-up. `close()` used to notify only `ready`,
        // never `space` — so a producer parked in `push_blocking` on a full queue was
        // not woken by a close at all, and this is the state ADR-0011's rescan spends
        // most of its life in on a vault larger than the queue.
        //
        // The queue is filled to capacity with no consumer running, so the producer
        // below is genuinely parked on `space` before `close()` runs — checking the
        // setup reaches the path (F-58), since a producer that was never actually
        // blocked would pass this test whether or not the fix exists.
        let queue = Arc::new(WorkQueue::with_capacity(1));
        assert!(queue.push(path("a.md")));

        let producer_queue = Arc::clone(&queue);
        let producer = std::thread::spawn(move || producer_queue.push_blocking(path("b.md")));

        // Give the producer thread time to actually reach the wait, not just to have
        // been spawned. Not a source of flakiness in the failing direction: if this
        // sleep were too short the producer might not yet be parked, which would make
        // the test pass falsely on the *old*, broken code too — so the assertion below
        // that it is still running after the sleep is what makes the setup honest.
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !producer.is_finished(),
            "the producer returned before close(); it was never blocked, so this test \
             cannot tell the fix from its absence"
        );

        queue.close();

        // A timeout that fails loudly rather than hanging the suite if the fix
        // regresses: a `JoinHandle` has no built-in one, so a second thread races the
        // join against a deadline.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(producer.join());
        });
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(joined) => {
                joined.expect("producer thread panicked");
            }
            Err(_) => panic!(
                "the producer blocked in push_blocking was not woken by close() within \
                 5 s"
            ),
        }
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
