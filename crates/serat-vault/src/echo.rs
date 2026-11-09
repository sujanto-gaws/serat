//! Recognising our own writes.
//!
//! Architecture §3.2. Serat's writes produce filesystem events indistinguishable from
//! anyone else's, so without this every write-back would be followed by a reindex of
//! the file we just finished writing — and, worse, the index would race its own
//! output.
//!
//! The set is keyed by the pair **(path, hash)**, and that is the whole design. A
//! genuine external edit that writes the same bytes to the same path is then
//! indistinguishable from our own write, and is correctly treated as a no-op —
//! because it is one. The index already holds those bytes at that path. Keying by path
//! alone would drop a real edit that landed in the same window, which is a lost
//! update; keying by hash alone (as P0 did) would let a copy of a just-saved note to
//! another path consume the entry and be skipped (code-reviewer-14).

use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;

use serat_core::{ContentHash, VaultPath};

/// What an echo is keyed by: the path that was written and the hash it was given.
/// Keyed by the pair (architecture 3.2, ruling of code-reviewer-14): the argument for
/// suppressing, that the index already holds those bytes, is true only for the path
/// the save wrote. By hash alone, `cp A.md B.md` right after a save of A would let B
/// consume A's entry and be skipped.
type Key = (VaultPath, ContentHash);

/// How many hashes the set holds before it evicts the oldest.
///
/// Defaults to this crate's [`crate::queue::DEFAULT_CAPACITY`] (F-252): bounded by
/// size, not by time. Two seconds was the original design, and its correctness
/// argument — a hash must not linger and swallow an unrelated edit made a minute
/// later — did not survive F-224. Under late recording an entry exists only after a
/// submit has already succeeded, so the index provably holds those bytes at the
/// moment the entry is created, and suppressing a later event carrying the same bytes
/// is correct however late it arrives, however long the entry has waited. What a
/// duration was actually bounding was memory, and a duration bounds memory only if you
/// already know how long the consumer takes; a capacity bounds it unconditionally.
///
/// The capacity is **at least the work queue's**, because the queue is what delays the
/// consumer that would otherwise take an echo — sizing the set against the thing that
/// causes the delay is the one figure here that is derived rather than guessed.
/// Thirty-two bytes a hash puts [`crate::queue::DEFAULT_CAPACITY`] (8,192) at about a
/// quarter of a megabyte, which is not a budget anyone needs to discuss.
///
/// Explicitly not kept alongside this: a long duration as a second bound behind the
/// capacity. That would reintroduce exactly the coupling to load this removes, at a
/// length nobody could justify by measurement either.
pub const DEFAULT_CAPACITY: usize = crate::queue::DEFAULT_CAPACITY;

/// The bounded set of hashes we have indexed and are waiting to hear echoed back.
///
/// Cheap to share: every method takes `&self`, so the writer and the watcher hold the
/// same one without either owning it.
#[derive(Debug)]
pub struct EchoSuppressor {
    inner: Mutex<Inner>,
    capacity: usize,
}

/// Insertion order plus a set, the same shape [`crate::queue::WorkQueue`] uses for the
/// same reason: `order` gives oldest-first eviction, `set` gives O(1) membership, and
/// keeping them in step is cheaper than a data structure that does both badly.
#[derive(Debug, Default)]
struct Inner {
    order: VecDeque<Key>,
    set: HashSet<Key>,
}

impl Default for EchoSuppressor {
    fn default() -> Self {
        Self::new()
    }
}

