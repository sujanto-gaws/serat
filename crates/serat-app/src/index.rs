//! The index actor thread, and the status it publishes.
//!
//! Architecture §3.4, first row: **the main thread never opens the index.** The actor
//! opens it on its own thread (ADR-0001) and runs the 5–8ms stamp check there (task #68); the
//! shell paints with whatever state the actor has last published. No command in
//! `contracts/ipc.rs` blocks on any of it — `index_status` reads the published value
//! and returns, which is what makes it callable from a render path.
//!
//! That is the load-bearing property of the whole cold start, and it is a property of
//! *this* file: if [`StatusPublisher::status`] ever took a lock the actor holds while
//! doing work, 400 ms would stop being achievable at 20,000 notes for a reason nobody
//! would find by reading `main`.
//!
//! # What P0 has, and does not
//!
//! The thread, the channel, the published state, the throttle and the event are all
//! real and tested. The stamp check itself is written and landed — I6 is
//! `serat_index::open_or_rebuild` — and task #65 is what wires this crate to it:
//! [`SeratIndexOpen`] now constructs the one production [`serat_core::NoteSource`]
//! (`serat_vault::Vault`) and [`serat_core::NoteParser`] (`serat_parse::CommonMarkParser`)
//! `open_or_rebuild` needs and were the reason the wiring was more than a one-line
//! change. [`IndexState::Ready`] is a state the app can now genuinely reach, and
//! [`IndexState::Unavailable`] means what it says: a rebuild failed, or the index
//! could not be reopened after one succeeded. It is still never a fabricated `Ready`
//! — a shell that claimed an index it does not have would make the cold-start number
//! meaningless and would be exactly the kind of test that cannot fail.
//!
//! Task #89 closes the gap #65 deliberately left: the [`serat_index::IndexHandle`]
//! `open_or_rebuild` returns is now kept, handed back through [`IndexOpen::into_handle`]
//! into an [`IndexHandleSlot`], and `AppState::write_file` submits every save through
//! it. Before this, the app opened an index it never updated — correct exactly until
//! the first save (architecture §3.1.1, §3.2).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::ipc::{IndexState, IndexStatus, REFUSED_NAMES_CAP, RebuildReason};
use crate::startup::Startup;

/// Minimum gap between two `index://status` emissions that do not change the kind of
/// state.
///
/// Ten per second, from architecture §3.1 step 6. A rebuild that emitted per note would
/// spend its time in the IPC bridge instead of in the parser; this is the mechanism
/// that prevents it, not a politeness.
pub const THROTTLE: Duration = Duration::from_millis(100);

/// Somewhere an [`IndexStatus`] can be sent.
///
/// In the application this is `AppHandle::emit`; in a test it is a `Vec`. The
/// abstraction exists so the throttle can be tested without a running Tauri
/// application, and for no other reason.
pub trait StatusSink: Send + Sync + 'static {
    /// Deliver one status to the front end.
    fn emit(&self, status: &IndexStatus);
}

/// Holds the last published index state and emits transitions.
///
/// Two readers with different needs: the front end, which subscribes to
/// `index://status` and must not be flooded, and `index_status()`, which polls and must
/// never block. Both are served from one small mutex that is never held across any work
/// — see the module note.
pub struct StatusPublisher<S> {
    vault_id: String,
    startup: Arc<Startup>,
    sink: S,
    throttle: Duration,
    inner: Mutex<Published>,
    observer: Mutex<Option<Observer>>,
    /// The open vault's refusal log, when this publisher belongs to one. Read at the
    /// moment a status is built, so the count is never older than the poll or the
    /// emission that carries it.
    refusals: Option<serat_vault::RefusalLog>,
    /// The checkpointer's facts, once the vault's `IndexHandle` exists (task #56). Set
    /// by [`IndexActor::spawn`]'s thread; read, as atomics, whenever a status is built.
    wal: Mutex<Option<Arc<serat_index::WalHealth>>>,
    /// Set when the open that created this publisher failed after `vault://opened` had
    /// announced its id (task #90): nothing may carry that id afterwards.
    retired: std::sync::atomic::AtomicBool,
}

#[derive(Debug)]
struct Published {
    state: IndexState,
    since_process_start_ms: u64,
    last_emit: Option<Instant>,
}

/// Called for every state the actor publishes, throttled or not.
///
/// Separate from [`StatusSink`] deliberately. The sink is the *event*, and events are
/// throttled to ten per second; this is the *fact*, and the startup trace has to see
/// every one of them — a `t_usable` precondition that could be dropped by a throttle
/// would be a precondition in name only.
type Observer = Box<dyn Fn(&IndexState) + Send + Sync>;

impl<S> std::fmt::Debug for StatusPublisher<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatusPublisher")
            .field("vault_id", &self.vault_id)
            .field("state", &self.lock().state)
            .finish_non_exhaustive()
    }
}

impl<S> StatusPublisher<S> {
    fn lock(&self) -> std::sync::MutexGuard<'_, Published> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<S: StatusSink> StatusPublisher<S> {
    /// Start publishing for `vault_id`, in [`IndexState::Opening`].
    ///
    /// `Opening` is the state the shell paints with. §3.4 measures the open check at
    /// about two milliseconds, so it is rarely seen — but "not known yet" has to be a
    /// value the front end can render rather than a null it has to interpret.
    pub fn new(vault_id: impl Into<String>, startup: Arc<Startup>, sink: S) -> Self {
        Self::with_throttle(vault_id, startup, sink, THROTTLE)
    }

