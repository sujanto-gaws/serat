//! The consumer of what changes behind Serat's back, and `index://changed`
//! (architecture section 3.1.2 steps 1-6; FR-VLT-04, NFR-REL-01, NFR-REL-02;
//! ADR-0011, ADR-0015 section 2).
//!
//! No domain logic. Every comparison, hash gate and submit is
//! [`serat_index::external::Reconciler`]'s; this module chooses which of its three entry
//! points a watcher output needs, and tells the front end when the index changed.
//!
//! | Watcher output | Entry point |
//! |---|---|
//! | the coalesced paths, drained as one batch | `index_paths` |
//! | `VaultEvent::DirChanged(d)` | `reconcile_dir` over `Vault::walk_dir(d)` |
//! | `RescanRequired`, or an overflow ticket | `catch_up(.., PathsAndMtime)` over `Vault::walk()` |
//! | open (warm path) | `catch_up(.., after_unclean_shutdown)` over the tree walk's entries |
//!
//! Every submit is on ADR-0011's bulk side (`submit_blocking`, inside the reconciler),
//! and every call here runs on the vault thread or on the open's catch-up thread, never
//! on the webview thread or the save path.
//!
//! # Locks
//!
//! The consumer takes leaf locks only and never blocks on application state
//! (architecture 2.2): the handle slot's mutex (held to build a `Reconciler`, which is
//! two clones, and released before any work), [`ChangeNotifier`]'s mutex, and the
//! journal's. None is held across another, across I/O or across a wait.
//!
//! # The journal, and the one race it closes
//!
//! The open's catch-up compares a walk with the rows the index holds *after* the walk.
//! A file created while the walk was running, and indexed by the watcher's batch before
//! the comparison read its rows, is in the rows and not in the walk: the catch-up would
//! remove it, and nothing would ever mention it again. So from the moment the walk
//! starts until the catch-up has finished, every path and directory the consumer
//! handles is also journaled, and the catch-up replays the journal through the same
//! hash-gated entry points when it is done. The replay reads the disk as it is now, so
//! a file that was created is re-added and a file that was deleted is removed again.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serat_core::{IndexError, SourceEntry, VaultPath};
use serat_index::external::{CatchUp, ChangeSignal, Outcome, Reconciler};
use serat_parse::CommonMarkParser;
use serat_vault::Vault;

use crate::index::IndexHandleSlot;
use crate::ipc::IndexChanged;
use crate::state::Events;

/// Paths held while the index has no handle, before the consumer gives up listing them
/// and asks for a catch-up instead.
const DEFERRED_CAP: usize = 4096;

/// `index://changed`, throttled (architecture 3.1 step 6) and numbered.
///
/// One per open vault. [`ChangeNotifier::changed`] is called by the save path after a
/// submit and by the consumer after a batch that changed the index; the consumer's tick
/// calls [`ChangeNotifier::poll`] for the trailing edge.
///
/// The event is emitted while the notifier's lock is held, so generations reach the
/// sink in order. The sink is `AppHandle::emit`, which queues and returns, and takes no
/// lock of this crate's.
pub struct ChangeNotifier {
    vault_id: String,
    events: Arc<Events>,
    inner: Mutex<NotifierState>,
    observer: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>>,
}

struct NotifierState {
    signal: ChangeSignal,
    closed: bool,
}

impl std::fmt::Debug for ChangeNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChangeNotifier")
            .field("vault_id", &self.vault_id)
            .finish_non_exhaustive()
    }
}

impl ChangeNotifier {
    /// A notifier for `vault_id` emitting through `events`.
    #[must_use]
    pub fn new(vault_id: String, events: Arc<Events>) -> Self {
        Self::with_interval(vault_id, events, serat_index::external::CHANGE_INTERVAL)
    }

    /// As [`ChangeNotifier::new`] with a chosen interval. For tests.
    #[must_use]
    pub fn with_interval(vault_id: String, events: Arc<Events>, interval: Duration) -> Self {
        Self {
            vault_id,
            events,
            inner: Mutex::new(NotifierState {
                signal: ChangeSignal::new(interval),
                closed: false,
            }),
            observer: std::sync::OnceLock::new(),
        }
    }

