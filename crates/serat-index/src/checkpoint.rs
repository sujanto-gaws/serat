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
//! * **Frame floor** (task #199, architecture §3.1.1 N8). An idle `PASSIVE` (and the
//!   idle-only pinned `RESTART`) runs only once the WAL has grown by
//!   [`Bounds::idle_floor_frames`] ([`IDLE_FLOOR_FRAMES`], 1,000) since the last
//!   `PASSIVE` that ran, idle or size-bound: the same baseline the size bound counts
//!   from. A run's cost is its two flushes, not its frames, so a run that moves a few
//!   commits is mostly flush. A deferral to work in flight does not move the baseline.
//!   A wake below the floor counts [`CheckpointStats::floor_skips`] and **disarms** the
//!   idle timer until the next commit, so nothing spins. The size bound is never
//!   floored, and `close()`'s `TRUNCATE` is not this thread's.
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
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
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
/// The frames the WAL must have grown by since the last `PASSIVE` before an idle one
/// runs (task #198 ruling, #199): the batch the auto-checkpoint gave before #56, a
/// quarter of the size bound, above one hub save (682 frames).
pub const IDLE_FLOOR_FRAMES: i32 = 1_000;
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
    /// WAL growth since the last `PASSIVE` below which an idle wake does nothing and
    /// waits for the next commit. 0 turns the floor off.
    pub idle_floor_frames: i32,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            idle: IDLE_GATE,
            size_frames: SIZE_BOUND_FRAMES,
            hard_bytes: HARD_BOUND_BYTES,
            retry: RETRY_AFTER_INCOMPLETE,
            idle_spacing: IDLE_SPACING,
            idle_floor_frames: IDLE_FLOOR_FRAMES,
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
    /// Idle wake-ups deferred because the writer or an `index_paths` round was in flight
    /// (including ones where only the pinned `RESTART` was due).
    pub deferred_busy: u64,
    /// Of `passive_runs`, how many were idle (the rest were the size bound).
    pub idle_runs: u64,
    /// Of `passive_runs`, how many were the size bound.
    pub size_runs: u64,
    /// Frames moved by the idle `PASSIVE` runs alone (task #199: frames per idle run).
    pub idle_frames: u64,
    /// Idle wake-ups below the frame floor that did nothing (task #199).
    pub floor_skips: u64,
}

/// Which trigger started a checkpoint run (task #199): the bench tags report it, because
/// an idle run overlapping a save is a defect of the idle path and a size-bound one is
/// an architect matter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    /// The idle gate: an idle `PASSIVE`, or the pinned `RESTART` (idle only).
    Idle,
    /// The size bound.
    SizeBound,
}

impl RunKind {
    /// `idle` or `size-bound`, as the tag line prints it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::SizeBound => "size-bound",
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Idle => 1,
            Self::SizeBound => 2,
        }
    }
}

