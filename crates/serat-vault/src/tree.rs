//! The file tree's producer side: the breadth-first streaming walk and the session that
//! turns walks, index reads and watcher events into one ordered stream
//! (ADR-0015 §1–§2; FR-VLT-01, FR-VLT-04, NFR-CMP-03).
//!
//! Two parts, deliberately separate:
//!
//! * [`Vault::walk_tree`] is the enumeration. Each `read_dir` yields one directory's
//!   raw entries, root first. It shares [`Vault::walk`]'s ignore rules, symlink refusal
//!   (T-07) and lexical prefix strip (F-12), and reads only what the tree displays:
//!   a name and a kind, taken from the directory entry's file type, so a file costs no
//!   `stat` at all. `Vault::walk`, the sorted `Vec` the rebuild needs for rowid
//!   determinism, is unchanged.
//! * [`TreeSession`] owns the [`TreeModel`] and the sink (in the application, the
//!   `ipc::Channel`) behind **one lock**, so the order in which the model is mutated is
//!   the order in which the stream is written. That is what makes ADR-0015's two
//!   obligations hold on the wire and not only in the model.
//!
//! Which source populates: the index's rows if no walk listing has been sent yet,
//! otherwise the walk alone. [`TreeOut::Complete`] records which. The walk always runs
//! afterwards; where the index populated, its listings are catch-up and only those that
//! differ are sent.

use std::collections::VecDeque;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serat_core::{
    RawEntry, Snapshot, TreeKind, TreeModel, TreeOp, VaultError, VaultPath, group_listings,
};

use crate::refusal::Refusal;
use crate::walk::relativise_walked;
use crate::{Vault, ignore::is_note_file};

/// Which source produced the initial population.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeSource {
    /// One read of the `Ready` index.
    Index,
    /// The breadth-first streaming walk.
    Walk,
}

/// One message on the tree stream. Maps one-to-one onto `contracts/ipc.rs`'s
/// `TreeMessage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeOut {
    /// A listing, upsert or remove.
    Op(TreeOp),
    /// The initial population is complete.
    Complete {
        /// File entries delivered: notes and assets in the listed directories, before
        /// any sidecar fold, so it is comparable with the vault manifest.
        entries: u32,
        /// Which source populated the tree.
        source: TreeSource,
    },
    /// The authoritative walk has finished. Sent once.
    WalkSettled {
        /// Empty folders the walk added after `Complete`: folders with no file
        /// anywhere beneath them, which an index-sourced tree cannot know. Always 0
        /// when the walk itself populated the tree, because they arrived before
        /// `Complete`. serat-app reports the empty-folder count for ADR-0015 §3 from
        /// [`WalkSummary::empty_folders`] (returned by the walk), not from this.
        empty_folders: u32,
    },
}

/// A callback handed each directory's raw entries as the walk streams them.
pub type ListingTap<'a> = dyn FnMut(&str, &[RawEntry]) + 'a;

/// What a walk found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WalkSummary {
    /// Directories listed, including the one the walk started from.
    pub dirs: u32,
    /// Files the ignore rules admit.
    pub files: u32,
    /// Directories with no admitted file anywhere beneath them (the start excluded).
    pub empty_folders: u32,
}

impl Vault {
    /// Walk the vault breadth-first, handing each directory's entries to `on_dir` as
    /// soon as that directory has been read: `""` (the root) first, then each level
    /// in turn. `start` is a vault-relative directory, or `""` for the whole vault.
    ///
    /// Entries are unordered; ordering is [`TreeModel`]'s. A name this platform cannot
    /// represent as a [`VaultPath`] is recorded in [`Vault::refusals`] and left out. That
    /// includes a **directory** name, which is one entry for the directory and is not
    /// entered; [`Vault::walk`] does the same (#13 N1). When the walk finishes, the
    /// log is replaced for the subtree walked (all of it when `start` is the root), so
    /// entries leave at the next walk that covers them. A subdirectory that
    /// cannot be read is listed empty rather than left pending for ever (design goal
    /// 5).
    ///
    /// # Errors
    ///
    /// [`VaultError::Io`] if the root cannot be read. When `start` is not the root and
    /// has vanished (a rename racing the walk) nothing is listed and that is not an
    /// error: the event that renamed it is on its way to the stream.
    pub fn walk_tree(
        &self,
        start: &str,
        on_dir: impl FnMut(&str, Vec<RawEntry>),
    ) -> Result<WalkSummary, VaultError> {
        self.walk_tree_inner(start, false, on_dir)
    }