    /// Call `observer` after every change, before the throttle: the switcher rebuilds
    /// its corpus 500 ms after the *last* change, which is not the last emission.
    /// Installed once; a second call is ignored.
    pub fn on_change(&self, observer: impl Fn() + Send + Sync + 'static) {
        let _ = self.observer.set(Box::new(observer));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, NotifierState> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn emit(&self, state: &NotifierState, generation: Option<u64>) {
        if let Some(generation) = generation
            && !state.closed
        {
            self.events.emit_changed(&IndexChanged {
                vault_id: self.vault_id.clone(),
                generation,
            });
        }
    }

    /// The index changed: emit now if the interval allows, else on a later
    /// [`ChangeNotifier::poll`].
    pub fn changed(&self) {
        {
            let mut state = self.lock();
            let generation = state.signal.changed(Instant::now());
            self.emit(&state, generation);
        }
        if let Some(observer) = self.observer.get() {
            observer();
        }
    }

    /// Emit the trailing edge if one is owed, and say when to look again.
    pub fn poll(&self, now: Instant) -> Option<Instant> {
        let mut state = self.lock();
        let generation = state.signal.due(now);
        self.emit(&state, generation);
        state.signal.next_due()
    }

    /// The current generation, for a front end that polls.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.lock().signal.generation()
    }

    /// Emit nothing further: the vault is closing, and a stale thread (the catch-up)
    /// may still be finishing.
    pub fn close(&self) {
        self.lock().closed = true;
    }
}

#[derive(Debug, Default)]
struct Deferred {
    paths: BTreeSet<VaultPath>,
    rescan: bool,
}

#[derive(Debug, Default)]
struct Journal {
    paths: BTreeSet<VaultPath>,
    dirs: BTreeSet<VaultPath>,
}

impl Journal {
    fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.dirs.is_empty()
    }
}

/// Brings the index into agreement with the disk, for one open vault.
pub struct Indexer {
    vault: Arc<Vault>,
    slot: IndexHandleSlot,
    notifier: Arc<ChangeNotifier>,
    deferred: Mutex<Deferred>,
    journal: Mutex<Option<Journal>>,
}

impl std::fmt::Debug for Indexer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Indexer").finish_non_exhaustive()
    }
}

impl Indexer {
    /// An indexer over `slot`, the place the index actor puts its handle.
    #[must_use]
    pub fn new(vault: Arc<Vault>, slot: IndexHandleSlot, notifier: Arc<ChangeNotifier>) -> Self {
        Self {
            vault,
            slot,
            notifier,
            deferred: Mutex::new(Deferred::default()),
            journal: Mutex::new(None),
        }
    }

    /// The change notifier, shared with the save path.
    #[must_use]
    pub fn notifier(&self) -> &Arc<ChangeNotifier> {
        &self.notifier
    }

    fn reconciler(&self) -> Option<Reconciler> {
        let guard = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        guard.as_ref().map(Reconciler::new)
    }

    /// Whether the previous session ended uncleanly, as the open index reports it.
    fn unclean(&self) -> bool {
        self.slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(serat_index::IndexHandle::opened_after_unclean_shutdown)
    }

    fn deferred(&self) -> std::sync::MutexGuard<'_, Deferred> {
        self.deferred.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn journal(&self) -> std::sync::MutexGuard<'_, Option<Journal>> {
        self.journal.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What a reconciliation did: tell the front end, or remember that the index may
    /// now disagree with the disk.
    fn settle(&self, result: Result<Outcome, IndexError>) {
        match result {
            Ok(outcome) => {
                if outcome.changed() {
                    self.notifier.changed();
                }
            }
            // Partly submitted: the only safe reading is that a catch-up is owed.
            Err(_) => self.deferred().rescan = true,
        }
    }

    /// One drained batch of watcher paths (architecture 3.1.2 step 1).
    pub fn index_paths(&self, paths: &[VaultPath]) {
        if paths.is_empty() {
            return;
        }
        if let Some(journal) = self.journal().as_mut() {
            journal.paths.extend(paths.iter().cloned());
        }
        let Some(reconciler) = self.reconciler() else {
            self.defer(paths);
            return;
        };
        let parser = CommonMarkParser::new();
        let vault = &*self.vault;
        self.settle(reconciler.index_paths(
            vault,
            &parser,
            &|path, hash| vault.echo().take_echo(path, hash),
            paths,
        ));
    }

