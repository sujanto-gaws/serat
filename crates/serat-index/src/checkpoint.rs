//! The WAL checkpointer (task #56; architecture §2.2 "WAL checkpoints", §3.1.1 remedy a).
//!
//! # Why it exists
//!
//! SQLite's auto-checkpoint runs inside the writer's `COMMIT`. #53 attributed half of
//! every hub save's cost to it. So the writer sets `wal_autocheckpoint = 0` and this
//! module does the work on **its own connection on its own thread**, running only
//! `wal_checkpoint(PASSIVE)`. It writes no rows, so ADR-0001's single writer of content
//! holds, and a `PASSIVE` checkpoint takes no lock that the writer or a reader waits on.
//!
//! # When it runs
//!
//! * **Idle gate.** [`Bounds::idle`] (250 ms) after the writer's last commit. The writer
//!   signals every commit from its WAL hook; the hook sends one channel message and
//!   returns, so the writer never waits for a checkpoint.
//! * **Size bound.** When the WAL has grown by [`Bounds::size_frames`] (4,000) frames
//!   since the last attempt, whether or not the writer ever pauses: a rebuild, a git
//!   checkout.
//! * **Never on the writer's commit, never inside a transaction.** The checkpointer
//!   connection runs `wal_checkpoint` and nothing else, so it is never inside a
//!   transaction of its own (asserted before every call).
//!
//! # Yielding to work in flight
//!
//! The idle path only *starts* a checkpoint when no work is in flight (task #183). The
//! writer holds a [`BusyGuard`] from the moment it takes a command until that turn's
//! `COMMIT` and reply, and [`Reconciler`](crate::Reconciler) holds one for a whole
//! `index_paths` call; if either is live when the idle gate fires, the checkpoint is
//! deferred (counted in [`CheckpointStats::deferred_busy`]) and the gate is re-armed from
//! now. A `PASSIVE` run cannot be interrupted once started, so this is the only lever:
//! it keeps a run from starting inside a commit or a reconcile round, where on a slow
//! volume its fsyncs compete with the writer's page writes. The pinned `RESTART` (idle
//! only) yields the same way. The size bound does not yield: growth forces a checkpoint
//! however busy the writer is.
//!
//! # The hard bound
//!
//! A `PASSIVE` checkpoint cannot pass a reader's snapshot. The WAL is bounded by an
//! invariant, not by this thread: **no reader holds a read transaction outside one read
//! call** (tested in `read/txn_invariant.rs`). If it is broken anyway, the WAL grows. Past
//! [`Bounds::hard_bytes`] (64 MiB) the checkpointer reports **"index WAL pinned"** as a
//! fact ([`WalHealth::pinned_bytes`], which serat-app puts on `IndexStatus`), and then,
//! 250 ms idle, tries **one** `RESTART` with a busy timeout of at most 50 ms. If it fails
//! the fact stays and there is no further `RESTART` until a checkpoint completes;
//! `PASSIVE` keeps retrying every [`Bounds::retry`] (2 s) while there are new frames,
//! and on every further 4,000 frames of growth.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rusqlite::hooks::Wal;
use rusqlite::{Connection, OpenFlags, TransactionState};

/// The idle gate: how long after the writer's last commit the checkpointer runs.
pub const IDLE_GATE: Duration = Duration::from_millis(250);
/// The size bound, in WAL frames (16 MiB at 4 KiB pages).
pub const SIZE_BOUND_FRAMES: i32 = 4_000;
/// The WAL size past which it is presumed pinned and reported.
pub const HARD_BOUND_BYTES: u64 = 64 * 1024 * 1024;
/// The longest the one `RESTART` may wait for readers. It holds the WAL write lock while
/// it waits, so a save arriving at that moment waits about this plus the checkpoint's
/// own work (a probe measured 56.8 ms).
pub const RESTART_BUSY: Duration = Duration::from_millis(50);
/// The least time between two idle checkpoints (architect ruling, #59): typing with
/// pauses over 250 ms would otherwise checkpoint after every save, two fsyncs each. The
/// size bound and the pinned `RESTART` are not spaced.
pub const IDLE_SPACING: Duration = Duration::from_secs(2);
/// How long to wait before trying again after a checkpoint a reader held back.
const RETRY_AFTER_INCOMPLETE: Duration = Duration::from_secs(2);

/// `journal_size_limit` on the writer: a WAL that grew under load shrinks to this at the
/// next reset (§3.1.1).
pub const WRITER_JOURNAL_SIZE_LIMIT: i64 = 16 * 1024 * 1024;
/// The writer's page cache ceiling, bytes (§2.2 cache table).
pub const WRITER_CACHE_BYTES: i64 = 16 * 1024 * 1024;
/// The search connection's page cache ceiling, bytes (§2.2 cache table).
pub const SEARCH_CACHE_BYTES: i64 = 64 * 1024 * 1024;

