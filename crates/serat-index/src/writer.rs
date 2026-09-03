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
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;

use rusqlite::Connection;
use serat_core::{IndexError, IndexOp, IndexWriter};

use crate::apply;
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
/// Batching is the whole of the tuning, and it is the number open design question 1
/// will move if the writer turns out to dominate rebuild time.
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
    MarkReady(String, SyncSender<Result<(), IndexError>>),
    /// Apply anything in hand and end the loop.
    ///
    /// An explicit message rather than relying on every sender being dropped: handles
    /// are cloneable, so "the channel closed" means "the last clone went", and a
    /// caller holding one while asking the actor to stop would wait forever. That is
    /// not hypothetical — the rebuild holds a handle for the whole run and shuts the
    /// actor down at the end of it.
    Stop,
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

    /// Run `ANALYZE` and wait for it.
    ///
    /// # Errors
    ///
    /// [`IndexError::WriterStopped`] if the writer is gone; whatever SQLite says
    /// otherwise.
    pub fn analyze(&self) -> Result<(), IndexError> {
        self.round_trip(Command::Analyze)
    }

    /// Mark the index usable, after a rebuild has committed everything.
    ///
    /// # Errors
    ///
    /// As [`WriterHandle::analyze`].
    pub fn mark_ready(&self, vault_root: &str) -> Result<(), IndexError> {
        self.round_trip(|reply| Command::MarkReady(vault_root.to_owned(), reply))
    }
}

/// The writer thread and the connection it owns.
#[derive(Debug)]
pub struct IndexActor {
    handle: WriterHandle,
    join: Option<JoinHandle<()>>,
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

    /// Start a writer over an already-open connection, for tests.
    #[must_use]
    pub fn with_connection(conn: Connection) -> Self {
        let (tx, rx) = sync_channel(DEFAULT_QUEUE_DEPTH);
        let join = std::thread::Builder::new()
            .name("serat-index-writer".to_owned())
            .spawn(move || run(conn, &rx))
            .expect("spawn the index writer");

        Self {
            handle: WriterHandle { tx },
            join: Some(join),
        }
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

    fn stop(&mut self) {
        // Send Stop rather than dropping the sender. Handles are clones, so the
        // channel only closes when the last one goes, and a caller that still holds
        // one while shutting the actor down would block here for ever.
        let _ = self.handle.tx.send(Command::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
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
fn run(conn: Connection, rx: &Receiver<Command>) {
    let mut pending: Vec<IndexOp> = Vec::with_capacity(BATCH_SIZE);
    let mut since_analyze = 0usize;

    loop {
        let Ok(first) = rx.recv() else {
            // Sender dropped: apply whatever is in hand and stop. Discarding a queued
            // change at shutdown would leave the index disagreeing with the disk.
            let _ = commit(&conn, &mut pending, &mut since_analyze);
            return;
        };

        let mut barrier: Option<(SyncSender<Result<(), IndexError>>, Barrier)> = None;
        match first {
            Command::Apply(op) => pending.push(*op),
            Command::Flush(reply) => barrier = Some((reply, Barrier::None)),
            Command::Analyze(reply) => barrier = Some((reply, Barrier::Analyze)),
            Command::MarkReady(root, reply) => barrier = Some((reply, Barrier::MarkReady(root))),
            Command::Stop => {
                let _ = commit(&conn, &mut pending, &mut since_analyze);
                return;
            }
        }

        // Take whatever else is already waiting, so a burst becomes one transaction.
        while barrier.is_none() && pending.len() < BATCH_SIZE {
            match rx.try_recv() {
                Ok(Command::Apply(op)) => pending.push(*op),
                Ok(Command::Flush(reply)) => barrier = Some((reply, Barrier::None)),
                Ok(Command::Analyze(reply)) => barrier = Some((reply, Barrier::Analyze)),
                Ok(Command::MarkReady(root, reply)) => {
                    barrier = Some((reply, Barrier::MarkReady(root)));
                }
                Ok(Command::Stop) => {
                    let _ = commit(&conn, &mut pending, &mut since_analyze);
                    return;
                }
                Err(_) => break,
            }
        }

        let mut result = commit(&conn, &mut pending, &mut since_analyze);

        if let Some((reply, extra)) = barrier {
            if result.is_ok() {
                result = match &extra {
                    Barrier::None => Ok(()),
                    Barrier::Analyze => analyze(&conn, &mut since_analyze),
                    Barrier::MarkReady(root) => mark_ready(&conn, root),
                };
            }
            // A receiver that has gone away means the caller stopped waiting; that is
            // their business, not a reason to stop writing.
            let _ = reply.send(result);
        }
    }
}

enum Barrier {
    None,
    Analyze,
    MarkReady(String),
}

/// Apply a batch in one transaction, keeping `note_count` in step inside it.
fn commit(
    conn: &Connection,
    pending: &mut Vec<IndexOp>,
    since_analyze: &mut usize,
) -> Result<(), IndexError> {
    if pending.is_empty() {
        return Ok(());
    }

    let tx = conn.unchecked_transaction().map_err(sqlite)?;
    let mut touched = 0usize;
    for op in pending.iter() {
        let applied = apply::apply(&tx, op)?;
        touched += applied.notes + applied.removed;
    }
    apply::sync_note_count(&tx)?;
    tx.commit().map_err(sqlite)?;

    pending.clear();
    *since_analyze += touched;

    // §3.1 step 5 and §4.3: re-run ANALYZE after a batch touching more than 5% of
    // notes, because stale statistics are what make a typed query a full scan.
    let total: i64 = conn
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .map_err(sqlite)?;
    if total > 0 && (*since_analyze as f64) > (total as f64) * ANALYZE_FRACTION {
        analyze(conn, since_analyze)?;
    }

    Ok(())
}

fn analyze(conn: &Connection, since_analyze: &mut usize) -> Result<(), IndexError> {
    conn.execute_batch("ANALYZE;").map_err(sqlite)?;
    *since_analyze = 0;
    Ok(())
}

fn mark_ready(conn: &Connection, vault_root: &str) -> Result<(), IndexError> {
    let tx = conn.unchecked_transaction().map_err(sqlite)?;
    apply::sync_note_count(&tx)?;
    apply::mark_ready(&tx, vault_root)?;
    tx.commit().map_err(sqlite)
}
