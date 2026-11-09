//! Indexing what changed behind Serat's back (FR-VLT-04, NFR-REL-01, NFR-REL-02).
//!
//! Three entry points, one rule. Each takes the *current state of the disk* (a path
//! list the watcher reported, a directory's walk, the whole walk) and brings the index
//! to agree with it. None of them is told what kind of change it was, because by the
//! time it looks the answer may be different (see `serat_vault::watch`).
//!
//! | Entry point | Fed by | Scope |
//! |---|---|---|
//! | [`Reconciler::index_paths`] | the coalesced work queue (architecture §3.1.2) | these paths |
//! | [`Reconciler::reconcile_dir`] | `VaultEvent::DirChanged` | one directory subtree |
//! | [`Reconciler::catch_up`] | open (ADR-0015 §2), `RescanRequired`, an ignore-file change | the vault |
//!
//! # The hash gate
//!
//! A file is parsed and submitted only if its content hash differs from `notes.hash`
//! (§3.1.2 step 3). Sync tools touch mtimes constantly without changing bytes, and the
//! gate discards those for the price of a read and a hash. [`index_paths`] also asks
//! the caller's echo test first, so a write Serat made itself and has already
//! submitted is not parsed a second time.
//!
//! # Which side of ADR-0011 each is on
//!
//! All three run on a consumer's own thread, never on the watcher's coalescer and never
//! on the save path, so every submit is `submit_blocking`: a full writer queue makes
//! the consumer wait, and nothing is dropped. The `IndexWriter::submit` that refuses
//! stays what the save path uses.
//!
//! # Unclean shutdown
//!
//! The catch-up always compares mtimes ([`CatchUp`]); the clean-close marker
//! ([`IndexHandle::opened_after_unclean_shutdown`]) no longer selects a cheaper mode.
//! A file whose mtime moved but whose bytes did not has the new mtime written back to
//! its row (`IndexOp::TouchMtime`, bulk side, through the one writer), so it is hashed
//! once, not at every open.
//!
//! [`index_paths`]: Reconciler::index_paths

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rusqlite::{Connection, params};
use serat_core::{
    ContentHash, IndexError, IndexOp, IndexWriter, NoteKind, NoteParser, NoteSource, SourceEntry,
    VaultError, VaultPath,
};

use crate::incremental::{IndexHandle, build_op};
use crate::schema::{self, sqlite};
use crate::writer::WriterHandle;

/// Whether a (path, hash) is the echo of our own save; see `EchoSuppressor::take_echo`.
type EchoTest<'a> = &'a dyn Fn(&VaultPath, ContentHash) -> bool;

/// What a comparison of the vault with the index looks at.
///
/// One mode. Every open-time catch-up compares paths and, for a path in both, `mtime`
/// against `notes.mtime`, `size` against `notes.size`, and re-hashes any file whose
/// disk mtime is within 2 s of `index_meta.caught_up_from_ms` (ADR-0015 §2 as amended;
/// the residual is a same-size rewrite outside that window that kept its mtime): the clean-close marker was a
/// heuristic for "the index committed everything", and it cannot see a file edited
/// while Serat was closed. The enum stays, with its one variant, so the call shape
/// `catch_up(.., CatchUp::PathsAndMtime)` that callers already use is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatchUp {
    /// Paths, and for a path in both, `mtime` against `notes.mtime`; a path whose
    /// mtime differs goes through the hash gate and is re-indexed if its content
    /// differs, or has its mtime refreshed if it does not. One stat per file the walk
    /// already made, and a hash only where the mtime moved.
    PathsAndMtime,
}

/// What one reconciliation did. Counted from what was submitted, so it says what the
/// writer was asked to do; [`Reconciler`] flushes before returning, so by then it is
/// also what the index holds.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// Notes and assets parsed (notes only) and submitted.
    pub upserted: usize,
    /// Paths removed from the index.
    pub removed: usize,
    /// Files read and hashed whose content the index already held.
    pub unchanged: usize,
    /// Of those unchanged, how many had a moved mtime that was written back to the row
    /// (`IndexOp::TouchMtime`) so the next comparison does not hash them again.
    pub touched: usize,
    /// Of those upserted, how many the parser refused and are now bare rows (`build_op`).
    pub unparsed: usize,
    /// Files that exist and could not be read (permissions, a sharing violation); their
    /// rows were kept.
    pub unreadable: usize,
}