/// The checkpointer's three thresholds. [`Bounds::default`] is the ruled values; tests
/// shrink them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// Quiet time after the last commit before a checkpoint runs.
    pub idle: Duration,
    /// WAL frames written since the last attempt that force one.
    pub size_frames: i32,
    /// WAL file size that counts as pinned.
    pub hard_bytes: u64,
    /// How long to wait before trying again after a checkpoint a reader held back.
    pub retry: Duration,
    /// The least time between two idle checkpoints. If the idle gate is reached sooner,
    /// the checkpoint is re-armed for the previous idle one plus this, and runs then if
    /// the writer is still idle.
    pub idle_spacing: Duration,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            idle: IDLE_GATE,
            size_frames: SIZE_BOUND_FRAMES,
            hard_bytes: HARD_BOUND_BYTES,
            retry: RETRY_AFTER_INCOMPLETE,
            idle_spacing: IDLE_SPACING,
        }
    }
}

/// What the checkpointer has done, for the benchmarks and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CheckpointStats {
    /// `PASSIVE` checkpoints run.
    pub passive_runs: u64,
    /// Of those, how many a reader (or another checkpoint) kept from finishing.
    pub incomplete: u64,
    /// `RESTART` attempts.
    pub restart_runs: u64,
    /// Longest single `PASSIVE`, microseconds.
    pub passive_max_micros: u64,
    /// Sum of every `PASSIVE`, microseconds.
    pub passive_total_micros: u64,
    /// Frames moved into the database by all `PASSIVE` runs together.
    pub frames_checkpointed: u64,
    /// Shortest time between the starts of two idle `PASSIVE` runs, microseconds; 0 until
    /// there have been two.
    pub idle_min_gap_micros: u64,
    /// Idle checkpoints deferred because the writer or a reconcile round was in flight.
    pub deferred_busy: u64,
}

type Notify = Arc<dyn Fn() + Send + Sync>;

/// What the checkpointer knows, readable from any thread without touching SQLite.
///
/// All atomics but one leaf mutex holding the change callback; no connection is behind
/// it.
#[derive(Default)]
pub struct WalHealth {
    /// 0 = not pinned.
    pinned_bytes: AtomicU64,
    running: AtomicBool,
    /// The WAL hook could not be installed (rusqlite 0.41.1: out of memory only), so
    /// the writer fell back to SQLite's own auto-checkpoint.
    hook_install_failed: AtomicBool,
    passive_runs: AtomicU64,
    incomplete: AtomicU64,
    restart_runs: AtomicU64,
    passive_max_micros: AtomicU64,
    passive_total_micros: AtomicU64,
    frames_checkpointed: AtomicU64,
    idle_min_gap_micros: AtomicU64,
    /// Live [`BusyGuard`]s: writer turns and reconcile rounds in flight.
    busy: AtomicUsize,
    deferred_busy: AtomicU64,
    on_change: Mutex<Option<Notify>>,
}

