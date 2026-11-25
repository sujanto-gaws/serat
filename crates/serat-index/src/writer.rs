//! The single-writer actor (ADR-0001).
//!
//! One thread owns the only write connection. Everything else sends it messages. There
//! is no lock ordering to reason about, no poisoned handle after a panic, and exactly
//! one queue whose depth is the backpressure signal.
//!
//! # The shape, stated honestly
//!
//! The channel is not the whole interface. [`WriterHandle::flush`] is a barrier, and a
//! barrier is synchronisation — so the real shape is *one writer, asynchronous, with an
//! explicit and expensive barrier*, not an interface with no synchronisation left in
//! it. The barrier must never appear on the save path, where blocking would put the
//! index back on the keystroke path (design goal 2).
//!
//! The test that keeps this design honest: **if a message needs a reply, the channel is
//! a lie and you have a mutex with extra steps.** [`IndexOp`]'s four variants need
//! none. A fifth that did would be the signal to redesign rather than to add a return
//! channel.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;

use rusqlite::Connection;
use serat_core::{IndexError, IndexOp, IndexWriter};

use crate::apply;
use crate::checkpoint::{Bounds, Checkpointer, WalHealth, WalObserver, WriterHook, tune_writer};
use crate::schema::{self, sqlite};

/// How many operations may wait for the writer.
///
/// Bounded, so a producer that outruns the writer is refused rather than allowed to
/// grow memory without limit. Refusal is [`IndexError::QueueFull`], which is
/// backpressure and not a fault.
pub const DEFAULT_QUEUE_DEPTH: usize = 4_096;

/// How many operations one transaction may carry.
///
/// A transaction per note would fsync per note; one transaction for everything would
/// hold a write lock for the length of a rebuild and lose all of it on a crash.
///
/// Batching is one of the tunables, and the obvious one. It was **not** what made the
/// writer dominate at P0 — that was SQL text being re-prepared inside per-row loops
/// (F-324), which no batch size would have reached. If the writer dominates again,
/// probe inside `commit()` before reaching for this number: *which phase* and *which
/// line* are different questions. (Open design question 1 is answered, not open; see
/// architecture §2.2.)
pub const BATCH_SIZE: usize = 512;

/// Rerun `ANALYZE` when a batch touched more than this share of the notes table.
///
/// Architecture §4.3: without statistics the planner prefers `ix_notes_kind`, which
/// matches nearly every row, over `ix_notes_type`, and every typed query degrades into
/// a full scan.
const ANALYZE_FRACTION: f64 = 0.05;

enum Command {
    Apply(Box<IndexOp>),
    /// Drain everything queued before this, then answer.
    Flush(SyncSender<Result<(), IndexError>>),
    /// Run ANALYZE now, regardless of what the batch counter says.
    Analyze(SyncSender<Result<(), IndexError>>),
    /// Mark the index ready and record the vault it was built from.
    MarkReady(String, i64, SyncSender<Result<(), IndexError>>),
    /// Record that a catch-up that began at this wall-clock time (ms) has completed.
    CaughtUp(i64, SyncSender<Result<(), IndexError>>),
    /// Capture and drop the blocks_fts triggers and ix_links_unresolved before a bulk
    /// load (F-373).
    BeginBulkLoad(SyncSender<Result<Vec<String>, IndexError>>),
    /// Rebuild the FTS index in one pass and restore what BeginBulkLoad dropped.
    EndBulkLoad(Vec<String>, SyncSender<Result<(), IndexError>>),
    /// Resolve every link whose target is now present.
    ResolveLinks(SyncSender<Result<usize, IndexError>>),
    /// Commit anything in hand, checkpoint, and mark the index as cleanly closed.
    CloseClean(SyncSender<Result<bool, IndexError>>),
    /// Apply anything in hand and end the loop.
    ///
    /// An explicit message rather than relying on every sender being dropped: handles
    /// are cloneable, so "the channel closed" means "the last clone went", and a
    /// caller holding one while asking the actor to stop would wait forever. That is
    /// not hypothetical — the rebuild holds a handle for the whole run and shuts the
    /// actor down at the end of it.
    Stop,
}

/// What one committed batch touched, as the writer already had it in hand.
///
/// Built after `COMMIT` returned, only when a [`CommitObserver`] is installed, and from
/// the batch's own ops: no extra read, no extra statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedBatch {
    /// How many [`IndexOp`]s the transaction carried. Never zero.
    pub ops: usize,
    /// Every path the batch named, in op order: an op's own path, and for a rename the
    /// path it left as well as the one it reached. Not deduplicated.
    pub paths: Vec<serat_core::VaultPath>,
}

/// Called by the writer after each non-bulk batch has committed (ADR-0020 decision 3).
///
/// Called once per committed batch that applied at least one [`IndexOp`], outside a
/// `begin_bulk_load` .. `end_bulk_load` bracket, **after** SQLite's `COMMIT` returned,
/// so a read connection opened inside the call sees the batch. Never called for a
/// barrier-only turn, a bulk-load batch or a batch that failed and was left in hand
/// (it reports when a later commit succeeds with it).
///
/// It runs **on the writer thread** and the writer holds no lock while it runs. The
/// writer is stalled for as long as it takes, and a call that waits on the writer
/// deadlocks it, so an observer must be quick and must never call
/// [`WriterHandle::flush`], `close`, [`WriterHandle::close_clean`],
/// [`WriterHandle::submit_blocking`] or anything else that waits on this writer, nor
/// take a lock that a thread waiting on the writer may hold. A panic in the observer
/// is caught and discarded: the writer outlives it.
///
/// One late case: if `COMMIT` succeeds but restoring `synchronous` after a clean close
/// then fails, the batch stays in hand, is applied again by the next commit and is
/// reported then, late and not lost.
///
/// Installed once, on [`ActorOptions`], and carried by `IndexHandle` so that the actor a
/// runtime rebuild starts reports to the same observer.
#[derive(Clone)]
pub struct CommitObserver(Arc<dyn Fn(CommittedBatch) + Send + Sync>);

impl CommitObserver {
    /// Wrap a callback.
    #[must_use]
    pub fn new(f: impl Fn(CommittedBatch) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }
}

impl std::fmt::Debug for CommitObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CommitObserver(..)")
    }
}

fn report(observer: &CommitObserver, pending: &[IndexOp]) {
    let mut paths = Vec::with_capacity(pending.len());
    for op in pending {
        if let IndexOp::Rename { from, .. } = op {
            paths.push(from.clone());
        }
        paths.push(op.path().clone());
    }
    let batch = CommittedBatch {
        ops: pending.len(),
        paths,
    };
    // The writer must survive its observer (ADR-0001: no poisoned writer).
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (observer.0)(batch)));
}

/// What the writer thread consults on each commit.
#[derive(Debug, Default)]
struct CommitHooks {
    observer: Option<CommitObserver>,
    gate: CommitGate,
}

