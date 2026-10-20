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
use std::sync::{Mutex, MutexGuard, PoisonError};

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
        /// `Complete` (see [`WalkSummary::empty_folders`] for the count regardless).
        empty_folders: u32,
    },
}

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
    /// represent as a [`VaultPath`] is recorded in [`Vault::refusals`] (without
    /// clearing it: only [`Vault::walk`] does that) and left out. A subdirectory that
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
        mut on_dir: impl FnMut(&str, Vec<RawEntry>),
    ) -> Result<WalkSummary, VaultError> {
        let start_path = if start.is_empty() {
            self.root().to_path_buf()
        } else {
            self.resolve(&VaultPath::new(start)?)?
        };

        // (parent index, whether a file sits at or below this directory), in the
        // order directories were read, so a reverse pass settles every ancestor.
        let mut nodes: Vec<(usize, bool)> = Vec::new();
        let mut queue: VecDeque<(PathBuf, String, usize)> = VecDeque::new();
        queue.push_back((start_path, start.to_owned(), usize::MAX));
        let mut summary = WalkSummary::default();

        while let Some((directory, relative, parent)) = queue.pop_front() {
            let listing: Vec<fs::DirEntry> = match fs::read_dir(&directory) {
                Ok(listing) => listing.filter_map(Result::ok).collect(),
                Err(err) if parent == usize::MAX => {
                    if start.is_empty() {
                        return Err(VaultError::io(crate::display(&directory), &err));
                    }
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
                let path = entry.path();
                let child = match relativise_walked(self, &path) {
                    Ok(child) => child,
                    Err(VaultError::InvalidPath { path, reason }) => {
                        self.refusals().record(Refusal { path, reason });
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
                    entries.push(RawEntry::new(name, kind));
                }
            }
            nodes[index].1 = has_file;
            summary.dirs += 1;
            on_dir(&relative, entries);
        }

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

struct Inner {
    model: TreeModel,
    sink: Box<dyn FnMut(TreeOut) + Send>,
    root_sent: bool,
    walk_sent: bool,
    complete: Option<(u32, TreeSource)>,
    settled: bool,
}

impl Inner {
    fn emit(&mut self, out: TreeOut) {
        if let TreeOut::Op(TreeOp::Listing { dir, .. }) = &out
            && dir.is_empty()
        {
            self.root_sent = true;
        }
        (self.sink)(out);
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

/// One open tree stream: the model of what was sent, and the sink it is sent to.
///
/// The sink runs **under the session's lock** and must not block: it is a channel
/// send. Coalescing small messages into batches (≤ 256 KiB or ≤ 16 ms, ADR-0015 §2) is
/// the sink's business, since it changes nothing about order.
pub struct TreeSession {
    inner: Mutex<Inner>,
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
        Self {
            inner: Mutex::new(Inner {
                model: TreeModel::new(),
                sink: Box::new(sink),
                root_sent: false,
                walk_sent: false,
                complete: None,
                settled: false,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What has been delivered so far.
    ///
    /// This is the hook for the tree mark (NFR-PRF-01, ADR-0015 §3): the front end
    /// stamps `tree_first_screen` and `tree_complete` after painting, through
    /// `mark_tree`, and serat-app reads this to validate the stamp: `FirstScreen`
    /// requires `root_sent`, `Complete` requires `complete`, and the budget job's
    /// check of `entries_in_model` is against `complete.0`.
    #[must_use]
    pub fn progress(&self) -> TreeProgress {
        let inner = self.lock();
        TreeProgress {
            root_sent: inner.root_sent,
            complete: inner.complete,
            settled: inner.settled,
        }
    }

    /// The warm path: populate from the index's `(path, kind)` rows, root listing
    /// first and alone, then breadth-first, then [`TreeOut::Complete`].
    ///
    /// Returns `false`, having sent nothing further, if a walk listing has been sent
    /// (before or during this call): the walk then populates alone and the index tree
    /// is not sent (ADR-0015 §2). Directories with no indexed file beneath them are
    /// absent until the walk arrives.
    pub fn populate_from_index<'a>(
        &self,
        rows: impl IntoIterator<Item = (&'a str, TreeKind)>,
    ) -> bool {
        // The grouping is pure and can be long; it happens outside the lock.
        let listings = group_listings(rows);
        let snapshot = self.lock().model.begin_snapshot();
        let mut proceeded = true;
        for (dir, raw) in listings {
            let mut inner = self.lock();
            if inner.walk_sent {
                proceeded = false;
                break;
            }
            if let Some(op) = inner.model.list(snapshot, &dir, raw) {
                inner.emit(TreeOut::Op(op));
            }
        }
        let mut inner = self.lock();
        inner.model.end_snapshot(snapshot);
        if !proceeded || inner.walk_sent {
            return false;
        }
        let entries = inner.files();
        inner.complete = Some((entries, TreeSource::Index));
        inner.emit(TreeOut::Complete {
            entries,
            source: TreeSource::Index,
        });
        true
    }

    /// The no-index path and, afterwards, the authoritative walk: list every directory
    /// breadth-first from the root. If nothing completed the tree yet, it completes
    /// here with `source = Walk`; if the index did, these listings are catch-up and
    /// only differing ones are sent, and `WalkSettled` follows.
    ///
    /// # Errors
    ///
    /// As [`Vault::walk_tree`]. On error no `Complete` is sent.
    pub fn populate_from_walk(&self, vault: &Vault) -> Result<WalkSummary, VaultError> {
        let summary = self.rescan_dir(vault, "", true)?;
        let mut inner = self.lock();
        if inner.complete.is_none() {
            let entries = inner.files();
            inner.complete = Some((entries, TreeSource::Walk));
            inner.emit(TreeOut::Complete {
                entries,
                source: TreeSource::Walk,
            });
            inner.settled = true;
            inner.emit(TreeOut::WalkSettled { empty_folders: 0 });
        } else if !inner.settled {
            inner.settled = true;
            inner.emit(TreeOut::WalkSettled {
                empty_folders: summary.empty_folders,
            });
        }
        Ok(summary)
    }

    /// Re-list `dir` and everything beneath it: a directory rename (its new name's
    /// subtree), or the rescan after a watcher overflow (`""`, ADR-0011). Differing
    /// listings only.
    ///
    /// # Errors
    ///
    /// As [`Vault::walk_tree`].
    pub fn rescan(&self, vault: &Vault, dir: &str) -> Result<WalkSummary, VaultError> {
        self.rescan_dir(vault, dir, false)
    }

    fn rescan_dir(
        &self,
        vault: &Vault,
        dir: &str,
        initial: bool,
    ) -> Result<WalkSummary, VaultError> {
        let snapshot: Snapshot = self.lock().model.begin_snapshot();
        let result = vault.walk_tree(dir, |listed, raw| {
            let mut inner = self.lock();
            if initial {
                inner.walk_sent = true;
            }
            if let Some(op) = inner.model.list(snapshot, listed, raw) {
                inner.emit(TreeOut::Op(op));
            }
        });
        self.lock().model.end_snapshot(snapshot);
        result
    }

    /// A file or directory was created, or is the new name of a rename. `path` is
    /// vault-relative. A directory that arrives with contents needs a [`Self::rescan`]
    /// as well.
    pub fn created(&self, path: &str, kind: TreeKind) {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        let mut inner = self.lock();
        let ops = inner.model.upsert(dir, RawEntry::new(name, kind));
        inner.emit_all(ops);
    }

    /// A file or directory was deleted, or is the old name of a rename.
    pub fn deleted(&self, path: &str) {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        let mut inner = self.lock();
        let ops = inner.model.remove(dir, name);
        inner.emit_all(ops);
    }
}
