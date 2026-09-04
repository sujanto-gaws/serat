//! Building the whole index from the vault.
//!
//! ADR-0002 makes this the answer to every schema change and every doubt about the
//! index's contents, so it has to stay fast (NFR-PRF-04: 20,000 notes under 30 s) and
//! it has to be the only path — a repair that produced a different result from a
//! rebuild would be a second implementation of the truth.

use std::path::Path;
use std::time::{Duration, Instant};

use serat_core::{IndexError, IndexOp, IndexWriter, NoteKind, NoteParser, NoteSource, VaultPath};

use crate::schema;
use crate::writer::{DEFAULT_QUEUE_DEPTH, IndexActor, WriterHandle};

/// What a rebuild produced.
///
/// Counted from the database after the writer has drained, not accumulated as we go:
/// a report that counted what it *sent* would be unable to notice the writer dropping
/// something, which is exactly the failure a benchmark's wall-clock number cannot see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RebuildReport {
    /// Rows in `notes`, both notes and assets.
    pub notes: usize,
    /// Rows in `blocks`.
    pub blocks: usize,
    /// Rows in `fields`.
    pub fields: usize,
    /// Rows in `links`.
    pub links: usize,
    /// Rows in `tasks`.
    pub tasks: usize,
    /// Links that now point at a note, filled by the second pass.
    pub links_resolved: usize,
    /// How long enumerating the vault took.
    ///
    /// The four phases exist because a single wall-clock number cannot settle open
    /// design question 1 — does the parse pool or the index writer dominate a rebuild
    /// — which architecture §1.2 says the P0 benchmark answers. Reported as one figure,
    /// "the writer dominates" and "reading 39 MiB off disk dominates" are
    /// indistinguishable while implying completely different fixes (F-10).
    pub walk: Duration,
    /// How long reading and hashing files took.
    pub read_hash: Duration,
    /// How long parsing took.
    pub parse: Duration,
    /// How long the writer took, measured as the barrier at the end: everything
    /// submitted but not yet applied when the last file was parsed.
    ///
    /// The four will not sum to the wall clock, and that gap is a finding rather than
    /// a defect — it is the producer and the writer overlapping, which is the whole
    /// point of the actor.
    pub write: Duration,
    /// Files the source refused to read. Not a failure: a file deleted while the
    /// rebuild was walking is ordinary, and one unreadable file must not stop a vault
    /// indexing (design goal 5).
    pub skipped: usize,
}

/// Rebuild the index at `index_dir` from `source`, parsing with `parser`.
///
/// Returns only after the writer has drained and `ANALYZE` has run, so wall-clock
/// around this call is the whole cost — if it returned while the actor still had a
/// queue, any number taken from it would be fiction.
///
/// # Why the dependencies arrive as arguments
///
/// A rebuild must walk and read (serat-vault), parse (serat-parse) and write
/// (serat-index), and no crate may depend on two of those (architecture §2.1). So the
/// caller constructs both and hands them in as trait objects declared in serat-core.
/// It is also what lets this be tested with a stub source and a stub parser, with no
/// filesystem and no Markdown anywhere in the test.
///
/// # Errors
///
/// [`IndexError`] if the database cannot be created or a write fails. An individual
/// file that cannot be read or parsed is counted in [`RebuildReport::skipped`], not
/// returned as an error.
pub fn rebuild(
    source: &dyn NoteSource,
    parser: &dyn NoteParser,
    index_dir: &Path,
) -> Result<RebuildReport, IndexError> {
    rebuild_with_queue_depth(source, parser, index_dir, DEFAULT_QUEUE_DEPTH)
}

