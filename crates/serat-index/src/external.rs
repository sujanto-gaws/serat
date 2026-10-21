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
//! [`CatchUp::after_unclean_shutdown`] turns on the mtime comparison. The reason, and
//! how the marker works, is on [`IndexHandle::opened_after_unclean_shutdown`] and in
//! `schema::begin_session`; this module only does the comparing.
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

/// What a comparison of the vault with the index may look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatchUp {
    /// Paths only: a path on disk that the index lacks is indexed, a path the index
    /// holds that the disk lacks is removed. Enough after a clean shutdown, when the
    /// watcher was the only thing that could have missed a change and a path it saw
    /// is a path the walk sees.
    PathsOnly,
    /// Paths, and for a path in both, `mtime` against `notes.mtime`; a path whose
    /// mtime differs goes through the hash gate and is re-indexed if its content
    /// differs. One stat per file the walk already made, and a hash only where the
    /// mtime moved. Required after an unclean shutdown (a lost index commit for a
    /// path the index already holds) and after any dropped watcher event (the drop
    /// may have been a modification).
    PathsAndMtime,
}

impl CatchUp {
    /// The mode for an open: [`IndexHandle::opened_after_unclean_shutdown`].
    #[must_use]
    pub fn after_unclean_shutdown(unclean: bool) -> Self {
        if unclean {
            Self::PathsAndMtime
        } else {
            Self::PathsOnly
        }
    }
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
    /// Files the parser refused. The earlier row, if any, stays (`build_op`).
    pub unparsed: usize,
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
        self.unparsed += other.unparsed;
    }
}

/// One row of `notes`, as far as a comparison needs it.
#[derive(Debug, Clone)]
struct Row {
    hash: String,
    mtime: i64,
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
    /// the index holds. `is_echo` is asked first with the hash, and should be
    /// `|hash| vault.echo().take_echo(hash)`: it consumes the entry, so it is asked
    /// once per file, and a `true` answer skips the file because the save path already
    /// submitted exactly these bytes.
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
        is_echo: &dyn Fn(ContentHash) -> bool,
        paths: &[VaultPath],
    ) -> Result<Outcome, IndexError> {
        let mut outcome = Outcome::default();
        if paths.is_empty() {
            return Ok(outcome);
        }
        let reader = self.reader()?;
        for path in paths {
            let known = row(&reader, path)?;
            match self.submit(source, parser, path, known.as_ref(), Some(is_echo))? {
                Step::Gone { .. } => {
                    // The path itself, and everything beneath it: a directory that
                    // was deleted or renamed away arrives here as the path of the
                    // directory, which the index knows only by its children.
                    let mut gone = descendants(&reader, path.as_str())?;
                    if known.is_some() {
                        gone.push(path.clone());
                    }
                    for victim in gone {
                        self.writer.submit_blocking(IndexOp::Remove { path: victim })?;
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
        let outcome = self.compare(source, parser, entries, rows, CatchUp::PathsAndMtime)?;
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
    ) -> Result<Outcome, IndexError> {
        let reader = self.reader()?;
        let rows = rows_under(&reader, None)?;
        let outcome = self.compare(source, parser, entries, rows, mode)?;
        self.finish(outcome)
    }

    fn finish(&self, outcome: Outcome) -> Result<Outcome, IndexError> {
        // One barrier per batch, not per file: the caller learns "committed" once.
        if outcome.changed() {
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
    ) -> Result<Outcome, IndexError> {
        let mut outcome = Outcome::default();
        for entry in entries {
            let known = rows.remove(entry.path.as_str());
            let candidate = match (&known, mode) {
                (None, _) => true,
                (Some(row), CatchUp::PathsAndMtime) => {
                    row.mtime != entry.mtime_ms || row.kind != entry.kind
                }
                (Some(_), CatchUp::PathsOnly) => false,
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
        is_echo: Option<&dyn Fn(ContentHash) -> bool>,
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
        if is_echo.is_some_and(|is_echo| is_echo(hash))
            || known.is_some_and(|row| row.hash == hash.to_hex() && row.kind == loaded.meta.kind)
        {
            outcome.unchanged = 1;
            return Ok(Step::Done(outcome));
        }
        match build_op(parser, path, loaded) {
            Some((op, _)) => {
                self.writer.submit_blocking(op)?;
                outcome.upserted = 1;
            }
            None => outcome.unparsed = 1,
        }
        Ok(Step::Done(outcome))
    }
}

enum Step {
    /// The file could not be read. `not_found` distinguishes a vanished file from one
    /// that exists and cannot be read (permissions, a symlink, a directory).
    Gone { not_found: bool },
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
        .prepare_cached("SELECT hash, mtime, kind FROM notes WHERE path = ?1")
        .map_err(sqlite)?;
    let mut rows = statement.query(params![path.as_str()]).map_err(sqlite)?;
    match rows.next().map_err(sqlite)? {
        Some(found) => Ok(Some(Row {
            hash: found.get(0).map_err(sqlite)?,
            mtime: found.get(1).map_err(sqlite)?,
            kind: kind_of(&found.get::<_, String>(2).map_err(sqlite)?),
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
            },
        ))
    };
    let collected = match dir {
        Some(dir) => {
            let mut statement = conn
                .prepare(
                    "SELECT path, hash, mtime, kind FROM notes WHERE path >= ?1 AND path < ?2",
                )
                .map_err(sqlite)?;
            statement
                .query_map(params![format!("{dir}/"), format!("{dir}0")], map)
                .map_err(sqlite)?
                .collect::<Result<HashMap<_, _>, _>>()
        }
        None => {
            let mut statement = conn
                .prepare("SELECT path, hash, mtime, kind FROM notes")
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