impl EchoSuppressor {
    /// A suppressor sized at [`DEFAULT_CAPACITY`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// A suppressor holding at most `capacity` hashes, for tests and for a caller with
    /// its own reason to size this against something other than the default queue.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero, which would be a set that suppresses nothing.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "an echo set with no capacity suppresses nothing"
        );
        Self {
            inner: Mutex::new(Inner::default()),
            capacity,
        }
    }

    /// Record a hash the watcher should recognise as our own write and drop.
    ///
    /// Called **after** a write has been indexed, not before (architecture §3.2,
    /// amended by F-226 following F-196/F-222/F-224). Recording ahead of the write
    /// reads as the safer order — a fast filesystem can deliver the watcher event
    /// before a later call would have returned — but it means a write recorded and
    /// then refused downstream suppresses the only event that would ever have indexed
    /// the note. `serat_core::EchoGuard` is the trait that decides *when* "after" is —
    /// `Vault`'s implementation of it calls this once a submit has been accepted; this
    /// method itself only ever records.
    ///
    /// Recording the same pair again — an editor's autosave after an undo, say —
    /// refreshes its position to the back of the eviction order rather than adding a
    /// second entry, so the newer write's own event is what gets
    /// suppressed, and it is the write most likely to still be waiting.
    pub fn record(&self, path: &VaultPath, hash: ContentHash) {
        let key = (path.clone(), hash);
        let mut inner = self.lock();
        if inner.set.contains(&key) {
            inner.order.retain(|candidate| *candidate != key);
            inner.order.push_back(key);
            return;
        }
        if inner.order.len() >= self.capacity
            && let Some(oldest) = inner.order.pop_front()
        {
            inner.set.remove(&oldest);
        }
        inner.set.insert(key.clone());
        inner.order.push_back(key);
    }

    /// Whether an event for `path` carrying these bytes is our own echo, consuming the
    /// entry if it is. An entry for a different path with the same bytes does not match.
    ///
    /// Consuming matters: two identical writes produce two events, and one recorded
    /// entry must suppress exactly one of them.
    pub fn take_echo(&self, path: &VaultPath, hash: ContentHash) -> bool {
        let key = (path.clone(), hash);
        let mut inner = self.lock();
        if inner.set.remove(&key) {
            inner.order.retain(|candidate| *candidate != key);
            true
        } else {
            false
        }
    }

    /// How many hashes are currently waiting. Diagnostics only.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.lock().order.len()
    }

    /// Recover from a poisoned mutex rather than propagating the panic.
    ///
    /// This holds no invariant worth protecting: it is a cache of hints, and the
    /// worst outcome of stale contents is one unnecessary reindex. Refusing to
    /// suppress echoes for the rest of the session would be the more expensive
    /// failure, and taking down the watcher thread over it more expensive still.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| {
            let mut guard = poisoned.into_inner();
            guard.order.clear();
            guard.set.clear();
            guard
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash_bytes;

    fn p(text: &str) -> VaultPath {
        VaultPath::new(text).expect("path")
    }

    #[test]
    fn our_own_write_is_recognised_once_and_only_once() {
        let echo = EchoSuppressor::new();
        let hash = hash_bytes(b"status:: done\n");

        echo.record(&p("a.md"), hash);
        assert!(
            echo.take_echo(&p("a.md"), hash),
            "our own write was not recognised"
        );
        assert!(
            !echo.take_echo(&p("a.md"), hash),
            "one recorded write suppressed two events"
        );
    }

    #[test]
    fn the_same_bytes_at_another_path_are_not_our_echo() {
        // code-reviewer-14: `cp A.md B.md` right after a save of A. B's event carries
        // A's hash, and must neither be suppressed nor consume A's entry.
        let echo = EchoSuppressor::new();
        let hash = hash_bytes(b"# A\n");
        echo.record(&p("A.md"), hash);

        assert!(
            !echo.take_echo(&p("B.md"), hash),
            "B was taken for A's echo"
        );
        assert_eq!(echo.pending(), 1, "B's event consumed A's entry");
        assert!(echo.take_echo(&p("A.md"), hash), "A's own event was lost");
    }

    #[test]
    fn an_unrelated_edit_is_not_suppressed() {
        let echo = EchoSuppressor::new();
        echo.record(&p("a.md"), hash_bytes(b"ours"));
        assert!(!echo.take_echo(&p("a.md"), hash_bytes(b"someone else's")));
        // And ours is still waiting.
        assert_eq!(echo.pending(), 1);
    }

    #[test]
    fn capacity_bounds_the_set_so_the_oldest_entry_is_evicted_first() {
        // F-252: bounded by size, not by time. Three entries into a capacity of two
        // leave exactly the two most recent; the oldest specifically must be gone.
        let echo = EchoSuppressor::with_capacity(2);
        let (first, second, third) = (
            hash_bytes(b"first"),
            hash_bytes(b"second"),
            hash_bytes(b"third"),
        );

        echo.record(&p("a.md"), first);
        echo.record(&p("a.md"), second);
        echo.record(&p("a.md"), third);

        assert_eq!(echo.pending(), 2, "the set grew past its capacity");
        assert!(
            !echo.take_echo(&p("a.md"), first),
            "the oldest entry was not the one evicted"
        );
        assert!(
            echo.take_echo(&p("a.md"), second),
            "a surviving entry was evicted"
        );
        assert!(
            echo.take_echo(&p("a.md"), third),
            "a surviving entry was evicted"
        );
    }

    #[test]
    fn recording_an_entry_again_moves_it_to_the_back_of_the_eviction_order() {
        // An autosave after an undo re-records the same pair. It must not count as a
        // second entry, and it should be the last one evicted.
        let echo = EchoSuppressor::with_capacity(2);
        let repeated = hash_bytes(b"same bytes");
        let other = hash_bytes(b"other");

        echo.record(&p("a.md"), repeated);
        echo.record(&p("b.md"), other);
        echo.record(&p("a.md"), repeated); // refresh: now the most recent
        let newcomer = hash_bytes(b"newcomer");
        echo.record(&p("c.md"), newcomer); // must evict b.md, the true oldest

        assert_eq!(echo.pending(), 2);
        assert!(
            !echo.take_echo(&p("b.md"), other),
            "the refreshed entry's original slot was evicted instead of it"
        );
        assert!(echo.take_echo(&p("a.md"), repeated));
        assert!(echo.take_echo(&p("c.md"), newcomer));
    }

    #[test]
    fn recording_the_same_pair_twice_still_suppresses_only_one_event() {
        let echo = EchoSuppressor::new();
        let hash = hash_bytes(b"same bytes");
        echo.record(&p("a.md"), hash);
        echo.record(&p("a.md"), hash);
        // Recorded once (the second refreshes the first), suppressed once; the second
        // event is a real change that the hash gate then discards for a hash.
        assert!(echo.take_echo(&p("a.md"), hash));
        assert!(!echo.take_echo(&p("a.md"), hash));
    }

    #[test]
    fn it_is_shareable_across_threads() {
        let echo = std::sync::Arc::new(EchoSuppressor::new());
        let writer = std::sync::Arc::clone(&echo);
        let handle = std::thread::spawn(move || {
            for i in 0..100u8 {
                writer.record(&p("a.md"), hash_bytes(&[i]));
            }
        });
        handle.join().expect("writer thread");
        assert_eq!(echo.pending(), 100);
        assert!(echo.take_echo(&p("a.md"), hash_bytes(&[7])));
    }
}
