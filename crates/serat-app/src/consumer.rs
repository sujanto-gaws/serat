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
//! | open (index `Ready`, not rebuilt) | `catch_up(.., PathsAndMtime)` over the open's one walk |
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
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serat_core::{IndexError, LoadedNote, NoteSource, SourceEntry, VaultError, VaultPath};
use serat_index::external::{CatchUp, ChangeSignal, Outcome, Reconciler};
use serat_parse::CommonMarkParser;
use serat_vault::Vault;

use crate::index::IndexHandleSlot;
use crate::ipc::{IndexChanged, IndexState};
use crate::state::Events;
use crate::vault::Latch;

/// Paths held while the index has no handle, before the consumer gives up listing them
/// and asks for a catch-up instead.
const DEFERRED_CAP: usize = 4096;

/// `index://changed`, throttled (architecture 3.1 step 6) and numbered.
///
/// One per open vault. [`ChangeNotifier::changed`] is called by the writer's commit
/// observer ([`ChangeNotifier::commit_observer`]) after every non-bulk committed batch,
/// whoever submitted it (the save path, the consumer, the catch-up), and once by a
/// runtime rebuild when it completes; the consumer's tick calls [`ChangeNotifier::poll`]
/// for the trailing edge. Nothing else announces (ADR-0020 decision 3), so an event
/// carrying a generation never precedes the commit it reports.
///
/// **Lock invariant (ADR-0020 decision 3, N3).** The observer runs on the index writer's
/// thread, so everything it takes must never be held by a thread that waits on the
/// writer. It takes, in this order and never nested the other way: the notifier's own
/// mutex (held across `Events::emit_changed`, whose sink only queues and takes a
/// read lock on the sink table), released; then the [`Switcher`](crate::switcher::Switcher)'s
/// work mutex inside `on_change`'s `index_changed` (held for one store). So no holder of
/// the notifier mutex or the switcher's work mutex may call `flush`, `close`,
/// `close_clean`, `submit_blocking` or join the writer while holding it. Every such call
/// in this crate (`OpenVault::shut_down`'s `handle.close()`, `rebuild_open_index`'s
/// `rebuild_in_place`, the reconciler's own flush inside `Consumer::index_paths`) is made
/// with neither held; `the_announcing_path_never_waits_on_the_writer` pins it.
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

    /// The observer to install on the vault's index writer: every non-bulk committed
    /// batch calls [`ChangeNotifier::changed`], after `COMMIT`. Weak, so the writer
    /// thread does not keep the notifier alive past its vault. After
    /// [`ChangeNotifier::close`] it emits nothing, which is how the last save's
    /// post-commit event is suppressed at close.
    #[must_use]
    pub fn commit_observer(self: &Arc<Self>) -> serat_index::CommitObserver {
        let weak = Arc::downgrade(self);
        serat_index::CommitObserver::new(move |_batch| {
            if let Some(notifier) = weak.upgrade() {
                notifier.changed();
            }
        })
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

    /// Whether [`ChangeNotifier::close`] has run. Tests.
    #[cfg(test)]
    pub(crate) fn is_closed(&self) -> bool {
        self.lock().closed
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
    /// A `RescanRequired` (or an overflow of this journal) arrived inside the window:
    /// the replay re-runs the whole-vault rescan instead of listing anything.
    rescan: bool,
}

impl Journal {
    fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.dirs.is_empty() && !self.rescan
    }

    /// Cap like [`DEFERRED_CAP`]: a walk that never finishes cannot grow this without
    /// bound, and an overflow is a full rescan in the replay.
    fn add_paths(&mut self, paths: &[VaultPath]) {
        if self.rescan {
            return;
        }
        self.paths.extend(paths.iter().cloned());
        self.enforce_cap();
    }

    fn add_dir(&mut self, dir: &VaultPath) {
        if self.rescan {
            return;
        }
        self.dirs.insert(dir.clone());
        self.enforce_cap();
    }

    fn enforce_cap(&mut self) {
        if self.paths.len() + self.dirs.len() > DEFERRED_CAP {
            self.paths.clear();
            self.dirs.clear();
            self.rescan = true;
        }
    }
}

/// The vault as the reconciler's note source, refusing reads once the vault is closing:
/// a catch-up over 50,000 files then finishes in the time it takes to skip them, rather
/// than holding the close (a skipped file keeps its row; see `Reconciler`). The error is
/// deliberately not `NotFound`, which would remove the row.
struct Stoppable<'a> {
    vault: &'a Vault,
    stop: &'a AtomicBool,
}

