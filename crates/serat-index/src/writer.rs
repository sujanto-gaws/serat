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
    MarkReady(String, SyncSender<Result<(), IndexError>>),
    /// Capture and drop the blocks_fts triggers before a bulk load.
    BeginBulkLoad(SyncSender<Result<Vec<String>, IndexError>>),
    /// Rebuild the FTS index in one pass and restore the captured triggers.
    EndBulkLoad(Vec<String>, SyncSender<Result<(), IndexError>>),
    /// Resolve every link whose target is now present.
    ResolveLinks(SyncSender<Result<usize, IndexError>>),
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

    /// Drop the `blocks_fts` triggers, returning their definitions.
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

    /// Rebuild the FTS index in one pass and put the triggers back.
    ///
    /// # Errors
    ///
    /// As [`WriterHandle::begin_bulk_load`]. A failure here leaves the index without
    /// its triggers, which is why the caller must not mark it ready.
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
    // Read back only by tests, through `fault_injector()`. Kept as a real field
    // rather than a `#[cfg(test)]` one: it is the same `FaultInjector` `commit`
    // consults on every run in every build, not a test-only shim, so a production
    // build genuinely never reads it back -- only ever hands a clone to the writer
    // thread at construction. The allow is for that write-only shape, not for
    // sloppiness.
    #[cfg_attr(not(test), allow(dead_code))]
    fault: FaultInjector,
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
        assert!(
            capacity > 0,
            "a writer queue with no capacity accepts nothing"
        );
        let (tx, rx) = sync_channel(capacity);
        let fault = FaultInjector::default();
        let actor_fault = fault.clone();
        let join = std::thread::Builder::new()
            .name("serat-index-writer".to_owned())
            .spawn(move || run(conn, &rx, &actor_fault))
            .expect("spawn the index writer");

        Self {
            handle: WriterHandle { tx },
            join: Some(join),
            fault,
        }
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
    /// other crate, and [`FaultInjector::fail_next_commit`] — the only method
    /// that can make it do anything — is the part gated to tests. A production
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
fn run(conn: Connection, rx: &Receiver<Command>, fault: &FaultInjector) {
    let mut pending: Vec<IndexOp> = Vec::with_capacity(BATCH_SIZE);
    let mut since_analyze = 0usize;
    // Whether the blocks_fts triggers are currently off, which is the writer's only
    // signal that it is inside a rebuild rather than serving saves. Set by
    // BeginBulkLoad and cleared by EndBulkLoad, so a writer that never sees either --
    // the incremental path -- keeps the opportunistic ANALYZE it was designed for.
    let mut bulk_load = false;
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
                    let _ = commit(&conn, &mut pending, &mut since_analyze, bulk_load, fault);
                    return;
                }
            },
        };

        let mut barrier: Option<(SyncSender<Result<(), IndexError>>, Barrier)> = None;
        match next {
            Command::Apply(op) => pending.push(*op),
            Command::Flush(reply) => barrier = Some((reply, Barrier::None)),
            Command::Analyze(reply) => barrier = Some((reply, Barrier::Analyze)),
            Command::MarkReady(root, reply) => barrier = Some((reply, Barrier::MarkReady(root))),
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
                let result = commit(&conn, &mut pending, &mut since_analyze, bulk_load, fault)
                    .and_then(|()| begin_bulk_load(&conn));
                // Only once the triggers are actually off. Setting it before would
                // suppress ANALYZE for a bulk load that failed to start.
                bulk_load = result.is_ok();
                let _ = reply.send(result);
                continue;
            }
            Command::EndBulkLoad(triggers, reply) => {
                let result = commit(&conn, &mut pending, &mut since_analyze, bulk_load, fault)
                    .and_then(|()| end_bulk_load(&conn, &triggers));
                // Cleared either way: a failed end_bulk_load leaves the index without
                // its triggers and the caller must not mark it ready, but the writer
                // is no longer bulk loading and must not keep suppressing ANALYZE.
                bulk_load = false;
                let _ = reply.send(result);
                continue;
            }
            Command::ResolveLinks(reply) => {
                let result = commit(&conn, &mut pending, &mut since_analyze, bulk_load, fault)
                    .and_then(|()| resolve_links(&conn));
                let _ = reply.send(result);
                continue;
            }
            Command::Stop => {
                let _ = commit(&conn, &mut pending, &mut since_analyze, bulk_load, fault);
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
                    let _ = commit(&conn, &mut pending, &mut since_analyze, bulk_load, fault);
                    return;
                }
                Ok(other) => {
                    deferred = Some(other);
                    break;
                }
                Err(_) => break,
            }
        }

        let mut result = commit(&conn, &mut pending, &mut since_analyze, bulk_load, fault);

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