    /// [`Vault::walk_tree`] with each file entry carrying `size` and `mtime_ms`, for the
    /// warm path's catch-up (ADR-0015 section 2, *One walk at open*): convert each
    /// listing with `serat_core::source_entries`, accumulate, and call
    /// `Reconciler::catch_up` when the walk completes. The no-index path keeps using
    /// [`Vault::walk_tree`], which is name-and-kind only (FR-VLT-01's timed walk).
    ///
    /// # Errors
    ///
    /// As [`Vault::walk_tree`].
    pub fn walk_tree_with_metadata(
        &self,
        start: &str,
        on_dir: impl FnMut(&str, Vec<RawEntry>),
    ) -> Result<WalkSummary, VaultError> {
        self.walk_tree_inner(start, true, on_dir)
    }

    fn walk_tree_inner(
        &self,
        start: &str,
        metadata: bool,
        mut on_dir: impl FnMut(&str, Vec<RawEntry>),
    ) -> Result<WalkSummary, VaultError> {
        let start_path = if start.is_empty() {
            self.root().to_path_buf()
        } else {
            self.resolve(&VaultPath::new(start)?)?
        };
        if !start.is_empty() && self.ignore_rules().is_ignored_dir_chain(start) {
            self.refusals().replace_scope(start, Vec::new());
            return Ok(WalkSummary::default());
        }

        // (parent index, whether a file sits at or below this directory), in the
        // order directories were read, so a reverse pass settles every ancestor.
        let mut nodes: Vec<(usize, bool)> = Vec::new();
        let mut queue: VecDeque<(PathBuf, String, usize)> = VecDeque::new();
        queue.push_back((start_path, start.to_owned(), usize::MAX));
        let mut summary = WalkSummary::default();
        let user = self.ignore_rules().user_patterns();
        let mut refused: Vec<Refusal> = Vec::new();

        while let Some((directory, relative, parent)) = queue.pop_front() {
            let listing: Vec<fs::DirEntry> = match fs::read_dir(&directory) {
                Ok(listing) => listing.filter_map(Result::ok).collect(),
                Err(err) if parent == usize::MAX => {
                    if start.is_empty() {
                        return Err(VaultError::io(crate::display(&directory), &err));
                    }
                    self.refusals().replace_scope(start, Vec::new());
                    return Ok(summary);
                }
                Err(_) => Vec::new(),
            };
            let index = nodes.len();
            nodes.push((parent, false));
            let mut entries = Vec::new();
            let mut has_file = false;

            for entry in listing {
                // File type from the directory entry: no syscall on Linux or Windows,
                // and it does not follow symlinks on either.
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_symlink() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_dir = file_type.is_dir();
                if is_dir && self.ignore_rules().is_ignored_dir(&name) {
                    continue;
                }
                if !is_dir && self.ignore_rules().is_ignored_file(&name) {
                    continue;
                }
                if !user.is_empty() {
                    let candidate = if relative.is_empty() {
                        name.clone()
                    } else {
                        format!("{relative}/{name}")
                    };
                    if user.is_ignored(&candidate, is_dir) {
                        continue;
                    }
                }
                let path = entry.path();
                let child = match relativise_walked(self, &path) {
                    Ok(child) => child,
                    Err(VaultError::InvalidPath { path, reason }) => {
                        refused.push(Refusal { path, reason });
                        continue;
                    }
                    Err(_) => continue,
                };
                if is_dir {
                    queue.push_back((path, child.as_str().to_owned(), index));
                    entries.push(RawEntry::new(name, TreeKind::Dir));
                } else {
                    has_file = true;
                    summary.files += 1;
                    let kind = if is_note_file(&name) {
                        TreeKind::Note
                    } else {
                        TreeKind::Asset
                    };
                    let raw = RawEntry::new(name, kind);
                    // One statx per file on Linux and macOS, free on Windows; read only
                    // when the caller will compare it (the warm path's catch-up).
                    // Not even a stat on the plain walk (FR-VLT-01, NFR-PRF-01).
                    entries.push(if metadata {
                        match entry.metadata() {
                            Ok(stat) => raw.with_metadata(stat.len(), crate::walk::mtime_ms(&stat)),
                            Err(_) => raw,
                        }
                    } else {
                        raw
                    });
                }
            }
            nodes[index].1 = has_file;
            summary.dirs += 1;
            on_dir(&relative, entries);
        }
        self.refusals().replace_scope(start, refused);

        for index in (1..nodes.len()).rev() {
            let (parent, has_file) = nodes[index];
            if has_file {
                nodes[parent].1 = true;
            } else {
                summary.empty_folders += 1;
            }
        }
        Ok(summary)
    }
}