impl std::fmt::Debug for WalHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalHealth")
            .field("pinned_bytes", &self.pinned_bytes())
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl WalHealth {
    /// The WAL's size in bytes when it is pinned ("index WAL pinned"), `None` when it
    /// is not. Never an error: a pinned WAL is a defect against the no-open-read-
    /// transaction invariant, made visible rather than left as unexplained disk use.
    #[must_use]
    pub fn pinned_bytes(&self) -> Option<u64> {
        match self.pinned_bytes.load(Ordering::Relaxed) {
            0 => None,
            bytes => Some(bytes),
        }
    }

    /// Whether the writer's WAL hook failed to install and SQLite's own
    /// auto-checkpoint (1000 pages) was restored instead.
    #[must_use]
    pub fn hook_install_failed(&self) -> bool {
        self.hook_install_failed.load(Ordering::Relaxed)
    }

    /// Whether a checkpoint is running right now.
    #[must_use]
    pub fn checkpoint_in_progress(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Counters since the checkpointer started.
    #[must_use]
    pub fn stats(&self) -> CheckpointStats {
        CheckpointStats {
            passive_runs: self.passive_runs.load(Ordering::Relaxed),
            incomplete: self.incomplete.load(Ordering::Relaxed),
            restart_runs: self.restart_runs.load(Ordering::Relaxed),
            passive_max_micros: self.passive_max_micros.load(Ordering::Relaxed),
            passive_total_micros: self.passive_total_micros.load(Ordering::Relaxed),
            frames_checkpointed: self.frames_checkpointed.load(Ordering::Relaxed),
            idle_min_gap_micros: self.idle_min_gap_micros.load(Ordering::Relaxed),
            deferred_busy: self.deferred_busy.load(Ordering::Relaxed),
        }
    }

    /// Mark work in flight until the guard drops: an idle checkpoint will not start
    /// meanwhile (it is deferred, see the module docs).
    #[must_use]
    pub fn busy_guard(self: &Arc<Self>) -> BusyGuard {
        self.busy.fetch_add(1, Ordering::Relaxed);
        BusyGuard(Arc::clone(self))
    }

    /// Whether any [`BusyGuard`] is live.
    #[must_use]
    pub fn work_in_flight(&self) -> bool {
        self.busy.load(Ordering::Relaxed) > 0
    }

    /// Call `f` (from the checkpointer thread) whenever the pinned fact appears,
    /// changes or clears, so a status that carries it can be re-emitted. `f` must not
    /// block and must not call back into this type.
    pub fn on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        *self
            .on_change
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(f));
    }

    /// Set or clear the pinned fact as the checkpointer would, so serat-app's tests can
    /// drive the status path without growing a 64 MiB WAL. Not for production use.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_pinned_for_test(&self, bytes: Option<u64>) {
        self.set_pinned(bytes);
    }

    fn set_pinned(&self, bytes: Option<u64>) {
        let new = bytes.unwrap_or(0);
        let old = self.pinned_bytes.swap(new, Ordering::Relaxed);
        // Notify only when the fact appears or clears. A size that merely moves is
        // carried by the next poll or status, and a growing WAL would otherwise emit a
        // status per attempt.
        if (old == 0) == (new == 0) {
            return;
        }
        let callback = self
            .on_change
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(callback) = callback {
            callback();
        }
    }

    fn record_passive(&self, took: Duration, complete: bool) {
        let micros = u64::try_from(took.as_micros()).unwrap_or(u64::MAX);
        self.passive_runs.fetch_add(1, Ordering::Relaxed);
        self.passive_total_micros
            .fetch_add(micros, Ordering::Relaxed);
        self.passive_max_micros.fetch_max(micros, Ordering::Relaxed);
        if !complete {
            self.incomplete.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Work in flight, for as long as it lives. See [`WalHealth::busy_guard`].
#[derive(Debug)]
pub struct BusyGuard(Arc<WalHealth>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.busy.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The writer's pragmas (§2.2, §3.1.1): `journal_size_limit` 16 MiB, `cache_size` 16 MiB
/// and, when a checkpointer is running, `wal_autocheckpoint = 0`. Returns whether the
/// auto-checkpoint is now off.
pub(crate) fn tune_writer(conn: &Connection, checkpointer: bool) -> bool {
    let _ = conn.execute_batch(&format!(
        "PRAGMA journal_size_limit = {WRITER_JOURNAL_SIZE_LIMIT};
         PRAGMA cache_size = -{};",
        WRITER_CACHE_BYTES / 1024
    ));
    checkpointer && conn.execute_batch("PRAGMA wal_autocheckpoint = 0;").is_ok()
}

enum Msg {
    /// The writer committed; the WAL now holds this many frames.
    Commit {
        frames: i32,
    },
    Stop,
}

/// Called from the writer's WAL hook, on the writer thread, with the `Wal` and the frame
/// count. For the benchmarks: `p1-save-tail` stamps the instant `COMMIT`'s own work is
/// done. A plain `fn`, because `rusqlite`'s hook is.
pub type WalObserver = fn(&mut Wal, i32) -> rusqlite::Result<()>;

/// The writer's side: what its WAL hook needs. Set on the writer thread, because the
/// hook is a plain `fn` and runs on whichever thread commits.
struct HookState {
    tx: Option<Sender<Msg>>,
    observer: Option<WalObserver>,
}

thread_local! {
    static HOOK: std::cell::RefCell<Option<HookState>> = const { std::cell::RefCell::new(None) };
}

fn on_wal(wal: &mut Wal, frames: std::os::raw::c_int) -> rusqlite::Result<()> {
    HOOK.with(|hook| {
        if let Some(state) = hook.borrow().as_ref() {
            if let Some(observer) = state.observer {
                let _ = observer(wal, frames);
            }
            if let Some(tx) = &state.tx {
                // Unbounded and non-blocking: the writer never waits for a checkpoint.
                let _ = tx.send(Msg::Commit { frames });
            }
        }
    });
    Ok(())
}

/// What the writer thread carries in: where to signal commits, and an optional observer.
#[derive(Debug)]
pub(crate) struct WriterHook {
    observer: Option<WalObserver>,
    signal: Option<Sender<Msg>>,
}

impl WriterHook {
    /// A hook with only an observer (no checkpointer behind it).
    pub(crate) fn observer_only(observer: WalObserver) -> Self {
        Self {
            observer: Some(observer),
            signal: None,
        }
    }

    /// Install on the writer thread: replaces SQLite's own auto-checkpoint with the
    /// signal (and the observer, when there is one).
    pub(crate) fn install(self, conn: &mut Connection, health: &WalHealth) {
        HOOK.with(|hook| {
            *hook.borrow_mut() = Some(HookState {
                tx: self.signal,
                observer: self.observer,
            });
        });
        // `tune_writer` set wal_autocheckpoint = 0, so a failed install (rusqlite 0.41.1:
        // out of memory only) would leave no auto-checkpoint and no Commit signals.
        // Clear the hook (no allocation; also drops one left pointing at freed data),
        // restore SQLite's own auto-checkpoint, and record it.
        if conn.wal_hook(Some(on_wal)).is_err() {
            let _ = conn.wal_hook(None::<WalObserver>);
            let _ = conn.execute_batch("PRAGMA wal_autocheckpoint = 1000;");
            health.hook_install_failed.store(true, Ordering::Relaxed);
        }
    }
}

/// The checkpointer thread and the means to stop it.
#[derive(Debug)]
pub(crate) struct Checkpointer {
    tx: Sender<Msg>,
    join: Option<JoinHandle<()>>,
}

impl Checkpointer {
    /// Open the checkpointer's own connection to the database at `db_path` and start its
    /// thread. `None` when it cannot (an in-memory database, an unopenable file): the
    /// caller then leaves SQLite's auto-checkpoint alone, which is the old behaviour --
    /// capability degrades, nothing breaks (design goal 5).
    pub(crate) fn start(db_path: &Path, bounds: Bounds, health: Arc<WalHealth>) -> Option<Self> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;
        // PASSIVE never invokes the busy handler; the RESTART sets its own.
        conn.busy_timeout(Duration::ZERO).ok()?;
        let (tx, rx) = channel();
        let wal_path = wal_path(db_path);
        let join = std::thread::Builder::new()
            .name("serat-index-checkpointer".to_owned())
            .spawn(move || run(&conn, &rx, bounds, &health, &wal_path))
            .ok()?;
        Some(Self {
            tx,
            join: Some(join),
        })
    }

    /// The writer-thread half: the hook that signals this checkpointer.
    pub(crate) fn writer_hook(&self, observer: Option<WalObserver>) -> WriterHook {
        WriterHook {
            observer,
            signal: Some(self.tx.clone()),
        }
    }

    /// Stop the thread and wait for it. An in-flight checkpoint finishes first.
    pub(crate) fn stop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn wal_path(db_path: &Path) -> PathBuf {
    let mut name = db_path.as_os_str().to_owned();
    name.push("-wal");
    PathBuf::from(name)
}

/// The checkpointer's own state between messages.
struct State {
    /// Commits since the last attempt finished completely.
    dirty: bool,
    last_commit: Instant,
    /// Frames at the last attempt; the size bound counts growth beyond it.
    base: i32,
    /// Frames in the WAL as of the last commit signal.
    frames: i32,
    /// Bytes per WAL frame (page size + 24-byte frame header).
    frame_bytes: u64,
    /// Do not attempt again before this (after an incomplete checkpoint).
    retry_at: Option<Instant>,
    /// The one `RESTART` of this pinned episode has been spent.
    restart_spent: bool,
    /// When the last idle `PASSIVE` started.
    last_idle: Option<Instant>,
    /// Frames backfilled as of the last result, to count what each run moved.
    prev_done: i64,
}

impl State {
    /// When the next idle `PASSIVE` may run: the idle gate, no sooner than the retry
    /// back-off and the spacing from the previous idle one.
    fn passive_at(&self, bounds: Bounds) -> Instant {
        let mut at = self.last_commit + bounds.idle;
        if let Some(retry) = self.retry_at {
            at = at.max(retry);
        }
        if let Some(last) = self.last_idle {
            at = at.max(last + bounds.idle_spacing);
        }
        at
    }

    /// When the next idle action is due, if any: the `PASSIVE` (spaced) or the pinned
    /// `RESTART` (the idle gate only).
    fn due(&self, bounds: Bounds, pinned: bool) -> Option<Instant> {
        let passive = self.dirty.then(|| self.passive_at(bounds));
        let restart = (pinned && !self.restart_spent).then(|| self.last_commit + bounds.idle);
        match (passive, restart) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

fn run(conn: &Connection, rx: &Receiver<Msg>, bounds: Bounds, health: &WalHealth, wal: &Path) {
    let page_size: u64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))
        .ok()
        .and_then(|size| u64::try_from(size).ok())
        .unwrap_or(4096);
    let mut state = State {
        dirty: false,
        last_commit: Instant::now(),
        base: 0,
        frames: 0,
        frame_bytes: page_size + 24,
        retry_at: None,
        restart_spent: false,
        last_idle: None,
        prev_done: 0,
    };
    loop {
        let pinned = health.pinned_bytes().is_some();
        let message = match state.due(bounds, pinned) {
            None => match rx.recv() {
                Ok(message) => Some(message),
                Err(_) => return,
            },
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(message) => Some(message),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
        };
        match message {
            Some(Msg::Stop) => return,
            Some(Msg::Commit { frames }) => {
                state.last_commit = Instant::now();
                state.dirty = true;
                if frames < state.base {
                    // The writer reset the WAL; count growth from its start again.
                    state.base = 0;
                }
                if frames < state.frames {
                    // SQLite's backfilled-frame count restarts from zero with the WAL.
                    state.prev_done = 0;
                }
                state.frames = frames;
                if frames - state.base >= bounds.size_frames {
                    attempt(conn, &mut state, bounds, health, wal, false);
                }
            }
            // Idle: nothing has committed for the gate.
            None => attempt(conn, &mut state, bounds, health, wal, true),
        }
    }
}

/// One `PASSIVE` checkpoint, the pinned report, and, at idle only, the one `RESTART`.
fn attempt(
    conn: &Connection,
    state: &mut State,
    bounds: Bounds,
    health: &WalHealth,
    wal: &Path,
    idle: bool,
) {
    debug_assert_eq!(
        conn.transaction_state::<&str>(None).ok(),
        Some(TransactionState::None),
        "the checkpointer connection must never be inside a transaction"
    );
    if idle && health.work_in_flight() {
        // Yield: re-arm the idle gate from now. The writer's next commit signal or the
        // gate wakes the thread again; nothing is lost, `dirty` stays set.
        health.deferred_busy.fetch_add(1, Ordering::Relaxed);
        state.last_commit = Instant::now();
        return;
    }
    let spaced_out = !idle || Instant::now() >= state.passive_at(bounds);
    if state.dirty && spaced_out {
        if idle {
            if let Some(last) = state.last_idle {
                let gap = u64::try_from(last.elapsed().as_micros()).unwrap_or(u64::MAX);
                let _ = health.idle_min_gap_micros.fetch_update(
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                    |old| (old == 0 || gap < old).then_some(gap.max(1)),
                );
            }
            state.last_idle = Some(Instant::now());
        }
        health.running.store(true, Ordering::Relaxed);
        let started = Instant::now();
        let outcome = checkpoint(conn, "PASSIVE");
        let took = started.elapsed();
        health.running.store(false, Ordering::Relaxed);
        let complete = outcome.is_some_and(|(busy, log, done)| busy == 0 && log == done);
        health.record_passive(took, complete);
        if let Some((_, _, done)) = outcome {
            // A smaller count than before means the writer reset the WAL in between.
            let moved = if done >= state.prev_done {
                done - state.prev_done
            } else {
                done
            };
            state.prev_done = done;
            health
                .frames_checkpointed
                .fetch_add(u64::try_from(moved).unwrap_or(0), Ordering::Relaxed);
        }
        // On an error there is no frame count from SQLite; use the last commit's, so a
        // failing checkpoint is not re-fired by every later commit.
        state.base = outcome.map_or(state.frames, |(_, log, _)| {
            i32::try_from(log).unwrap_or(0).max(0)
        });
        if complete {
            state.dirty = false;
            state.retry_at = None;
            state.restart_spent = false;
            health.set_pinned(None);
        } else {
            state.retry_at = Some(Instant::now() + bounds.retry);
            let size = wal_bytes(outcome, state.frame_bytes, wal);
            health.set_pinned((size > bounds.hard_bytes).then_some(size));
        }
    }
    // Only at idle, only once per pinned episode.
    if idle && health.pinned_bytes().is_some() && !state.restart_spent {
        state.restart_spent = true;
        health.restart_runs.fetch_add(1, Ordering::Relaxed);
        let _ = conn.busy_timeout(RESTART_BUSY);
        health.running.store(true, Ordering::Relaxed);
        let outcome = checkpoint(conn, "RESTART");
        health.running.store(false, Ordering::Relaxed);
        let _ = conn.busy_timeout(Duration::ZERO);
        if outcome.is_some_and(|(busy, log, done)| busy == 0 && log == done) {
            state.dirty = false;
            state.retry_at = None;
            state.restart_spent = false;
            health.set_pinned(None);
        } else {
            // The fact stays, with the size as it is now.
            let size = wal_bytes(outcome, state.frame_bytes, wal);
            health.set_pinned(Some(size.max(1)));
        }
    }
}

/// The WAL's logical size from a checkpoint result: frames in use times frame size, plus
/// the 32-byte header. The file's length is only a high-water mark (it keeps its size
/// after a reset until `journal_size_limit` trims it), so it would report a pin that is
/// long gone; it is the fallback when SQLite gave no count.
fn wal_bytes(outcome: Option<(i64, i64, i64)>, frame_bytes: u64, wal: &Path) -> u64 {
    match outcome {
        Some((_, log, _)) if log >= 0 => u64::try_from(log).unwrap_or(0) * frame_bytes + 32,
        _ => std::fs::metadata(wal).map_or(0, |meta| meta.len()),
    }
}

/// `PRAGMA wal_checkpoint(<mode>)`: (busy, frames in the WAL, frames checkpointed).
fn checkpoint(conn: &Connection, mode: &str) -> Option<(i64, i64, i64)> {
    conn.query_row(&format!("PRAGMA wal_checkpoint({mode})"), [], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    })
    .ok()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serat_core::{ContentHash, IndexOp, IndexWriter as _, NoteKind, NoteMeta, VaultPath};

    use super::*;
    use crate::schema;
    use crate::writer::{ActorOptions, IndexActor};

    fn asset(i: usize) -> IndexOp {
        IndexOp::UpsertAsset {
            meta: NoteMeta {
                kind: NoteKind::Asset,
                hash: ContentHash::from_bytes([2u8; 32]),
                size: 0,
                mtime_ms: 0,
                path: VaultPath::new(format!("assets/a{i}.pdf")).expect("path"),
            },
        }
    }

    fn actor(bounds: Bounds) -> (tempfile::TempDir, PathBuf, IndexActor) {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("index.db");
        let conn = schema::open(&path).expect("open");
        schema::bootstrap(&conn).expect("bootstrap");
        let actor = IndexActor::with_options(
            conn,
            ActorOptions {
                bounds,
                ..ActorOptions::default()
            },
        );
        (dir, path, actor)
    }

    fn save(actor: &IndexActor, i: usize) {
        let writer = actor.handle();
        writer.submit(asset(i)).expect("submit");
        writer.flush().expect("flush");
    }

    fn len(path: &Path) -> u64 {
        std::fs::metadata(path).map_or(0, |meta| meta.len())
    }

    fn wait_for(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
        let started = Instant::now();
        while !done() {
            assert!(started.elapsed() < limit, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Bounds that never fire on their own, so a test enables exactly one.
    fn never() -> Bounds {
        Bounds {
            idle: Duration::from_secs(3_600),
            size_frames: i32::MAX,
            hard_bytes: u64::MAX,
            retry: Duration::from_millis(100),
            idle_spacing: Duration::ZERO,
        }
    }

    #[test]
    fn the_writer_does_not_auto_checkpoint() {
        // With SQLite's default the writer checkpoints inside COMMIT every ~1000
        // frames, whatever the checkpointer does. With the pragma at 0 and every
        // checkpointer bound out of reach, the database file must not grow while the
        // WAL does.
        let (dir, path, actor) = actor(never());
        let db_before = len(&path);
        let wal = wal_path(&path);
        let mut i = 0;
        while len(&wal) < 2_000 * 4_120 {
            save(&actor, i);
            i += 1;
            assert!(i < 20_000, "the WAL never reached 2,000 frames");
        }
        assert_eq!(
            len(&path),
            db_before,
            "something checkpointed: the database file grew"
        );
        assert_eq!(actor.wal_health().stats().passive_runs, 0);
        actor.shutdown().expect("shutdown");
        drop(dir);
    }

    /// Task #182 (NFR-REL-01, NFR-MNT-01): the "not before" half of the idle gate, decided
    /// on the scheduler's own state with instants the test makes up, not on how long a
    /// loop of saves happened to take. It used to commit every 30 ms against a 300 ms gate
    /// and assert that nothing ran: one stall of the whole process between two saves (a
    /// loaded runner, a recompile beside it) let the gate honestly expire, and it failed
    /// (seen 2026-11-24, reproduced under two pinned CPU loops).
    #[test]
    fn the_idle_gate_is_measured_from_the_last_commit_and_each_commit_moves_it() {
        let bounds = Bounds {
            idle: Duration::from_millis(300),
            ..never()
        };
        let t0 = Instant::now();
        let mut state = State {
            dirty: false,
            last_commit: t0,
            base: 0,
            frames: 0,
            frame_bytes: 4120,
            retry_at: None,
            restart_spent: false,
            last_idle: None,
            prev_done: 0,
        };
        // Nothing committed since the last attempt: nothing is due, however long it waits.
        assert_eq!(state.due(bounds, false), None);

        // A commit makes the idle checkpoint due one gate later, and no sooner.
        state.dirty = true;
        assert_eq!(state.due(bounds, false), Some(t0 + bounds.idle));

        // Each later commit inside the gate pushes it out again; the first deadline,
        // which a run of saves 30 ms apart would have been racing, is never reached.
        for step in 1..=10u32 {
            state.last_commit = t0 + Duration::from_millis(30) * step;
            assert_eq!(
                state.due(bounds, false),
                Some(t0 + Duration::from_millis(30) * step + bounds.idle),
                "commit {step} did not move the gate"
            );
        }

        // The pinned RESTART is gated the same way, and only while it is unspent.
        assert_eq!(
            state.due(bounds, true),
            Some(state.last_commit + bounds.idle)
        );
        state.dirty = false;
        state.restart_spent = true;
        assert_eq!(state.due(bounds, true), None);
    }

    /// The "fires" half, on the live actor: an event wait (the checkpoint counter), whose
    /// only clock is the bound that turns a hang into a failure.
    #[test]
    fn it_fires_once_the_writer_has_been_idle() {
        let (dir, path, actor) = actor(Bounds {
            idle: Duration::from_millis(20),
            ..never()
        });
        let health = actor.wal_health();
        // Before the saves: under a 20 ms gate the checkpoint may run as soon as they end.
        let db_before = len(&path);
        for i in 0..5 {
            save(&actor, i);
        }
        wait_for("the idle checkpoint", Duration::from_secs(30), || {
            health.stats().passive_runs >= 1
        });
        assert!(len(&path) > db_before, "the checkpoint copied nothing");
        assert_eq!(health.pinned_bytes(), None);
        actor.shutdown().expect("shutdown");
        drop(dir);
    }

    #[test]
    fn an_idle_checkpoint_yields_to_work_in_flight_and_runs_when_it_ends() {
        // Task #183 (FR-VLT-04): an idle checkpoint does not start inside a writer turn
        // or a reconcile round. No wall-clock assertion: the deferral is a counted event
        // the test waits for, and no checkpoint can run while the guard is live.
        let (dir, _path, actor) = actor(Bounds {
            idle: Duration::from_millis(20),
            ..never()
        });
        let health = actor.wal_health();
        let busy = health.busy_guard();
        assert!(health.work_in_flight());
        save(&actor, 0);
        wait_for("a deferral", Duration::from_secs(30), || {
            health.stats().deferred_busy >= 1
        });
        assert_eq!(
            health.stats().passive_runs,
            0,
            "it ran under work in flight"
        );
        // The data is still owed: ending the work lets the re-armed gate fire.
        drop(busy);
        wait_for("the checkpoint", Duration::from_secs(30), || {
            health.stats().passive_runs >= 1
        });
        assert!(!health.work_in_flight());
        actor.shutdown().expect("shutdown");
        drop(dir);
    }

    #[test]
    fn the_writer_counts_as_work_in_flight_from_taking_a_command_to_its_commit() {
        let (dir, _path, actor) = actor(never());
        let health = actor.wal_health();
        assert!(!health.work_in_flight());
        let hold = actor.commit_gate().hold();
        actor.handle().submit(asset(0)).expect("submit");
        assert!(hold.wait_reached(Duration::from_secs(30)), "never reached");
        assert!(health.work_in_flight(), "the writer is mid-commit");
        drop(hold);
        actor.handle().flush().expect("flush");
        wait_for("the turn to end", Duration::from_secs(30), || {
            !health.work_in_flight()
        });
        actor.shutdown().expect("shutdown");
        drop(dir);
    }

    #[test]
    fn it_fires_on_the_size_bound_though_the_writer_never_pauses() {
        // Idle gate out of reach; only the 40-frame bound can fire.
        let (dir, _path, actor) = actor(Bounds {
            size_frames: 40,
            ..never()
        });
        let health = actor.wal_health();
        let mut i = 0;
        while health.stats().passive_runs == 0 {
            save(&actor, i);
            i += 1;
            assert!(i < 2_000, "the size bound never fired");
        }
        actor.shutdown().expect("shutdown");
        drop(dir);
    }

    #[test]
    fn it_runs_outside_a_transaction_and_stops_with_the_actor() {
        // The debug_assert in `attempt` is the check that the checkpointer connection
        // is never inside a transaction; this drives enough attempts for it to matter,
        // then shuts down and proves the thread is gone.
        let (dir, path, actor) = actor(Bounds {
            idle: Duration::from_millis(20),
            size_frames: 10,
            ..never()
        });
        let health = actor.wal_health();
        for i in 0..200 {
            save(&actor, i);
        }
        wait_for("some checkpoints", Duration::from_secs(5), || {
            health.stats().passive_runs >= 3
        });
        actor.shutdown().expect("shutdown");
        let runs = health.stats().passive_runs;
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            health.stats().passive_runs,
            runs,
            "the checkpointer kept running after the actor was shut down"
        );
        // No connection of ours is left: the database opens, and the rows are all there.
        let conn = schema::open(&path).expect("reopen");
        let notes: i64 = conn
            .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
            .expect("count");
        assert_eq!(notes, 200);
        drop(dir);
    }

    #[test]
    fn a_pinned_wal_is_reported_once_restarted_once_and_the_fact_stays_until_it_clears() {
        // 1,000 saves with a reader parked inside a read transaction: the invariant
        // broken on purpose. The hard bound is shrunk to 256 KiB so 1,000 small saves
        // pass it.
        let (dir, path, actor) = actor(Bounds {
            idle: Duration::from_millis(50),
            size_frames: 100,
            hard_bytes: 256 * 1024,
            retry: Duration::from_millis(100),
            idle_spacing: Duration::ZERO,
        });
        let health = actor.wal_health();
        let parked = Connection::open(&path).expect("reader");
        parked
            .execute_batch("BEGIN; SELECT count(*) FROM notes;")
            .expect("park");

        let changes = Arc::new(AtomicU64::new(0));
        let seen = Arc::clone(&changes);
        health.on_change(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });

        for i in 0..1_000 {
            save(&actor, i);
        }
        let wal = wal_path(&path);
        assert!(
            len(&wal) > 256 * 1024,
            "the WAL did not grow past the bound"
        );

        // Reported, with the WAL's size, and not as an error.
        wait_for("the pinned report", Duration::from_secs(5), || {
            health.pinned_bytes().is_some()
        });
        assert!(health.pinned_bytes().expect("pinned") > 256 * 1024);
        assert!(
            changes.load(Ordering::SeqCst) >= 1,
            "no change notification"
        );

        // One RESTART at idle, which a parked reader makes fail; then nothing else is
        // tried however long it stays pinned.
        wait_for("the one RESTART", Duration::from_secs(5), || {
            health.stats().restart_runs >= 1
        });
        std::thread::sleep(Duration::from_millis(700));
        assert_eq!(health.stats().restart_runs, 1, "RESTART was tried again");
        assert!(health.pinned_bytes().is_some(), "the fact did not stay");
        assert!(health.stats().incomplete >= 1);

        // The reader lets go; the next retry completes, and the fact clears.
        parked.execute_batch("COMMIT").expect("release");
        wait_for("the fact to clear", Duration::from_secs(5), || {
            health.pinned_bytes().is_none()
        });
        let conn = schema::open(&path).expect("reopen");
        let notes: i64 = conn
            .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
            .expect("count");
        assert_eq!(notes, 1_000);
        actor.shutdown().expect("shutdown");
        drop(dir);
    }

    #[test]
    fn idle_checkpoints_are_spaced_and_a_trailing_one_fires_after_typing_stops() {
        // The idle gate (50 ms) is reached between every pair of saves 100 ms apart, so
        // without spacing every save would be checkpointed. With 600 ms spacing,
        // 2 s of typing gives at most four, never closer than the spacing, and a pause
        // still gets a checkpoint within the spacing.
        let spacing = Duration::from_millis(600);
        let (dir, _path, actor) = actor(Bounds {
            idle: Duration::from_millis(50),
            idle_spacing: spacing,
            ..never()
        });
        let health = actor.wal_health();
        let typed_from = Instant::now();
        let typing_until = typed_from + Duration::from_millis(2_000);
        let mut i = 0;
        while Instant::now() < typing_until {
            save(&actor, i);
            i += 1;
            std::thread::sleep(Duration::from_millis(100));
        }
        let during = health.stats();
        // Task #182: the ceiling is taken from the time the loop actually ran, not from the
        // 2 s it meant to: a stall that stretched the loop past 2 s legitimately allows a
        // further checkpoint, and a fixed "at most 4" failed on it. Spacing is the claim:
        // one idle checkpoint per spacing, plus the first.
        let ran = typed_from.elapsed();
        let ceiling = ran.as_micros() / spacing.as_micros() + 1;
        assert!(
            during.passive_runs >= 2,
            "no idle checkpoint while typing: {during:?} over {ran:?}"
        );
        assert!(
            u128::from(during.passive_runs) <= ceiling,
            "{} checkpoints in {ran:?} at 600 ms spacing (at most {ceiling})",
            during.passive_runs
        );
        // One more save, then the writer stops: its frames are not yet checkpointed, and
        // a trailing checkpoint fires within the spacing.
        let before = health.stats().passive_runs;
        save(&actor, i);
        wait_for("the trailing checkpoint", Duration::from_secs(30), || {
            health.stats().passive_runs > before
        });
        let after = health.stats();
        assert!(
            Duration::from_micros(after.idle_min_gap_micros) >= spacing,
            "two idle checkpoints {} us apart; spacing is {spacing:?}",
            after.idle_min_gap_micros
        );
        assert!(
            after.frames_checkpointed >= i as u64,
            "{} frames checkpointed for {i} saves",
            after.frames_checkpointed
        );
        actor.shutdown().expect("shutdown");
        drop(dir);
    }

    #[test]
    fn an_in_memory_writer_starts_no_checkpointer_and_keeps_sqlites_own() {
        let conn = schema::open_in_memory().expect("open");
        schema::bootstrap(&conn).expect("bootstrap");
        let actor = IndexActor::with_connection(conn);
        save(&actor, 0);
        assert_eq!(actor.wal_health().stats().passive_runs, 0);
        assert!(actor.checkpointer.is_none());
        actor.shutdown().expect("shutdown");
    }

    #[test]
    fn the_writer_and_search_page_caches_are_the_ruled_sizes() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("index.db");
        let conn = schema::open(&path).expect("open");
        schema::bootstrap(&conn).expect("bootstrap");
        let _ = tune_writer(&conn, true);
        let kib = |conn: &Connection, pragma: &str| -> i64 {
            conn.query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
                .expect("pragma")
        };
        assert_eq!(kib(&conn, "cache_size"), -16 * 1024);
        assert_eq!(kib(&conn, "journal_size_limit"), WRITER_JOURNAL_SIZE_LIMIT);
        assert_eq!(kib(&conn, "wal_autocheckpoint"), 0);
        let search = crate::SearchConn::open(&path).expect("search");
        search.with_connection(|conn| {
            assert_eq!(kib(conn, "cache_size"), -64 * 1024);
        });
        // The others stay at SQLite's default (-2000 KiB).
        let reader = crate::Reader::open(&path).expect("reader");
        reader.with_connection(|conn| assert_eq!(kib(conn, "cache_size"), -2_000));
    }
}