impl Outcome {
    /// Whether the index changed, which is when `index://changed` is owed.
    #[must_use]
    pub fn changed(&self) -> bool {
        self.upserted + self.removed > 0
    }

    /// Fold another outcome into this one.
    pub fn absorb(&mut self, other: Self) {
        self.upserted += other.upserted;
        self.removed += other.removed;
        self.unchanged += other.unchanged;
        self.touched += other.touched;
        self.unparsed += other.unparsed;
        self.unreadable += other.unreadable;
    }
}

/// How far before the start of the last completed catch-up a file's disk mtime may be
/// and still make it a candidate: the coarsest mtime granularity in the field (FAT and
/// exFAT, 2 s), ADR-0015 section 2 as amended.
const WINDOW_MS: i64 = 2000;

/// `index_meta.caught_up_from_ms`: when the last completed catch-up (or rebuild) began.
fn caught_up_from_ms(conn: &Connection) -> Result<i64, IndexError> {
    conn.query_row(
        "SELECT caught_up_from_ms FROM index_meta WHERE id = 1",
        [],
        |row| row.get(0),
    )
    .map_err(sqlite)
}

/// One row of `notes`, as far as a comparison needs it.
#[derive(Debug, Clone)]
struct Row {
    hash: String,
    mtime: i64,
    size: i64,
    kind: NoteKind,
}

/// Brings the index into agreement with the disk. Cheap to build and to clone: it holds
/// a writer handle and a path, and opens a short-lived read connection per call
/// (ADR-0001: reads never go through the writer).
#[derive(Debug, Clone)]
pub struct Reconciler {
    writer: WriterHandle,
    db: PathBuf,
}

impl Reconciler {
    /// A reconciler for an open index.
    #[must_use]
    pub fn new(handle: &IndexHandle) -> Self {
        Self {
            writer: handle.writer(),
            db: crate::index_path(handle.index_dir()),
        }
    }

    fn reader(&self) -> Result<Connection, IndexError> {
        let conn = schema::open(&self.db)?;
        conn.pragma_update(None, "query_only", true)
            .map_err(sqlite)?;
        Ok(conn)
    }

    /// Index the paths the watcher reported, in one batch.
    ///
    /// Each path is read as it is *now*: gone means remove it (and anything the index
    /// holds beneath it, which is how a deleted or renamed directory leaves);
    /// present means hash it, and parse and submit it only if it is not already what
    /// the index holds. `is_echo` is asked first with the path and the hash, and should
    /// be `|path, hash| vault.echo().take_echo(path, hash)`: it consumes the entry, so
    /// it is asked once per file, and a `true` answer skips the file because the save
    /// path already submitted exactly these bytes at exactly this path.
    ///
    /// Call it with whatever the queue has accumulated (`WorkQueue::drain`) rather
    /// than once per path. It ends in one `flush`, so when it returns the changes are
    /// committed and the caller may tell the front end (`index://changed`).
    ///
    /// # Errors
    ///
    /// [`IndexError::WriterStopped`] or SQLite's, in which case the batch is partly
    /// submitted and the caller should treat the index as needing a catch-up.
    pub fn index_paths(
        &self,
        source: &dyn NoteSource,
        parser: &dyn NoteParser,
        is_echo: EchoTest<'_>,
        paths: &[VaultPath],
    ) -> Result<Outcome, IndexError> {
        let mut outcome = Outcome::default();
        if paths.is_empty() {
            return Ok(outcome);
        }
        let reader = self.reader()?;
        let mut flushed = 0;
        for path in paths {
            self.pace(&outcome, &mut flushed)?;
            let known = row(&reader, path)?;
            match self.submit(source, parser, path, known.as_ref(), Some(is_echo))? {
                Step::Gone { not_found: false } => {
                    // It exists and cannot be read now (permissions, a sharing
                    // violation, a symlink, a directory in its place). That is not a
                    // delete: the row stays, and the next walk decides.
                    outcome.unreadable += 1;
                }
                Step::Gone { not_found: true } => {
                    // The path itself, and everything beneath it: a directory that
                    // was deleted or renamed away arrives here as the path of the
                    // directory, which the index knows only by its children.
                    let mut gone = descendants(&reader, path.as_str())?;
                    if known.is_some() {
                        gone.push(path.clone());
                    }
                    for victim in gone {
                        self.writer
                            .submit_blocking(IndexOp::Remove { path: victim })?;
                        outcome.removed += 1;
                    }
                }
                Step::Done(step) => outcome.absorb(step),
            }
        }
        self.finish(outcome)
    }