impl NoteSource for Stoppable<'_> {
    fn entries(&self) -> Result<Vec<SourceEntry>, VaultError> {
        self.vault.entries()
    }

    fn load(&self, path: &VaultPath) -> Result<LoadedNote, VaultError> {
        if self.stop.load(Ordering::Relaxed) {
            return Err(VaultError::Io {
                path: path.to_string(),
                message: "the vault is closing".to_owned(),
            });
        }
        self.vault.load(path)
    }
}

/// Decides, once per open vault, whether the open's catch-up runs and which walk feeds
/// it (ADR-0015 section 2, FR-VLT-04). Owned by the application, not by `open_tree`:
/// a vault whose tree nobody has opened still reconciles what changed while it was
/// closed, and a vault whose tree was opened while the index was still `Opening` does
/// too.
///
/// Whoever wants the walk calls [`OpenCatchUp::claim`]: it waits, with a bound, for the
/// index to settle, and grants the walk at most once, and only if the index is `Ready`
/// and was not rebuilt this session (a rebuild is itself the reconciliation). A stream
/// that has been opened ([`OpenCatchUp::attach_tree`]) takes precedence over the
/// application's own trigger, so one walk serves both the tree and the catch-up; a
/// later `open_tree` (a reload) or a restarted session is refused the claim and walks
/// name-and-kind only, with no journal and no second catch-up.
#[derive(Debug)]
pub struct OpenCatchUp {
    settled: Latch,
    /// Set once the vault's index actor exists. The settle clock does not start before
    /// it: an open that had to wait for a detached actor (task #96) publishes nothing
    /// for however long that takes, and a clock running over the wait would give the
    /// open's catch-up up for good.
    actor_started: Latch,
    /// Called when a claim is refused because the index never settled: the catch-up
    /// that did not run is owed as a rescan instead of being dropped.
    on_unsettled: Mutex<Option<Owe>>,
    bound: Duration,
    grace: Duration,
    state: Mutex<Claim>,
    /// Signalled when a tree attaches or the vault is cancelled.
    attached: std::sync::Condvar,
}

/// The unsettled callback, wrapped so `OpenCatchUp` keeps its `Debug`.
struct Owe(Box<dyn Fn() + Send + Sync>);

impl std::fmt::Debug for Owe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Owe")
    }
}

#[derive(Debug, Default)]
struct Claim {
    ready: bool,
    rebuilt: bool,
    claimed: bool,
    tree_attached: bool,
    cancelled: bool,
    /// The trigger refused itself because a tree was attached.
    stood_aside: bool,
}

/// How long a claimant waits for the index to settle.
pub const SETTLE_BOUND: Duration = Duration::from_secs(30);

/// How long the application's own trigger waits, after the gate and the index have
/// settled, for the front end's `open_tree` to arrive before walking on its own: the
/// window is visible at about the moment the gate opens, and a front end that mounts a
/// few hundred milliseconds later would otherwise cause a second enumeration. **When no
/// tree mounts it adds up to this long to `t0_to_walk_complete_ms`**; it is skipped when
/// the first paint never arrived, and when the index was rebuilt (nothing to claim).
pub const TREE_GRACE: Duration = Duration::from_millis(750);

impl OpenCatchUp {
    /// A coordinator that waits at most `bound` for the index to settle, and the
    /// application's own trigger at most `grace` more for a tree to attach.
    #[must_use]
    pub fn new(bound: Duration, grace: Duration) -> Self {
        let this = Self::build(bound, grace);
        this.actor_started.set();
        this
    }

    fn build(bound: Duration, grace: Duration) -> Self {
        let actor_started = Latch::default();
        Self {
            settled: Latch::default(),
            actor_started,
            on_unsettled: Mutex::new(None),
            bound,
            grace,
            state: Mutex::new(Claim::default()),
            attached: std::sync::Condvar::new(),
        }
    }

    /// As [`OpenCatchUp::new`], for an open whose index actor does not exist yet: no
    /// claimant starts its settle clock until [`OpenCatchUp::actor_started`] (or a
    /// cancel).
    #[must_use]
    pub fn awaiting_actor(bound: Duration, grace: Duration) -> Self {
        Self::build(bound, grace)
    }