    /// `VaultEvent::DirChanged(dir)`: one subtree.
    pub fn dir_changed(&self, dir: &VaultPath) {
        if let Some(journal) = self.journal().as_mut() {
            journal.dirs.insert(dir.clone());
        }
        let Some(reconciler) = self.reconciler() else {
            self.deferred().rescan = true;
            return;
        };
        self.reconcile_dir(&reconciler, dir);
    }

    fn reconcile_dir(&self, reconciler: &Reconciler, dir: &VaultPath) {
        let entries = match self.vault.walk_dir(dir) {
            Ok(entries) => entries,
            Err(_) => {
                self.deferred().rescan = true;
                return;
            }
        };
        let parser = CommonMarkParser::new();
        self.settle(reconciler.reconcile_dir(&*self.vault, &parser, dir, &entries));
    }

    /// `RescanRequired` or an overflow: the whole vault, mtimes included.
    pub fn rescan(&self) {
        let Some(reconciler) = self.reconciler() else {
            self.deferred().rescan = true;
            return;
        };
        match self.vault.walk() {
            Ok(entries) => {
                let parser = CommonMarkParser::new();
                self.settle(reconciler.catch_up(
                    &*self.vault,
                    &parser,
                    &entries,
                    CatchUp::PathsAndMtime,
                ));
            }
            Err(_) => self.deferred().rescan = true,
        }
    }

    fn defer(&self, paths: &[VaultPath]) {
        let mut deferred = self.deferred();
        if deferred.rescan {
            return;
        }
        deferred.paths.extend(paths.iter().cloned());
        if deferred.paths.len() > DEFERRED_CAP {
            deferred.paths.clear();
            deferred.rescan = true;
        }
    }

    /// Work that arrived while the index had no handle (a rebuild was running), or
    /// that a failed reconciliation left owed. Called on the consumer's tick; a no-op
    /// until a handle exists.
    pub fn retry_deferred(&self) {
        {
            let deferred = self.deferred();
            if deferred.paths.is_empty() && !deferred.rescan {
                return;
            }
        }
        if self.reconciler().is_none() {
            return;
        }
        let (paths, rescan) = {
            let mut deferred = self.deferred();
            (
                std::mem::take(&mut deferred.paths),
                std::mem::take(&mut deferred.rescan),
            )
        };
        if rescan {
            self.rescan();
        } else {
            let paths: Vec<VaultPath> = paths.into_iter().collect();
            self.index_paths(&paths);
        }
    }

    /// The warm path's authoritative walk is about to start: journal what the consumer
    /// handles until the catch-up has finished.
    pub fn walk_started(&self) {
        *self.journal() = Some(Journal::default());
    }

    /// The open's catch-up (ADR-0015 section 2): `entries` are the tree walk's, sorted.
    /// Runs on the caller's thread; the application gives it one of its own. Replays
    /// the journal when done (module note).
    pub fn catch_up(&self, entries: &[SourceEntry]) {
        let Some(reconciler) = self.reconciler() else {
            // No handle: the open produced none (Unavailable) or a rebuild is running,
            // and the rebuild is the reconciliation.
            *self.journal() = None;
            return;
        };
        let parser = CommonMarkParser::new();
        let mode = CatchUp::after_unclean_shutdown(self.unclean());
        self.settle(reconciler.catch_up(&*self.vault, &parser, entries, mode));
        self.replay_journal(&reconciler);
    }

    fn replay_journal(&self, reconciler: &Reconciler) {
        loop {
            let batch = {
                let mut guard = self.journal();
                match guard.as_mut() {
                    Some(journal) if !journal.is_empty() => std::mem::take(journal),
                    _ => {
                        *guard = None;
                        return;
                    }
                }
            };
            for dir in &batch.dirs {
                self.reconcile_dir(reconciler, dir);
            }
            let paths: Vec<VaultPath> = batch.paths.into_iter().collect();
            let parser = CommonMarkParser::new();
            let vault = &*self.vault;
            self.settle(reconciler.index_paths(vault, &parser, &|_, _| false, &paths));
        }
    }