/// What the session has delivered, for a caller validating a front-end mark against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeProgress {
    /// The first listing (the root) has been written to the sink.
    pub root_sent: bool,
    /// The initial population finished: its file count and source.
    pub complete: Option<(u32, TreeSource)>,
    /// `WalkSettled` has been written.
    pub settled: bool,
}

/// Progress as atomics, outside the session lock: a synchronous `mark_tree` must never
/// wait behind a sink call (architecture §2.2). Each flag is set after the sink returns,
/// so a reader never sees a message claimed that has not been written.
#[derive(Debug, Default)]
struct ProgressCell {
    root_sent: AtomicBool,
    settled: AtomicBool,
    /// 0 = not complete; otherwise `PRESENT | source << 32 | entries`.
    complete: AtomicU64,
}

const PRESENT: u64 = 1 << 40;

impl ProgressCell {
    fn set_complete(&self, entries: u32, source: TreeSource) {
        let tag = match source {
            TreeSource::Index => 0,
            TreeSource::Walk => 1,
        };
        self.complete.store(
            PRESENT | (tag << 32) | u64::from(entries),
            Ordering::Release,
        );
    }

    fn complete(&self) -> Option<(u32, TreeSource)> {
        let packed = self.complete.load(Ordering::Acquire);
        if packed & PRESENT == 0 {
            return None;
        }
        let source = if (packed >> 32) & 1 == 0 {
            TreeSource::Index
        } else {
            TreeSource::Walk
        };
        Some((
            u32::try_from(packed & 0xFFFF_FFFF).unwrap_or(u32::MAX),
            source,
        ))
    }
}

struct Inner {
    model: TreeModel,
    sink: Box<dyn FnMut(TreeOut) + Send>,
    progress: Arc<ProgressCell>,
    walk_sent: bool,
}

impl Inner {
    fn emit(&mut self, out: TreeOut) {
        let root = matches!(&out, TreeOut::Op(TreeOp::Listing { dir, .. }) if dir.is_empty());
        let settled = matches!(&out, TreeOut::WalkSettled { .. });
        let complete = match &out {
            TreeOut::Complete { entries, source } => Some((*entries, *source)),
            _ => None,
        };
        // Recorded BEFORE the sink runs, not after (task #141): the sink may write to the
        // webview inline, the front end marks exactly once, and `mark_tree` rejects a mark
        // whose `progress()` says the message was not sent yet. Recording after the sink
        // returned left a window, however short, in which a mark for a batch the webview
        // already holds was rejected and the cold start never got its trace. A mark can only
        // come from a front end that received the message, which the sink hands over after
        // this point, so recording first cannot accept a mark that was not earned.
        if root {
            self.progress.root_sent.store(true, Ordering::Release);
        }
        if let Some((entries, source)) = complete {
            self.progress.set_complete(entries, source);
        }
        (self.sink)(out);
        if settled {
            self.progress.settled.store(true, Ordering::Release);
        }
    }