    /// Run the unsettled callback: the open's catch-up did not happen.
    fn owe(&self) {
        if let Some(owe) = self
            .on_unsettled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            (owe.0)();
        }
    }

    /// The index actor now exists; claimants' settle clocks start.
    pub fn actor_started(&self) {
        self.actor_started.set();
    }

    /// What to do when a claim gives up because the index never settled.
    pub fn on_unsettled(&self, f: impl Fn() + Send + Sync + 'static) {
        *self
            .on_unsettled
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Owe(Box::new(f)));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Claim> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every state the publisher publishes.
    pub fn index_published(&self, state: &IndexState) {
        {
            let mut claim = self.lock();
            match state {
                IndexState::Ready { .. } => claim.ready = true,
                IndexState::Rebuilding { .. } => {
                    claim.ready = false;
                    claim.rebuilt = true;
                }
                _ => {}
            }
        }
        if !matches!(state, IndexState::Opening { .. }) {
            self.settled.set();
        }
    }

    /// The vault is closing: wake every claimant and grant nothing further.
    pub fn cancel(&self) {
        self.lock().cancelled = true;
        self.actor_started.set();
        self.settled.set();
        self.attached.notify_all();
    }

    /// `open_tree` was called: the tree's walk will ask for the claim itself.
    pub fn attach_tree(&self) {
        self.lock().tree_attached = true;
        self.attached.notify_all();
    }

    /// `open_tree` failed to start its thread after [`OpenCatchUp::attach_tree`]: the
    /// trigger may walk after all (if it has not already stood aside, see `claim`).
    pub fn detach_tree(&self) {
        let mut claim = self.lock();
        claim.tree_attached = false;
        if claim.stood_aside && !claim.claimed {
            eprintln!(
                "serat: the tree's thread could not start after the open's trigger stood \
                 aside; the open's catch-up is owed to the next open"
            );
        }
    }

    /// Whether a tree stream is attached.
    #[must_use]
    pub fn tree_attached(&self) -> bool {
        self.lock().tree_attached
    }

    /// Whether the caller's walk is the open's one authoritative walk. `from_tree` is
    /// the tree's own request; the other caller is the application's trigger, which
    /// stands aside for a tree.
    #[must_use]
    pub fn claim(&self, from_tree: bool) -> bool {
        self.claim_inner(from_tree, false, false)
    }

    /// The application's own trigger's claim. `painted` is whether the first paint
    /// arrived (no paint, no tree to wait for); `bench_walk` is the benchmark's need for
    /// a walk even when the index was rebuilt. The grace for a tree is spent only when
    /// there is something to claim (or the benchmark needs the walk), and only after a
    /// paint.
    #[must_use]
    pub fn claim_standalone(&self, painted: bool, bench_walk: bool) -> bool {
        self.claim_inner(false, painted, bench_walk)
    }

    fn claim_inner(&self, from_tree: bool, wait_for_tree: bool, bench_walk: bool) -> bool {
        // The tree never waits for an actor that does not exist yet (FR-VLT-01's 2 s,
        // goal 5): it is refused at once and walks name-and-kind only, and the
        // catch-up it would have fed is owed as a rescan.
        if from_tree && !self.actor_started.is_set() {
            self.owe();
            return false;
        }
        // The application's own trigger runs on a thread of its own and may wait: this
        // ends when the actor is spawned, when its spawn failed (the caller starts it
        // regardless) or at the vault's cancel.
        while !self.actor_started.wait(Duration::from_secs(1)) {}
        if !self.settled.wait(self.bound) {
            eprintln!(
                "serat: the index had not settled after {:?}; the open's catch-up is owed \
                 as a rescan",
                self.bound
            );
            self.owe();
            return false;
        }
        let mut claim = self.lock();
        if !from_tree
            && wait_for_tree
            && !claim.tree_attached
            && !claim.cancelled
            && ((claim.ready && !claim.rebuilt) || bench_walk)
        {
            // The grace window: the front end's tree, if it is coming, is coming now.
            claim = self
                .attached
                .wait_timeout_while(claim, self.grace, |c| !c.tree_attached && !c.cancelled)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        if !from_tree && claim.tree_attached {
            claim.stood_aside = true;
        }
        if claim.cancelled
            || claim.claimed
            || !claim.ready
            || claim.rebuilt
            || (!from_tree && claim.tree_attached)
        {
            return false;
        }
        claim.claimed = true;
        true
    }

    /// Whether the claim has been granted.
    #[must_use]
    pub fn claimed(&self) -> bool {
        self.lock().claimed
    }
}

/// On unwind out of the catch-up: close the journal and owe a rescan.
struct UnwindGuard<'a>(&'a Indexer);

impl Drop for UnwindGuard<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.walk_failed();
        }
    }
}

/// What runs once the open's catch-up has finished (see [`Indexer::on_caught_up`]).
type CaughtUpHook = Arc<dyn Fn(bool) + Send + Sync>;