/// Capture the `blocks_fts` trigger definitions and drop them.
fn begin_bulk_load(conn: &Connection) -> Result<Vec<String>, IndexError> {
    let definitions: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT sql FROM sqlite_master                  WHERE type = 'trigger' AND tbl_name = 'blocks' ORDER BY name",
            )
            .map_err(sqlite)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(sqlite)?;
        rows.collect::<Result<_, _>>().map_err(sqlite)?
    };

    for definition in &definitions {
        // The name is the third whitespace-separated token of CREATE TRIGGER <name>.
        if let Some(name) = definition.split_whitespace().nth(2) {
            conn.execute_batch(&format!("DROP TRIGGER IF EXISTS {name};"))
                .map_err(sqlite)?;
        }
    }
    Ok(definitions)
}

/// Build the FTS index in one pass and restore the triggers.
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
/// P0 resolves by exact path and by path plus the Markdown extension, which is what
/// the benchmark vault and an Obsidian vault using full paths both produce. Obsidian's
/// shortest-unique-name resolution is P1 and belongs with the rest of link handling;
/// what matters here is that unresolved stays NULL rather than becoming wrong.
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
    const RESOLVE: &str = "
        UPDATE links
           SET dst_note = (
                SELECT n.id FROM notes n
                 WHERE n.path = links.dst_path
                    OR n.path = links.dst_path || '.md'
                    OR (instr(links.dst_path, '/') = 0
                        AND (n.file_name = links.dst_path
                             OR n.file_name = links.dst_path || '.md'))
              ORDER BY length(n.path), n.path
                 LIMIT 1
           )
         WHERE dst_note IS NULL
           AND kind <> 'tag'
           AND dst_path IS NOT NULL
           AND EXISTS (
                SELECT 1 FROM notes n
                 WHERE n.path = links.dst_path
                    OR n.path = links.dst_path || '.md'
                    OR (instr(links.dst_path, '/') = 0
                        AND (n.file_name = links.dst_path
                             OR n.file_name = links.dst_path || '.md'))
           )";
    let resolved = conn.execute(RESOLVE, []).map_err(sqlite)?;
    Ok(resolved)
}

enum Barrier {
    None,
    Analyze,
    MarkReady(String),
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
pub(crate) struct FaultInjector(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl FaultInjector {
    /// Make the next commit through this handle's actor fail once.
    #[cfg(test)]
    pub(crate) fn fail_next_commit(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Consume the injected failure, if one is armed. Always false in production: the
    /// only way to arm it is [`FaultInjector::fail_next_commit`], which does not exist
    /// outside `#[cfg(test)]`.
    fn take(&self) -> bool {
        self.0.swap(false, std::sync::atomic::Ordering::SeqCst)
    }
}

fn commit(
    conn: &Connection,
    pending: &mut Vec<IndexOp>,
    since_analyze: &mut usize,
    bulk_load: bool,
    fault: &FaultInjector,
) -> Result<(), IndexError> {
    if pending.is_empty() {
        return Ok(());
    }

    if fault.take() {
        return Err(IndexError::Corrupt {
            message: "simulated commit failure".to_owned(),
        });
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

fn mark_ready(conn: &Connection, vault_root: &str) -> Result<(), IndexError> {
    let tx = conn.unchecked_transaction().map_err(sqlite)?;
    apply::sync_note_count(&tx)?;
    apply::mark_ready(&tx, vault_root)?;
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

        writer.submit(asset("assets/a.pdf")).expect("submit");
        actor.fault_injector().fail_next_commit();

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
}