    /// Reconcile one directory subtree with `entries`, which are what
    /// `Vault::walk_dir` found there (nothing, if it is gone).
    ///
    /// Always compares mtimes: a directory event is the platform telling us it does
    /// not know what changed below.
    ///
    /// # Errors
    ///
    /// As [`Reconciler::index_paths`].
    pub fn reconcile_dir(
        &self,
        source: &dyn NoteSource,
        parser: &dyn NoteParser,
        dir: &VaultPath,
        entries: &[SourceEntry],
    ) -> Result<Outcome, IndexError> {
        let reader = self.reader()?;
        let rows = rows_under(&reader, Some(dir.as_str()))?;
        let mut scan = Scan::default();
        let outcome = self.compare(
            source,
            parser,
            entries,
            rows,
            CatchUp::PathsAndMtime,
            &mut scan,
        )?;
        self.finish(outcome)
    }

    /// Reconcile the whole vault with `entries`, the authoritative walk (ADR-0015 §2):
    /// every path found that the index lacks is submitted, every path the index holds
    /// that the walk did not find is removed, and with [`CatchUp::PathsAndMtime`] a
    /// path in both whose mtime moved goes through the hash gate.
    ///
    /// Everything it submits is on ADR-0011's bulk side. Run it on a background thread
    /// after the window is usable (architecture §3.4, "after usable").
    ///
    /// # Errors
    ///
    /// As [`Reconciler::index_paths`].
    pub fn catch_up(
        &self,
        source: &dyn NoteSource,
        parser: &dyn NoteParser,
        entries: &[SourceEntry],
        mode: CatchUp,
        started_at_ms: Option<i64>,
    ) -> Result<Outcome, IndexError> {
        let reader = self.reader()?;
        let rows = rows_under(&reader, None)?;
        let stored = caught_up_from_ms(&reader)?;
        // A stored value ahead of the clock (the clock stepped back) would shrink the
        // window to nothing: treat it as unset, which makes every file a candidate.
        let stored = if stored > crate::rebuild::now_ms() {
            0
        } else {
            stored
        };
        let window = stored.saturating_sub(WINDOW_MS);
        let mut scan = Scan {
            window: Some(window),
            unread_from: None,
        };
        let outcome = self.compare(source, parser, entries, rows, mode, &mut scan)?;
        let outcome = self.finish(outcome)?;
        // Only here, when every step above succeeded: a catch-up that died part way
        // leaves the window where it was, so the next one looks at the same files again.
        // `None` is a comparison that is not the open's (a rescan): it never writes.
        // A file that was a candidate and could not be read holds the window back to its
        // own mtime, so the next catch-up compares it again; it is the minimum, not a
        // refusal to write, so one permanently unreadable file does not freeze the window.
        if let Some(started) = started_at_ms {
            self.writer
                .set_caught_up_from(scan.unread_from.map_or(started, |m| m.min(started).max(0)))?;
        }
        Ok(outcome)
    }