/// Brings the index into agreement with the disk, for one open vault.
pub struct Indexer {
    vault: Arc<Vault>,
    slot: IndexHandleSlot,
    notifier: Arc<ChangeNotifier>,
    deferred: Mutex<Deferred>,
    journal: Mutex<Option<Journal>>,
    /// The vault is closing: reconciliation reads are refused ([`Stoppable`]).
    stop: AtomicBool,
    /// Wall-clock ms taken immediately before the session's watcher was armed (ADR-0015
    /// section 2 as amended; task #78); the open's catch-up records it as
    /// `caught_up_from_ms` when it completes. 0 until [`Indexer::window_opened`] has
    /// been called, which makes the next catch-up hash every file: conservative, never
    /// wrong.
    window_start_ms: AtomicI64,
    /// When the index writer was last given work (task #69: the background check waits for
    /// it to be idle).
    activity: Arc<crate::quick_check::Activity>,
    /// Called once the open's catch-up has finished, with whether the previous session ended
    /// uncleanly (task #69).
    after_catch_up: Mutex<Option<CaughtUpHook>>,
    /// The catch-up found no index handle yet and owes a rescan; the hook runs once that
    /// rescan has run (task #69), so an unclean open cannot skip its background check.
    hook_owed: AtomicBool,
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
            stop: AtomicBool::new(false),
            window_start_ms: AtomicI64::new(0),
            activity: Arc::new(crate::quick_check::Activity::new()),
            after_catch_up: Mutex::new(None),
            hook_owed: AtomicBool::new(false),
        }
    }

    /// The writer's activity clock, shared with the save path.
    #[must_use]
    pub fn activity(&self) -> &Arc<crate::quick_check::Activity> {
        &self.activity
    }

    /// Run `f` when the open's catch-up has finished, with whether the previous session
    /// ended uncleanly. One hook; a second call replaces the first.
    pub fn on_caught_up(&self, f: impl Fn(bool) + Send + Sync + 'static) {
        *self
            .after_catch_up
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(f));
    }

    /// The vault is closing: abandon work in progress, including the detached catch-up
    /// thread, and start none.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Whether the vault is closing.
    #[must_use]
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn source(&self) -> Stoppable<'_> {
        Stoppable {
            vault: &self.vault,
            stop: &self.stop,
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
            // Nothing is announced here (ADR-0020 decision 3): a reconciliation that
            // changed the index did so through a committed batch, and the writer's commit
            // observer announced it after that commit.
            Ok(_) => {}
            // Partly submitted: the only safe reading is that a catch-up is owed.
            Err(_) => self.deferred().rescan = true,
        }
    }

    /// One drained batch of watcher paths (architecture 3.1.2 step 1).
    pub fn index_paths(&self, paths: &[VaultPath]) {
        if paths.is_empty() || self.stopped() {
            return;
        }
        self.activity.touch();
        if let Some(journal) = self.journal().as_mut() {
            journal.add_paths(paths);
        }
        let Some(reconciler) = self.reconciler() else {
            self.defer(paths);
            return;
        };
        let parser = CommonMarkParser::new();
        let vault = &*self.vault;
        self.settle(reconciler.index_paths(
            &self.source(),
            &parser,
            &|path, hash| vault.echo().take_echo(path, hash),
            paths,
        ));
    }

    /// `VaultEvent::DirChanged(dir)`: one subtree.
    pub fn dir_changed(&self, dir: &VaultPath) {
        if self.stopped() {
            return;
        }
        self.activity.touch();
        if let Some(journal) = self.journal().as_mut() {
            journal.add_dir(dir);
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
        self.settle(reconciler.reconcile_dir(&self.source(), &parser, dir, &entries));
    }

    /// `RescanRequired` or an overflow: the whole vault, mtimes included.
    ///
    /// Inside the open's journal window it is journaled instead, and the replay runs it
    /// once the catch-up has finished: the catch-up's own comparison is what is in
    /// progress, and a rescan racing it would compare against rows it is still writing.
    pub fn rescan(&self) {
        if self.stopped() {
            return;
        }
        if let Some(journal) = self.journal().as_mut() {
            journal.rescan = true;
            return;
        }
        let Some(reconciler) = self.reconciler() else {
            self.deferred().rescan = true;
            return;
        };
        self.rescan_with(&reconciler);
    }

    fn rescan_with(&self, reconciler: &Reconciler) {
        match self.vault.walk() {
            Ok(entries) => {
                let parser = CommonMarkParser::new();
                self.settle(reconciler.catch_up(
                    &self.source(),
                    &parser,
                    &entries,
                    CatchUp::PathsAndMtime,
                    // Compare only: ADR-0015 section 2 lets the open-time catch-up and a
                    // finished rebuild move the window, nothing else.
                    None,
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
        // The catch-up that found no handle owed this rescan; now the handle is here, so the
        // background check it would have started is started (task #69).
        if self.hook_owed.swap(false, Ordering::SeqCst) {
            self.run_caught_up_hook(self.unclean());
        }
    }

    /// Records the open-time window start: the time taken immediately before the
    /// watcher is armed, which the open's catch-up hands to the reconciler (task #78).
    /// Call before any walk.
    pub fn window_opened(&self, ms: i64) {
        self.window_start_ms.store(ms, Ordering::SeqCst);
    }

    /// The open's authoritative walk is about to start: journal what the consumer
    /// handles until the catch-up has finished. The window start is not touched here:
    /// it predates the watcher ([`Indexer::window_opened`]).
    pub fn walk_started(&self) {
        *self.journal() = Some(Journal::default());
    }

    /// Whether a whole-vault rescan is owed. For tests.
    #[cfg(test)]
    pub(crate) fn rescan_owed(&self) -> bool {
        self.deferred().rescan
    }

    /// Record that the open's catch-up did not run and a rescan is owed.
    pub fn owe_rescan(&self) {
        self.deferred().rescan = true;
    }

    /// The walk that was to feed the catch-up failed or panicked: close the journal
    /// (it would otherwise grow for the rest of the session) and owe a whole-vault
    /// rescan, which is what the catch-up would have been.
    pub fn walk_failed(&self) {
        *self.journal() = None;
        self.deferred().rescan = true;
    }

    /// The open's catch-up (ADR-0015 section 2): `entries` are the tree walk's, sorted.
    /// Runs on the caller's thread; the application gives it one of its own. Replays
    /// the journal when done (module note).
    pub fn catch_up(&self, entries: &[SourceEntry]) {
        if self.stopped() {
            *self.journal() = None;
            return;
        }
        let Some(reconciler) = self.reconciler() else {
            // `Ready` is published before the actor's handle reaches the slot, and a
            // rebuild takes the handle out; either way the comparison is owed, and a
            // rescan is it. (The open's own rebuild never gets here: it is not claimed.)
            *self.journal() = None;
            self.deferred().rescan = true;
            self.hook_owed.store(true, Ordering::SeqCst);
            return;
        };
        // A panic below (the reconciler, the replay) must not leave the journal open for
        // the rest of the session, which would swallow every `RescanRequired`.
        let _unwind = UnwindGuard(self);
        self.activity.touch();
        let parser = CommonMarkParser::new();
        // The architect's ruling (ADR-0015 section 2): the open compares mtimes whatever
        // the last shutdown was. The marker only schedules the background quick_check (task
        // #69, `crate::quick_check`), started from the hook below once this catch-up is done.
        let unclean = self.unclean();
        if unclean {
            eprintln!("serat: the previous session did not close cleanly; comparing mtimes");
        }
        self.settle(reconciler.catch_up(
            &self.source(),
            &parser,
            entries,
            CatchUp::PathsAndMtime,
            Some(self.window_start_ms.load(Ordering::SeqCst)),
        ));
        self.replay_journal(&reconciler);
        self.activity.touch();
        self.run_caught_up_hook(unclean);
    }

    fn run_caught_up_hook(&self, unclean: bool) {
        let hook = self
            .after_catch_up
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(hook) = hook {
            hook(unclean);
        }
    }

    /// Replays what the consumer journaled while the walk and catch-up ran. A catch-up
    /// that was stopped mid-run (the vault is closing) skips the replay: whatever it
    /// left is found by the next open's catch-up, which compares every mtime, so the
    /// index self-heals (C-02).
    fn replay_journal(&self, reconciler: &Reconciler) {
        loop {
            let batch = {
                let mut guard = self.journal();
                match guard.as_mut() {
                    Some(journal) if !journal.is_empty() && !self.stopped() => {
                        std::mem::take(journal)
                    }
                    _ => {
                        *guard = None;
                        return;
                    }
                }
            };
            if batch.rescan {
                // Covers every path and directory in the batch.
                self.rescan_with(reconciler);
                continue;
            }
            for dir in &batch.dirs {
                self.reconcile_dir(reconciler, dir);
            }
            let paths: Vec<VaultPath> = batch.paths.into_iter().collect();
            let parser = CommonMarkParser::new();
            self.settle(reconciler.index_paths(&self.source(), &parser, &|_, _| false, &paths));
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
            crate::test_temp_dir().join(format!("serat-consumer-{name}-{}", std::process::id()));
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

    /// As [`indexer`], with the writer's commit observer driving the notifier, the way
    /// `open_vault` wires it (ADR-0020 decision 3): the only producer of `changed()`.
    fn observed_indexer(dir: &std::path::Path) -> (Indexer, Arc<Mutex<Vec<u64>>>) {
        let events = Arc::new(Events::default());
        let seen: Arc<Mutex<Vec<u64>>> = Arc::default();
        let sink = Arc::clone(&seen);
        events.install_changed(move |c| sink.lock().expect("seen").push(c.generation));
        let notifier = Arc::new(ChangeNotifier::with_interval(
            "v1".to_owned(),
            events,
            Duration::ZERO,
        ));
        let handle =
            IndexHandle::open_observed(&dir.join(".serat"), Some(notifier.commit_observer()))
                .expect("index");
        let vault = Arc::new(Vault::open(dir).expect("vault"));
        let slot: IndexHandleSlot = Arc::new(Mutex::new(Some(handle)));
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
        let (indexer, seen) = observed_indexer(&dir);
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
        let (indexer, seen) = observed_indexer(&dir);
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

    /// Session one stores `stored_window`; session two edits `a.md` (same size, mtime
    /// reset to `T`) and catches up. Returns whether the edit was picked up.
    fn edit_at_equal_mtime_is_picked_up(name: &str, stored_offset_ms: i64) -> bool {
        let dir = vault_dir(name);
        let path = dir.join("a.md");
        let set_mtime = |t: std::time::SystemTime| {
            std::fs::File::options()
                .write(true)
                .open(&path)
                .expect("open")
                .set_modified(t)
                .expect("set_modified");
        };
        let t = std::time::SystemTime::now() - Duration::from_secs(60);
        let t_ms = i64::try_from(
            t.duration_since(std::time::UNIX_EPOCH)
                .expect("epoch")
                .as_millis(),
        )
        .expect("ms");
        set_mtime(t);
        let (indexer, _) = indexer(&dir, slot_with(&dir));
        indexer.window_opened(t_ms + stored_offset_ms);
        indexer.walk_started();
        indexer.catch_up(&walk(&indexer));
        let before = hash_of(&dir, "a.md").expect("indexed");

        let mut bytes = std::fs::read(&path).expect("read");
        let at = bytes.len() - 2;
        bytes[at] = b'Z';
        std::fs::write(&path, &bytes).expect("rewrite");
        set_mtime(t);
        indexer.window_opened(serat_index::rebuild::now_ms());
        indexer.walk_started();
        indexer.catch_up(&walk(&indexer));
        let changed = hash_of(&dir, "a.md").expect("indexed") != before;
        let _ = std::fs::remove_dir_all(&dir);
        changed
    }

    #[test]
    fn a_same_size_same_mtime_edit_after_the_window_start_is_a_candidate() {
        // Task #78: the stored window decides. A window at the file's mtime keeps the
        // equal-mtime edit a candidate; the negative control, a window 5 s later, does
        // not (so the test fails if the stored value is wrong in either direction).
        assert!(edit_at_equal_mtime_is_picked_up("window-equal-mtime", 0));
        assert!(!edit_at_equal_mtime_is_picked_up(
            "window-equal-mtime-control",
            5_000
        ));
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

    // Task #69 review. Mutant, run by hand: dropping the `hook_owed` store leaves the hook
    // uncalled after the owed rescan, so an unclean open whose catch-up found no handle skips
    // its check.
    #[test]
    fn an_unclean_open_whose_catch_up_found_no_handle_still_starts_its_check_after_the_rescan() {
        let dir = vault_dir("owed-check");
        let index_dir = dir.join(".serat");
        // A session that ended uncleanly: opened and dropped without close().
        drop(IndexHandle::open(&index_dir).expect("index"));
        let empty: IndexHandleSlot = Arc::new(Mutex::new(None));
        let (indexer, _) = indexer(&dir, Arc::clone(&empty));
        let calls: Arc<Mutex<Vec<bool>>> = Arc::default();
        let seen = Arc::clone(&calls);
        indexer.on_caught_up(move |unclean| seen.lock().expect("calls").push(unclean));

        // `Ready` before the handle reaches the slot: the catch-up cannot run.
        indexer.catch_up(&walk(&indexer));
        assert!(indexer.rescan_owed());
        assert!(
            calls.lock().expect("calls").is_empty(),
            "no handle, no verdict"
        );

        // The handle arrives; the owed rescan runs and the check is started, as unclean.
        *empty.lock().expect("slot") = Some(IndexHandle::open(&index_dir).expect("index"));
        indexer.retry_deferred();
        assert_eq!(*calls.lock().expect("calls"), vec![true]);
        // Once: another tick owes nothing.
        indexer.retry_deferred();
        assert_eq!(calls.lock().expect("calls").len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_open_compares_mtimes_after_a_clean_shutdown_and_after_an_unclean_one() {
        let dir = vault_dir("modes");
        let index_dir = dir.join(".serat");
        {
            let (indexer, _) = indexer(&dir, slot_with(&dir));
            indexer.catch_up(&walk(&indexer));
            let handle = indexer.slot.lock().expect("slot").take().expect("handle");
            handle.close().expect("clean close");
        }
        let before = hash_of(&dir, "a.md").expect("indexed");
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.join("a.md"), "# a, edited while closed\n").expect("edit");

        // Clean: the marker is set, and the content comparison runs all the same.
        let (indexer, _) = indexer(&dir, slot_with(&dir));
        assert!(!indexer.unclean());
        indexer.catch_up(&walk(&indexer));
        let after_clean = hash_of(&dir, "a.md").expect("row");
        assert_ne!(after_clean, before, "a clean shutdown is not trusted");

        // Unclean: the handle was opened and dropped without close().
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.join("a.md"), "# a, edited again\n").expect("edit");
        drop(indexer.slot.lock().expect("slot").take());
        let handle = IndexHandle::open(&index_dir).expect("index");
        drop(handle);
        let (indexer, _) = indexer_for(&dir);
        assert!(indexer.unclean());
        indexer.catch_up(&walk(&indexer));
        assert_ne!(hash_of(&dir, "a.md").expect("row"), after_clean);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rescan_inside_the_journal_window_is_replayed_after_the_catch_up() {
        let dir = vault_dir("journal-rescan");
        let (indexer, _) = indexer(&dir, slot_with(&dir));
        indexer.walk_started();
        let stale_walk = walk(&indexer);
        std::fs::write(dir.join("c.md"), "# c\n").expect("new");
        indexer.rescan();
        assert!(hash_of(&dir, "c.md").is_none(), "journaled, not run");
        indexer.catch_up(&stale_walk);
        assert!(
            hash_of(&dir, "c.md").is_some(),
            "the replay re-ran the rescan"
        );
        assert!(indexer.journal().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_journal_is_capped_and_a_failed_walk_closes_it_and_owes_a_rescan() {
        let dir = vault_dir("journal-cap");
        let (indexer, _) = indexer(&dir, slot_with(&dir));
        indexer.walk_started();
        let many: Vec<VaultPath> = (0..DEFERRED_CAP + 1)
            .map(|n| p(&format!("n{n}.md")))
            .collect();
        indexer.journal().as_mut().expect("window").add_paths(&many);
        {
            let guard = indexer.journal();
            let journal = guard.as_ref().expect("window");
            assert!(
                journal.rescan && journal.paths.is_empty(),
                "overflow is a rescan"
            );
        }
        indexer.walk_failed();
        assert!(indexer.journal().is_none());
        assert!(indexer.deferred().rescan);
        indexer.tick(Instant::now());
        assert!(hash_of(&dir, "a.md").is_some(), "the owed rescan ran");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_catch_up_with_no_handle_owes_a_rescan_instead_of_vanishing() {
        let dir = vault_dir("no-handle");
        let slot: IndexHandleSlot = Arc::new(Mutex::new(None));
        let (indexer, _) = indexer(&dir, Arc::clone(&slot));
        indexer.walk_started();
        indexer.catch_up(&walk(&indexer));
        assert!(indexer.deferred().rescan);
        *slot.lock().expect("slot") = Some(IndexHandle::open(&dir.join(".serat")).expect("index"));
        indexer.tick(Instant::now());
        assert!(hash_of(&dir, "a.md").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stopped_indexer_does_no_further_work() {
        let dir = vault_dir("stopped");
        let (indexer, _) = indexer(&dir, slot_with(&dir));
        indexer.stop();
        indexer.catch_up(&walk(&indexer));
        indexer.index_paths(&[p("a.md")]);
        assert!(hash_of(&dir, "a.md").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stopped_reconciliation_keeps_every_row_it_could_not_read() {
        // `Stoppable::load` itself, which the Indexer's early returns never reach:
        // a catch-up over paths that have rows, run with the stop flag set.
        let dir = vault_dir("stoppable");
        let (indexer, _) = indexer(&dir, slot_with(&dir));
        indexer.catch_up(&walk(&indexer));
        let (a, b) = (hash_of(&dir, "a.md"), hash_of(&dir, "b.md"));
        assert!(a.is_some() && b.is_some());
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.join("a.md"), "# a, edited\n").expect("edit");

        indexer.stop();
        let reconciler = indexer.reconciler().expect("handle");
        let parser = CommonMarkParser::new();
        let entries = walk(&indexer);
        let oldest = entries.iter().map(|e| e.mtime_ms).min().expect("files");
        let outcome = reconciler
            .catch_up(
                &indexer.source(),
                &parser,
                &entries,
                CatchUp::PathsAndMtime,
                Some(serat_index::rebuild::now_ms()),
            )
            .expect("catch-up");
        // Every load failed, so the window may not move past any file's mtime: a file
        // the catch-up never read must stay a candidate for the next one (task #66).
        let window = indexer
            .slot
            .lock()
            .expect("slot")
            .as_ref()
            .expect("handle")
            .caught_up_from_ms()
            .expect("window");
        assert!(
            window <= oldest,
            "window {window} passed an unread file at {oldest}"
        );
        assert_eq!(outcome.removed, 0, "nothing removed");
        assert_eq!(outcome.upserted, 0, "nothing read, so nothing submitted");
        let by_path = reconciler
            .index_paths(&indexer.source(), &parser, &|_, _| false, &[p("a.md")])
            .expect("index_paths");
        assert_eq!(by_path.unreadable, 1, "unreadable is counted: {by_path:?}");
        assert_eq!(by_path.removed, 0);
        assert_eq!(hash_of(&dir, "a.md"), a, "rows survive");
        assert_eq!(hash_of(&dir, "b.md"), b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_panic_in_the_catch_up_closes_the_journal_and_owes_a_rescan() {
        let dir = vault_dir("unwind");
        let (indexer, _) = indexer(&dir, slot_with(&dir));
        indexer.walk_started();
        let guarded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _unwind = UnwindGuard(&indexer);
            panic!("catch-up failed (test)");
        }));
        assert!(guarded.is_err());
        assert!(indexer.journal().is_none());
        assert!(indexer.deferred().rescan);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_claim_is_granted_once_only_when_ready_and_never_after_a_rebuild() {
        let ready = IndexState::Ready { note_count: 1 };
        let rebuilding = IndexState::Rebuilding {
            reason: crate::ipc::RebuildReason::NoIndex,
            done: 0,
            total: 0,
            walk_complete: false,
        };
        let c = OpenCatchUp::new(Duration::from_secs(5), Duration::ZERO);
        c.index_published(&IndexState::Opening { reason: None });
        c.index_published(&ready);
        assert!(c.claim(false));
        assert!(!c.claim(true), "once per open vault");

        let c = OpenCatchUp::new(Duration::from_secs(5), Duration::ZERO);
        c.index_published(&rebuilding);
        c.index_published(&ready);
        assert!(!c.claim(true), "a rebuild is the reconciliation");

        let c = OpenCatchUp::new(Duration::from_secs(5), Duration::ZERO);
        c.attach_tree();
        c.index_published(&ready);
        assert!(!c.claim(false), "the trigger stands aside for a tree");
        assert!(c.claim(true));

        // The trigger waits its grace for a tree, and walks if none comes.
        let c = OpenCatchUp::new(Duration::from_secs(5), Duration::from_millis(40));
        c.index_published(&ready);
        assert!(c.claim_standalone(true, false));

        // No first paint, no grace: a headless run must not pay it.
        let c = OpenCatchUp::new(Duration::from_secs(5), Duration::from_secs(30));
        c.index_published(&ready);
        let started = Instant::now();
        assert!(c.claim_standalone(false, false));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "waited the grace"
        );

        // ...and stands aside for one that arrives within it.
        let c = Arc::new(OpenCatchUp::new(
            Duration::from_secs(5),
            Duration::from_secs(10),
        ));
        c.index_published(&ready);
        let attaching = Arc::clone(&c);
        let late = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            attaching.attach_tree();
        });
        assert!(!c.claim_standalone(true, false));
        late.join().expect("attach");

        let c = OpenCatchUp::new(Duration::from_millis(30), Duration::ZERO);
        assert!(
            !c.claim(true),
            "a bounded wait for an index that never settles"
        );
    }

    fn indexer_for(dir: &std::path::Path) -> (Indexer, Arc<Mutex<Vec<u64>>>) {
        indexer(dir, slot_with(dir))
    }
}
