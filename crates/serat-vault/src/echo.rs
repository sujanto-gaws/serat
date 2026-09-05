//! Recognising our own writes.
//!
//! Architecture §3.2. Serat's writes produce filesystem events indistinguishable from
//! anyone else's, so without this every write-back would be followed by a reindex of
//! the file we just finished writing — and, worse, the index would race its own
//! output.
//!
//! The set is keyed by **hash, not by path**, and that is the whole design. A genuine
//! external edit that happens to produce the same bytes is then indistinguishable
//! from our own write, and is correctly treated as a no-op — because it is one. The
//! index already holds those bytes. Keying by path would instead drop a real edit
//! that landed in the same window, which is a lost update.

use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;

use serat_core::ContentHash;

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
    order: VecDeque<ContentHash>,
    set: HashSet<ContentHash>,
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
    /// Recording the same hash again — an editor's autosave after an undo, say —
    /// refreshes its position to the back of the eviction order rather than adding a
    /// second entry: keyed by hash, so the newer write's own event is what gets
    /// suppressed, and it is the write most likely to still be waiting.
    pub fn record(&self, hash: ContentHash) {
        let mut inner = self.lock();
        if inner.set.contains(&hash) {
            inner.order.retain(|candidate| *candidate != hash);
            inner.order.push_back(hash);
            return;
        }
        if inner.order.len() >= self.capacity
            && let Some(oldest) = inner.order.pop_front()
        {
            inner.set.remove(&oldest);
        }
        inner.set.insert(hash);
        inner.order.push_back(hash);
    }

    /// Whether an event carrying these bytes is our own echo, consuming the entry if
    /// it is.
    ///
    /// Consuming matters: two identical writes produce two events, and one recorded
    /// hash must suppress exactly one of them.
    pub fn take_echo(&self, hash: ContentHash) -> bool {
        let mut inner = self.lock();
        if inner.set.remove(&hash) {
            inner.order.retain(|candidate| *candidate != hash);
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

    #[test]
    fn our_own_write_is_recognised_once_and_only_once() {
        let echo = EchoSuppressor::new();
        let hash = hash_bytes(b"status:: done\n");

        echo.record(hash);
        assert!(echo.take_echo(hash), "our own write was not recognised");
        assert!(
            !echo.take_echo(hash),
            "one recorded write suppressed two events"
        );
    }

    #[test]
    fn an_unrelated_edit_is_not_suppressed() {
        let echo = EchoSuppressor::new();
        echo.record(hash_bytes(b"ours"));
        assert!(!echo.take_echo(hash_bytes(b"someone else's")));
        // And ours is still waiting.
        assert_eq!(echo.pending(), 1);
    }

    #[test]
    fn capacity_bounds_the_set_so_the_oldest_entry_is_evicted_first() {
        // F-252: the set is now bounded by size, not by time. Two seconds could not
        // be trusted to expire an entry (F-224 removed the reason it needed to), and
        // this replaces `entries_expire_so_a_stale_hash_cannot_swallow_a_later_edit`,
        // which asserted the behaviour this ruling reverses — it would either fail, or
        // worse, keep passing while meaning nothing, since a duration-based test and a
        // capacity-based implementation can coincidentally agree on small numbers.
        //
        // Checked here that the setup actually reaches eviction, not just that the
        // assertion is well-formed (F-58): three hashes into a capacity of two must
        // leave exactly the two most recent, and the oldest specifically, not merely
        // "some hash", is what must be gone.
        let echo = EchoSuppressor::with_capacity(2);
        let first = hash_bytes(b"first");
        let second = hash_bytes(b"second");
        let third = hash_bytes(b"third");

        echo.record(first);
        echo.record(second);
        echo.record(third);

        assert_eq!(echo.pending(), 2, "the set grew past its capacity");
        assert!(
            !echo.take_echo(first),
            "the oldest entry was not the one evicted"
        );
        assert!(echo.take_echo(second), "a surviving entry was evicted");
        assert!(echo.take_echo(third), "a surviving entry was evicted");
    }

    #[test]
    fn recording_an_entry_again_moves_it_to_the_back_of_the_eviction_order() {
        // An editor's autosave after an undo re-records the same hash. It must not
        // count as a second entry against the capacity, and — since it is the more
        // recently written note — it should be the last one evicted, not the first.
        let echo = EchoSuppressor::with_capacity(2);
        let repeated = hash_bytes(b"same bytes");
        let other = hash_bytes(b"other");

        echo.record(repeated);
        echo.record(other);
        echo.record(repeated); // refresh: repeated is now the most recent
        let newcomer = hash_bytes(b"newcomer");
        echo.record(newcomer); // must evict `other`, the true oldest, not `repeated`

        assert_eq!(echo.pending(), 2);
        assert!(
            !echo.take_echo(other),
            "the refreshed entry's original slot was evicted instead of it"
        );
        assert!(echo.take_echo(repeated));
        assert!(echo.take_echo(newcomer));
    }

    #[test]
    fn recording_the_same_hash_twice_still_suppresses_only_one_event() {
        // Saving the same bytes twice is ordinary — an editor's autosave after an
        // undo. Each write records its own entry, and each event consumes one.
        let echo = EchoSuppressor::new();
        let hash = hash_bytes(b"same bytes");
        echo.record(hash);
        echo.record(hash);
        // Keyed by hash, so the second record refreshes the first rather than
        // stacking: recorded once, suppressed once, and the second event is treated
        // as a real change that the hash gate then discards for the cost of a hash.
        assert!(echo.take_echo(hash));
        assert!(!echo.take_echo(hash));
    }

    #[test]
    fn it_is_shareable_across_threads() {
        let echo = std::sync::Arc::new(EchoSuppressor::new());
        let writer = std::sync::Arc::clone(&echo);
        let handle = std::thread::spawn(move || {
            for i in 0..100u8 {
                writer.record(hash_bytes(&[i]));
            }
        });
        handle.join().expect("writer thread");
        assert_eq!(echo.pending(), 100);
        assert!(echo.take_echo(hash_bytes(&[7])));
    }
}