    /// Keep at most one batch of this caller's work outstanding in the writer's channel.
    ///
    /// A large catch-up submitting freely keeps the 4,096-deep channel full, and the
    /// save path's non-blocking `submit` is then refused and falls back to the watcher
    /// (ADR-0011 item 4: bulk work must not starve the interactive producer). One flush
    /// per `BATCH_SIZE` submits leaves the rest of the depth to saves, at the cost of
    /// a round trip per batch.
    fn pace(&self, outcome: &Outcome, flushed: &mut usize) -> Result<(), IndexError> {
        let submitted = outcome.upserted + outcome.removed + outcome.touched;
        if submitted >= *flushed + crate::writer::BATCH_SIZE {
            self.writer.flush()?;
            *flushed = submitted;
        }
        Ok(())
    }

    fn finish(&self, outcome: Outcome) -> Result<Outcome, IndexError> {
        // One barrier per batch, not per file: the caller learns "committed" once.
        // Touches are not a change to announce, but they are queued work: the
        // contract is that everything submitted is committed on return.
        if outcome.changed() || outcome.touched > 0 {
            self.writer.flush()?;
        }
        Ok(outcome)
    }

    fn compare(
        &self,
        source: &dyn NoteSource,
        parser: &dyn NoteParser,
        entries: &[SourceEntry],
        mut rows: HashMap<String, Row>,
        mode: CatchUp,
        scan: &mut Scan,
    ) -> Result<Outcome, IndexError> {
        let mut outcome = Outcome::default();
        let mut flushed = 0;
        for entry in entries {
            self.pace(&outcome, &mut flushed)?;
            let known = rows.remove(entry.path.as_str());
            let CatchUp::PathsAndMtime = mode;
            let candidate = match &known {
                None => true,
                Some(row) => {
                    row.mtime != entry.mtime_ms
                        || row.size != i64::try_from(entry.size).unwrap_or(i64::MAX)
                        || row.kind != entry.kind
                        || scan.window.is_some_and(|from| entry.mtime_ms >= from)
                }
            };
            if !candidate {
                continue;
            }
            match self.submit(source, parser, &entry.path, known.as_ref(), None)? {
                Step::Gone { not_found } => {
                    // Vanished between the walk and the read. A file the walk found
                    // and that exists but cannot be read keeps its row: the walk is
                    // the authority on existence, and a permission error is not a
                    // delete.
                    if not_found && known.is_some() {
                        self.writer.submit_blocking(IndexOp::Remove {
                            path: entry.path.clone(),
                        })?;
                        outcome.removed += 1;
                    } else if !not_found {
                        outcome.unreadable += 1;
                        scan.unread_from = Some(
                            scan.unread_from
                                .map_or(entry.mtime_ms, |m| m.min(entry.mtime_ms)),
                        );
                    }
                }
                Step::Done(step) => outcome.absorb(step),
            }
        }
        // What is left is in the index and not on disk.
        let mut leftover: Vec<String> = rows.into_keys().collect();
        leftover.sort();
        for path in leftover {
            if let Ok(path) = VaultPath::new(&path) {
                self.writer.submit_blocking(IndexOp::Remove { path })?;
                outcome.removed += 1;
            }
        }
        Ok(outcome)
    }

    /// Read one path and submit it if it is not already what the index holds.
    fn submit(
        &self,
        source: &dyn NoteSource,
        parser: &dyn NoteParser,
        path: &VaultPath,
        known: Option<&Row>,
        is_echo: Option<EchoTest<'_>>,
    ) -> Result<Step, IndexError> {
        let loaded = match source.load(path) {
            Ok(loaded) => loaded,
            Err(error) => {
                return Ok(Step::Gone {
                    not_found: matches!(error, VaultError::NotFound { .. }),
                });
            }
        };
        let hash = loaded.meta.hash;
        let mut outcome = Outcome::default();
        // The echo is keyed by (path, hash): a copy of a just-saved note at another
        // path carries the same hash and does not match.
        if is_echo.is_some_and(|is_echo| is_echo(path, hash)) {
            outcome.unchanged = 1;
            return Ok(Step::Done(outcome));
        }
        if let Some(row) =
            known.filter(|row| row.hash == hash.to_hex() && row.kind == loaded.meta.kind)
        {
            outcome.unchanged = 1;
            if row.mtime != loaded.meta.mtime_ms {
                self.writer.submit_blocking(IndexOp::TouchMtime {
                    path: path.clone(),
                    hash,
                    mtime_ms: loaded.meta.mtime_ms,
                })?;
                outcome.touched = 1;
            }
            return Ok(Step::Done(outcome));
        }
        let (op, _, refused) = build_op(parser, path, loaded);
        self.writer.submit_blocking(op)?;
        outcome.upserted = 1;
        outcome.unparsed = usize::from(refused);
        Ok(Step::Done(outcome))
    }
}