    fn emit_all(&mut self, ops: impl IntoIterator<Item = TreeOp>) {
        for op in ops {
            self.emit(TreeOut::Op(op));
        }
    }

    fn files(&self) -> u32 {
        u32::try_from(self.model.file_count()).unwrap_or(u32::MAX)
    }
}

/// The follow-up [`TreeSession::created`] owes: a created directory must be rescanned,
/// because it may have arrived with contents. Run it with [`Self::run`].
#[derive(Debug)]
#[must_use = "a created directory may have arrived with contents; call .run(session, vault)"]
pub struct RescanNeeded {
    dir: Option<String>,
}

impl RescanNeeded {
    /// The directory to rescan, or `None` for a created file.
    #[must_use]
    pub fn dir(&self) -> Option<&str> {
        self.dir.as_deref()
    }

    /// Do the rescan, if one is owed: [`TreeSession::rescan`] of the directory.
    ///
    /// # Errors
    ///
    /// As [`TreeSession::rescan`].
    pub fn run(self, session: &TreeSession, vault: &Vault) -> Result<(), VaultError> {
        if let Some(dir) = self.dir {
            session.rescan(vault, &dir)?;
        }
        Ok(())
    }
}

/// One open tree stream: the model of what was sent, and the sink it is sent to.
///
/// The sink runs **under the session's lock** and must not block: it is a channel
/// send. Coalescing small messages into batches (≤ 256 KiB or ≤ 16 ms, ADR-0015 §2) is
/// the sink's business, since it changes nothing about order.
///
/// **A panic in the sink poisons the session, and a poisoned session is dead**, not
/// recovered: the model is mutated before the message is emitted, so after a panic it
/// may claim messages the stream never got. Every method then does nothing (the walk
/// and populate methods report it), [`Self::is_poisoned`] says so, and the caller
/// restarts the stream with a new session, which re-sends the tree from the root
/// (`open_tree` already means that).
pub struct TreeSession {
    inner: Mutex<Inner>,
    progress: Arc<ProgressCell>,
}

impl fmt::Debug for TreeSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TreeSession")
            .field("progress", &self.progress())
            .finish_non_exhaustive()
    }
}

impl TreeSession {
    /// A session writing to `sink`.
    pub fn new(sink: impl FnMut(TreeOut) + Send + 'static) -> Self {
        let progress = Arc::new(ProgressCell::default());
        Self {
            inner: Mutex::new(Inner {
                model: TreeModel::new(),
                sink: Box::new(sink),
                progress: Arc::clone(&progress),
                walk_sent: false,
            }),
            progress,
        }
    }

    /// Run `f` under the lock; `None` if the session is poisoned (dead).
    fn with<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> Option<R> {
        let mut guard = self.inner.lock().ok()?;
        Some(f(&mut guard))
    }

    fn poisoned() -> VaultError {
        VaultError::io(
            "tree session".to_owned(),
            &std::io::Error::other(
                "the tree session was poisoned by a panic in its sink; restart the stream",
            ),
        )
    }

