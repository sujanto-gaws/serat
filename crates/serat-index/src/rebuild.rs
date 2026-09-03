//! Building the whole index from the vault.
//!
//! ADR-0002 makes this the answer to every schema change and every doubt about the
//! index's contents, so it has to stay fast (NFR-PRF-04: 20,000 notes under 30 s) and
//! it has to be the only path — a repair that produced a different result from a
//! rebuild would be a second implementation of the truth.

use std::path::Path;

use serat_core::{IndexError, IndexOp, IndexWriter, NoteKind, NoteParser, NoteSource, VaultPath};

use crate::schema;
use crate::writer::{IndexActor, WriterHandle};

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

    let actor = IndexActor::open(&path)?;
    let writer = actor.handle();

    let entries = source.entries().map_err(|err| IndexError::Corrupt {
        message: format!("could not enumerate the vault: {err}"),
    })?;

    let mut skipped = 0usize;
    for path in entries {
        match index_one(source, parser, &writer, &path) {
            Ok(()) => {}
            Err(Skip) => skipped += 1,
        }
    }

    // The barrier. Everything above is asynchronous; nothing below may observe a
    // partial index.
    writer.flush()?;
    writer.analyze()?;
    writer.mark_ready(&index_dir.to_string_lossy())?;
    actor.shutdown()?;

    let report = count(&path, skipped)?;
    Ok(report)
}

/// A file that could not be read or parsed. Counted, not propagated.
struct Skip;

fn index_one(
    source: &dyn NoteSource,
    parser: &dyn NoteParser,
    writer: &WriterHandle,
    path: &VaultPath,
) -> Result<(), Skip> {
    let loaded = source.load(path).map_err(|_| Skip)?;

    let op = match loaded.meta.kind {
        NoteKind::Asset => IndexOp::UpsertAsset { meta: loaded.meta },
        NoteKind::Note => {
            let parsed = parser.parse(path, &loaded.bytes).map_err(|_| Skip)?;
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
    writer.submit_blocking(op).map_err(|_| Skip)
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
    })
}

/// The database file inside an index directory.
#[must_use]
pub fn index_path(index_dir: &Path) -> std::path::PathBuf {
    index_dir.join("index.db")
}