    /// As [`StatusPublisher::new`], with an explicit throttle. For tests.
    pub fn with_throttle(
        vault_id: impl Into<String>,
        startup: Arc<Startup>,
        sink: S,
        throttle: Duration,
    ) -> Self {
        let since_process_start_ms = startup.elapsed_ms() as u64;
        Self {
            vault_id: vault_id.into(),
            startup,
            sink,
            throttle,
            inner: Mutex::new(Published {
                state: IndexState::Opening { reason: None },
                since_process_start_ms,
                last_emit: None,
            }),
            observer: Mutex::new(None),
            refusals: None,
            wal: Mutex::new(None),
            retired: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Start in `Opening` with `reason` (task #117): a deferred open (task #96) says why it
    /// has no actor yet. `None` is the plain `Opening` every publisher starts in.
    #[must_use]
    pub fn with_opening_reason(self, reason: Option<crate::ipc::OpeningReason>) -> Self {
        self.lock().state = IndexState::Opening { reason };
        self
    }

    /// Count the names `refusals` holds into every status this publisher builds
    /// (`IndexStatus::refused_names`, FR-VLT-01, task #34).
    ///
    /// Without it `refused_names` is `None`, which the contract defines as "not
    /// counting" -- the right answer for a publisher with no vault behind it.
    #[must_use]
    pub fn with_refusals(mut self, refusals: serat_vault::RefusalLog) -> Self {
        self.refusals = Some(refusals);
        self
    }

    /// Carry the checkpointer's pinned-WAL fact into every status this publisher builds
    /// (`IndexStatus::wal_pinned_bytes`, task #56), and re-emit the status whenever the
    /// fact appears, changes or clears, because it is not part of the state and would
    /// otherwise reach a listening front end only with the next state change.
    ///
    /// The callback holds the publisher weakly: the health is owned by the index handle,
    /// and a reference back would make a cycle.
    pub fn watch_wal(self: &Arc<Self>, health: Arc<serat_index::WalHealth>) {
        let weak = Arc::downgrade(self);
        health.on_change(move || {
            if let Some(publisher) = weak.upgrade() {
                publisher.re_emit();
            }
        });
        *self
            .wal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(health);
    }

    /// The pinned-WAL size, if the checkpointer reports one. Atomics only.
    fn wal_pinned_bytes(&self) -> Option<u64> {
        self.wal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|health| health.pinned_bytes())
    }

    /// Watch every published state, including the ones the throttle swallows.
    ///
    /// One observer; installing a second replaces the first. Called before the actor
    /// is spawned, so nothing is missed.
    pub fn on_publish(&self, observer: impl Fn(&IndexState) + Send + Sync + 'static) {
        *self
            .observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(observer));
    }

    /// The current status, without blocking and without touching SQLite.
    ///
    /// This is `index_status()`. It reads what the actor last published and returns.
    #[must_use]
    pub fn status(&self) -> IndexStatus {
        let inner = self.lock();
        IndexStatus {
            vault_id: self.vault_id.clone(),
            state: inner.state.clone(),
            since_process_start_ms: inner.since_process_start_ms,
            refused_names: self.refused_count(),
            wal_pinned_bytes: self.wal_pinned_bytes(),
        }
    }

    /// `RefusalLog::len()`, saturating at [`REFUSED_NAMES_CAP`]; `None` with no log.
    ///
    /// `len()` is the count of distinct stored paths and the length of the
    /// `refused_names` list. It is deliberately not added to `overflowed()`, which
    /// counts refusal *events* past the cap (a path refused twice counts twice) and
    /// would overstate the number of names. The log already stops storing at the
    /// same cap, so the `min` is a belt over braces: the contract's "saturates at
    /// 1000" holds even if the log's bound is later raised.
    fn refused_count(&self) -> Option<u32> {
        self.refusals.as_ref().map(|log| {
            u32::try_from(log.len())
                .unwrap_or(REFUSED_NAMES_CAP)
                .min(REFUSED_NAMES_CAP)
        })
    }

    /// The vault this publisher belongs to.
    #[must_use]
    pub fn vault_id(&self) -> &str {
        &self.vault_id
    }

    /// Stop emitting for good: the open this belongs to failed (task #90). Statuses
    /// already in flight on another thread are not recalled; the caller joins the actor
    /// after retiring, so none can start afterwards.
    pub fn retire(&self) {
        self.retired
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether [`StatusPublisher::retire`] has been called.
    #[must_use]
    pub fn is_retired(&self) -> bool {
        self.retired.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Emit the current state again, unthrottled, so a fact that is not part of the
    /// state -- the refused-name count, which the walk changes while the state stays
    /// `Ready` -- reaches a front end that only listens (invalidation, not proof:
    /// `index_status()` is still what establishes it).
    pub fn re_emit(&self) {
        if self.is_retired() {
            return;
        }
        let status = self.status();
        self.lock().last_emit = Some(Instant::now());
        self.sink.emit(&status);
    }

    /// Publish a new state.
    ///
    /// The stored state is updated every time, so a poll never sees a stale value. The
    /// *emission* is throttled, and only within one kind of state: a change of kind —
    /// `Opening` to `Rebuilding`, `Rebuilding` to `Ready` — is always emitted
    /// immediately, because throttling a transition is how a UI ends up showing
    /// "rebuilding" over a finished index.
    ///
    /// Returns whether an event was emitted, which is what the throttle test asserts.
    pub fn publish(&self, state: IndexState) -> bool {
        if self.is_retired() {
            return false;
        }
        let now_ms = self.startup.elapsed_ms() as u64;

        // Before the throttle, and outside the state lock: the observer records facts
        // the trace depends on, and a throttled event must not be able to hide one.
        if let Some(observer) = self
            .observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            observer(&state);
        }

        let status = {
            let mut inner = self.lock();
            // A change of `Opening`'s reason is a transition too: a reason that clears
            // must not be held back by the throttle (task #117).
            let transition = kind_of(&inner.state) != kind_of(&state)
                || matches!(
                    (&inner.state, &state),
                    (IndexState::Opening { reason: a }, IndexState::Opening { reason: b }) if a != b
                );
            let due = inner
                .last_emit
                .is_none_or(|last| last.elapsed() >= self.throttle);

            inner.state = state;
            inner.since_process_start_ms = now_ms;

            if !transition && !due {
                return false;
            }
            inner.last_emit = Some(Instant::now());
            IndexStatus {
                vault_id: self.vault_id.clone(),
                state: inner.state.clone(),
                since_process_start_ms: inner.since_process_start_ms,
                refused_names: self.refused_count(),
                wal_pinned_bytes: self.wal_pinned_bytes(),
            }
        };

        // Emitted outside the lock. Holding it across the IPC bridge would let a slow
        // front end block `index_status()`, which is the one thing this type promises
        // it cannot do.
        if self.is_retired() {
            return false;
        }
        self.sink.emit(&status);
        true
    }
}

/// The discriminant of an [`IndexState`], for deciding what counts as a transition.
///
/// A `Rebuilding` whose `done` moved is progress; a `Rebuilding` that became `Ready` is
/// a transition. `PartialEq` on the whole value would make every progress tick a
/// transition and defeat the throttle entirely.
fn kind_of(state: &IndexState) -> u8 {
    match state {
        IndexState::Opening { .. } => 0,
        IndexState::Ready { .. } => 1,
        IndexState::Rebuilding { .. } => 2,
        IndexState::Unavailable { .. } => 3,
    }
}

/// Whether this state is the open check's *last word* — the one after which the actor
/// has nothing further to publish and its [`serat_index::IndexHandle`], if it produced
/// one, exists.
///
/// Distinct from [`kind_of`], which answers a different question (is this a transition
/// the throttle must let through). `Opening` is not terminal because the check has not
/// run; `Rebuilding` is not terminal because it is progress, and holding it back is the
/// one thing [`IndexActor::spawn`] must never do.
fn is_terminal(state: &IndexState) -> bool {
    match state {
        IndexState::Ready { .. } | IndexState::Unavailable { .. } => true,
        IndexState::Opening { .. } | IndexState::Rebuilding { .. } => false,
    }
}

/// Opening an index, as the actor thread performs it.
///
/// A trait rather than a function so the actor — the thread, the publishing, the
/// ordering — can be driven by a scripted implementation in a test. It is **not** a
/// device for avoiding a dependency edge: `serat-app` depends on `serat-index` already
/// (architecture §2.1), and the real implementation lives one type below.
pub trait IndexOpen: Send + 'static {
    /// Open the index at `index_dir` for the vault rooted at `vault_root`, publishing
    /// each state as it is reached.
    ///
    /// Runs on the actor's own thread and may block for as long as it needs to. The
    /// contract is that it publishes; it returns nothing, because nothing on the main
    /// thread is waiting for it.
    fn open(&mut self, vault_root: &Path, index_dir: &Path, publish: &dyn Fn(IndexState));

    /// The incremental writer this opener produced, if `open` got far enough to build
    /// one. Called once, by [`IndexActor::spawn`]'s thread, immediately after `open`
    /// returns.
    ///
    /// Defaults to `None` so every existing test double keeps compiling unchanged —
    /// most of them have no real [`serat_index::IndexHandle`] to offer and do not need
    /// one, because nothing in this file's own tests submits an incremental write.
    /// [`SeratIndexOpen`] is the one implementation that overrides it.
    fn into_handle(self) -> Option<serat_index::IndexHandle>
    where
        Self: Sized,
    {
        None
    }
}

/// Where [`AppState::write_file`](crate::state::AppState::write_file) finds the
/// [`serat_index::IndexHandle`] once the actor thread has produced one.
///
/// `Arc<Mutex<Option<..>>>` rather than a channel the caller waits on: `open_vault`
/// must return before the open check — possibly a full rebuild — has finished
/// (§3.4), so there is a real window in which a save can land before a handle exists.
/// A save in that window is not an error; it degrades to the watcher's slower path
/// (FR-VLT-04) instead of NFR-PRF-03's, because nothing recorded an echo for it —
/// design goal 5, applied to a race rather than to a missing capability.
pub type IndexHandleSlot = std::sync::Arc<Mutex<Option<serat_index::IndexHandle>>>;

/// App-owned identity of the index database a vault currently has: bumped by
/// [`IndexActor::spawn`]'s thread each time it puts a new handle in the
/// [`IndexHandleSlot`]. [`crate::reads::SearchSlot`] keys its connection on it, because
/// file metadata is not an identity (NTFS tunnelling lets a recreated `index.db`
/// inherit the old creation time).
pub type HandleEpoch = Arc<std::sync::atomic::AtomicU64>;

/// The real open check, wired to the vault and the production parser.
///
/// Architecture §3.4 specifies six ordered steps over `user_version`, `index_meta` and
/// `count(*) FROM notes`, measured at 5–8ms on the 20k fixture (task #68); every one of them
/// reads SQLite, so every one of them lives in `serat-index` (§2.1: `serat-app` owns
/// no domain logic) as [`serat_index::open_or_rebuild`]. This type is the wiring, not
/// a second copy of the check: it opens the one production [`serat_vault::Vault`] as
/// the [`serat_core::NoteSource`] and hands `open_or_rebuild` the one production
/// [`serat_parse::CommonMarkParser`] as the [`serat_core::NoteParser`], because a
/// rebuild has no other way to read or parse a note and duplicating either here would
/// be exactly the divergence §2.1 rules out.
///
/// The [`serat_index::IndexHandle`] `open_or_rebuild` returns is used here for
/// [`serat_index::IndexHandle::note_count`] — read once, to answer the one question
/// [`IndexState::Ready`] carries that `open_or_rebuild`'s status stream does not — and
/// then **kept**, in `handle` below, so [`IndexOpen::into_handle`] can hand it to the
/// actor thread that owns this opener. Task #65 dropped it here deliberately, because
/// nothing on that branch submitted incremental writes to it; task #89 is the writer,
/// and holding the handle open is what makes submitting to it possible at all.
#[derive(Debug, Default)]
pub struct SeratIndexOpen {
    handle: Option<serat_index::IndexHandle>,
}

impl IndexOpen for SeratIndexOpen {
    fn open(&mut self, vault_root: &Path, index_dir: &Path, publish: &dyn Fn(IndexState)) {
        let source = match serat_vault::Vault::open(vault_root) {
            Ok(vault) => vault,
            Err(err) => {
                // The caller already opened this same root successfully before
                // spawning the actor (`AppState::open_vault_with`); reaching this
                // arm means the directory went away or became unreadable in the
                // gap, which is design goal 5's territory, not a bug to unwrap past.
                publish(IndexState::Unavailable {
                    message: format!(
                        "could not reopen the vault to check its index: {err}. Search and \
                         queries are unavailable; the vault may still be editable if the \
                         underlying problem clears."
                    ),
                    path: index_dir.display().to_string(),
                });
                return;
            }
        };
        let parser = serat_parse::CommonMarkParser;

        let result =
            serat_index::open_or_rebuild(vault_root, index_dir, &source, &parser, &|status| {
                // `Ready` is deliberately not published from here: `open_or_rebuild`
                // reaches it before the `IndexHandle` it is about to return exists
                // (see the function's own ordering guarantee), so there is nothing to
                // read `note_count` from yet. The real `Ready` — carrying a real count
                // — is published below, once the handle is in hand. Skipping it here
                // does not reorder anything §3.4 depends on: the load-bearing ordering
                // this task must preserve is `Rebuilding` before the rebuild starts,
                // which this closure still forwards immediately, and untouched.
                if let Some(state) = map_open_status(status, index_dir) {
                    publish(state);
                }
            });

        match result {
            Ok((handle, _report)) => match handle.note_count() {
                Ok(note_count) => {
                    publish(IndexState::Ready { note_count });
                    // Kept, not dropped (task #89): this is the handle `write_file`
                    // will submit saves through, once `into_handle` hands it on.
                    self.handle = Some(handle);
                }
                Err(err) => publish(IndexState::Unavailable {
                    message: format!(
                        "the index opened but its note count could not be read: {err}"
                    ),
                    path: index_dir.display().to_string(),
                }),
            },
            Err(err) => publish(IndexState::Unavailable {
                message: format!(
                    "the index could not be opened or rebuilt: {err}. Search and queries \
                     are unavailable for the rest of this session; the vault remains fully \
                     editable."
                ),
                path: index_dir.display().to_string(),
            }),
        }
    }

