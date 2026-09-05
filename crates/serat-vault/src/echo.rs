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

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serat_core::ContentHash;

/// How long a recorded hash stays interesting.
///
/// Long enough to cover the gap between our write and the watcher's coalescing
/// window (150 ms) plus the platform's own latency; short enough that a hash cannot
/// linger and swallow an unrelated edit made a minute later. Two seconds is what
/// §3.2 specifies.
pub const ECHO_LIFETIME: Duration = Duration::from_secs(2);

/// The short-lived set of hashes we are about to produce.
///
/// Cheap to share: every method takes `&self`, so the writer and the watcher hold the
/// same one without either owning it.
#[derive(Debug)]
pub struct EchoSuppressor {
    entries: Mutex<HashMap<ContentHash, Instant>>,
    lifetime: Duration,
}

impl Default for EchoSuppressor {
    fn default() -> Self {
        Self::new()
    }
}

impl EchoSuppressor {
    /// A suppressor with the standard two-second lifetime.
    #[must_use]
    pub fn new() -> Self {
        Self::with_lifetime(ECHO_LIFETIME)
    }

    /// A suppressor with a custom lifetime, for tests.
    #[must_use]
    pub fn with_lifetime(lifetime: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            lifetime,
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
    pub fn record(&self, hash: ContentHash) {
        let now = Instant::now();
        let mut entries = self.lock();
        entries.retain(|_, recorded| now.duration_since(*recorded) < self.lifetime);
        entries.insert(hash, now);
    }

    /// Whether an event carrying these bytes is our own echo, consuming the entry if
    /// it is.
    ///
    /// Consuming matters: two identical writes produce two events, and one recorded
    /// hash must suppress exactly one of them.
    pub fn take_echo(&self, hash: ContentHash) -> bool {
        let now = Instant::now();
        let mut entries = self.lock();
        entries.retain(|_, recorded| now.duration_since(*recorded) < self.lifetime);
        entries.remove(&hash).is_some()
    }

    /// How many hashes are currently being waited on. Diagnostics only.
    #[must_use]
    pub fn pending(&self) -> usize {
        let now = Instant::now();
        let mut entries = self.lock();
        entries.retain(|_, recorded| now.duration_since(*recorded) < self.lifetime);
        entries.len()
    }

    /// Recover from a poisoned mutex rather than propagating the panic.
    ///
    /// This holds no invariant worth protecting: it is a cache of hints, and the
    /// worst outcome of stale contents is one unnecessary reindex. Refusing to
    /// suppress echoes for the rest of the session would be the more expensive
    /// failure, and taking down the watcher thread over it more expensive still.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ContentHash, Instant>> {
        self.entries.lock().unwrap_or_else(|poisoned| {
            let mut guard = poisoned.into_inner();
            guard.clear();
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
    fn entries_expire_so_a_stale_hash_cannot_swallow_a_later_edit() {
        let echo = EchoSuppressor::with_lifetime(Duration::from_millis(1));
        let hash = hash_bytes(b"body");
        echo.record(hash);
        std::thread::sleep(Duration::from_millis(10));
        assert!(!echo.take_echo(hash), "an expired entry still suppressed");
        assert_eq!(echo.pending(), 0);
    }

    #[test]
    fn recording_the_same_hash_twice_still_suppresses_only_one_event() {
        // Saving the same bytes twice is ordinary — an editor's autosave after an
        // undo. Each write records its own entry, and each event consumes one.
        let echo = EchoSuppressor::new();
        let hash = hash_bytes(b"same bytes");
        echo.record(hash);
        echo.record(hash);
        // Keyed by hash, so the second record replaces the first rather than stacking:
        // recorded once, suppressed once, and the second event is treated as a real
        // change that the hash gate then discards for the cost of a hash.
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