/// A rebuild whose writer queue holds at most `queue_depth` operations.
///
/// [`rebuild`] is this with the default depth, and that is what callers want. This
/// exists because at the default depth a test would have to submit more than 4,096
/// operations before the queue ever fills — so a smaller rebuild cannot tell a
/// blocking producer from a dropping one, and the regression test for F-19 did not in
/// fact test F-19 (F-35). With a small depth here, the rebuild's own choice of a
/// blocking submit is observable.
///
/// # Errors
///
/// As [`rebuild`].
pub fn rebuild_with_queue_depth(
    source: &dyn NoteSource,
    parser: &dyn NoteParser,
    index_dir: &Path,
    queue_depth: usize,
) -> Result<RebuildReport, IndexError> {
    let path = index_path(index_dir);

    // ADR-0002: there is no repair, so an existing index is removed rather than
    // reconciled. Deleting the WAL and shared-memory files too, because leaving them
    // beside a new database is how a rebuild inherits the state it was meant to
    // discard.
    for suffix in ["", "-wal", "-shm"] {
        let victim = path.with_file_name(format!("index.db{suffix}"));
        if victim.exists() {
            std::fs::remove_file(&victim).map_err(|err| IndexError::Corrupt {
                message: format!("could not remove {}: {err}", victim.display()),
            })?;
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| IndexError::Corrupt {
            message: format!("could not create {}: {err}", parent.display()),
        })?;
    }

    let conn = schema::open(&path)?;
    schema::bootstrap_if_empty(&conn)?;
    let actor = IndexActor::with_capacity(conn, queue_depth);
    let writer = actor.handle();

    // Step 2 of the sequence in schema/001_init.sql: the triggers come off before any
    // row is loaded. Loading 26,000 blocks through them costs about 6x — 2.1 s live
    // against 0.32 s dropped-and-rebuilt, reproduced independently at 5.8x — and the
    // resulting search index is identical, which
    // the_bulk_load_and_the_trigger_path_produce_the_same_search_index asserts.
    let triggers = writer.begin_bulk_load()?;

    let started = Instant::now();
    let entries = source.entries().map_err(|err| IndexError::Corrupt {
        message: format!("could not enumerate the vault: {err}"),
    })?;
    let walk = started.elapsed();

    let mut timing = Phases::default();
    let mut skipped = 0usize;
    for entry in entries {
        // F-29b: entries() now carries kind, size and mtime as well as the path, so
        // the rebuild no longer re-derives what the walk already knew. index_one still
        // takes the path because load() is what supplies the hash, and the hash is
        // what the index stores.
        match index_one(source, parser, &writer, &entry.path, &mut timing) {
            Ok(()) => {}
            Err(Skip) => skipped += 1,
        }
    }

    // The barrier. Everything above is asynchronous; nothing below may observe a
    // partial index. What it costs is the writer's backlog — the work the producer
    // did not have to wait for — which is the honest reading of the write phase.
    let drain = Instant::now();
    writer.flush()?;
    let write = timing.write + drain.elapsed();

    // Steps 4 and 5: build the FTS index in one pass, then put the triggers back so
    // the incremental path keeps working.
    writer.end_bulk_load(triggers)?;

    // The second pass the foreign key forces.
    let links_resolved = writer.resolve_links()?;

    // Step 6, and only after everything above has committed: marking the index ready
    // is what makes a crash mid-rebuild recoverable, because a rebuild that died
    // between dropping the triggers and restoring them leaves the state building.
    writer.analyze()?;
    writer.mark_ready(&index_dir.to_string_lossy())?;
    actor.shutdown()?;

    let mut report = count(&path, skipped)?;
    report.links_resolved = links_resolved;
    report.walk = walk;
    report.read_hash = timing.read_hash;
    report.parse = timing.parse;
    report.write = write;
    Ok(report)
}

/// A file that could not be read or parsed. Counted, not propagated.
struct Skip;

/// Time spent in each phase, accumulated across files.
#[derive(Debug, Default)]
struct Phases {
    read_hash: Duration,
    parse: Duration,
    write: Duration,
}

fn index_one(
    source: &dyn NoteSource,
    parser: &dyn NoteParser,
    writer: &WriterHandle,
    path: &VaultPath,
    timing: &mut Phases,
) -> Result<(), Skip> {
    let reading = Instant::now();
    let loaded = source.load(path).map_err(|_| Skip)?;
    timing.read_hash += reading.elapsed();

    let op = match loaded.meta.kind {
        NoteKind::Asset => IndexOp::UpsertAsset { meta: loaded.meta },
        NoteKind::Note => {
            let parsing = Instant::now();
            let parsed = parser.parse(path, &loaded.bytes).map_err(|_| Skip)?;
            timing.parse += parsing.elapsed();
            IndexOp::UpsertNote {
                meta: loaded.meta,
                parsed: Box::new(parsed),
            }
        }
    };

    // Blocking submit, deliberately, and this is the one place it is right.
    //
    // IndexWriter::submit refuses when the queue is full, because on the watcher path
    // backpressure means "drop it and rescan" (a dropped path is rediscovered by the
    // walk). During a rebuild nothing will ever tell us about this file again, so
    // dropping it would mean quietly indexing part of the vault — the failure a
    // wall-clock benchmark number cannot see. Here the producer waits for the writer.
    // Only the waiting counts as write time here: the writer's own work happens on
    // its thread and is charged at the barrier. What this measures is the producer
    // being held up, which is the number that says the writer is the bottleneck.
    let submitting = Instant::now();
    let result = writer.submit_blocking(op).map_err(|_| Skip);
    timing.write += submitting.elapsed();
    result
}

/// Count what actually landed.
fn count(path: &Path, skipped: usize) -> Result<RebuildReport, IndexError> {
    let conn = schema::open(path)?;
    let one = |sql: &str| -> Result<usize, IndexError> {
        conn.query_row(sql, [], |row| row.get::<_, i64>(0))
            .map(|count| count as usize)
            .map_err(schema::sqlite)
    };

    Ok(RebuildReport {
        notes: one("SELECT count(*) FROM notes")?,
        blocks: one("SELECT count(*) FROM blocks")?,
        fields: one("SELECT count(*) FROM fields")?,
        links: one("SELECT count(*) FROM links")?,
        tasks: one("SELECT count(*) FROM tasks")?,
        skipped,
        ..RebuildReport::default()
    })
}

/// The database file inside an index directory.
#[must_use]
pub fn index_path(index_dir: &Path) -> std::path::PathBuf {
    index_dir.join("index.db")
}