    fn into_handle(self) -> Option<serat_index::IndexHandle> {
        self.handle
    }
}

/// Map one [`serat_index::IndexStatus`] onto the contract's [`IndexState`], or
/// `None` for the one variant this file answers itself (see [`SeratIndexOpen::open`]).
///
/// Exhaustive by construction — no wildcard arm — because `serat_index::IndexStatus`
/// and `IndexState` are deliberately the same outcomes under two names (§2.1:
/// `serat-index` may not link `contracts/`), and this match is the one place the two
/// could silently drift if a variant were ever added to one and not mapped here.
fn map_open_status(status: serat_index::IndexStatus, index_dir: &Path) -> Option<IndexState> {
    match status {
        serat_index::IndexStatus::Opening => Some(IndexState::Opening { reason: None }),
        // Task #120. Not a contract state and deliberately not one: the index is
        // usable, nothing is rebuilding, and inventing an `IndexState` for it would put
        // a transient on the wire that every consumer would have to learn to ignore.
        // It reaches a user through boot info, which is where the ruling put it, and
        // that is `serat-app`'s to wire when boot info exists -- named here rather than
        // dropped silently, because a status the shell swallows and a status nobody
        // publishes look identical from outside (F-459's own complaint).
        serat_index::IndexStatus::RootAdopted { .. } => None,
        serat_index::IndexStatus::Rebuilding { reason } => Some(IndexState::Rebuilding {
            reason: map_rebuild_reason(reason),
            // `open_or_rebuild` does not report walk/index progress yet — there is
            // no producer of it on this branch — so these are an honest "unknown",
            // not a fabricated count. A future progress-reporting task widens
            // `open_or_rebuild`'s status stream, not this mapping.
            done: 0,
            total: 0,
            walk_complete: false,
        }),
        serat_index::IndexStatus::Ready => None,
        serat_index::IndexStatus::Unavailable => Some(IndexState::Unavailable {
            message: "the rebuild failed; search and queries are unavailable for the rest \
                       of this session, and the vault remains fully editable"
                .to_owned(),
            path: index_dir.display().to_string(),
        }),
    }
}

/// Map one [`serat_index::RebuildReason`] onto the contract's [`RebuildReason`].
///
/// Exhaustive by construction, for the same reason as [`map_open_status`]:
/// `serat-index` cannot depend on `contracts/` (§2.1), so this file is the one place
/// the two enumerations are asserted to agree. `UserRequested` has no `serat_index`
/// counterpart — the open check cannot produce it, only a future manual-rebuild
/// command can — so it is not a case this match needs to cover.
fn map_rebuild_reason(reason: serat_index::RebuildReason) -> RebuildReason {
    match reason {
        serat_index::RebuildReason::NoIndex => RebuildReason::NoIndex,
        serat_index::RebuildReason::Corrupt => RebuildReason::Corrupt,
        serat_index::RebuildReason::SchemaChanged => RebuildReason::IndexVersion,
        serat_index::RebuildReason::Interrupted => RebuildReason::Interrupted,
        // The contract's variant keeps its name for now: renaming it moves a
        // `ts-rs`-generated type and a frontend string, which is architect's and
        // vue-frontend-dev's to take. `serat_index`'s side is renamed on task #120
        // because the old name asserted a rationale that is false of the case it was
        // named after -- `notes.path` is vault-relative, so a vault that moved keeps a
        // correct index. This arm is where the two disagree, and it is the only place
        // they can.
        serat_index::RebuildReason::ForeignVault => RebuildReason::VaultMoved,
    }
}

/// The index actor: one thread, owning the index, publishing what it finds.
#[derive(Debug)]
pub struct IndexActor {
    handle: Option<std::thread::JoinHandle<()>>,
    exited: Arc<AtomicBool>,
}

/// Sets its flag on drop: the thread's exit, normal or by panic.
struct ExitFlag(Arc<AtomicBool>);

impl Drop for ExitFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Actors detached past their deadline, by the vault root that holds the index they
/// may still be writing (task #96, architecture §3.4 "What a failed open leaves
/// behind"). ADR-0001's single writer: while an entry here is live, no second actor may
/// open the same `index.db`. An entry is dropped the first time it is looked up after
/// its flag has fired.
///
/// Keyed by the root's **file identity** ([`serat_vault::FileIdentity`], read once by
/// `Vault::open`; a value, no handle is kept), not by its path, so a case-insensitive
/// mount or a bind mount that reaches one directory by two names still has one key. The
/// index is always `<root>/.serat/index.db`, so one root is one index; the root is used
/// rather than `.serat` because `.serat` may not exist yet when an actor is registered.
/// Fired entries are pruned on every `register`, `live` and `live_count`.
///
/// **Limits.** The registry is per process: nothing stops a second *Serat process* from
/// opening the same vault, and no single-instance lock exists. If a platform cannot give
/// an identity, the actor is not recorded and a log line says so.
#[derive(Debug, Default)]
pub struct DetachedWriters {
    by_root: Mutex<std::collections::HashMap<serat_vault::FileIdentity, Vec<Arc<AtomicBool>>>>,
}

impl DetachedWriters {
    /// Record an actor that has not exited.
    pub fn register(&self, root: Option<serat_vault::FileIdentity>, exited: Arc<AtomicBool>) {
        let Some(key) = root else {
            eprintln!(
                "serat: a vault root cannot be identified; a detached index thread is not recorded"
            );
            return;
        };
        let mut map = self
            .by_root
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::prune(&mut map);
        map.entry(key).or_default().push(exited);
    }