/// What one comparison is told and what it reports back besides the [`Outcome`].
#[derive(Default)]
struct Scan {
    /// Disk mtimes at or after this make a path a candidate (`None`: no window).
    window: Option<i64>,
    /// The smallest disk mtime of a candidate that exists and could not be read.
    unread_from: Option<i64>,
}

enum Step {
    /// The file could not be read. `not_found` distinguishes a vanished file from one
    /// that exists and cannot be read (permissions, a symlink, a directory).
    Gone {
        not_found: bool,
    },
    Done(Outcome),
}

fn kind_of(text: &str) -> NoteKind {
    if text == "asset" {
        NoteKind::Asset
    } else {
        NoteKind::Note
    }
}

fn row(conn: &Connection, path: &VaultPath) -> Result<Option<Row>, IndexError> {
    let mut statement = conn
        .prepare_cached("SELECT hash, mtime, kind, size FROM notes WHERE path = ?1")
        .map_err(sqlite)?;
    let mut rows = statement.query(params![path.as_str()]).map_err(sqlite)?;
    match rows.next().map_err(sqlite)? {
        Some(found) => Ok(Some(Row {
            hash: found.get(0).map_err(sqlite)?,
            mtime: found.get(1).map_err(sqlite)?,
            kind: kind_of(&found.get::<_, String>(2).map_err(sqlite)?),
            size: found.get(3).map_err(sqlite)?,
        })),
        None => Ok(None),
    }
}

/// Every row at or below `dir`, or every row when `dir` is `None`.
///
/// The directory is a range over the unique index on `path`: `dir + "/"` up to
/// `dir + "0"`, since `'0'` is the character after `'/'`. Not `LIKE`, which would
/// treat `_` and `%` in a folder name as wildcards.
fn rows_under(conn: &Connection, dir: Option<&str>) -> Result<HashMap<String, Row>, IndexError> {
    let map = |found: &rusqlite::Row<'_>| {
        Ok((
            found.get::<_, String>(0)?,
            Row {
                hash: found.get(1)?,
                mtime: found.get(2)?,
                kind: kind_of(&found.get::<_, String>(3)?),
                size: found.get(4)?,
            },
        ))
    };
    let collected = match dir {
        Some(dir) => {
            let mut statement = conn
                .prepare("SELECT path, hash, mtime, kind, size FROM notes WHERE path >= ?1 AND path < ?2")
                .map_err(sqlite)?;
            statement
                .query_map(params![format!("{dir}/"), format!("{dir}0")], map)
                .map_err(sqlite)?
                .collect::<Result<HashMap<_, _>, _>>()
        }
        None => {
            let mut statement = conn
                .prepare("SELECT path, hash, mtime, kind, size FROM notes")
                .map_err(sqlite)?;
            statement
                .query_map([], map)
                .map_err(sqlite)?
                .collect::<Result<HashMap<_, _>, _>>()
        }
    };
    collected.map_err(sqlite)
}

fn descendants(conn: &Connection, dir: &str) -> Result<Vec<VaultPath>, IndexError> {
    let mut statement = conn
        .prepare_cached("SELECT path FROM notes WHERE path >= ?1 AND path < ?2")
        .map_err(sqlite)?;
    let found = statement
        .query_map(params![format!("{dir}/"), format!("{dir}0")], |found| {
            found.get::<_, String>(0)
        })
        .map_err(sqlite)?;
    let mut out = Vec::new();
    for path in found {
        if let Ok(path) = VaultPath::new(&path.map_err(sqlite)?) {
            out.push(path);
        }
    }
    Ok(out)
}

