//! The save-to-index path, and the parse pool that feeds a rebuild.
//!
//! Two things live here because they are the same pipeline at two scales: one note
//! arriving from a save, and twenty thousand arriving from a walk.
//!
//! # Which interval NFR-PRF-03's 50 ms governs
//!
//! There are two paths into the index and they belong to different requirements.
//!
//! A save made *in Serat* goes bytes → hash → parse → writer, with no watcher and no
//! debounce. That is what [`IndexHandle::apply_and_wait`] measures and what
//! NFR-PRF-03's 50 ms governs. It is not a shortcut for benchmarking: architecture
//! §3.2 requires it, because echo suppression drops the watcher event for our own
//! write — an index fed only by the watcher would never index a note saved in Serat
//! at all.
//!
//! An *external* change goes through the watcher's 150 ms coalescing window (§3.1
//! step 1) before any of this runs. That interval is FR-VLT-04's, whose criterion is
//! 500 ms, and no amount of fast code brings it under 50 ms because the delay exists
//! before the code runs.
//!
//! Both are reported. A gate number that does not say which one it is means nothing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rayon::iter::{IntoParallelRefIterator as _, ParallelIterator as _};
use serat_core::{
    IndexError, IndexOp, IndexWriter as _, LoadedNote, NoteKind, NoteParser, NoteSource,
    SourceEntry, VaultPath,
};

use crate::schema;
use crate::writer::{DEFAULT_QUEUE_DEPTH, IndexActor, WriterHandle};

/// How many notes are parsed in parallel before the batch is handed to the writer.
///
/// The pool has to stay ahead of the writer without holding the whole vault in memory:
/// 20,000 parsed notes at once would be gigabytes, and one at a time would leave the
/// cores idle. A few hundred is enough to keep every core busy while the writer drains
/// the previous chunk.
pub const PARSE_CHUNK: usize = 256;

/// Size the parse pool at cores minus one.
///
/// Architecture §2.2. Leaving a core free is what keeps the UI responsive during a
/// rebuild — parsing is pure and CPU-bound, so a pool sized to every core competes
/// with the thread the user is typing on, and design goal 2 says nothing may block the
/// keystroke.
#[must_use]
pub fn parse_pool_threads() -> usize {
    std::thread::available_parallelism()
        .map(|cores| cores.get().saturating_sub(1).max(1))
        .unwrap_or(1)
}

/// Build the rayon pool the parse phase runs on.
///
/// # Errors
///
/// [`IndexError::Corrupt`] if the pool cannot be created, which in practice means the
/// process cannot spawn threads.
pub fn build_parse_pool() -> Result<rayon::ThreadPool, IndexError> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(parse_pool_threads())
        .thread_name(|i| format!("serat-parse-{i}"))
        .build()
        .map_err(|err| IndexError::Corrupt {
            message: format!("could not build the parse pool: {err}"),
        })
}

/// What one save cost, split at the boundary that matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveTiming {
    /// Reading the file and hashing it.
    pub read_hash: Duration,
    /// Parsing.
    pub parse: Duration,
    /// Submitting and waiting for the writer to commit — the point at which a new
    /// reader would see the change.
    pub write: Duration,
}

impl SaveTiming {
    /// The whole interval NFR-PRF-03 governs.
    #[must_use]
    pub fn total(self) -> Duration {
        self.read_hash + self.parse + self.write
    }
}

/// An open index, kept open across saves.
///
/// Holds the writer actor so that a save does not pay to open a database. That is most
/// of what makes 50 ms plausible: opening SQLite, applying pragmas and checking the
/// schema costs more than the work of one note.
#[derive(Debug)]
pub struct IndexHandle {
    actor: IndexActor,
    index_dir: PathBuf,
}

impl IndexHandle {
    /// Open (or create) the index in `index_dir` and start its writer.
    ///
    /// # Errors
    ///
    /// [`IndexError::Sqlite`] if the database cannot be opened or created.
    pub fn open(index_dir: &Path) -> Result<Self, IndexError> {
        Self::with_queue_depth(index_dir, DEFAULT_QUEUE_DEPTH)
    }