    fn prune(map: &mut std::collections::HashMap<serat_vault::FileIdentity, Vec<Arc<AtomicBool>>>) {
        map.retain(|_, flags| {
            flags.retain(|f| !f.load(Ordering::SeqCst));
            !flags.is_empty()
        });
    }

    /// A flag that fires when the last live detached actor on `root`'s index has
    /// exited, or `None` if none is live.
    #[must_use]
    pub fn live(&self, root: Option<serat_vault::FileIdentity>) -> Option<DetachedWait> {
        let key = root?;
        let mut map = self
            .by_root
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::prune(&mut map);
        Some(DetachedWait(map.get(&key)?.clone()))
    }

    /// How many detached actors are live. For tests and diagnostics.
    #[must_use]
    pub fn live_count(&self) -> usize {
        let mut map = self
            .by_root
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::prune(&mut map);
        map.values().map(Vec::len).sum()
    }
}

/// The detached actors a new actor on the same file must wait for.
#[derive(Debug)]
pub struct DetachedWait(Vec<Arc<AtomicBool>>);

impl DetachedWait {
    /// Whether every one of them has exited.
    #[must_use]
    pub fn done(&self) -> bool {
        self.0.iter().all(|f| f.load(Ordering::SeqCst))
    }
}

impl IndexActor {
    /// Spawn the actor.
    ///
    /// Returns as soon as the thread is created. Nothing about this call is
    /// proportional to the size of the vault, and nothing on the main thread waits for
    /// what the thread does next — which is the first row of §3.4's table, expressed as
    /// code rather than as a comment.
    ///
    /// `epoch` is bumped whenever the slot receives a handle (see [`HandleEpoch`]).
    ///
    /// Also returns an [`IndexHandleSlot`], empty until `opener` finishes and
    /// [`IndexOpen::into_handle`] fills it (task #89). The caller polls it rather than
    /// waiting on it, for the same reason the caller does not wait on this thread at
    /// all: a save that lands before the slot is filled degrades to the watcher path
    /// instead of blocking.
    ///
    /// # The terminal state is published after the slot is filled (#119, F-382)
    ///
    /// [`IndexOpen::open`] publishes `Ready` or `Unavailable` from *inside* itself, and
    /// [`IndexOpen::into_handle`] cannot run until `open` has returned. So the state
    /// that tells [`Startup::mark_index_settled`] the open check is over was, until
    /// this task, observable while the slot was still empty — F-382 named that window
    /// and called it narrow without measuring it. This thread therefore holds the last
    /// terminal state back and publishes it *after* `into_handle` has filled the slot,
    /// which turns the race into an ordering: a caller that has seen `Ready` finds a
    /// handle, because the slot write happens-before the publish on this one thread and
    /// both cross a mutex.
    ///
    /// **Intermediate states are not held back, and that constraint outranks the race
    /// it closes.** `Rebuilding` is forwarded the instant the opener publishes it, so
    /// `mark_index_settled` still fires *before* a rebuild starts and the shell still
    /// comes up on the vault that needs one most — §3.4's first precondition, and the
    /// reason written above that mark's own signature. Deferring everything until the
    /// slot was filled would make the shell wait for `Ready`, which is strictly worse
    /// than the window it would close.
    ///
    /// This does not close, and cannot close, the window a save hits *during* a
    /// rebuild. The slot is legitimately empty for the length of one, `Rebuilding`
    /// settles the mark by design, and a save landing there degrades to the watcher
    /// path — that is F-450, and it is not a defect. What is closed is the ordering
    /// between the open check's *last word* and the handle it produced.
    ///
    /// One consequence, stated rather than guarded: an opener that publishes a terminal
    /// state and then panics now leaves the published state at `Opening` instead of at
    /// that terminal state. A panicking opener already loses its handle and its thread
    /// with no recovery path, so this changes what an already-unhandled failure looks
    /// like, not whether it is handled. Neither implementation in this crate can reach
    /// it: [`SeratIndexOpen::open`] returns on every error rather than unwinding.
    ///
    /// # Errors
    ///
    /// Only if the operating system refuses to create a thread, in which case the
    /// caller publishes [`IndexState::Unavailable`] and the shell runs on as an editor.
    pub fn spawn<S: StatusSink, O: IndexOpen>(
        publisher: Arc<StatusPublisher<S>>,
        opener: O,
        vault_root: PathBuf,
        index_dir: PathBuf,
        epoch: HandleEpoch,
    ) -> std::io::Result<(Self, IndexHandleSlot)> {
        let slot: IndexHandleSlot = Arc::new(Mutex::new(None));
        let actor = Self::spawn_into(
            Arc::clone(&slot),
            publisher,
            opener,
            vault_root,
            index_dir,
            epoch,
        )?;
        Ok((actor, slot))
    }