/// The throttle `index://changed` needs (architecture §3.1 step 6): at most one event
/// per interval, never a lost final one. **Leading edge and trailing edge**: the first
/// change after a quiet period emits at once, further changes inside the interval
/// collapse into one emission when it ends.
///
/// Pure state, no clock and no thread: the owner passes `now` and decides when to
/// look. The owner calls [`ChangeSignal::changed`] after every batch that changed the
/// index (the save path's too, not only [`Outcome::changed`]) and
/// [`ChangeSignal::due`] on its tick, sleeping no longer than
/// [`ChangeSignal::next_due`]. The `generation` is the event's payload
/// (`IndexChanged.generation`: monotonic per session, so a response computed against
/// an older one is stale).
#[derive(Debug, Clone)]
pub struct ChangeSignal {
    interval: Duration,
    generation: u64,
    emitted: u64,
    last_emit: Option<Instant>,
}

/// Ten per second.
pub const CHANGE_INTERVAL: Duration = Duration::from_millis(100);

impl Default for ChangeSignal {
    fn default() -> Self {
        Self::new(CHANGE_INTERVAL)
    }
}

impl ChangeSignal {
    /// A throttle emitting at most once per `interval`.
    #[must_use]
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            generation: 0,
            emitted: 0,
            last_emit: None,
        }
    }

    /// The index changed. Returns the generation to emit now, or `None` when the
    /// emission is deferred to [`ChangeSignal::due`].
    pub fn changed(&mut self, now: Instant) -> Option<u64> {
        self.generation += 1;
        self.due(now)
    }

    /// Returns the generation to emit if a change is waiting and the interval has
    /// passed since the last emission.
    pub fn due(&mut self, now: Instant) -> Option<u64> {
        if self.emitted == self.generation {
            return None;
        }
        if self
            .last_emit
            .is_some_and(|last| now.duration_since(last) < self.interval)
        {
            return None;
        }
        self.emitted = self.generation;
        self.last_emit = Some(now);
        Some(self.generation)
    }

    /// When [`ChangeSignal::due`] next has something to say, if a change is waiting.
    #[must_use]
    pub fn next_due(&self) -> Option<Instant> {
        if self.emitted == self.generation {
            return None;
        }
        self.last_emit.map(|last| last + self.interval)
    }

    /// The current generation, for a late subscriber that polls instead of listening.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// The integration tests' stubs, shared (they use `serat_core` and `std::fs` only).
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod support;

#[cfg(test)]
mod tests {
    use super::support::{DiskVault, LineParser};
    use super::*;

    /// Task #66: a catch-up whose final flush fails writes nothing, so the window the
    /// next one uses is the one the last completed catch-up left.
    #[test]
    fn a_catch_up_whose_flush_fails_leaves_the_window_where_it_was() {
        let dir = tempfile::tempdir().expect("temp");
        let root = dir.path().join("vault");
        let index = dir.path().join("index");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(root.join("a.md"), "# A\n").expect("write");
        let source = DiskVault::new(&root);
        let parser = LineParser::default();
        crate::rebuild::rebuild(&source, &parser, &root, &index).expect("rebuild");
        let handle = IndexHandle::open(&index).expect("open");
        let before = handle.caught_up_from_ms().expect("value");
        assert!(before > 0);

        std::fs::write(root.join("a.md"), "# A, edited, longer\n").expect("edit");
        let entries = source.entries().expect("walk");
        handle.fault_injector().fail_every_commit();
        let failed = Reconciler::new(&handle).catch_up(
            &source,
            &parser,
            &entries,
            CatchUp::PathsAndMtime,
            Some(before + 60_000),
        );
        assert!(failed.is_err(), "the injected commit failure must surface");
        assert_eq!(handle.caught_up_from_ms().expect("value"), before);
        // The writer still holds the batch it could not commit; closing says so.
        let _ = handle.close();
    }
}
