//! The save-to-index path (NFR-PRF-03).
//!
//! # Which interval NFR-PRF-03's 50 ms governs
//!
//! There are two paths into the index and they belong to different requirements.
//!
//! A save made *in Serat* goes bytes → hash → parse → writer, with no watcher and no
//! debounce. That is what [`IndexHandle::apply`] serves and what NFR-PRF-03's 50 ms
//! governs. It is not a shortcut for benchmarking: architecture
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

use serat_core::{
    EchoGuard, IndexError, IndexOp, IndexWriter, LoadedNote, NoteKind, NoteParser, NoteSource,
    VaultPath,
};

use crate::schema;
use crate::writer::{DEFAULT_QUEUE_DEPTH, IndexActor, WriterHandle};

/// Size a parse pool at cores minus one.
///
/// Architecture §2.2. Leaving a core free is what keeps the UI responsive during a
/// rebuild — parsing is pure and CPU-bound, so a pool sized to every core competes
/// with the thread the user is typing on, and design goal 2 says nothing may block the
/// keystroke.
///
/// Sizing logic only: this crate's `rebuild()` does not currently run parsing across a
/// pool built from this (its own `index_one` is sequential — a separate, already-
/// merged implementation of I4 that this branch does not revisit). Kept because the
/// number is architecture's answer regardless of who builds the pool, and a future
/// caller — most plausibly whoever wires a real `NoteParser` in task #8/I3 and revisits
/// NFR-PRF-04 — needs it unchanged from what §2.2 specifies.
#[must_use]
pub fn parse_pool_threads() -> usize {
    std::thread::available_parallelism()
        .map(|cores| cores.get().saturating_sub(1).max(1))
        .unwrap_or(1)
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

    /// How many rows `notes` holds — the count `IndexState::Ready`'s own invariant
    /// already trusts, not one recomputed here.
    ///
    /// Reads `index_meta.note_count` directly rather than `count(*) FROM notes`: the
    /// two are already asserted equal on every open by `schema::inspect`'s
    /// `CountMismatch` check (a disagreement between them means a torn transaction,
    /// not a live vault), so recomputing would spend a table scan re-deriving a number
    /// the open check already stands behind. `count(*)` includes assets as well as
    /// notes (`apply::sync_note_count`'s query has no `kind` filter), which is what
    /// `contracts/ipc.rs`'s `IndexState::Ready.note_count` documents itself as.
    ///
    /// A short-lived read connection rather than the writer (ADR-0001): a plain
    /// `SELECT` needs none of the single-writer's ordering guarantees, and routing it
    /// through the writer's channel would be a round trip to answer a question the
    /// writer has no better view of than any other connection does.
    ///
    /// # Errors
    ///
    /// [`IndexError::Sqlite`] if the database cannot be opened or the row is missing —
    /// the latter should not happen against an index `schema::inspect` reported
    /// `Ready`, since `index_meta` always has exactly one row (`schema/001_init.sql`).
    pub fn note_count(&self) -> Result<u32, IndexError> {
        let path = crate::index_path(&self.index_dir);
        let conn = schema::open(&path)?;
        let count: i64 = conn
            .query_row(
                "SELECT note_count FROM index_meta WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(schema::sqlite)?;
        // Saturating rather than erroring: a vault holding more than u32::MAX notes
        // is not a case this contract needs to represent faithfully, only harmlessly.
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    /// Index one note. Submits and returns; it does **not** wait.
    ///
    /// This is the save path of architecture §3.1.1, and not waiting is the whole of
    /// it. The UI reports "saved" when the rename completes and learns "indexed" from
    /// `index.changed`; a save that blocked on the writer would put the index back on
    /// the keystroke path, which design goal 2 forbids.
    ///
    /// There is deliberately no `apply_and_wait`. It would be a barrier called once
    /// per note rather than once per phase, and — the reason that actually settles it —
    /// `apply_and_wait()` reads like a write with the wait hidden inside it, one
    /// copy-paste away from the path where waiting is forbidden. A caller that must
    /// wait writes `apply(...)` then `writer().flush()`, which shows the wait at the
    /// call site. That is the benchmark harness's business, not the product's.
    ///
    /// It reads through `source` rather than taking bytes, because the hash is what
    /// the index stores and what both the hash gate and the stale guard compare; a
    /// caller passing bytes would have to hash them itself, and then two places would
    /// define what a note's identity is.
    ///
    /// `echo` is a separate trait object rather than a fourth thing `source` knows how
    /// to do (F-210): once — and only once — the submit below is accepted,
    /// [`EchoGuard::record_echo`] is called with the hash just indexed, so the watcher
    /// recognises the write that produced it and drops the event instead of reindexing
    /// what this call already indexed. Recording only *after* a successful submit is
    /// F-224's resolution to F-196/F-219/F-222: recording before the write could
    /// suppress the only event that would ever have indexed the note, if the submit
    /// that followed was then refused. Late recording removes that race rather than
    /// narrowing it — a refused submit records nothing here, so the watcher's own
    /// event carries the save instead, and a watcher event that wins a race against
    /// this call merely indexes the same note a moment early, which is safe because
    /// `upsert_note` is delete-then-insert by path.
    ///
    /// # Errors
    ///
    /// [`IndexError::QueueFull`] if the writer is behind — ordinary backpressure. No
    /// special recovery is needed on this path any more: nothing was recorded, so the
    /// watcher's own event for this write, whenever it arrives, indexes it normally.
    /// [`IndexError::WriterStopped`] if the writer is gone.
    ///
    /// # Links resolve on this path too (task #13, closing F-244)
    ///
    /// `upsert_note` is delete-then-insert, and `ON DELETE CASCADE` plus
    /// `links.dst_note`'s `ON DELETE SET NULL` (schema/001_init.sql) mean every save
    /// through this path used to null every inbound link to the note before
    /// re-inserting its own rows, with nothing to put them back until the next full
    /// rebuild — FR-NAV-04 (Backlinks, Must at P1), filed as F-244 and fixed as task
    /// #13. `apply::upsert_note` and `apply::upsert_asset` now each call
    /// `resolve_inbound` after re-inserting, scoped to the one note that just changed
    /// rather than the rebuild's full-table `resolve_links`, and `upsert_note` also
    /// calls `resolve_outbound` for its own just-inserted links. Both cost an indexed
    /// lookup (`ix_links_unresolved`, schema/001_init.sql) rather than a table scan,
    /// so this stays inside NFR-PRF-03's 50 ms rather than growing with vault size.
    pub fn apply(
        &self,
        source: &dyn NoteSource,
        parser: &dyn NoteParser,
        echo: &dyn EchoGuard,
        path: &VaultPath,
    ) -> Result<(), IndexError> {
        apply_via(&self.actor.handle(), source, parser, echo, path)
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

/// The body of [`IndexHandle::apply`], taking the writer as a trait object.
///
/// Split out so the ordering it implements — submit, then record the echo only on
/// success — can be tested against a writer that always refuses and one that always
/// accepts, deterministically and without racing the real writer thread's draining
/// speed (F-58: a test that cannot reliably reach the refusal branch proves nothing
/// about it).
fn apply_via(
    writer: &dyn IndexWriter,
    source: &dyn NoteSource,
    parser: &dyn NoteParser,
    echo: &dyn EchoGuard,
    path: &VaultPath,
) -> Result<(), IndexError> {
    let (op, hash) = match source.load(path) {
        // A file the save path was told about but cannot read has been deleted.
        // Nothing was indexed, so there is nothing to record an echo for.
        Err(_) => (IndexOp::Remove { path: path.clone() }, None),
        Ok(loaded) => {
            let hash = loaded.meta.hash;
            match build_op(parser, path, loaded) {
                Some((op, _)) => (op, Some(hash)),
                // Nothing to index: the previous row stays, which is better than
                // deleting a note because one save of it did not parse. Nothing was
                // submitted, so again nothing to record.
                None => return Ok(()),
            }
        }
    };

    // submit, not submit_blocking. On the save path a full queue means the writer is
    // behind, and waiting here is precisely what design goal 2 forbids — the refusal
    // is `?`'d straight back to the caller before anything is recorded below.
    writer.submit(op)?;

    // Late recording (F-224): only once the submit above has actually been accepted.
    // See the doc comment on `IndexHandle::apply` for why "after" rather than "before"
    // is the whole fix.
    if let Some(hash) = hash {
        echo.record_echo(hash);
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use serat_core::{ContentHash, NoteMeta, ParseError, ParsedNote, SourceEntry, VaultError};
    use std::sync::Mutex;

    /// A source with exactly one file, whose hash is fixed so a test can assert on it.
    struct OneFile {
        hash: ContentHash,
    }

    impl NoteSource for OneFile {
        fn entries(&self) -> Result<Vec<SourceEntry>, VaultError> {
            unimplemented!("apply_via never enumerates; only load")
        }

        fn load(&self, path: &VaultPath) -> Result<LoadedNote, VaultError> {
            Ok(LoadedNote {
                meta: NoteMeta {
                    path: path.clone(),
                    hash: self.hash,
                    kind: NoteKind::Note,
                    size: 0,
                    mtime_ms: 0,
                },
                bytes: b"# Note\n".to_vec(),
            })
        }
    }

    /// Parses nothing; just proves `apply_via` reached the submit step.
    struct TrivialParser;

    impl NoteParser for TrivialParser {
        fn parse(&self, _path: &VaultPath, _bytes: &[u8]) -> Result<ParsedNote, ParseError> {
            Ok(ParsedNote::default())
        }
    }

    /// Records every hash it is told about, so a test can assert exactly what was
    /// recorded and in what order — including "nothing".
    #[derive(Default)]
    struct SpyEcho {
        recorded: Mutex<Vec<ContentHash>>,
    }

    impl EchoGuard for SpyEcho {
        fn record_echo(&self, hash: ContentHash) {
            self.recorded.lock().expect("lock").push(hash);
        }
    }

    /// A writer that refuses every submission, standing in for "the writer is behind"
    /// without racing a real writer thread's draining speed.
    struct AlwaysFull;

    impl IndexWriter for AlwaysFull {
        fn submit(&self, _op: IndexOp) -> Result<(), IndexError> {
            Err(IndexError::QueueFull { depth: 0 })
        }

        fn flush(&self) -> Result<(), IndexError> {
            Ok(())
        }
    }

    /// A writer that accepts everything, standing in for the ordinary case.
    struct AlwaysAccepts;

    impl IndexWriter for AlwaysAccepts {
        fn submit(&self, _op: IndexOp) -> Result<(), IndexError> {
            Ok(())
        }

        fn flush(&self) -> Result<(), IndexError> {
            Ok(())
        }
    }

    /// Accepts everything, and — the point of it — looks at `echo` from *inside*
    /// `submit`, so the ordering between the two calls is observable rather than
    /// merely asserted (F-243: the earlier version of this test only checked the hash
    /// ended up recorded, which is true whether recording happens before or after
    /// submit, and passed unchanged under a mutation that swapped the order).
    struct OrderingWriter<'a> {
        echo: &'a SpyEcho,
        already_recorded_at_submit_time: Mutex<bool>,
    }

    impl<'a> OrderingWriter<'a> {
        fn new(echo: &'a SpyEcho) -> Self {
            Self {
                echo,
                already_recorded_at_submit_time: Mutex::new(false),
            }
        }
    }

    impl IndexWriter for OrderingWriter<'_> {
        fn submit(&self, _op: IndexOp) -> Result<(), IndexError> {
            let already = !self.echo.recorded.lock().expect("lock").is_empty();
            *self.already_recorded_at_submit_time.lock().expect("lock") = already;
            Ok(())
        }

        fn flush(&self) -> Result<(), IndexError> {
            Ok(())
        }
    }

    fn path(text: &str) -> VaultPath {
        VaultPath::new(text).expect("path")
    }

    #[test]
    fn a_refused_submit_records_no_echo() {
        // F-196 / F-219 / F-222, closed by F-224's late recording rather than narrowed.
        // The old design recorded the hash before the write, so a submit refused here
        // would have left an entry that permanently suppressed the only event that
        // would ever have indexed this note. Under late recording, a refusal must
        // leave the echo suppressor untouched: the setup below genuinely reaches the
        // refusal branch every time, deterministically (F-58), rather than racing a
        // real writer's draining speed as `IndexHandle::with_queue_depth(dir, 1)` does
        // in the integration tests.
        let hash = ContentHash::from_bytes([9u8; 32]);
        let echo = SpyEcho::default();

        let result = apply_via(
            &AlwaysFull,
            &OneFile { hash },
            &TrivialParser,
            &echo,
            &path("note.md"),
        );

        assert!(
            matches!(result, Err(IndexError::QueueFull { .. })),
            "the refusal was not propagated to the caller: {result:?}"
        );
        assert!(
            echo.recorded.lock().expect("lock").is_empty(),
            "a refused submit recorded an echo; the watcher event that is now the only \
             way this save reaches the index would be suppressed for nothing"
        );
    }

    #[test]
    fn a_successful_submit_records_the_echo_after_not_before() {
        // The mirror case: once the submit is accepted, the echo must be recorded so
        // the watcher drops its own event for the write that already indexed it —
        // otherwise every save is indexed twice, once here and once by the watcher.
        //
        // F-243: asserting only that the hash ends up recorded does not test the
        // ordering the name promises — that is true whether `record_echo` runs before
        // or after `submit`. `OrderingWriter` observes the echo set from inside
        // `submit` itself, so a mutation that swapped the two calls would make this
        // fail rather than pass unchanged.
        let hash = ContentHash::from_bytes([9u8; 32]);
        let echo = SpyEcho::default();
        let writer = OrderingWriter::new(&echo);

        apply_via(
            &writer,
            &OneFile { hash },
            &TrivialParser,
            &echo,
            &path("note.md"),
        )
        .expect("submit");

        assert!(
            !*writer.already_recorded_at_submit_time.lock().expect("lock"),
            "the echo was recorded before submit ran, not after (F-243)"
        );
        assert_eq!(
            *echo.recorded.lock().expect("lock"),
            vec![hash],
            "a successful submit did not record exactly the hash it indexed"
        );
    }

    #[test]
    fn a_deleted_file_records_no_echo_either() {
        // A Remove op is not a write this vault produced bytes for, so there is no
        // hash to suppress an event for — and none should be invented.
        struct Gone;
        impl NoteSource for Gone {
            fn entries(&self) -> Result<Vec<SourceEntry>, VaultError> {
                unimplemented!()
            }
            fn load(&self, path: &VaultPath) -> Result<LoadedNote, VaultError> {
                Err(VaultError::NotFound {
                    path: path.as_str().to_owned(),
                })
            }
        }

        let echo = SpyEcho::default();
        apply_via(
            &AlwaysAccepts,
            &Gone,
            &TrivialParser,
            &echo,
            &path("gone.md"),
        )
        .expect("remove");

        assert!(
            echo.recorded.lock().expect("lock").is_empty(),
            "a removal recorded an echo, which has no bytes to be one"
        );
    }
}