    /// [`IndexActor::spawn`] filling a slot the caller already holds, which is what lets
    /// an actor be started *later* (task #96: after a detached one has exited) while the
    /// vault that owns the slot is already open.
    ///
    /// # Errors
    ///
    /// As [`IndexActor::spawn`].
    pub fn spawn_into<S: StatusSink, O: IndexOpen>(
        slot: IndexHandleSlot,
        publisher: Arc<StatusPublisher<S>>,
        opener: O,
        vault_root: PathBuf,
        index_dir: PathBuf,
        epoch: HandleEpoch,
    ) -> std::io::Result<Self> {
        let slot_for_thread = slot;
        let exited = Arc::new(AtomicBool::new(false));
        let exit_flag = ExitFlag(Arc::clone(&exited));
        let handle = std::thread::Builder::new()
            .name("serat-index".to_owned())
            .spawn(move || {
                // Set when this thread ends, however it ends (task #96). The captures
                // are rebound *after* the flag so they drop first -- locals drop in
                // reverse order, closure captures only after every local -- which makes
                // the flag the last thing to go even on a panic: it means "the handle
                // is closed", not merely "the thread is ending".
                let _exit = exit_flag;
                let (slot_for_thread, publisher, mut opener) = (slot_for_thread, publisher, opener);
                // The opener's last word, withheld until the slot below is filled. A
                // `RefCell` and not a `Mutex`: `publish` is a `&dyn Fn` with no `Send`
                // bound, so it cannot leave this thread, and only `open` can call it.
                let terminal: std::cell::RefCell<Option<IndexState>> =
                    std::cell::RefCell::new(None);
                opener.open(&vault_root, &index_dir, &|state| {
                    if is_terminal(&state) {
                        // Last one wins. Nothing in this crate publishes two, but
                        // keeping the latest is the only answer that cannot publish a
                        // state the opener has already superseded.
                        *terminal.borrow_mut() = Some(state);
                    } else {
                        publisher.publish(state);
                    }
                });
                if let Some(index_handle) = opener.into_handle() {
                    // The checkpointer lives with this handle (stopped when the slot's
                    // handle is dropped on vault close); the status carries its fact.
                    publisher.watch_wal(index_handle.wal_health());
                    *slot_for_thread
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(index_handle);
                    epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                if let Some(state) = terminal.borrow_mut().take() {
                    publisher.publish(state);
                }
                // A vault that closed or failed to open while this thread was still working
                // retired the publisher before looking in the slot; the handle that
                // arrived afterwards is closed here, so the exit flag (set last, as
                // this closure's final act) means "connection closed" and not merely
                // "thread over" (task #96).
                if publisher.is_retired()
                    && let Some(handle) = slot_for_thread
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                    && let Err(err) = handle.close()
                {
                    eprintln!("serat: the index did not close cleanly: {err}");
                }
            })?;
        Ok(Self {
            handle: Some(handle),
            exited,
        })
    }

    /// The flag the actor's thread sets as it ends.
    #[must_use]
    pub fn exited(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.exited)
    }

    /// Hand an actor that may still be running to `registry` without waiting for it.
    /// Returns whether it had already finished.
    pub fn detach_into(
        &mut self,
        root: Option<serat_vault::FileIdentity>,
        registry: &DetachedWriters,
    ) -> bool {
        if self.exited.load(Ordering::SeqCst) {
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
            return true;
        }
        self.handle = None;
        registry.register(root, Arc::clone(&self.exited));
        false
    }

    /// [`IndexActor::join_within`], registering the actor in `registry` when the
    /// deadline expires (task #96).
    pub fn join_or_register(
        &mut self,
        deadline: Duration,
        root: Option<serat_vault::FileIdentity>,
        registry: &DetachedWriters,
    ) -> bool {
        if self.join_within(deadline) {
            return true;
        }
        registry.register(root, Arc::clone(&self.exited));
        false
    }