    /// The consumer's tick: owed work, then the trailing `index://changed`. Returns when
    /// the tick next has something to do.
    pub fn tick(&self, now: Instant) -> Option<Instant> {
        self.retry_deferred();
        self.notifier.poll(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serat_index::IndexHandle;

    fn vault_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("serat-consumer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture");
        std::fs::write(dir.join("a.md"), "# a\n").expect("fixture");
        std::fs::write(dir.join("b.md"), "# b\n").expect("fixture");
        dir
    }

    fn hash_of(dir: &std::path::Path, path: &str) -> Option<String> {
        let conn = rusqlite::Connection::open(dir.join(".serat").join("index.db")).ok()?;
        conn.query_row("SELECT hash FROM notes WHERE path = ?1", [path], |row| {
            row.get(0)
        })
        .ok()
    }

    fn slot_with(dir: &std::path::Path) -> IndexHandleSlot {
        let handle = IndexHandle::open(&dir.join(".serat")).expect("index");
        Arc::new(Mutex::new(Some(handle)))
    }

    fn indexer(dir: &std::path::Path, slot: IndexHandleSlot) -> (Indexer, Arc<Mutex<Vec<u64>>>) {
        let events = Arc::new(Events::default());
        let seen: Arc<Mutex<Vec<u64>>> = Arc::default();
        let sink = Arc::clone(&seen);
        events.install_changed(move |c| sink.lock().expect("seen").push(c.generation));
        let notifier = Arc::new(ChangeNotifier::with_interval(
            "v1".to_owned(),
            events,
            Duration::ZERO,
        ));
        let vault = Arc::new(Vault::open(dir).expect("vault"));
        (Indexer::new(vault, slot, notifier), seen)
    }

    fn walk(indexer: &Indexer) -> Vec<SourceEntry> {
        indexer.vault.walk().expect("walk")
    }

    fn p(path: &str) -> VaultPath {
        VaultPath::new(path).expect("path")
    }

    #[test]
    fn the_notifier_emits_the_leading_edge_collapses_a_burst_and_never_loses_the_last() {
        let events = Arc::new(Events::default());
        let seen: Arc<Mutex<Vec<u64>>> = Arc::default();
        let sink = Arc::clone(&seen);
        events.install_changed(move |c| sink.lock().expect("seen").push(c.generation));
        let interval = Duration::from_secs(3600);
        let notifier = ChangeNotifier::with_interval("v1".to_owned(), events, interval);

        notifier.changed();
        notifier.changed();
        notifier.changed();
        assert_eq!(
            *seen.lock().expect("seen"),
            [1],
            "leading edge, then collapsed"
        );
        let due = notifier
            .poll(Instant::now())
            .expect("a trailing edge is owed");
        assert_eq!(*seen.lock().expect("seen"), [1], "not before the interval");
        assert!(notifier.poll(due).is_none());
        assert_eq!(
            *seen.lock().expect("seen"),
            [1, 3],
            "the final change is not lost"
        );
        assert_eq!(notifier.generation(), 3);

        notifier.close();
        notifier.changed();
        assert_eq!(
            *seen.lock().expect("seen"),
            [1, 3],
            "a closed vault emits nothing"
        );
    }

    #[test]
    fn a_batch_is_indexed_once_and_announced_only_when_it_changed_the_index() {
        let dir = vault_dir("batch");
        let (indexer, seen) = indexer(&dir, slot_with(&dir));
        indexer.catch_up(&walk(&indexer));
        assert!(hash_of(&dir, "a.md").is_some());
        let announced = seen.lock().expect("seen").len();

        // Nothing changed: the hash gate discards it and nothing is announced.
        indexer.index_paths(&[p("a.md"), p("b.md")]);
        assert_eq!(seen.lock().expect("seen").len(), announced);

        std::fs::write(dir.join("a.md"), "# a, edited\n").expect("edit");
        std::fs::remove_file(dir.join("b.md")).expect("delete");
        indexer.index_paths(&[p("a.md"), p("b.md"), p("never-existed.md")]);
        assert!(hash_of(&dir, "b.md").is_none(), "the delete was applied");
        assert!(seen.lock().expect("seen").len() > announced);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn our_own_echo_is_not_parsed_or_announced_again() {
        let dir = vault_dir("echo");
        let (indexer, seen) = indexer(&dir, slot_with(&dir));
        indexer.catch_up(&walk(&indexer));
        let announced = seen.lock().expect("seen").len();

        std::fs::write(dir.join("a.md"), "# a, saved by us\n").expect("edit");
        let hash = indexer.vault.read(&p("a.md")).expect("read").meta.hash;
        // The save path recorded this (path, hash) after its own submit.
        indexer.vault.echo().record(&p("a.md"), hash);
        indexer.index_paths(&[p("a.md")]);
        assert_eq!(
            seen.lock().expect("seen").len(),
            announced,
            "the echo was consumed"
        );
        assert!(
            hash_of(&dir, "a.md").is_none_or(|h| h != hash.to_hex()),
            "nothing was submitted for the echo"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn work_that_arrives_before_the_index_has_a_handle_is_kept_not_dropped() {
        let dir = vault_dir("deferred");
        let slot: IndexHandleSlot = Arc::new(Mutex::new(None));
        let (indexer, _) = indexer(&dir, Arc::clone(&slot));
        indexer.index_paths(&[p("a.md")]);
        indexer.dir_changed(&p("sub"));
        indexer.retry_deferred(); // still no handle: a no-op
        assert!(!dir.join(".serat").exists() || hash_of(&dir, "a.md").is_none());

        *slot.lock().expect("slot") = Some(IndexHandle::open(&dir.join(".serat")).expect("index"));
        indexer.tick(Instant::now());
        assert!(
            hash_of(&dir, "a.md").is_some(),
            "the owed work ran on the next tick"
        );
        assert!(
            hash_of(&dir, "b.md").is_some(),
            "a directory event owes a catch-up"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rescan_walks_and_compares_mtimes() {
        let dir = vault_dir("rescan");
        let (indexer, _) = indexer(&dir, slot_with(&dir));
        indexer.catch_up(&walk(&indexer));
        let before = hash_of(&dir, "a.md").expect("indexed");
        std::fs::write(dir.join("a.md"), "# a, changed while events were dropped\n").expect("edit");
        std::fs::write(dir.join("c.md"), "# c\n").expect("new");
        indexer.rescan();
        assert_ne!(hash_of(&dir, "a.md").expect("indexed"), before);
        assert!(hash_of(&dir, "c.md").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_created_during_the_walk_survives_the_catch_up() {
        // The race the journal closes: the walk predates the file, the watcher's batch
        // indexes it, and the catch-up then sees a row the walk lacks.
        let dir = vault_dir("journal");
        let (indexer, _) = indexer(&dir, slot_with(&dir));
        indexer.walk_started();
        let stale_walk = walk(&indexer);

        std::fs::write(dir.join("created-during.md"), "# created during the walk\n").expect("new");
        indexer.index_paths(&[p("created-during.md")]);
        assert!(hash_of(&dir, "created-during.md").is_some());

        indexer.catch_up(&stale_walk);
        assert!(
            hash_of(&dir, "created-during.md").is_some(),
            "the catch-up removed a file the watcher had just indexed"
        );
        assert!(
            indexer.journal().is_none(),
            "the journal ends with the catch-up"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clean_shutdown_compares_paths_and_an_unclean_one_compares_mtimes() {
        let dir = vault_dir("modes");
        let index_dir = dir.join(".serat");
        {
            let (indexer, _) = indexer(&dir, slot_with(&dir));
            indexer.catch_up(&walk(&indexer));
            let handle = indexer.slot.lock().expect("slot").take().expect("handle");
            handle.close().expect("clean close");
        }
        let before = hash_of(&dir, "a.md").expect("indexed");
        std::fs::write(dir.join("a.md"), "# a, edited while closed\n").expect("edit");

        // Clean: the marker is set, so the content comparison is skipped.
        let (indexer, _) = indexer(&dir, slot_with(&dir));
        assert!(!indexer.unclean());
        indexer.catch_up(&walk(&indexer));
        assert_eq!(hash_of(&dir, "a.md").expect("row"), before);

        // Unclean: the handle was opened and dropped without close().
        drop(indexer.slot.lock().expect("slot").take());
        let handle = IndexHandle::open(&index_dir).expect("index");
        drop(handle);
        let (indexer, _) = indexer_for(&dir);
        assert!(indexer.unclean());
        indexer.catch_up(&walk(&indexer));
        assert_ne!(hash_of(&dir, "a.md").expect("row"), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn indexer_for(dir: &std::path::Path) -> (Indexer, Arc<Mutex<Vec<u64>>>) {
        indexer(dir, slot_with(dir))
    }
}