    /// Whether the session died (a panic in its sink). Restart the stream.
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.inner.is_poisoned()
    }

    /// What has been delivered so far. Lock-free: it never waits for a sink call.
    ///
    /// This is the hook for the tree mark (NFR-PRF-01, ADR-0015 §3): the front end
    /// stamps `tree_first_screen` and `tree_complete` after painting, through
    /// `mark_tree`, and serat-app reads this to validate the stamp: `FirstScreen`
    /// requires `root_sent`, `Complete` requires `complete`, and the budget job's
    /// check of `entries_in_model` is against `complete.0`.
    #[must_use]
    pub fn progress(&self) -> TreeProgress {
        TreeProgress {
            root_sent: self.progress.root_sent.load(Ordering::Acquire),
            complete: self.progress.complete(),
            settled: self.progress.settled.load(Ordering::Acquire),
        }
    }

    /// Mark the start of an index read. **Call this before reading the rows**: the
    /// snapshot must be older than the read, or a delete that lands between the read
    /// and the listing is not applied to it (ADR-0015 §1). Then pass the token to
    /// [`Self::populate_from_index`], or to [`Self::abandon_index_read`] if the read
    /// failed. `None` if the session is poisoned.
    #[must_use]
    pub fn begin_index_read(&self) -> Option<Snapshot> {
        self.with(|inner| inner.model.begin_snapshot())
    }

    /// The index read failed; release its snapshot.
    pub fn abandon_index_read(&self, snapshot: Snapshot) {
        self.with(|inner| inner.model.end_snapshot(snapshot));
    }

    /// The warm path: populate from the index's `(path, kind)` rows, read after
    /// [`Self::begin_index_read`] returned `snapshot`. Root listing first and alone,
    /// then breadth-first, then [`TreeOut::Complete`].
    ///
    /// Returns `false`, having sent nothing further, if a walk listing has been sent
    /// (before or during this call), or the session is poisoned: the walk then
    /// populates alone and the index tree is not sent (ADR-0015 §2). Directories with
    /// no indexed file beneath them are absent until the walk arrives.
    pub fn populate_from_index<'a>(
        &self,
        snapshot: Snapshot,
        rows: impl IntoIterator<Item = (&'a str, TreeKind)>,
    ) -> bool {
        // The grouping is pure and can be long; it happens outside the lock.
        let listings = group_listings(rows);
        let mut proceeded = true;
        for (dir, raw) in listings {
            let sent = self.with(|inner| {
                if inner.walk_sent {
                    return false;
                }
                let ops = inner.model.list(snapshot, &dir, raw);
                inner.emit_all(ops);
                true
            });
            if sent != Some(true) {
                proceeded = false;
                break;
            }
        }
        self.with(|inner| {
            inner.model.end_snapshot(snapshot);
            if !proceeded || inner.walk_sent {
                return false;
            }
            let entries = inner.files();
            inner.emit(TreeOut::Complete {
                entries,
                source: TreeSource::Index,
            });
            true
        })
        .unwrap_or(false)
    }

    /// The no-index path and, afterwards, the authoritative walk: list every directory
    /// breadth-first from the root. If nothing completed the tree yet, it completes
    /// here with `source = Walk`; if the index did, these listings are catch-up and
    /// only differing ones are sent, and `WalkSettled` follows.
    ///
    /// The returned [`WalkSummary::empty_folders`] is the count serat-app reports as
    /// the arrival of empty folders (ADR-0015 §3); the `WalkSettled` message carries
    /// only those added after `Complete`.
    ///
    /// # Errors
    ///
    /// As [`Vault::walk_tree`], and an I/O error if the session is poisoned. On error
    /// no `Complete` is sent.
    pub fn populate_from_walk(&self, vault: &Vault) -> Result<WalkSummary, VaultError> {
        self.populate_walk(vault, None)
    }

    /// [`Self::populate_from_walk`] for the warm path's open (ADR-0015 section 2, *One
    /// walk at open*): the walk is [`Vault::walk_tree_with_metadata`], and each
    /// directory's raw entries (with `size` and `mtime_ms`) are handed to `on_listing`
    /// as they are streamed, so the caller can accumulate
    /// `serat_core::source_entries(dir, entries)` for `Reconciler::catch_up`. The
    /// stream the front end sees is identical to [`Self::populate_from_walk`]'s.
    ///
    /// `on_listing` runs under the session's lock, in stream order; it must not call
    /// back into the session.
    ///
    /// # Errors
    ///
    /// As [`Self::populate_from_walk`].
    pub fn populate_from_walk_with_metadata(
        &self,
        vault: &Vault,
        on_listing: &mut ListingTap<'_>,
    ) -> Result<WalkSummary, VaultError> {
        self.populate_walk(vault, Some(on_listing))
    }

    fn populate_walk(
        &self,
        vault: &Vault,
        tap: Option<&mut ListingTap<'_>>,
    ) -> Result<WalkSummary, VaultError> {
        let summary = self.rescan_dir(vault, "", true, tap)?;
        self.with(|inner| {
            if inner.progress.complete().is_none() {
                let entries = inner.files();
                inner.emit(TreeOut::Complete {
                    entries,
                    source: TreeSource::Walk,
                });
                inner.emit(TreeOut::WalkSettled { empty_folders: 0 });
            } else if !inner.progress.settled.load(Ordering::Acquire) {
                inner.emit(TreeOut::WalkSettled {
                    empty_folders: summary.empty_folders,
                });
            }
        })
        .ok_or_else(Self::poisoned)?;
        Ok(summary)
    }

    /// Re-list `dir` and everything beneath it: a directory rename (its new name's
    /// subtree), or the rescan after a watcher overflow (`""`, ADR-0011). Differing
    /// listings only.
    ///
    /// # Errors
    ///
    /// As [`Vault::walk_tree`], and an I/O error if the session is poisoned.
    pub fn rescan(&self, vault: &Vault, dir: &str) -> Result<WalkSummary, VaultError> {
        self.rescan_dir(vault, dir, false, None)
    }

    fn rescan_dir(
        &self,
        vault: &Vault,
        dir: &str,
        initial: bool,
        mut tap: Option<&mut ListingTap<'_>>,
    ) -> Result<WalkSummary, VaultError> {
        let snapshot: Snapshot = self
            .with(|inner| inner.model.begin_snapshot())
            .ok_or_else(Self::poisoned)?;
        let metadata = tap.is_some();
        let mut on_dir = |listed: &str, raw: Vec<RawEntry>| {
            self.with(|inner| {
                if initial {
                    inner.walk_sent = true;
                }
                if let Some(tap) = tap.as_mut() {
                    tap(listed, &raw);
                }
                let ops = inner.model.list(snapshot, listed, raw);
                inner.emit_all(ops);
            });
        };
        let result = if metadata {
            vault.walk_tree_with_metadata(dir, &mut on_dir)
        } else {
            vault.walk_tree(dir, &mut on_dir)
        };
        self.with(|inner| inner.model.end_snapshot(snapshot))
            .ok_or_else(Self::poisoned)?;
        result
    }

    /// A file or directory was created, or is the new name of a rename. `path` is
    /// vault-relative.
    ///
    /// **A directory that arrives may arrive with contents** (a folder moved or
    /// copied in): this lists it empty, and the returned [`RescanNeeded`] must be run
    /// to list what is in it. It is `#[must_use]` so that a caller cannot forget:
    /// every created directory is rescanned, unconditionally (code-reviewer-3's
    /// obligation on #13). For a file it is a no-op.
    pub fn created(&self, path: &str, kind: TreeKind) -> RescanNeeded {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        self.with(|inner| {
            let ops = inner.model.upsert(dir, RawEntry::new(name, kind));
            inner.emit_all(ops);
        });
        RescanNeeded {
            dir: (kind == TreeKind::Dir).then(|| path.to_owned()),
        }
    }

    /// A file or directory was deleted, or is the old name of a rename.
    pub fn deleted(&self, path: &str) {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        self.with(|inner| {
            let ops = inner.model.remove(dir, name);
            inner.emit_all(ops);
        });
    }
}