/// Holds a batch between its statements and its `COMMIT`, for tests (task #175).
///
/// Behind the `test-hooks` feature (and `cfg(test)`) it can hold; otherwise it is an
/// inert (a shared allocation holding nothing) and the writer's call into it does
/// nothing. Every actor has one
/// ([`IndexActor::commit_gate`]); a handle produced by a runtime rebuild has its own,
/// so fetch it again after one.
#[derive(Debug, Clone, Default)]
pub struct CommitGate(
    #[cfg_attr(not(any(test, feature = "test-hooks")), allow(dead_code))] Arc<GateState>,
);

#[derive(Debug, Default)]
struct GateState {
    #[cfg(any(test, feature = "test-hooks"))]
    state: std::sync::Mutex<GateFlags>,
    #[cfg(any(test, feature = "test-hooks"))]
    changed: std::sync::Condvar,
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug, Default)]
struct GateFlags {
    held: bool,
    reached: bool,
}

/// While alive, the next commit that gets as far as its `COMMIT` waits there.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug)]
pub struct CommitHold(Arc<GateState>);

#[cfg(any(test, feature = "test-hooks"))]
impl CommitGate {
    /// Hold the next batch between its last statement and its `COMMIT` until the returned
    /// guard is dropped. The transaction stays open meanwhile, so other connections see
    /// the pre-batch state, which is what makes "nothing announced before the commit"
    /// provable without a race.
    ///
    /// While a hold is outstanding the writer cannot finish: `close`, `shutdown` and
    /// dropping the actor all block until the hold is dropped, so drop it first.
    #[must_use]
    pub fn hold(&self) -> CommitHold {
        let mut flags = lock(&self.0.state);
        flags.held = true;
        flags.reached = false;
        CommitHold(Arc::clone(&self.0))
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl CommitHold {
    /// Block until the writer is parked at the hold, or `timeout` passes. Returns
    /// whether it is parked.
    #[must_use]
    pub fn wait_reached(&self, timeout: std::time::Duration) -> bool {
        let flags = lock(&self.0.state);
        let (flags, _) = self
            .0
            .changed
            .wait_timeout_while(flags, timeout, |flags| !flags.reached)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        flags.reached
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl Drop for CommitHold {
    fn drop(&mut self) {
        lock(&self.0.state).held = false;
        self.0.changed.notify_all();
    }
}

#[cfg(any(test, feature = "test-hooks"))]
fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl CommitGate {
    /// Writer thread: park here while a hold is in force.
    fn wait_before_commit(&self) {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            let mut flags = lock(&self.0.state);
            if !flags.held {
                return;
            }
            flags.reached = true;
            self.0.changed.notify_all();
            while flags.held {
                flags = self
                    .0
                    .changed
                    .wait(flags)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
    }
}

/// A handle to the writer. Cloneable, shareable, cheap.
#[derive(Debug, Clone)]
pub struct WriterHandle {
    tx: SyncSender<Command>,
}

impl IndexWriter for WriterHandle {
    fn submit(&self, op: IndexOp) -> Result<(), IndexError> {
        self.tx
            .try_send(Command::Apply(Box::new(op)))
            .map_err(|err| match err {
                TrySendError::Full(_) => IndexError::QueueFull {
                    depth: DEFAULT_QUEUE_DEPTH,
                },
                TrySendError::Disconnected(_) => IndexError::WriterStopped,
            })
    }

    fn flush(&self) -> Result<(), IndexError> {
        self.round_trip(Command::Flush)
    }
}

impl WriterHandle {
    /// Send a command that carries a reply channel and wait for its answer.
    fn round_trip(
        &self,
        make: impl FnOnce(SyncSender<Result<(), IndexError>>) -> Command,
    ) -> Result<(), IndexError> {
        let (reply, answer) = sync_channel(1);
        self.tx
            .send(make(reply))
            .map_err(|_| IndexError::WriterStopped)?;
        answer.recv().map_err(|_| IndexError::WriterStopped)?
    }

    /// Queue an operation, waiting if the queue is full.
    ///
    /// The counterpart to [`IndexWriter::submit`], which refuses instead. Refusing is
    /// right on the watcher path, where a dropped path is rediscovered by a rescan.
    /// It is wrong during a rebuild, where nothing will ever mention this file again
    /// and dropping it means quietly indexing part of the vault. It is not on the
    /// trait because that would offer every caller a way to block, and the save path
    /// must never take it (design goal 2).
    ///
    /// # Errors
    ///
    /// [`IndexError::WriterStopped`] if the writer has gone.
    pub fn submit_blocking(&self, op: IndexOp) -> Result<(), IndexError> {
        self.tx
            .send(Command::Apply(Box::new(op)))
            .map_err(|_| IndexError::WriterStopped)
    }

    /// Drop the `blocks_fts` triggers and `ix_links_unresolved`, returning their
    /// definitions.
    ///
    /// Step 2 of the rebuild sequence in `schema/001_init.sql`. The definitions come
    /// out of `sqlite_master` rather than from a second copy of the DDL, so what is
    /// restored is identical to what was delivered by construction rather than by a
    /// test that has to catch a stale copy.
    ///
    /// Loading through the triggers costs about 6x at 26,000 blocks: 2.1 s live against
    /// 0.32 s dropped-and-rebuilt, reproduced independently at 5.8x. The search index
    /// is identical either way, which is what makes the faster shape a free choice
    /// rather than a trade.
    ///
    /// `ix_links_unresolved` is the same trade for a different reason (F-373, task
    /// #82): it is a per-row obligation SQLite discharges on every insert into
    /// `links`, and no branch inside `apply::apply` can decline it. Measured at
    /// 11.5 s of a 20k rebuild's writer time.
    ///
    /// # Errors
    ///
    /// [`IndexError::WriterStopped`], or whatever SQLite says.
    pub fn begin_bulk_load(&self) -> Result<Vec<String>, IndexError> {
        let (reply, answer) = sync_channel(1);
        self.tx
            .send(Command::BeginBulkLoad(reply))
            .map_err(|_| IndexError::WriterStopped)?;
        answer.recv().map_err(|_| IndexError::WriterStopped)?
    }

    /// Rebuild the FTS index in one pass and put back everything
    /// [`WriterHandle::begin_bulk_load`] dropped.
    ///
    /// # Errors
    ///
    /// As [`WriterHandle::begin_bulk_load`]. A failure here leaves the index without
    /// its triggers and `ix_links_unresolved`, which is why the caller must not mark
    /// it ready.
    pub fn end_bulk_load(&self, triggers: Vec<String>) -> Result<(), IndexError> {
        self.round_trip(|reply| Command::EndBulkLoad(triggers, reply))
    }

    /// Point every link at the note it names, where that note now exists.
    ///
    /// Necessarily a second pass: a link can name a note that has not been inserted
    /// yet, and `links.dst_note` is a foreign key. Returns how many resolved.
    ///
    /// # Errors
    ///
    /// As [`WriterHandle::begin_bulk_load`].
    pub fn resolve_links(&self) -> Result<usize, IndexError> {
        let (reply, answer) = sync_channel(1);
        self.tx
            .send(Command::ResolveLinks(reply))
            .map_err(|_| IndexError::WriterStopped)?;
        answer.recv().map_err(|_| IndexError::WriterStopped)?
    }

    /// End the session: apply everything queued, checkpoint the WAL into the database
    /// file and, only if that completed, durably set `index_meta.clean_close`. Returns
    /// whether it was set; `false` (a reader pinned the WAL) means the next open will
    /// treat this shutdown as unclean, which costs a little work and loses nothing.
    ///
    /// A barrier, called once per session at close, never on the save path.
    ///
    /// # Errors
    ///
    /// [`IndexError::WriterStopped`], or whatever SQLite says.
    pub fn close_clean(&self) -> Result<bool, IndexError> {
        let (reply, answer) = sync_channel(1);
        self.tx
            .send(Command::CloseClean(reply))
            .map_err(|_| IndexError::WriterStopped)?;
        answer.recv().map_err(|_| IndexError::WriterStopped)?
    }

    /// Run `ANALYZE` and wait for it.
    ///
    /// # Errors
    ///
    /// [`IndexError::WriterStopped`] if the writer is gone; whatever SQLite says
    /// otherwise.
    pub fn analyze(&self) -> Result<(), IndexError> {
        self.round_trip(Command::Analyze)
    }

    /// Mark the index usable, after a rebuild has committed everything. Also sets
    /// `clean_close`: a finished rebuild owes nothing, and the connection's close
    /// checkpoints it.
    ///
    /// # Errors
    ///
    /// As [`WriterHandle::analyze`].
    pub fn mark_ready(&self, vault_root: &str, caught_up_from_ms: i64) -> Result<(), IndexError> {
        self.round_trip(|reply| Command::MarkReady(vault_root.to_owned(), caught_up_from_ms, reply))
    }

    /// Commit everything queued, then record `index_meta.caught_up_from_ms`: the one
    /// UPDATE that ends a catch-up that ran to completion (task #66). Call it only
    /// then; a catch-up that did not finish must not advance the window.
    ///
    /// # Errors
    ///
    /// As [`WriterHandle::analyze`].
    #[doc(hidden)]
    pub fn set_caught_up_from(&self, started_at_ms: i64) -> Result<(), IndexError> {
        self.round_trip(|reply| Command::CaughtUp(started_at_ms, reply))
    }
}

/// The writer thread and the connection it owns.
#[derive(Debug)]
pub struct IndexActor {
    handle: WriterHandle,
    join: Option<JoinHandle<()>>,
    // Read back only by tests, through `fault_injector()`. Kept as a real field
    // rather than a `#[cfg(test)]` one: it is the same `FaultInjector` `commit`
    // consults on every run in every build, not a test-only shim, so a production
    // build genuinely never reads it back -- only ever hands a clone to the writer
    // thread at construction. The allow is for that write-only shape, not for
    // sloppiness.
    #[cfg_attr(not(test), allow(dead_code))]
    fault: FaultInjector,
    /// The WAL checkpointer (task #56), stopped after the writer. `None` for an
    /// in-memory database, or when its connection could not be opened, in which case
    /// SQLite's own auto-checkpoint stays on.
    pub(crate) checkpointer: Option<Checkpointer>,
    health: Arc<WalHealth>,
    #[cfg_attr(not(any(test, feature = "test-hooks")), allow(dead_code))]
    gate: CommitGate,
}

/// How an [`IndexActor`] is started, beyond its connection and queue depth.
#[derive(Debug, Clone)]
pub struct ActorOptions {
    /// How many operations may wait for the writer.
    pub capacity: usize,
    /// Whether to start the checkpointer and turn the writer's auto-checkpoint off.
    /// Off only for `p1-save-tail`'s diagnostic runs of the old behaviour.
    pub checkpointer: bool,
    /// The checkpointer's thresholds; the ruled values unless a test shrinks them.
    pub bounds: Bounds,
    /// Called from the writer's WAL hook with each commit's frame count. For the
    /// benchmarks. When the checkpointer is off this *replaces* SQLite's auto-checkpoint,
    /// so the observer is responsible for any checkpoint it wants.
    pub wal_observer: Option<WalObserver>,
    /// Called after each committed non-bulk batch (ADR-0020 decision 3). Default none.
    pub commit_observer: Option<CommitObserver>,
}

impl Default for ActorOptions {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_QUEUE_DEPTH,
            checkpointer: true,
            bounds: Bounds::default(),
            wal_observer: None,
            commit_observer: None,
        }
    }
}

impl IndexActor {
    /// Open (or create) the index at `path` and start the writer.
    ///
    /// # Errors
    ///
    /// [`IndexError::Sqlite`] if the database cannot be opened or bootstrapped.
    pub fn open(path: &Path) -> Result<Self, IndexError> {
        let conn = schema::open(path)?;
        schema::bootstrap_if_empty(&conn)?;
        Ok(Self::with_connection(conn))
    }

    /// Start a writer over an already-open connection.
    #[must_use]
    pub fn with_connection(conn: Connection) -> Self {
        Self::with_capacity(conn, DEFAULT_QUEUE_DEPTH)
    }

    /// Start a writer whose queue holds at most `capacity` operations.
    ///
    /// Exists because the default depth makes the refusal path unreachable in a test:
    /// a test that submits fewer operations than the queue holds never fills it, so it
    /// cannot tell a blocking producer from a dropping one. That is how the first
    /// version of this crate's own regression test failed to catch the bug it was
    /// written for (F-19, F-35).
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero.
    #[must_use]
    pub fn with_capacity(conn: Connection, capacity: usize) -> Self {
        Self::with_options(
            conn,
            ActorOptions {
                capacity,
                ..ActorOptions::default()
            },
        )
    }

    /// As [`IndexActor::with_capacity`], with the checkpointer's switches and thresholds.
    ///
    /// Sets the writer's pragmas (task #56, §2.2, §3.1.1): `journal_size_limit` 16 MiB,
    /// `cache_size` 16 MiB, and, when the checkpointer starts, `wal_autocheckpoint = 0`,
    /// because the checkpointer is what checkpoints now.
    ///
    /// # Panics
    ///
    /// Panics if `options.capacity` is zero.
    #[must_use]
    pub fn with_options(mut conn: Connection, options: ActorOptions) -> Self {
        assert!(
            options.capacity > 0,
            "a writer queue with no capacity accepts nothing"
        );
        let (tx, rx) = sync_channel(options.capacity);
        let fault = FaultInjector::default();
        let actor_fault = fault.clone();
        let health = Arc::new(WalHealth::default());

        let db_path = conn
            .path()
            .filter(|path| !path.is_empty() && *path != ":memory:")
            .map(std::path::PathBuf::from);
        let checkpointer = match (&db_path, options.checkpointer) {
            (Some(path), true) => Checkpointer::start(path, options.bounds, Arc::clone(&health)),
            _ => None,
        };
        // The pragma installs SQLite's hook as NULL, so it is set before the writer
        // thread installs the checkpointer's hook. Pragma failures degrade rather than
        // panic: a writer that kept SQLite's own auto-checkpoint is slower on the hub
        // save, and is still correct.
        let autocheckpoint_off = tune_writer(&conn, checkpointer.is_some());
        let hook = match (&checkpointer, options.wal_observer) {
            (Some(checkpointer), observer) if autocheckpoint_off => {
                Some(checkpointer.writer_hook(observer))
            }
            (Some(_), _) => None,
            (None, Some(observer)) => Some(WriterHook::observer_only(observer)),
            (None, None) => None,
        };
        let hook_health = Arc::clone(&health);
        let gate = CommitGate::default();
        let hooks = CommitHooks {
            observer: options.commit_observer,
            gate: gate.clone(),
        };
        let busy_health = Arc::clone(&health);
        let join = std::thread::Builder::new()
            .name("serat-index-writer".to_owned())
            .spawn(move || {
                if let Some(hook) = hook {
                    hook.install(&mut conn, &hook_health);
                }
                run(conn, &rx, &actor_fault, &hooks, &busy_health);
            })
            .expect("spawn the index writer");

        Self {
            handle: WriterHandle { tx },
            join: Some(join),
            fault,
            checkpointer,
            health,
            gate,
        }
    }

    /// The seam that holds a batch before its `COMMIT`, for tests.
    #[cfg(any(test, feature = "test-hooks"))]
    #[must_use]
    pub fn commit_gate(&self) -> CommitGate {
        self.gate.clone()
    }

    /// What the checkpointer knows: the pinned-WAL fact and its counters. Safe to read
    /// from any thread; touches no connection.
    #[must_use]
    pub fn wal_health(&self) -> Arc<WalHealth> {
        Arc::clone(&self.health)
    }

    /// A handle to make this actor's *next* commit fail once, for tests.
    ///
    /// Scoped to this one actor (task #28). The seam used to be a single
    /// process-wide static, which raced every other test in the same binary that
    /// also drove a commit: cargo runs a crate's unit tests concurrently by
    /// default, so a test elsewhere with its own `IndexActor` could reach
    /// `commit` first and consume the flag this test had just set, before this
    /// actor's own writer thread ever saw it. Reproduced once in some runs and
    /// not others, which is exactly the shape a shared global produces and a
    /// per-instance one cannot. Owned by the actor instead, so setting it can
    /// only ever race *this* actor's own writer thread — the ordering the test
    /// is actually trying to prove.
    ///
    /// Not itself `#[cfg(test)]`: `pub(crate)` already keeps it out of every
    /// other crate, and its seams ([`FaultInjector::fail_until_a_flush_fails`],
    /// [`FaultInjector::fail_every_commit`], [`FaultInjector::hold`]) — the only
    /// methods that can make it do anything — are the part gated to tests. A production
    /// build has no caller for this one, which is the same write-only shape as
    /// the field it reads; see the `allow` there.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn fault_injector(&self) -> FaultInjector {
        self.fault.clone()
    }

    /// A handle to send work on.
    #[must_use]
    pub fn handle(&self) -> WriterHandle {
        self.handle.clone()
    }

    /// Drain the queue and stop the thread.
    ///
    /// # Errors
    ///
    /// [`IndexError::WriterStopped`] if it had already died.
    pub fn shutdown(mut self) -> Result<(), IndexError> {
        self.handle.flush()?;
        self.stop();
        Ok(())
    }

    /// Stop the checkpointer thread and wait for it. `IndexHandle::close` calls this
    /// before `CloseClean`; idempotent.
    pub(crate) fn stop_checkpointer(&mut self) {
        if let Some(mut checkpointer) = self.checkpointer.take() {
            checkpointer.stop();
        }
    }

    fn stop(&mut self) {
        // Send Stop rather than dropping the sender. Handles are clones, so the
        // channel only closes when the last one goes, and a caller that still holds
        // one while shutting the actor down would block here for ever.
        let _ = self.handle.tx.send(Command::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        // After the writer, so its last commit's signal is not sent into a stopped
        // thread. The checkpointer is stopped on vault close, not left to the process.
        if let Some(mut checkpointer) = self.checkpointer.take() {
            checkpointer.stop();
        }
    }
}

impl Drop for IndexActor {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The writer loop.
///
/// Collects up to [`BATCH_SIZE`] operations, applies them in one transaction, and
/// answers any barrier that arrives. A batch ends when the queue is empty, the batch
/// is full, or a command needs an answer — so a quiet vault commits immediately and a
/// rebuild commits in batches.
fn run(
    conn: Connection,
    rx: &Receiver<Command>,
    fault: &FaultInjector,
    hooks: &CommitHooks,
    health: &Arc<WalHealth>,
) {
    let mut pending: Vec<IndexOp> = Vec::with_capacity(BATCH_SIZE);
    let mut since_analyze = 0usize;
    // Whether the blocks_fts triggers are currently off, which is the writer's only
    // signal that it is inside a rebuild rather than serving saves. Set by
    // BeginBulkLoad and cleared by EndBulkLoad, so a writer that never sees either --
    // the incremental path -- keeps the opportunistic ANALYZE it was designed for.
    let mut bulk_load = false;
    // Set once the session has been closed cleanly (clean_close = 1 is durable). Any
    // commit after that clears it again, durably, in the same transaction: an op
    // submitted by a handle clone between CloseClean and the writer stopping would
    // otherwise commit at synchronous = NORMAL under a marker saying nothing is owed,
    // and a power cut would lose it for good.
    let mut closed = false;
    // A command taken off the channel while draining a batch that cannot be folded
    // into it. Held here rather than pushed back, because a channel has no way to
    // un-receive and dropping it would lose a caller's barrier.
    let mut deferred: Option<Command> = None;

    loop {
        let next = match deferred.take() {
            Some(command) => command,
            None => match rx.recv() {
                Ok(command) => command,
                // Every sender dropped: apply whatever is in hand and stop. Discarding
                // a queued change at shutdown would leave the index disagreeing with
                // the disk.
                Err(_) => {
                    // (No busy guard here: the final commit after every sender dropped
                    // runs outside a turn, and the checkpointer is stopping anyway.)
                    // Nobody is waiting on this commit, whatever the last turn's
                    // `continue` arm left `answering` as.
                    fault.set_answering(false);
                    let _ = commit(
                        &conn,
                        &mut pending,
                        &mut since_analyze,
                        bulk_load,
                        fault,
                        &mut closed,
                        hooks,
                    );
                    return;
                }
            },
        };

        // Work in flight until this turn's commit and reply are done (task #183): an idle
        // WAL checkpoint does not start inside it.
        let _busy = health.busy_guard();
        fault.wait_if_held();
        // Every command but an `Apply` has a caller waiting on a reply, including the
        // ones that commit in their own arm and `continue` (resolve, bulk load, close).
        fault.set_answering(!matches!(next, Command::Apply(_)));

        let mut barrier: Option<(SyncSender<Result<(), IndexError>>, Barrier)> = None;
        match next {
            Command::Apply(op) => pending.push(*op),
            Command::Flush(reply) => barrier = Some((reply, Barrier::None)),
            Command::Analyze(reply) => barrier = Some((reply, Barrier::Analyze)),
            Command::MarkReady(root, from, reply) => {
                barrier = Some((reply, Barrier::MarkReady(root, from)));
            }
            Command::CaughtUp(from, reply) => barrier = Some((reply, Barrier::CaughtUp(from))),
            // These three answer with something other than `()`, so they are handled
            // here and not through the Barrier path. Each drains the batch first, and
            // only one of the three drains is load-bearing — which is worth naming,
            // because the version of this comment that claimed all three were guarding
            // the same hazard was wrong.
            //
            // ResolveLinks is the one. Link rows still pending when the resolve pass
            // runs stay unresolved, and there is no second pass to catch them.
            //
            // The other two repair themselves, from opposite ends. Without the drain,
            // BeginBulkLoad lets pending rows commit after the triggers come off, so
            // their FTS rows are missing — and then end_bulk_load issues 'rebuild',
            // which regenerates the whole index from `blocks` and restores exactly
            // those rows. EndBulkLoad without the drain lets them commit after the
            // triggers are back, so the triggers create their FTS rows correctly.
            // They become load-bearing the moment the sequence changes: if 'rebuild'
            // moves, or the triggers are restored later, neither repair happens.
            //
            // On the success path none of the three is entered with work in hand at
            // all: every iteration of this loop ends in a commit, and these arms are
            // reached either as an iteration's first command or from `deferred`. The
            // route that does reach them is a failed commit — `commit` clears `pending`
            // at its end, so an error before that returns through `?` with the batch
            // still held, which is the IndexFault case §3.4 exists for.
            //
            // An earlier version asserted that could not happen. The assertion was
            // false on exactly that route and panicked the writer in every debug build,
            // at the moment the index was already in trouble. An assertion is a claim
            // about all paths; a test claims less and is therefore safe to get partly
            // wrong. FaultInjector is the seam that makes the real route testable.
            Command::BeginBulkLoad(reply) => {
                let result = commit(
                    &conn,
                    &mut pending,
                    &mut since_analyze,
                    bulk_load,
                    fault,
                    &mut closed,
                    hooks,
                )
                .and_then(|()| begin_bulk_load(&conn));
                // Only once the triggers are actually off. Setting it before would
                // suppress ANALYZE for a bulk load that failed to start.
                bulk_load = result.is_ok();
                let _ = reply.send(result);
                continue;
            }
            Command::EndBulkLoad(triggers, reply) => {
                let result = commit(
                    &conn,
                    &mut pending,
                    &mut since_analyze,
                    bulk_load,
                    fault,
                    &mut closed,
                    hooks,
                )
                .and_then(|()| end_bulk_load(&conn, &triggers));
                // Cleared either way: a failed end_bulk_load leaves the index without
                // its triggers and the caller must not mark it ready, but the writer
                // is no longer bulk loading and must not keep suppressing ANALYZE.
                bulk_load = false;
                let _ = reply.send(result);
                continue;
            }
            Command::CloseClean(reply) => {
                let result = commit(
                    &conn,
                    &mut pending,
                    &mut since_analyze,
                    bulk_load,
                    fault,
                    &mut closed,
                    hooks,
                )
                .and_then(|()| schema::end_session(&conn));
                if matches!(result, Ok(true)) {
                    closed = true;
                }
                let _ = reply.send(result);
                continue;
            }
            // ResolveLinks, Analyze and EndBulkLoad write outside `commit()`, so they do not
            // re-open the clean marker the way a commit after a clean close does. Nothing
            // calls them after `close_clean` today; one that did would need the same latch.
            Command::ResolveLinks(reply) => {
                let result = commit(
                    &conn,
                    &mut pending,
                    &mut since_analyze,
                    bulk_load,
                    fault,
                    &mut closed,
                    hooks,
                )
                .and_then(|()| resolve_links(&conn));
                let _ = reply.send(result);
                continue;
            }
            Command::Stop => {
                let _ = commit(
                    &conn,
                    &mut pending,
                    &mut since_analyze,
                    bulk_load,
                    fault,
                    &mut closed,
                    hooks,
                );
                return;
            }
        }

        // Take whatever else is already waiting, so a burst becomes one transaction.
        while barrier.is_none() && pending.len() < BATCH_SIZE {
            match rx.try_recv() {
                Ok(Command::Apply(op)) => pending.push(*op),
                Ok(Command::Flush(reply)) => barrier = Some((reply, Barrier::None)),
                Ok(Command::Analyze(reply)) => barrier = Some((reply, Barrier::Analyze)),
                Ok(Command::MarkReady(root, from, reply)) => {
                    barrier = Some((reply, Barrier::MarkReady(root, from)));
                }
                Ok(Command::CaughtUp(from, reply)) => {
                    barrier = Some((reply, Barrier::CaughtUp(from)));
                }
                Ok(Command::Stop) => {
                    let _ = commit(
                        &conn,
                        &mut pending,
                        &mut since_analyze,
                        bulk_load,
                        fault,
                        &mut closed,
                        hooks,
                    );
                    return;
                }
                Ok(other) => {
                    deferred = Some(other);
                    break;
                }
                Err(_) => break,
            }
        }

        fault.set_answering(barrier.is_some());
        let mut result = commit(
            &conn,
            &mut pending,
            &mut since_analyze,
            bulk_load,
            fault,
            &mut closed,
            hooks,
        );
        fault.set_answering(false);

        if let Some((reply, extra)) = barrier {
            if result.is_ok() {
                result = match &extra {
                    Barrier::None => Ok(()),
                    Barrier::Analyze => analyze(&conn, &mut since_analyze),
                    Barrier::MarkReady(root, from) => mark_ready(&conn, root, *from),
                    Barrier::CaughtUp(from) => conn
                        .execute(
                            "UPDATE index_meta SET caught_up_from_ms = ?1 WHERE id = 1",
                            [from],
                        )
                        .map(|_| ())
                        .map_err(sqlite),
                };
            }
            // A receiver that has gone away means the caller stopped waiting; that is
            // their business, not a reason to stop writing.
            let _ = reply.send(result);
        }
    }
}

/// Capture the `blocks_fts` trigger definitions, `ix_links_unresolved` and the four
/// foreign-key child indexes of user_version 7 (architecture §4.4, task #51), then
/// drop them. The four exist only to serve the cascade on a delete; a rebuild deletes
/// nothing, so it builds them once in [`end_bulk_load`] instead of maintaining them.
///
/// F-373 (task #82): a trigger is not the only per-row obligation a bulk load cannot
/// afford. `ix_links_unresolved` is maintained by SQLite on every insert into `links`
/// regardless of what `apply::apply`'s `bulk_load` branch chooses to skip — a partial
/// index is still an index, and nothing inside `apply` can decline the write-time
/// upkeep of one that exists. Measured at 11.5 s of a 20k rebuild's writer time.
/// Dropping it here and rebuilding it once in [`end_bulk_load`] turns ~220,000
/// incremental B-tree updates into one sorted build, the same trade this function
/// already makes for `blocks_fts`'s triggers.
///
/// Both come out of `sqlite_master` rather than a second copy of the DDL, so what is
/// restored is identical to what was delivered by construction rather than by a test
/// that has to catch a stale copy.
fn begin_bulk_load(conn: &Connection) -> Result<Vec<String>, IndexError> {
    let definitions: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT type, sql FROM sqlite_master
                  WHERE (type = 'trigger' AND tbl_name = 'blocks')
                     OR (type = 'index' AND name IN ('ix_links_unresolved',
                         'ix_links_src_block', 'ix_fields_block', 'ix_tasks_block',
                         'ix_tasks_note'))
                  ORDER BY type, name",
            )
            .map_err(sqlite)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sqlite)?;
        rows.collect::<Result<_, _>>().map_err(sqlite)?
    };

    for (kind, definition) in &definitions {
        // The name is the third whitespace-separated token of both
        // CREATE TRIGGER <name> and CREATE INDEX <name>.
        if let Some(name) = definition.split_whitespace().nth(2) {
            let drop_kind = match kind.as_str() {
                "trigger" => "TRIGGER",
                "index" => "INDEX",
                // sqlite_master's own CHECK on `type` rules out anything else; the
                // query above only ever asks for these two.
                other => {
                    return Err(IndexError::Corrupt {
                        message: format!("sqlite_master named an unexpected type: {other}"),
                    });
                }
            };
            conn.execute_batch(&format!("DROP {drop_kind} IF EXISTS {name};"))
                .map_err(sqlite)?;
        }
    }
    Ok(definitions.into_iter().map(|(_, sql)| sql).collect())
}

/// Build the FTS index in one pass and restore the triggers and
/// `ix_links_unresolved`.
fn end_bulk_load(conn: &Connection, triggers: &[String]) -> Result<(), IndexError> {
    conn.execute_batch("INSERT INTO blocks_fts(blocks_fts) VALUES('rebuild');")
        .map_err(sqlite)?;
    for definition in triggers {
        conn.execute_batch(&format!("{definition};"))
            .map_err(sqlite)?;
    }
    Ok(())
}

/// Point every link at the note it names, where that note exists.
///
/// The second pass the foreign key forces: a link can name a note that has not been
/// inserted yet, so `dst_note` starts NULL and is filled once every note is present.
///
/// Case-insensitive (Unicode, `serat_core::fold_case`, on the stored `*_fold` columns;
/// the statement is `resolve::RESOLVE_ALL`, shared with the incremental passes). A
/// link names a note by path, by path plus `.md`, or by file name (F-312).
/// Unresolved stays NULL rather than becoming wrong.
fn resolve_links(conn: &Connection) -> Result<usize, IndexError> {
    // The EXISTS clause is load-bearing twice over. Without it the UPDATE touches
    // every unresolved link and sets dst_note to NULL again where the target is
    // missing — so the row count comes back as "rows touched" rather than "links
    // resolved", and a dangling link reads as resolved. It also keeps a rebuild from
    // writing to rows it has nothing to say about.
    //
    // F-312: a bare `[[wikilink]]` names a FILE, not a path. Obsidian resolves a
    // target containing no `/` by file name anywhere in the vault (NFR-CMP-01),
    // so matching `notes.path` alone resolved only the notes sitting at the
    // vault root — 8,359 of 212,378 on the 20k vault, because 18,605 of its
    // 20,002 notes are nested. `notes.file_name` carries the file name and is
    // indexed for this join.
    //
    // F-313: the match deliberately does NOT restrict to `kind = 'note'`. An
    // `![[report.pdf]]` names an asset, and §7.3 says an asset stays typed,
    // linkable and queryable; excluding assets drops every embed — 2,525 of them
    // here, every one an `.xlsx`, `.pdf`, `.png` or `.docx`. `file_name` carries
    // the extension precisely so both halves match on one column: a note link
    // omits `.md` and matches `file_name = target || '.md'`, an embed carries its
    // own extension and matches `file_name = target`.
    //
    // Ties break on shortest path, which is Obsidian's rule, and the ORDER BY
    // names both keys so the choice is total rather than left to the planner: a
    // rebuild resolving the same link two ways on two runs would move the corpus
    // digest without the corpus moving.
    let resolved = conn
        .execute(crate::resolve::RESOLVE_ALL, [])
        .map_err(sqlite)?;
    pair_sidecars(conn)?;
    Ok(resolved)
}

/// Pair every asset with its sidecar note, in one `UPDATE` (task #17; architecture
/// §7.4; FR-BIN-02).
///
/// `x.pdf.md` is `x.pdf`'s sidecar when both exist; the comparison is byte for byte
/// (`notes.path` is the unique binary index), so `X.PDF` and `x.pdf.md` are not a pair
/// (ADR-0017). Run after the load beside the link resolution because a bulk load skips
/// the per-row pairing, and because it must see both orders (sidecar first, asset
/// first). It touches only the asset rows (~200 on the 20k vault), one unique-index
/// probe each, so NFR-PRF-04 pays nothing measurable. The incremental path pairs in
/// `apply`, one probe per upsert, and the equivalence test holds the two together.
fn pair_sidecars(conn: &Connection) -> Result<(), IndexError> {
    conn.execute(
        "UPDATE notes SET sidecar_path =
                (SELECT s.path FROM notes s WHERE s.path = notes.path || '.md' AND s.kind = 'note')
          WHERE kind = 'asset'",
        [],
    )
    .map_err(sqlite)?;
    Ok(())
}

enum Barrier {
    None,
    Analyze,
    MarkReady(String, i64),
    CaughtUp(i64),
}

/// Makes an actor's next commit fail once, so a test can reach the code that runs
/// when a batch is left in hand.
///
/// A commit failure cannot be provoked from outside — it needs SQLite to refuse — and
/// the path it opens is the one that turned an assertion into a panic in every debug
/// build. A one-line seam is a smaller price than leaving that route untested.
///
/// One per [`IndexActor`] rather than one process-wide static (task #28). The static
/// version raced every other test in the same binary that also drove a commit: cargo
/// runs a crate's unit tests concurrently by default, and any test elsewhere with its
/// own actor could reach `commit` first and consume the flag this test had just set,
/// before this actor's own writer thread ever saw it — reproducing once in some runs
/// and passing in others, which is exactly the shape a shared global produces. Cloning
/// this shares the same underlying flag with the writer thread it was made for and no
/// other.
#[derive(Debug, Clone, Default)]
pub(crate) struct FaultInjector(#[cfg_attr(not(test), allow(dead_code))] Arc<FaultState>);

/// Every field exists only under `cfg(test)`: a production build carries an empty
/// struct, and the writer's calls into it compile to nothing.
#[derive(Debug, Default)]
struct FaultState {
    /// [`FaultInjector::fail_until_a_flush_fails`]: fail commits until one that answers
    /// a caller has failed.
    #[cfg(test)]
    armed: std::sync::atomic::AtomicBool,
    /// Set by the writer thread: true while the turn it is on has a caller waiting for
    /// its answer (a flush or any other round-trip command), so a failure has somebody
    /// to reach. Only an `armed` fault reads it.
    #[cfg(test)]
    answering: std::sync::atomic::AtomicBool,
    /// [`FaultInjector::fail_every_commit`]: every commit fails, whoever is waiting.
    #[cfg(test)]
    every: std::sync::atomic::AtomicBool,
    /// [`FaultInjector::fail_inside_transaction`].
    #[cfg(test)]
    in_tx: std::sync::atomic::AtomicBool,
    /// [`FaultInjector::hold`]: the writer thread waits here, at the top of a turn,
    /// while this is true.
    #[cfg(test)]
    held: std::sync::Mutex<bool>,
    #[cfg(test)]
    released: std::sync::Condvar,
}

/// The writer thread is held until this is dropped ([`FaultInjector::hold`]).
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Held(Arc<FaultState>);

#[cfg(test)]
impl Drop for Held {
    fn drop(&mut self) {
        *self
            .0
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
        self.0.released.notify_all();
    }
}

impl FaultInjector {
    /// Fail every commit through this handle's actor from now until one that answers a
    /// caller (a flush, or any command with a reply: resolve, bulk load, close) has
    /// failed, then disarm. Commits with nobody waiting (an `Apply` the writer commits
    /// on its own) fail and leave the batch in hand, as documented on `commit`; they do
    /// not disarm it.
    ///
    /// Not "the next commit", because that is racy by construction (task #67): a quiet
    /// writer commits the moment an `Apply` arrives, and the test thread arms the fault
    /// a few instructions after `submit`. Under load the writer wins, the batch commits
    /// cleanly before the fault is armed, the flush finds nothing pending and returns
    /// Ok, and the test fails for a reason that says nothing about the code. Armed
    /// before the `submit`, an auto-commit that gets in first fails too and leaves the
    /// batch in hand; the flush's commit then fails and reports. A writer that folds
    /// the `Apply` and the flush into one turn fails once, in the flush. Either
    /// interleaving ends with the batch in hand and the error at the caller.
    #[cfg(test)]
    pub(crate) fn fail_until_a_flush_fails(&self) {
        self.0
            .armed
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Make every commit through this handle's actor fail until the actor is dropped.
    /// For a test whose failing commit must be the one a barrier waits on: the writer
    /// may commit a lone `Apply` before the barrier arrives, and a one-shot fault is
    /// then spent on a commit nobody is waiting for.
    #[cfg(test)]
    pub(crate) fn fail_every_commit(&self) {
        self.0
            .every
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Make every commit fail after its statements have run and before `COMMIT`, so the
    /// transaction rolls back with work done in it.
    #[cfg(test)]
    pub(crate) fn fail_inside_transaction(&self) {
        self.0
            .in_tx
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Writer thread: whether to fail inside the open transaction. False in production.
    fn take_in_tx(&self) -> bool {
        #[cfg(test)]
        {
            self.0.in_tx.load(std::sync::atomic::Ordering::SeqCst)
        }
        #[cfg(not(test))]
        false
    }

    /// Stop the writer thread at the top of its next turn, after it has taken one
    /// command off the channel and before it acts on it, until the returned guard is
    /// dropped. The channel then fills behind it, which is how a test observes what a
    /// producer does when the writer is behind without depending on how fast the
    /// writer is (task #67).
    #[cfg(test)]
    pub(crate) fn hold(&self) -> Held {
        *self
            .0
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        Held(Arc::clone(&self.0))
    }

    /// Called by the writer thread at the top of a turn.
    fn wait_if_held(&self) {
        #[cfg(test)]
        {
            let mut held = self
                .0
                .held
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while *held {
                held = self
                    .0
                    .released
                    .wait(held)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
    }

    /// Called by the writer thread: whether the commit it is about to make has a caller
    /// waiting for the answer.
    fn set_answering(&self, answering: bool) {
        #[cfg(test)]
        self.0
            .answering
            .store(answering, std::sync::atomic::Ordering::SeqCst);
        #[cfg(not(test))]
        let _ = answering;
    }

    /// Whether this commit fails. Always false in production: the only ways to arm it
    /// are [`FaultInjector::fail_until_a_flush_fails`] and
    /// [`FaultInjector::fail_every_commit`], which do not exist outside `#[cfg(test)]`.
    fn take(&self) -> bool {
        #[cfg(test)]
        {
            use std::sync::atomic::Ordering::SeqCst;
            if self.0.every.load(SeqCst) {
                return true;
            }
            if !self.0.armed.load(SeqCst) {
                return false;
            }
            if self.0.answering.load(SeqCst) {
                self.0.armed.store(false, SeqCst);
            }
            true
        }
        #[cfg(not(test))]
        false
    }
}

fn commit(
    conn: &Connection,
    pending: &mut Vec<IndexOp>,
    since_analyze: &mut usize,
    bulk_load: bool,
    fault: &FaultInjector,
    closed: &mut bool,
    hooks: &CommitHooks,
) -> Result<(), IndexError> {
    if pending.is_empty() {
        return Ok(());
    }

    if fault.take() {
        return Err(IndexError::Corrupt {
            message: "simulated commit failure".to_owned(),
        });
    }

    // The first commit after a clean close re-opens the session: clear the marker in
    // this transaction and make the commit durable (see `closed` in `run`).
    let reopen = *closed;
    if reopen {
        conn.pragma_update(None, "synchronous", schema::SYNCHRONOUS_FULL)
            .map_err(sqlite)?;
    }
    let committed = (|| -> Result<usize, IndexError> {
        let tx = conn.unchecked_transaction().map_err(sqlite)?;
        let mut touched = 0usize;
        for op in pending.iter() {
            let applied = apply::apply(&tx, op, bulk_load)?;
            touched += applied.notes + applied.removed;
        }
        apply::sync_note_count(&tx)?;
        if reopen {
            tx.execute("UPDATE index_meta SET clean_close = 0 WHERE id = 1", [])
                .map_err(sqlite)?;
        }
        if fault.take_in_tx() {
            return Err(IndexError::Corrupt {
                message: "simulated failure inside the transaction".to_owned(),
            });
        }
        // Test seam: hold the batch between its statements and its COMMIT.
        hooks.gate.wait_before_commit();
        tx.commit().map_err(sqlite)?;
        Ok(touched)
    })();
    if reopen {
        let restored = conn.pragma_update(None, "synchronous", schema::SYNCHRONOUS_NORMAL);
        if committed.is_ok() {
            *closed = false;
        }
        restored.map_err(sqlite)?;
    }
    let touched = committed?;

    // After COMMIT has returned (ADR-0020 decision 3), outside a bulk-load bracket, and
    // before the ANALYZE below so an announcement does not wait on statistics. The
    // batch is described from what is already in hand; nothing is built when no
    // observer is installed.
    if !bulk_load && let Some(observer) = &hooks.observer {
        report(observer, pending);
    }

    pending.clear();
    *since_analyze += touched;

    // §3.1 step 5 and §4.3: re-run ANALYZE after a batch touching more than 5% of
    // notes, because stale statistics are what make a typed query a full scan.
    //
    // Not during a bulk load. The 5% rule is written for a steady-state table --
    // enough saves have touched enough of `notes` that the planner's statistics no
    // longer describe it. A rebuild is the opposite case: `notes` starts EMPTY and
    // grows, so "this batch touched more than 5% of the table" is true of nearly
    // every batch, and ANALYZE ran 35 times over a database that was still being
    // filled. Every one of those was superseded by the next, and the only
    // statistics that survive the rebuild are the ones from the explicit ANALYZE
    // the caller runs at the end (rebuild.rs, step 5). Measured at 2.8 s of a
    // 21 s writer on the 20k vault (F-325).
    if !bulk_load {
        let total: i64 = conn
            .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
            .map_err(sqlite)?;
        if total > 0 && (*since_analyze as f64) > (total as f64) * ANALYZE_FRACTION {
            analyze(conn, since_analyze)?;
        }
    }

    Ok(())
}

fn analyze(conn: &Connection, since_analyze: &mut usize) -> Result<(), IndexError> {
    conn.execute_batch("ANALYZE;").map_err(sqlite)?;
    *since_analyze = 0;
    Ok(())
}

fn mark_ready(
    conn: &Connection,
    vault_root: &str,
    caught_up_from_ms: i64,
) -> Result<(), IndexError> {
    let tx = conn.unchecked_transaction().map_err(sqlite)?;
    apply::sync_note_count(&tx)?;
    apply::mark_ready(&tx, vault_root, caught_up_from_ms)?;
    tx.commit().map_err(sqlite)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serat_core::{ContentHash, NoteKind, NoteMeta, VaultPath};

    fn asset(path: &str) -> IndexOp {
        let path = VaultPath::new(path).expect("path");
        IndexOp::UpsertAsset {
            meta: NoteMeta {
                kind: NoteKind::Asset,
                hash: ContentHash::from_bytes([2u8; 32]),
                size: 0,
                mtime_ms: 0,
                path,
            },
        }
    }

    #[test]
    fn a_failed_commit_leaves_the_batch_for_the_next_command_and_does_not_kill_the_writer() {
        // The route the unreachability analysis missed. commit clears `pending` at its
        // end, so an error before that returns with the batch still in hand and the
        // next command meets a non-empty queue. An earlier version asserted that could
        // not happen; the assertion panicked the writer thread here, in every debug
        // build, at the moment the index was already in trouble.
        //
        // What must be true instead: the failure reaches the caller that provoked it,
        // the actor stays alive, the next command drains what was left, and no work is
        // lost.
        //
        // The fault is scoped to this actor (task #28): earlier this was a
        // process-wide static, and cargo runs a crate's unit tests concurrently by
        // default, so any other test's actor reaching `commit` first could consume
        // the flag this test had just armed, before this actor's own writer thread
        // ever saw it — a test that fails for a reason that says nothing about the
        // code, and only some of the time.
        // File-backed rather than in-memory, so a second connection can observe what
        // the actor has committed between one command and the next. That is what makes
        // the drain observable: with it removed, resolve_links returns Ok(0) either
        // way, because an empty links table resolves to nothing whether or not the
        // batch was applied first.
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("index.db");
        let conn = schema::open(&path).expect("open");
        schema::bootstrap(&conn).expect("bootstrap");
        let actor = IndexActor::with_connection(conn);
        let writer = actor.handle();

        // Armed before the submit, not after (task #67): a quiet writer commits an
        // `Apply` the moment it arrives, so arming after raced the writer thread and,
        // under load, lost -- the batch committed cleanly and the flush had nothing to
        // fail. See `FaultInjector::fail_until_a_flush_fails` for why this order holds
        // under every interleaving.
        actor.fault_injector().fail_until_a_flush_fails();
        writer.submit(asset("assets/a.pdf")).expect("submit");

        let failed = writer.flush();
        assert!(
            matches!(failed, Err(IndexError::Corrupt { .. })),
            "the injected failure was not reported to the caller: {failed:?}"
        );

        // The batch is still in hand. A command arriving now must drain it rather than
        // run against an index missing the work — and must not find a dead actor.
        let resolved = writer.resolve_links();
        assert!(
            !matches!(resolved, Err(IndexError::WriterStopped)),
            "the writer died on a failed commit; the caller was told the actor had              stopped when it had only failed a transaction"
        );
        assert!(resolved.is_ok(), "{resolved:?}");

        // The work the failed commit was carrying is in the index by now, because
        // resolve_links drained it first. Read it through a separate connection: this
        // is the assertion that fails if the drain is removed.
        let reader = schema::open(&path).expect("reader");
        let notes: i64 = reader
            .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
            .expect("count");
        assert_eq!(
            notes, 1,
            "the batch left in hand by the failed commit was not applied by the next              command; the work is still queued and a bulk load would have run without it"
        );

        actor.shutdown().expect("shutdown");
    }

    #[test]
    fn a_failed_commit_reports_nothing_and_the_batch_reports_once_it_lands() {
        // A batch left in hand by a failed commit is not announced (nothing changed); the
        // commit that finally applies it is, once.
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("index.db");
        let conn = schema::open(&path).expect("open");
        schema::bootstrap(&conn).expect("bootstrap");
        let seen = Arc::new(std::sync::Mutex::new(Vec::<CommittedBatch>::new()));
        let sink = Arc::clone(&seen);
        let actor = IndexActor::with_options(
            conn,
            ActorOptions {
                commit_observer: Some(CommitObserver::new(move |batch| {
                    sink.lock().expect("lock").push(batch);
                })),
                ..ActorOptions::default()
            },
        );
        let writer = actor.handle();

        actor.fault_injector().fail_until_a_flush_fails();
        writer.submit(asset("assets/a.pdf")).expect("submit");
        assert!(writer.flush().is_err(), "the injected failure");
        assert!(
            seen.lock().expect("lock").is_empty(),
            "a failed commit was reported"
        );

        writer.flush().expect("the retry commits it");
        let seen = seen.lock().expect("lock");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].ops, 1);
        drop(seen);
        actor.shutdown().expect("shutdown");
    }

    #[test]
    fn a_batch_rolled_back_after_its_statements_reports_nothing() {
        // The failure the other two tests cannot reach: it happens inside the open
        // transaction, after the statements ran. A call placed before COMMIT would
        // report here.
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("index.db");
        let conn = schema::open(&path).expect("open");
        schema::bootstrap(&conn).expect("bootstrap");
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        let actor = IndexActor::with_options(
            conn,
            ActorOptions {
                commit_observer: Some(CommitObserver::new(move |_| {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                })),
                ..ActorOptions::default()
            },
        );
        actor.fault_injector().fail_inside_transaction();
        let writer = actor.handle();
        writer.submit(asset("assets/a.pdf")).expect("submit");
        assert!(writer.flush().is_err());
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
        let reader = schema::open(&path).expect("reader");
        let rows: i64 = reader
            .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
            .expect("count");
        assert_eq!(rows, 0, "the rolled-back statements were visible");
        drop(actor);
    }

    #[test]
    fn a_failing_commit_with_nobody_waiting_reports_nothing() {
        let dir = tempfile::tempdir().expect("temp");
        let conn = schema::open(&dir.path().join("index.db")).expect("open");
        schema::bootstrap(&conn).expect("bootstrap");
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        let actor = IndexActor::with_options(
            conn,
            ActorOptions {
                commit_observer: Some(CommitObserver::new(move |_| {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                })),
                ..ActorOptions::default()
            },
        );
        actor.fault_injector().fail_every_commit();
        let writer = actor.handle();
        writer.submit(asset("assets/a.pdf")).expect("submit");
        assert!(writer.flush().is_err());
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
        drop(actor);
    }
}