/// Which kind of checkpoint run overlapped a window: one was running when it opened or
/// closed (`running_*`), or the run counters moved between (`before`, `after`). Idle wins
/// when both kinds are seen (the pinned `RESTART` counts as idle). Pure, so the harness
/// and the tests share it.
#[must_use]
pub fn overlap_kind(
    running_at_start: Option<RunKind>,
    running_at_end: Option<RunKind>,
    before: &CheckpointStats,
    after: &CheckpointStats,
) -> Option<RunKind> {
    let idle = after.idle_runs + after.restart_runs != before.idle_runs + before.restart_runs;
    let size = after.size_runs != before.size_runs;
    let seen = |kind: RunKind| {
        running_at_start == Some(kind)
            || running_at_end == Some(kind)
            || match kind {
                RunKind::Idle => idle,
                RunKind::SizeBound => size,
            }
    };
    if seen(RunKind::Idle) {
        Some(RunKind::Idle)
    } else if seen(RunKind::SizeBound) {
        Some(RunKind::SizeBound)
    } else {
        None
    }
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
    /// 0 = none; else [`RunKind::code`] of the run in progress.
    running: AtomicU8,
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
    idle_runs: AtomicU64,
    size_runs: AtomicU64,
    idle_frames: AtomicU64,
    floor_skips: AtomicU64,
    /// Finished [`BusyGuard`] turns that lasted at least [`SLOW_TURN`], oldest first,
    /// capped at [`SLOW_TURNS_KEPT`] (task #196). Written only when a slow turn ends.
    slow_turns: Mutex<Vec<(Instant, Instant)>>,
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
        self.running.load(Ordering::Relaxed) != 0
    }

    /// The trigger of the checkpoint running right now, if one is.
    #[must_use]
    pub fn checkpoint_kind_running(&self) -> Option<RunKind> {
        match self.running.load(Ordering::Relaxed) {
            1 => Some(RunKind::Idle),
            2 => Some(RunKind::SizeBound),
            _ => None,
        }
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
            idle_runs: self.idle_runs.load(Ordering::Relaxed),
            size_runs: self.size_runs.load(Ordering::Relaxed),
            idle_frames: self.idle_frames.load(Ordering::Relaxed),
            floor_skips: self.floor_skips.load(Ordering::Relaxed),
        }
    }

    /// Mark work in flight until the guard drops: an idle checkpoint will not start
    /// meanwhile (it is deferred, see the module docs).
    #[must_use]
    pub fn busy_guard(self: &Arc<Self>) -> BusyGuard {
        self.busy.fetch_add(1, Ordering::Relaxed);
        BusyGuard {
            health: Arc::clone(self),
            since: Instant::now(),
            logged: false,
        }
    }

    /// As [`WalHealth::busy_guard`], for one writer turn: when the turn is slow it is
    /// also kept for [`WalHealth::slow_turn_overlapping`]. A reconcile round holds
    /// the plain guard for a whole `index_paths` call (seconds, on a catch-up) and
    /// would otherwise overlap every save beside it.
    #[must_use]
    pub fn writer_turn_guard(self: &Arc<Self>) -> BusyGuard {
        let mut guard = self.busy_guard();
        guard.logged = true;
        guard
    }

    /// Whether a writer turn (a [`WalHealth::writer_turn_guard`]; reconcile rounds are not logged) that lasted at least [`SLOW_TURN`]
    /// overlapped `[from, to]` (task #196). Only finished turns are known: ask after
    /// the work has drained ([`WalHealth::work_in_flight`] false), or a turn still
    /// running is not yet in the log.
    ///
    /// The save path's interval ends at the submit, not at the commit (architecture
    /// 3.1.1), so a long commit can only reach a save sample by overlapping it. This is
    /// the read-only fact that says one did.
    #[must_use]
    pub fn slow_turn_overlapping(&self, from: Instant, to: Instant) -> bool {
        self.slow_turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|&(start, end)| start <= to && end >= from)
    }

    /// Log a finished turn if it was slow.
    pub(crate) fn note_turn(&self, since: Instant, until: Instant) {
        if until.saturating_duration_since(since) < SLOW_TURN {
            return;
        }
        let mut log = self
            .slow_turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if log.len() == SLOW_TURNS_KEPT {
            log.remove(0);
        }
        log.push((since, until));
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

    fn record_passive(&self, took: Duration, complete: bool, kind: RunKind) {
        match kind {
            RunKind::Idle => self.idle_runs.fetch_add(1, Ordering::Relaxed),
            RunKind::SizeBound => self.size_runs.fetch_add(1, Ordering::Relaxed),
        };
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
pub struct BusyGuard {
    health: Arc<WalHealth>,
    since: Instant,
    logged: bool,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        // Log before the count falls, so "no work in flight" implies the log is complete.
        if self.logged {
            self.health.note_turn(self.since, Instant::now());
        }
        self.health.busy.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A [`BusyGuard`] that lived at least this long is kept for
/// [`WalHealth::slow_turn_overlapping`]. An ordinary single-note turn is a few
/// milliseconds; the FTS5 merge turns task #183 found are 180 to 330 ms.
pub const SLOW_TURN: Duration = Duration::from_millis(50);

/// How many slow turns the log keeps. A 1,000-save run sees a handful.
const SLOW_TURNS_KEPT: usize = 256;

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
    /// When the idle gate was last re-armed by a deferral (not a commit); the gate is
    /// measured from the later of this and `last_commit`.
    deferred_at: Option<Instant>,
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
    /// An idle wake found the WAL below the frame floor: nothing is due until the next
    /// commit (task #199), or the timer would spin.
    disarmed: bool,
}

impl State {
    /// What the idle gate counts from: the last commit, or a later deferral.
    fn idle_base(&self) -> Instant {
        self.deferred_at
            .map_or(self.last_commit, |d| d.max(self.last_commit))
    }

    /// When the next idle `PASSIVE` may run: the idle gate, no sooner than the retry
    /// back-off and the spacing from the previous idle one.
    fn passive_at(&self, bounds: Bounds) -> Instant {
        let mut at = self.idle_base() + bounds.idle;
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
        if self.disarmed {
            return None;
        }
        let passive = self.dirty.then(|| self.passive_at(bounds));
        let restart = (pinned && !self.restart_spent).then(|| self.idle_base() + bounds.idle);
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
        deferred_at: None,
        base: 0,
        frames: 0,
        frame_bytes: page_size + 24,
        retry_at: None,
        restart_spent: false,
        last_idle: None,
        prev_done: 0,
        disarmed: false,
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
                state.disarmed = false;
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
    if idle && state.frames - state.base < bounds.idle_floor_frames {
        // Below the floor (task #199): a run would be mostly flush. Not a deferral, so
        // the baseline stays; disarm so the timer waits for the next commit.
        health.floor_skips.fetch_add(1, Ordering::Relaxed);
        state.disarmed = true;
        return;
    }
    if idle && health.work_in_flight() {
        // Yield: re-arm the idle gate from now. The writer's next commit signal or the
        // gate wakes the thread again; nothing is lost, `dirty` stays set.
        health.deferred_busy.fetch_add(1, Ordering::Relaxed);
        state.deferred_at = Some(Instant::now());
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
        let kind = if idle {
            RunKind::Idle
        } else {
            RunKind::SizeBound
        };
        health.running.store(kind.code(), Ordering::Relaxed);
        let started = Instant::now();
        let outcome = checkpoint(conn, "PASSIVE");
        let took = started.elapsed();
        health.running.store(0, Ordering::Relaxed);
        let complete = outcome.is_some_and(|(busy, log, done)| busy == 0 && log == done);
        health.record_passive(took, complete, kind);
        if let Some((_, _, done)) = outcome {
            // A smaller count than before means the writer reset the WAL in between.
            let moved = if done >= state.prev_done {
                done - state.prev_done
            } else {
                done
            };
            state.prev_done = done;
            let moved = u64::try_from(moved).unwrap_or(0);
            health
                .frames_checkpointed
                .fetch_add(moved, Ordering::Relaxed);
            if idle {
                health.idle_frames.fetch_add(moved, Ordering::Relaxed);
            }
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
        health
            .running
            .store(RunKind::Idle.code(), Ordering::Relaxed);
        let outcome = checkpoint(conn, "RESTART");
        health.running.store(0, Ordering::Relaxed);
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
            idle_floor_frames: 0,
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
            deferred_at: None,
            base: 0,
            frames: 0,
            frame_bytes: 4120,
            retry_at: None,
            restart_spent: false,
            last_idle: None,
            prev_done: 0,
            disarmed: false,
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
    fn a_slow_turn_is_logged_and_found_by_the_window_it_overlapped() {
        // Task #196, FR-VLT-04 / NFR-PRF-03: synthetic instants, no sleeping.
        let health = Arc::new(WalHealth::default());
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        health.note_turn(at(0), at(10)); // an ordinary turn: not kept
        health.note_turn(at(100), at(100) + SLOW_TURN); // exactly the threshold: kept
        assert!(!health.slow_turn_overlapping(at(0), at(99)), "before it");
        assert!(
            health.slow_turn_overlapping(at(0), at(100)),
            "touches the start"
        );
        assert!(health.slow_turn_overlapping(at(120), at(121)), "inside it");
        assert!(
            health.slow_turn_overlapping(at(150), at(400)),
            "touches the end"
        );
        assert!(!health.slow_turn_overlapping(at(151), at(400)), "after it");
        // The ordinary turn left no trace.
        assert!(!health.slow_turn_overlapping(at(0), at(10)));
    }

    #[test]
    fn the_slow_turn_log_is_bounded_and_forgets_the_oldest() {
        let health = Arc::new(WalHealth::default());
        let t0 = Instant::now();
        for i in 0..=(SLOW_TURNS_KEPT as u64) {
            let start = t0 + Duration::from_secs(i * 10);
            health.note_turn(start, start + SLOW_TURN);
        }
        assert!(!health.slow_turn_overlapping(t0, t0 + Duration::from_secs(1)));
        let last = t0 + Duration::from_secs(SLOW_TURNS_KEPT as u64 * 10);
        assert!(health.slow_turn_overlapping(last, last));
    }

    #[test]
    fn a_dropped_guard_clears_work_in_flight_and_a_fast_turn_is_not_logged() {
        let health = Arc::new(WalHealth::default());
        let guard = health.writer_turn_guard();
        assert!(health.work_in_flight());
        drop(guard);
        assert!(!health.work_in_flight());
        // A guard that lived microseconds is not slow: the log stays empty.
        let now = Instant::now();
        assert!(!health.slow_turn_overlapping(now - Duration::from_secs(60), now));
    }

    #[test]
    fn only_a_writer_turn_guard_reaches_the_slow_turn_log() {
        // Task #196: a reconcile round's plain guard must not stand for a commit. Built
        // in-module with a start a second in the past, so the turn is slow without
        // sleeping; only the `logged` check in `drop` separates the two outcomes.
        let past = |logged: bool| {
            let health = Arc::new(WalHealth::default());
            health.busy.fetch_add(1, Ordering::Relaxed);
            let guard = BusyGuard {
                health: Arc::clone(&health),
                since: Instant::now() - Duration::from_secs(1),
                logged,
            };
            drop(guard);
            let now = Instant::now();
            let found = health.slow_turn_overlapping(now - Duration::from_millis(500), now);
            (found, health.work_in_flight())
        };
        assert_eq!(past(false), (false, false), "a plain guard is not logged");
        assert_eq!(past(true), (true, false), "a writer turn guard is");
    }

    #[test]
    fn the_size_bound_is_never_deferred_by_work_in_flight() {
        let (dir, _path, actor) = actor(Bounds {
            size_frames: 40,
            ..never()
        });
        let health = actor.wal_health();
        let _busy = health.busy_guard();
        for i in 0..40 {
            save(&actor, i);
        }
        wait_for("the size-bound checkpoint", Duration::from_secs(30), || {
            health.stats().passive_runs >= 1
        });
        assert_eq!(health.stats().deferred_busy, 0);
        actor.shutdown().expect("shutdown");
        drop(dir);
    }

    /// A scheduler state with `frames` in the WAL and growth counted from `base`, dirty.
    fn state_at(frames: i32, base: i32) -> State {
        State {
            dirty: true,
            last_commit: Instant::now(),
            deferred_at: None,
            base,
            frames,
            frame_bytes: 4120,
            retry_at: None,
            restart_spent: false,
            last_idle: None,
            prev_done: 0,
            disarmed: false,
        }
    }

    /// One `attempt` driven directly on a scratch database: no thread, no timer, so the
    /// floor's decision is the only thing under test (task #199; no clock assertions).
    fn attempt_once(state: &mut State, bounds: Bounds, health: &WalHealth, idle: bool) {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("index.db");
        let conn = schema::open(&path).expect("open");
        schema::bootstrap(&conn).expect("bootstrap");
        conn.busy_timeout(Duration::ZERO).expect("timeout");
        attempt(&conn, state, bounds, health, &wal_path(&path), idle);
    }

    fn floored() -> Bounds {
        // The gate is open (zero) so a direct `attempt` is a wake that has arrived.
        Bounds {
            idle: Duration::ZERO,
            idle_floor_frames: IDLE_FLOOR_FRAMES,
            ..never()
        }
    }

    #[test]
    fn the_default_floor_is_the_ruled_1000_frames_and_a_quarter_of_the_size_bound() {
        assert_eq!(Bounds::default().idle_floor_frames, 1_000);
        assert_eq!(IDLE_FLOOR_FRAMES * 4, SIZE_BOUND_FRAMES);
    }

    #[test]
    fn an_idle_wake_below_the_floor_runs_nothing_and_keeps_the_baseline() {
        let health = WalHealth::default();
        let mut state = state_at(IDLE_FLOOR_FRAMES - 1, 0);
        attempt_once(&mut state, floored(), &health, true);
        let stats = health.stats();
        assert_eq!((stats.passive_runs, stats.restart_runs), (0, 0));
        assert_eq!(stats.floor_skips, 1);
        assert_eq!(state.base, 0, "a below-floor wake moved the baseline");
        assert!(state.dirty, "the frames are still owed");
    }

    #[test]
    fn an_idle_wake_at_the_floor_runs_one_idle_passive_and_moves_the_baseline() {
        let health = WalHealth::default();
        let mut state = state_at(IDLE_FLOOR_FRAMES, 0);
        attempt_once(&mut state, floored(), &health, true);
        let stats = health.stats();
        assert_eq!(
            (stats.passive_runs, stats.idle_runs, stats.size_runs),
            (1, 1, 0)
        );
        assert_eq!(stats.floor_skips, 0);
        assert!(!state.dirty && state.last_idle.is_some());
    }

    #[test]
    fn a_deferral_to_work_in_flight_does_not_reset_the_floor_baseline() {
        let health = Arc::new(WalHealth::default());
        let mut state = state_at(IDLE_FLOOR_FRAMES + 500, 0);
        let busy = health.busy_guard();
        attempt_once(&mut state, floored(), &health, true);
        assert_eq!(health.stats().deferred_busy, 1);
        assert_eq!(health.stats().passive_runs, 0);
        assert_eq!(health.stats().floor_skips, 0, "past the floor: a deferral");
        assert_eq!(state.base, 0, "a deferral moved the baseline");
        drop(busy);
        attempt_once(&mut state, floored(), &health, true);
        assert_eq!(health.stats().idle_runs, 1, "the owed run happens after it");
    }

    #[test]
    fn the_size_bound_is_never_floored() {
        // Growth far below the idle floor, a floor that nothing reaches: a non-idle
        // attempt (what the size bound calls) still runs.
        let health = WalHealth::default();
        let mut state = state_at(10, 0);
        let bounds = Bounds {
            idle_floor_frames: i32::MAX,
            ..never()
        };
        attempt_once(&mut state, bounds, &health, false);
        let stats = health.stats();
        assert_eq!(
            (stats.size_runs, stats.idle_runs, stats.floor_skips),
            (1, 0, 0)
        );
    }

    #[test]
    fn the_pinned_restart_is_floored_like_the_idle_passive() {
        let health = WalHealth::default();
        health.set_pinned_for_test(Some(1));
        let mut state = state_at(IDLE_FLOOR_FRAMES - 1, 0);
        attempt_once(&mut state, floored(), &health, true);
        assert_eq!(health.stats().restart_runs, 0);
        assert_eq!(health.stats().floor_skips, 1);
        assert_eq!(
            state.due(floored(), true),
            None,
            "disarmed: no spin on RESTART"
        );
    }

    #[test]
    fn a_below_floor_wake_disarms_the_timer_until_the_next_commit() {
        let health = WalHealth::default();
        let mut state = state_at(10, 0);
        assert!(
            state.due(floored(), false).is_some(),
            "armed after a commit"
        );
        attempt_once(&mut state, floored(), &health, true);
        assert_eq!(state.due(floored(), false), None);
        assert_eq!(state.due(floored(), true), None, "nor for a pinned RESTART");
    }

    #[test]
    fn the_live_checkpointer_does_not_spin_below_the_floor_and_wakes_on_a_commit() {
        let (dir, _path, actor) = actor(Bounds {
            idle: Duration::from_millis(10),
            idle_floor_frames: IDLE_FLOOR_FRAMES,
            ..never()
        });
        let health = actor.wal_health();
        save(&actor, 0);
        wait_for("the below-floor wake", Duration::from_secs(30), || {
            health.stats().floor_skips >= 1
        });
        // A spin would count again every gate (10 ms); the count can only be wrong
        // upward, so a stall cannot fail this and a spin cannot pass it.
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(health.stats().floor_skips, 1, "the idle timer spun");
        assert_eq!(health.stats().passive_runs, 0);
        // The next commit re-arms it.
        save(&actor, 1);
        wait_for("the second wake", Duration::from_secs(30), || {
            health.stats().floor_skips >= 2
        });
        assert_eq!(health.stats().passive_runs, 0);
        actor.shutdown().expect("shutdown");
        drop(dir);
    }

    #[test]
    fn the_live_checkpointer_runs_idle_once_the_wal_has_grown_by_the_floor() {
        let floor = 30;
        let (dir, _path, actor) = actor(Bounds {
            idle: Duration::from_millis(10),
            idle_floor_frames: floor,
            ..never()
        });
        let health = actor.wal_health();
        // Far more commits than the floor needs, then quiet: the wake that follows is past it.
        for i in 0..100 {
            save(&actor, i);
        }
        wait_for("an idle run", Duration::from_secs(30), || {
            health.stats().idle_runs >= 1
        });
        let stats = health.stats();
        assert_eq!(stats.passive_runs, stats.idle_runs);
        assert!(
            stats.idle_frames >= u64::try_from(floor).expect("floor"),
            "an idle run moved {} frames, under the floor {floor}",
            stats.idle_frames
        );
        actor.shutdown().expect("shutdown");
        drop(dir);
    }

    #[test]
    fn the_size_bound_fires_on_a_live_writer_whatever_the_floor() {
        let (dir, _path, actor) = actor(Bounds {
            size_frames: 40,
            idle_floor_frames: i32::MAX,
            ..never()
        });
        let health = actor.wal_health();
        let mut i = 0;
        while health.stats().passive_runs == 0 {
            save(&actor, i);
            i += 1;
            assert!(i < 2_000, "the size bound never fired");
        }
        assert_eq!(health.stats().size_runs, health.stats().passive_runs);
        assert_eq!(health.stats().idle_runs, 0);
        actor.shutdown().expect("shutdown");
        drop(dir);
    }

    #[test]
    fn close_checkpoints_a_wal_far_below_the_floor() {
        // close() is the writer's `TRUNCATE`, not this thread's: nothing below the floor
        // is left for a power cut or a reopen to replay.
        let (dir, path, actor) = actor(Bounds {
            idle_floor_frames: i32::MAX,
            ..never()
        });
        save(&actor, 0);
        let health = actor.wal_health();
        actor.shutdown().expect("shutdown");
        assert_eq!(health.stats().passive_runs, 0, "the checkpointer ran");
        assert_eq!(
            len(&wal_path(&path)),
            0,
            "the WAL was not truncated at close"
        );
        let conn = schema::open(&path).expect("reopen");
        let notes: i64 = conn
            .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
            .expect("count");
        assert_eq!(notes, 1);
        drop(dir);
    }

    #[test]
    fn the_overlap_kind_names_the_run_and_idle_wins() {
        let none = CheckpointStats::default();
        let idle = CheckpointStats {
            idle_runs: 1,
            ..none
        };
        let restart = CheckpointStats {
            restart_runs: 1,
            ..none
        };
        let size = CheckpointStats {
            size_runs: 1,
            ..none
        };
        let both = CheckpointStats {
            idle_runs: 1,
            size_runs: 1,
            ..none
        };
        assert_eq!(overlap_kind(None, None, &none, &none), None);
        assert_eq!(overlap_kind(None, None, &none, &idle), Some(RunKind::Idle));
        assert_eq!(
            overlap_kind(None, None, &none, &restart),
            Some(RunKind::Idle)
        );
        assert_eq!(
            overlap_kind(None, None, &none, &size),
            Some(RunKind::SizeBound)
        );
        assert_eq!(overlap_kind(None, None, &none, &both), Some(RunKind::Idle));
        assert_eq!(
            overlap_kind(Some(RunKind::SizeBound), None, &none, &none),
            Some(RunKind::SizeBound)
        );
        assert_eq!(
            overlap_kind(Some(RunKind::SizeBound), Some(RunKind::Idle), &none, &none),
            Some(RunKind::Idle)
        );
        assert_eq!(RunKind::Idle.label(), "idle");
        assert_eq!(RunKind::SizeBound.label(), "size-bound");
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
            idle_floor_frames: 0,
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