    /// As [`IndexHandle::open`], with a queue of a given depth.
    ///
    /// # Errors
    ///
    /// As [`IndexHandle::open`].
    pub fn with_queue_depth(index_dir: &Path, queue_depth: usize) -> Result<Self, IndexError> {
        let path = crate::index_path(index_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| IndexError::Corrupt {
                message: format!("could not create {}: {err}", parent.display()),
            })?;
        }
        let conn = schema::open(&path)?;
        schema::bootstrap_if_empty(&conn)?;
        Ok(Self {
            actor: IndexActor::with_capacity(conn, queue_depth),
            index_dir: index_dir.to_path_buf(),
        })
    }

    /// A writer handle, for callers that want to submit without waiting.
    #[must_use]
    pub fn writer(&self) -> WriterHandle {
        self.actor.handle()
    }

    /// Where the index lives.
    #[must_use]
    pub fn index_dir(&self) -> &Path {
        &self.index_dir
    }

    /// Index one note and return when a new reader would see it.
    ///
    /// This is NFR-PRF-03's interval. It reads through `source` rather than taking
    /// bytes, because the hash is what the index stores and what the hash gate and the
    /// stale guard compare — a caller passing bytes would have to hash them itself,
    /// and then two places would define what a note's identity is.
    ///
    /// It ends at the barrier, so "reflected in the index" means committed and visible
    /// to a reader that opens a transaction afterwards, not merely queued. That is the
    /// only reading of the requirement a benchmark can honestly take.
    ///
    /// # Errors
    ///
    /// [`IndexError`] if the write fails. A file that cannot be read is indexed as a
    /// removal, because a save path told about a file that is no longer there is
    /// describing a delete.
    pub fn apply_and_wait(
        &self,
        source: &dyn NoteSource,
        parser: &dyn NoteParser,
        path: &VaultPath,
    ) -> Result<SaveTiming, IndexError> {
        let writer = self.actor.handle();

        let reading = Instant::now();
        let loaded = source.load(path);
        let read_hash = reading.elapsed();

        let (op, parse) = match loaded {
            // A file the save path was told about but cannot read has been deleted.
            Err(_) => (IndexOp::Remove { path: path.clone() }, Duration::ZERO),
            Ok(loaded) => match build_op(parser, path, loaded) {
                Some(pair) => pair,
                // Nothing to index and nothing to report: the previous row stays, which
                // is better than deleting a note because one save of it did not parse.
                None => {
                    return Ok(SaveTiming {
                        read_hash,
                        parse: Duration::ZERO,
                        write: Duration::ZERO,
                    });
                }
            },
        };

        let writing = Instant::now();
        writer.submit_blocking(op)?;
        writer.flush()?;
        let write = writing.elapsed();

        Ok(SaveTiming {
            read_hash,
            parse,
            write,
        })
    }

    /// Stop the writer, applying anything queued.
    ///
    /// # Errors
    ///
    /// [`IndexError::WriterStopped`] if it had already died.
    pub fn close(self) -> Result<(), IndexError> {
        self.actor.shutdown()
    }
}

/// Turn a loaded file into the operation that indexes it, or `None` if it cannot be.
///
/// A note the parser refuses is skipped, which is the behaviour I4 established and
/// which this deliberately does not change.
///
/// It is worth flagging rather than quietly deciding: design goal 5 says capability
/// degrades and nothing breaks, and skipping means the vault's *contents* depend on
/// whether the parser understood a file — an unparseable note vanishes from the index
/// rather than appearing without blocks. Indexing it as a bare row instead would keep
/// it listable and linkable and lose only its text. That is a change to what the index
/// contains, so it belongs to architect and not to a timing task.
fn build_op(
    parser: &dyn NoteParser,
    path: &VaultPath,
    loaded: LoadedNote,
) -> Option<(IndexOp, Duration)> {
    match loaded.meta.kind {
        NoteKind::Asset => Some((IndexOp::UpsertAsset { meta: loaded.meta }, Duration::ZERO)),
        NoteKind::Note => {
            let parsing = Instant::now();
            let parsed = parser.parse(path, &loaded.bytes).ok()?;
            Some((
                IndexOp::UpsertNote {
                    meta: loaded.meta,
                    parsed: Box::new(parsed),
                },
                parsing.elapsed(),
            ))
        }
    }
}

/// Read and parse a chunk of notes across the pool, in order.
///
/// Order is preserved because a rebuild that visits files in a different order each
/// run produces different rowids, and "delete the index, rebuild, get the same
/// answers" is easier to assert when only the content varies.
///
/// Returns the operations and the time spent reading and parsing. The two are summed
/// across threads, so on a pool of eight they can exceed the wall clock — which is the
/// point: it is CPU time, and comparing it against the writer's is what answers
/// architecture's open design question 1.
pub(crate) fn parse_chunk(
    pool: &rayon::ThreadPool,
    source: &(dyn NoteSource + Sync),
    parser: &(dyn NoteParser + Sync),
    entries: &[SourceEntry],
) -> (Vec<Option<IndexOp>>, Duration, Duration) {
    pool.install(|| {
        let results: Vec<(Option<IndexOp>, Duration, Duration)> = entries
            .par_iter()
            .map(|entry| {
                let path = &entry.path;
                let reading = Instant::now();
                let loaded = source.load(path);
                let read_hash = reading.elapsed();

                match loaded {
                    Ok(loaded) => match build_op(parser, path, loaded) {
                        Some((op, parse)) => (Some(op), read_hash, parse),
                        None => (None, read_hash, Duration::ZERO),
                    },
                    Err(_) => (None, read_hash, Duration::ZERO),
                }
            })
            .collect();

        let mut ops = Vec::with_capacity(results.len());
        let mut read_hash = Duration::ZERO;
        let mut parse = Duration::ZERO;
        for (op, read, parsed) in results {
            ops.push(op);
            read_hash += read;
            parse += parsed;
        }
        (ops, read_hash, parse)
    })
}