    /// Wait, at most `deadline`, for the actor to finish; past it the thread is
    /// detached and `false` is returned. A failed open uses this (task #90): the
    /// publisher is retired first so nothing further is emitted, but a detached actor
    /// may still be inside a synchronous rebuild and writing `index.db` -- that residue
    /// is accepted rather than holding `opening` (and so a window close) for a whole
    /// rebuild.
    pub fn join_within(&mut self, deadline: Duration) -> bool {
        let Some(handle) = self.handle.take() else {
            return true;
        };
        let limit = Instant::now() + deadline;
        while !handle.is_finished() {
            if Instant::now() >= limit {
                eprintln!("serat: the index thread did not stop within {deadline:?}; detached");
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = handle.join();
        true
    }

    /// Wait for the actor to finish, unbounded. Tests only: nothing a user waits on may
    /// call it.
    #[cfg(test)]
    pub fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The index directory for a vault: `.serat/` under the root (C-02 — disposable).
#[must_use]
pub fn index_dir_for(vault_root: &Path) -> PathBuf {
    vault_root.join(".serat")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::RebuildReason;

    /// Collects everything emitted, so the throttle can be counted.
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<IndexStatus>>>);

    impl Recorder {
        fn taken(&self) -> Vec<IndexStatus> {
            self.0.lock().expect("not poisoned").clone()
        }
    }

    impl StatusSink for Recorder {
        fn emit(&self, status: &IndexStatus) {
            self.0.lock().expect("not poisoned").push(status.clone());
        }
    }

    fn publisher(throttle: Duration) -> (Arc<StatusPublisher<Recorder>>, Recorder) {
        let recorder = Recorder::default();
        let publisher = Arc::new(StatusPublisher::with_throttle(
            "vault-1",
            Arc::new(Startup::begin()),
            recorder.clone(),
            throttle,
        ));
        (publisher, recorder)
    }

    fn rebuilding(done: u32) -> IndexState {
        IndexState::Rebuilding {
            reason: RebuildReason::NoIndex,
            done,
            total: 20_200,
            walk_complete: true,
        }
    }

    // Task #117. Mutant, run by hand: dropping the reason comparison from `publish`'s
    // transition test makes the clear below throttled (`false`) and this fail.
    #[test]
    fn an_opening_reason_is_polled_and_its_clearing_is_emitted_through_the_throttle() {
        use crate::ipc::OpeningReason::PreviousIndexStillClosing;
        let recorder = Recorder::default();
        let publisher = StatusPublisher::with_throttle(
            "vault-1",
            Arc::new(Startup::begin()),
            recorder.clone(),
            Duration::from_secs(60),
        )
        .with_opening_reason(Some(PreviousIndexStillClosing));
        assert_eq!(
            publisher.status().state,
            IndexState::Opening {
                reason: Some(PreviousIndexStillClosing)
            }
        );
        // An unthrottled first emit, then the clear inside the 60 s window.
        assert!(publisher.publish(IndexState::Opening {
            reason: Some(PreviousIndexStillClosing)
        }));
        assert!(
            publisher.publish(IndexState::Opening { reason: None }),
            "the clearing of a reason is a transition, not a throttled tick"
        );
        assert_eq!(
            recorder.taken().last().map(|s| s.state.clone()),
            Some(IndexState::Opening { reason: None })
        );
    }

    #[test]
    fn the_pinned_wal_fact_rides_on_the_status_and_is_re_emitted_when_it_changes() {
        // Task #56. The fact is not part of the state, so a listening front end gets it
        // only if the change itself emits; and a poll reads it without SQLite.
        let (publisher, recorder) = publisher(Duration::from_secs(60));
        assert_eq!(publisher.status().wal_pinned_bytes, None);

        let health = Arc::new(serat_index::WalHealth::default());
        publisher.watch_wal(Arc::clone(&health));
        assert_eq!(publisher.status().wal_pinned_bytes, None);
        assert!(recorder.taken().is_empty());

        health.set_pinned_for_test(Some(70 * 1024 * 1024));
        assert_eq!(publisher.status().wal_pinned_bytes, Some(70 * 1024 * 1024));
        let seen = recorder.taken();
        assert_eq!(seen.len(), 1, "the change did not emit: {seen:?}");
        assert_eq!(seen[0].wal_pinned_bytes, Some(70 * 1024 * 1024));

        // Unchanged is not a change; clearing is.
        health.set_pinned_for_test(Some(70 * 1024 * 1024));
        assert_eq!(recorder.taken().len(), 1);
        // A size that only moves is carried by the next poll, not emitted.
        health.set_pinned_for_test(Some(80 * 1024 * 1024));
        assert_eq!(recorder.taken().len(), 1);
        assert_eq!(publisher.status().wal_pinned_bytes, Some(80 * 1024 * 1024));
        health.set_pinned_for_test(None);
        assert_eq!(publisher.status().wal_pinned_bytes, None);
        assert_eq!(recorder.taken().len(), 2);
        assert_eq!(recorder.taken()[1].wal_pinned_bytes, None);
    }

    #[test]
    fn a_poll_always_sees_the_latest_state_even_when_the_event_was_throttled() {
        // The distinction the type exists for. Throttling the *event* is required;
        // throttling the *value* would make index_status() lie.
        let (publisher, recorder) = publisher(Duration::from_secs(60));
        publisher.publish(rebuilding(1));
        for done in 2..=500 {
            publisher.publish(rebuilding(done));
        }

        assert_eq!(
            publisher.status().state,
            rebuilding(500),
            "the poll must see the newest state, not the last one emitted"
        );
        assert_eq!(
            recorder.taken().len(),
            1,
            "500 progress ticks inside one throttle window are one event"
        );
    }

    #[test]
    fn progress_is_throttled_but_a_change_of_state_never_is() {
        // The mutation this catches: dropping the `transition` term from `publish` so
        // the throttle applies to everything. The UI then shows "rebuilding" over a
        // finished index until the next tick that happens to be due.
        let (publisher, recorder) = publisher(Duration::from_secs(60));

        assert!(
            publisher.publish(rebuilding(1)),
            "Opening -> Rebuilding is a transition"
        );
        assert!(
            !publisher.publish(rebuilding(2)),
            "progress inside the window"
        );
        assert!(
            !publisher.publish(rebuilding(3)),
            "progress inside the window"
        );
        assert!(
            publisher.publish(IndexState::Ready { note_count: 20_200 }),
            "Rebuilding -> Ready must be emitted however recently we emitted"
        );

        let emitted = recorder.taken();
        assert_eq!(emitted.len(), 2, "{emitted:?}");
        assert_eq!(emitted[0].state, rebuilding(1));
        assert_eq!(emitted[1].state, IndexState::Ready { note_count: 20_200 });
    }

    #[test]
    fn the_throttle_lets_progress_through_once_the_window_has_passed() {
        // Without this, `progress_is_throttled...` would also pass against an
        // implementation that emitted progress exactly once and then never again.
        let (publisher, recorder) = publisher(Duration::from_millis(10));
        publisher.publish(rebuilding(1));
        for done in 2..=6 {
            std::thread::sleep(Duration::from_millis(12));
            assert!(
                publisher.publish(rebuilding(done)),
                "tick {done} was due and should have been emitted"
            );
        }
        assert_eq!(recorder.taken().len(), 6);
    }

    #[test]
    fn the_observer_sees_states_the_throttle_swallows() {
        // The startup trace's index-settled mark rides on this. If the observer were
        // called from the emit path instead, a `Rebuilding` that lost its event to the
        // throttle would never settle the mark, and `vault_usable` would wait for a
        // state change that had already happened.
        let (publisher, recorder) = publisher(Duration::from_secs(60));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        publisher.on_publish(move |state| sink.lock().expect("not poisoned").push(state.clone()));

        for done in 1..=5 {
            publisher.publish(rebuilding(done));
        }

        assert_eq!(
            recorder.taken().len(),
            1,
            "the throttle should have swallowed four of the five events"
        );
        assert_eq!(
            seen.lock().expect("not poisoned").len(),
            5,
            "the observer must see every published state, throttled or not"
        );
    }

    #[test]
    fn every_status_carries_the_vault_it_belongs_to() {
        // A front end that has switched vaults discards statuses that do not match
        // (contracts/ipc.rs, IndexStatus::vault_id). It cannot do that if we do not
        // stamp them.
        let (publisher, recorder) = publisher(Duration::ZERO);
        publisher.publish(IndexState::Ready { note_count: 3 });
        assert_eq!(publisher.status().vault_id, "vault-1");
        assert!(recorder.taken().iter().all(|s| s.vault_id == "vault-1"));
    }

    #[test]
    fn the_actor_publishes_from_its_own_thread_and_the_caller_does_not_wait() {
        /// Publishes a full rebuild, slowly enough that a caller who waited would be
        /// caught doing so.
        struct SlowRebuild;
        impl IndexOpen for SlowRebuild {
            fn open(&mut self, _root: &Path, _dir: &Path, publish: &dyn Fn(IndexState)) {
                std::thread::sleep(Duration::from_millis(80));
                publish(IndexState::Rebuilding {
                    reason: RebuildReason::NoIndex,
                    done: 0,
                    total: 0,
                    walk_complete: false,
                });
                std::thread::sleep(Duration::from_millis(80));
                publish(IndexState::Ready { note_count: 7 });
            }
        }

        let (publisher, recorder) = publisher(Duration::ZERO);
        let started = Instant::now();
        let (mut actor, _handle) = IndexActor::spawn(
            Arc::clone(&publisher),
            SlowRebuild,
            PathBuf::from("."),
            PathBuf::from("./.serat"),
            HandleEpoch::default(),
        )
        .expect("spawn");
        let spawn_cost = started.elapsed();

        // §3.4: the main thread never opens the index. If `spawn` ever became
        // synchronous this is the assertion that notices.
        assert!(
            spawn_cost < Duration::from_millis(50),
            "spawning the index actor blocked the caller for {spawn_cost:?}"
        );
        assert_eq!(
            publisher.status().state,
            IndexState::Opening { reason: None },
            "the shell paints Opening while the actor is still working"
        );

        actor.join();
        assert_eq!(
            publisher.status().state,
            IndexState::Ready { note_count: 7 }
        );
        assert_eq!(recorder.taken().len(), 2);
    }

    /// A throwaway, empty vault directory, unique per test run.
    ///
    /// Real directories rather than `.` or a fixed name: task #65's wiring runs a
    /// genuine `open_or_rebuild` (real SQLite file, real `.serat/`), and a fixed or
    /// relative root would create that state in whatever directory `cargo test`
    /// happens to run from — the failure this replaces (`PathBuf::from(".")` against
    /// the crate's own source tree) rather than merely a narrower one.
    fn throwaway_vault(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("serat-app-index-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("fixture");
        dir
    }

    #[test]
    fn the_p0_open_check_now_reaches_a_real_ready_over_an_empty_vault() {
        // #65: SeratIndexOpen is wired to the real check. An empty vault has no
        // `.serat/`, so the check reaches `NoIndex`, `open_or_rebuild` runs a
        // (trivial) rebuild, and the state that would make the cold-start number
        // meaningless — a fabricated `Ready` — is exactly what this is not: the
        // count came from a database `open_or_rebuild` actually built.
        let root = throwaway_vault("ready");
        let (publisher, _) = publisher(Duration::ZERO);
        let (mut actor, handle_slot) = IndexActor::spawn(
            Arc::clone(&publisher),
            SeratIndexOpen::default(),
            root.clone(),
            index_dir_for(&root),
            HandleEpoch::default(),
        )
        .expect("spawn");
        actor.join();

        assert_eq!(
            publisher.status().state,
            IndexState::Ready { note_count: 0 },
            "an empty vault should index to zero notes, not stay Unavailable"
        );
        assert!(
            handle_slot.lock().expect("not poisoned").is_some(),
            "task #89: a successful open must hand its IndexHandle on through \
             into_handle, not drop it"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_wiring_publishes_rebuilding_before_the_rebuild_starts_and_ready_after() {
        // The ordering `open_or_rebuild`'s own doc calls load-bearing (§3.4:
        // `t_usable` waits for the index state to be *known*, and `Rebuilding`
        // settles that as much as `Ready` does): reordering or swallowing it here
        // would let the window's cold-start budget silently start including a full
        // reindex. `on_publish` sees every state, throttled or not, so this pins
        // the sequence rather than only the final value.
        let root = throwaway_vault("ordering");
        let (publisher, _) = publisher(Duration::from_secs(60));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        publisher.on_publish(move |state| sink.lock().expect("not poisoned").push(state.clone()));

        let (mut actor, _handle) = IndexActor::spawn(
            Arc::clone(&publisher),
            SeratIndexOpen::default(),
            root.clone(),
            index_dir_for(&root),
            HandleEpoch::default(),
        )
        .expect("spawn");
        actor.join();

        assert_eq!(
            seen.lock().expect("not poisoned").clone(),
            vec![
                IndexState::Opening { reason: None },
                IndexState::Rebuilding {
                    reason: RebuildReason::NoIndex,
                    done: 0,
                    total: 0,
                    walk_complete: false,
                },
                IndexState::Ready { note_count: 0 },
            ]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_slot_is_filled_before_the_terminal_state_and_rebuilding_still_flows_before_it() {
        // #119, closing the window F-382 named and F-449 measured the wrong thing
        // with. Two rows in one sequence, because the fix has two halves that pull
        // against each other:
        //
        //   Ready       must not be observable while the slot is empty  -> `true`
        //   Rebuilding  must still be observable while it is            -> `false`
        //
        // A "fix" that deferred *every* state until the slot was filled satisfies the
        // first and breaks the second: that is the shell waiting for `Ready`, which
        // §3.4's first precondition forbids and which `mark_index_settled`'s own doc
        // explains is worse than the race it would close. So the `false` on the
        // `Rebuilding` row is exactly as load-bearing as the `true` on the `Ready`
        // one, and neither assertion is worth having without the other.
        //
        // Deterministic rather than raced: the opener blocks on a barrier until the
        // test has handed the observer the slot, so every publish below happens with
        // the slot already reachable and the observation is never "not yet shared".
        struct Scripted {
            start: Arc<std::sync::Barrier>,
            handle: Option<serat_index::IndexHandle>,
        }
        impl IndexOpen for Scripted {
            fn open(&mut self, _root: &Path, _dir: &Path, publish: &dyn Fn(IndexState)) {
                self.start.wait();
                publish(IndexState::Rebuilding {
                    reason: RebuildReason::NoIndex,
                    done: 0,
                    total: 0,
                    walk_complete: false,
                });
                publish(IndexState::Ready { note_count: 0 });
            }

            fn into_handle(self) -> Option<serat_index::IndexHandle> {
                self.handle
            }
        }

        // A real `IndexHandle`, built by the production opener against a real
        // database, rather than a stand-in: what is under test is when the slot is
        // filled, and a slot filled with something the application would never put
        // there proves less than it appears to.
        let root = throwaway_vault("slot-ordering");
        let mut real = SeratIndexOpen::default();
        real.open(&root, &index_dir_for(&root), &|_| {});
        let handle = real
            .into_handle()
            .expect("the production opener produced a handle over a real index");

        let (publisher, _) = publisher(Duration::ZERO);
        let slot_cell: Arc<std::sync::OnceLock<IndexHandleSlot>> =
            Arc::new(std::sync::OnceLock::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let cell = Arc::clone(&slot_cell);
        let sink = Arc::clone(&seen);
        publisher.on_publish(move |state| {
            // `on_publish` and not the sink: the throttle must not be able to decide
            // what this test sees (`the_observer_sees_states_the_throttle_swallows`).
            let filled = cell
                .get()
                .expect("the actor is released only after the slot has been shared")
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some();
            sink.lock()
                .expect("not poisoned")
                .push((state.clone(), filled));
        });

        let start = Arc::new(std::sync::Barrier::new(2));
        let (mut actor, slot) = IndexActor::spawn(
            Arc::clone(&publisher),
            Scripted {
                start: Arc::clone(&start),
                handle: Some(handle),
            },
            root.clone(),
            index_dir_for(&root),
            HandleEpoch::default(),
        )
        .expect("spawn");
        slot_cell.set(Arc::clone(&slot)).expect("shared once");
        start.wait();
        actor.join();

        assert_eq!(
            seen.lock().expect("not poisoned").clone(),
            vec![
                (
                    IndexState::Rebuilding {
                        reason: RebuildReason::NoIndex,
                        done: 0,
                        total: 0,
                        walk_complete: false,
                    },
                    false,
                ),
                (IndexState::Ready { note_count: 0 }, true),
            ],
            "the state that settles the open check must not be observable before the \
             handle it produced is reachable (#119/F-382), and the states that \
             precede it must still be published while the slot is empty (§3.4)"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_the_states_that_end_the_open_check_are_deferred() {
        // code-reviewer's SHOULD-FIX on #119, by mutation: splitting `Unavailable`
        // out of `is_terminal` -- so it publishes from inside `open()` instead of
        // being deferred -- left all 108 tests green. The behavioural tests could not
        // see it, because on the `Unavailable` path `into_handle()` returns `None`,
        // so there is no handle for the publish to race and *both* orderings are
        // correct today. What was unpinned was the **classification**, and pinning it
        // directly is cheaper than proving its consequences.
        //
        // Each row is a decision, and two of them are load-bearing:
        //
        //   `Rebuilding` false -- deferring it would make the shell wait for `Ready`
        //       and hang for the length of a rebuild on exactly the vault where the
        //       window most needs to be up (§3.4's first precondition, and the reason
        //       written above `Startup::mark_index_settled`).
        //
        //   `Unavailable` true -- the deferral is uniform across terminal states *by
        //       design*, not conditional on a handle having appeared. `is_terminal`'s
        //       own doc says so ("its `IndexHandle`, **if it produced one**, exists"),
        //       and until this test nothing held it to that. A later reader who
        //       simplified the arm to just `Ready` would get a green suite, and any
        //       future opener that published `Unavailable` while still yielding a
        //       handle -- a degraded-but-open index is the obvious candidate --
        //       would silently reopen F-382's window on that path with nothing
        //       objecting.
        for (state, expected) in [
            (IndexState::Ready { note_count: 0 }, true),
            (
                IndexState::Unavailable {
                    message: "the rebuild failed".to_owned(),
                    path: "/vault/.serat".to_owned(),
                },
                true,
            ),
            (IndexState::Opening { reason: None }, false),
            (
                IndexState::Rebuilding {
                    reason: RebuildReason::NoIndex,
                    done: 0,
                    total: 0,
                    walk_complete: false,
                },
                false,
            ),
        ] {
            assert_eq!(
                is_terminal(&state),
                expected,
                "{state:?} is classified wrongly: the deferral must cover every \
                 state after which the open check is over, and no state \
                 before it"
            );
        }
    }

    #[test]
    fn an_open_that_produced_no_handle_still_publishes_its_terminal_state() {
        // The other half of #119's deferral, and the reason it is a deferral rather
        // than a condition on the handle existing. `Unavailable` is terminal with
        // nothing to wait for, and `into_handle` returning `None` must not swallow it:
        // if it did, the shell would sit on `Opening` forever on exactly the vault
        // whose index failed, and `mark_index_settled` -- which fires on `Unavailable`
        // as readily as on `Ready` -- would never fire at all, so `t_usable` would
        // never be reached and the vault would never be declared usable.
        struct FailsWithNoHandle;
        impl IndexOpen for FailsWithNoHandle {
            fn open(&mut self, _root: &Path, _dir: &Path, publish: &dyn Fn(IndexState)) {
                publish(IndexState::Unavailable {
                    message: "the rebuild failed".to_owned(),
                    path: "/vault/.serat".to_owned(),
                });
                // into_handle() is not overridden: the trait default returns None.
            }
        }

        let (publisher, recorder) = publisher(Duration::ZERO);
        let (mut actor, slot) = IndexActor::spawn(
            Arc::clone(&publisher),
            FailsWithNoHandle,
            PathBuf::from("."),
            PathBuf::from("./.serat"),
            HandleEpoch::default(),
        )
        .expect("spawn");
        actor.join();

        let expected = IndexState::Unavailable {
            message: "the rebuild failed".to_owned(),
            path: "/vault/.serat".to_owned(),
        };
        assert_eq!(
            publisher.status().state,
            expected,
            "a terminal state held back for the slot must still be published when \
             the slot is never filled"
        );
        assert_eq!(
            recorder
                .taken()
                .iter()
                .map(|s| &s.state)
                .collect::<Vec<_>>(),
            vec![&expected],
            "and emitted exactly once, not swallowed and not duplicated"
        );
        assert!(
            slot.lock().expect("not poisoned").is_none(),
            "there was no handle to fill it with"
        );
    }

    #[test]
    fn the_index_directory_is_dot_serat_under_the_root() {
        assert_eq!(
            index_dir_for(Path::new("/vault")),
            PathBuf::from("/vault").join(".serat")
        );
    }
}
