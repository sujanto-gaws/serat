//! The file tree's model: per-directory order, and the stream's ordering rules
//! (ADR-0015 §1; FR-VLT-01, FR-VLT-04, FR-BIN-03).
//!
//! Pure data and logic, no I/O (architecture §2.1). Two things live here because
//! both sources of a tree, the index read and the streaming walk, and the live
//! watcher path must agree on them byte for byte:
//!
//! * **Order.** Directories first, then files; case-insensitive by name under
//!   [`fold_case`]; ties broken by byte order. Decided here, once, so the front end
//!   never sorts and cannot disagree with Rust's collation.
//! * **What was sent.** [`TreeModel`] keeps the listings it has handed out. That is
//!   what lets an identical catch-up listing cost nothing, lets a live create name
//!   the sibling it follows, and is what makes the two ordering obligations of
//!   ADR-0015 §1 hold:
//!   1. a listing sent after a remove already reflects that remove, and
//!   2. an upsert into a directory that has not been listed yet is superseded by
//!      that directory's listing.
//!
//! The model is not thread-safe on purpose. Whoever owns the channel keeps it behind
//! the same lock as the send, so that the order of mutations is the order of the
//! stream (`serat_vault::TreeSession` does).
//!
//! # Snapshots
//!
//! A listing is built from a directory read that took time. [`TreeModel::begin_snapshot`]
//! marks "the state of the world as of now"; every live change made after it is
//! remembered until [`TreeModel::end_snapshot`], and [`TreeModel::list`] overlays those
//! changes on the read it is handed. That is what upholds obligations 1 and 2: the read
//! may be older than a remove already on the stream, and the overlay stops the removed
//! name coming back.

use std::collections::{BTreeSet, HashMap};

use crate::fold::fold_case;

/// What a tree entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeKind {
    /// A Markdown note.
    Note,
    /// Any other file.
    Asset,
    /// A directory.
    Dir,
}

/// A directory entry as a source found it: a name and a kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEntry {
    /// File or directory name, no path.
    pub name: String,
    /// What it is.
    pub kind: TreeKind,
}

impl RawEntry {
    /// An entry.
    #[must_use]
    pub fn new(name: impl Into<String>, kind: TreeKind) -> Self {
        Self {
            name: name.into(),
            kind,
        }
    }
}

/// An entry as the tree displays it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// File or directory name, no path.
    pub name: String,
    /// What it is.
    pub kind: TreeKind,
    /// An asset whose sidecar note (`<name>.md`, ADR-0005) is folded into this entry
    /// and not listed separately (FR-BIN-03).
    pub sidecar: bool,
}

/// One message of the tree stream, minus the two that are about the stream and not
/// the tree (`Complete`, `WalkSettled`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeOp {
    /// The complete, final-order contents of one directory.
    Listing {
        /// Vault-relative directory; `""` is the root.
        dir: String,
        /// Display order.
        entries: Vec<TreeEntry>,
    },
    /// One entry created, or changed what it displays as.
    Upsert {
        /// Parent directory.
        dir: String,
        /// The entry.
        entry: TreeEntry,
        /// The sibling it follows; `None` = first.
        after: Option<String>,
    },
    /// One entry gone. A directory takes its subtree with it.
    Remove {
        /// Parent directory.
        dir: String,
        /// Entry name.
        name: String,
    },
}

/// Where in a directory an entry sorts. Directories before files, then the folded
/// name, then the raw name so that two names folding alike still have one order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SortKey {
    is_file: bool,
    folded: String,
    name: String,
}

impl SortKey {
    fn new(name: &str, kind: TreeKind) -> Self {
        Self {
            is_file: kind != TreeKind::Dir,
            folded: fold_case(name),
            name: name.to_owned(),
        }
    }
}

/// The raw contents of one directory, as last sent.
#[derive(Debug, Default)]
struct DirState {
    names: HashMap<String, TreeKind>,
    order: BTreeSet<SortKey>,
}

impl DirState {
    fn from_raw(raw: Vec<RawEntry>) -> Self {
        let mut state = Self::default();
        for entry in raw {
            state.insert(entry.name, entry.kind);
        }
        state
    }

    /// Insert, replacing an entry of the same name.
    fn insert(&mut self, name: String, kind: TreeKind) {
        self.remove(&name);
        self.order.insert(SortKey::new(&name, kind));
        self.names.insert(name, kind);
    }

    fn remove(&mut self, name: &str) {
        if let Some(kind) = self.names.remove(name) {
            self.order.remove(&SortKey::new(name, kind));
        }
    }

    /// How `name` displays, or `None` if it is absent or folded into an asset.
    fn display(&self, name: &str) -> Option<TreeEntry> {
        let kind = *self.names.get(name)?;
        let mut sidecar = false;
        match kind {
            TreeKind::Dir => {}
            TreeKind::Note => {
                if let Some(stem) = name.strip_suffix(".md")
                    && self.names.get(stem) == Some(&TreeKind::Asset)
                {
                    return None;
                }
            }
            TreeKind::Asset => {
                sidecar = self.names.get(&format!("{name}.md")) == Some(&TreeKind::Note);
            }
        }
        Some(TreeEntry {
            name: name.to_owned(),
            kind,
            sidecar,
        })
    }

    /// Every displayed entry, in display order.
    fn entries(&self) -> Vec<TreeEntry> {
        self.order
            .iter()
            .filter_map(|key| self.display(&key.name))
            .collect()
    }

    /// The displayed sibling before `name`, or `None` if it is first.
    fn after(&self, name: &str) -> Option<String> {
        let kind = *self.names.get(name)?;
        let key = SortKey::new(name, kind);
        self.order
            .range(..key)
            .rev()
            .find_map(|prior| self.display(&prior.name))
            .map(|entry| entry.name)
    }

    fn files(&self) -> usize {
        self.names.values().filter(|k| **k != TreeKind::Dir).count()
    }
}

/// A point in the model's history that a directory read is taken against.
#[derive(Debug, Clone, Copy)]
pub struct Snapshot(u64);

/// What the stream has said about each directory, and the rules that keep it
/// consistent. See the module documentation.
#[derive(Debug, Default)]
pub struct TreeModel {
    dirs: HashMap<String, DirState>,
    generation: u64,
    outstanding: u32,
    /// Live changes made while a snapshot was outstanding: `(dir, name)` to the
    /// generation of the change and the state it left (`None` = removed).
    overlay: HashMap<(String, String), (u64, Option<TreeKind>)>,
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

impl TreeModel {
    /// An empty model: nothing has been sent.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a directory read (or a batch of them). Pair with [`Self::end_snapshot`].
    pub fn begin_snapshot(&mut self) -> Snapshot {
        self.outstanding += 1;
        Snapshot(self.generation)
    }

    /// The read is over; once none is outstanding the overlay is dropped.
    pub fn end_snapshot(&mut self, _snapshot: Snapshot) {
        self.outstanding = self.outstanding.saturating_sub(1);
        if self.outstanding == 0 {
            self.overlay.clear();
        }
    }

    /// Whether a listing for `dir` has been produced.
    #[must_use]
    pub fn is_listed(&self, dir: &str) -> bool {
        self.dirs.contains_key(dir)
    }

    /// Files (notes and assets) in the directories listed so far, counted before any
    /// sidecar fold: the number `Complete.entries` reports against the manifest.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.dirs.values().map(DirState::files).sum()
    }

    /// Whether this directory, or any directory above it, was removed after `snapshot`.
    fn removed_since(&self, snapshot: Snapshot, dir: &str) -> bool {
        if self.overlay.is_empty() || dir.is_empty() {
            return false;
        }
        dir.match_indices('/')
            .map(|(i, _)| &dir[..i])
            .chain(std::iter::once(dir))
            .any(|path| {
                let (parent, name) = split(path);
                matches!(
                    self.overlay.get(&(parent.to_owned(), name.to_owned())),
                    Some((generation, None)) if *generation > snapshot.0
                )
            })
    }

    /// Offer a directory read taken against `snapshot`. Returns the listing to send,
    /// or `None` when it would repeat what was sent, or when the directory (or an
    /// ancestor) has been removed since the read began.
    pub fn list(&mut self, snapshot: Snapshot, dir: &str, raw: Vec<RawEntry>) -> Option<TreeOp> {
        if self.removed_since(snapshot, dir) {
            return None;
        }
        let mut state = DirState::from_raw(raw);
        for ((d, name), (generation, kind)) in &self.overlay {
            if d == dir && *generation > snapshot.0 {
                match kind {
                    Some(kind) => state.insert(name.clone(), *kind),
                    None => state.remove(name),
                }
            }
        }
        if let Some(old) = self.dirs.get(dir) {
            if old.names == state.names {
                return None;
            }
            let gone: Vec<String> = old
                .names
                .iter()
                .filter(|(name, kind)| {
                    **kind == TreeKind::Dir && state.names.get(*name) != Some(kind)
                })
                .map(|(name, _)| join(dir, name))
                .collect();
            for path in gone {
                self.drop_subtree(&path);
            }
        }
        let entries = state.entries();
        self.dirs.insert(dir.to_owned(), state);
        Some(TreeOp::Listing {
            dir: dir.to_owned(),
            entries,
        })
    }

    /// An entry was created (or is the new name of a rename). Returns the messages
    /// to send, in order: nothing if `dir` has not been listed, because that
    /// directory's listing will carry the entry.
    pub fn upsert(&mut self, dir: &str, entry: RawEntry) -> Vec<TreeOp> {
        self.record(dir, &entry.name, Some(entry.kind));
        let RawEntry { name, kind } = entry;
        let Some(previous) = self.dirs.get(dir).map(|s| s.names.get(&name).copied()) else {
            return Vec::new();
        };
        let mut ops = self.change(dir, &name, |state| state.insert(name.clone(), kind));
        if previous == Some(TreeKind::Dir) && kind != TreeKind::Dir {
            self.drop_subtree(&join(dir, &name));
        }
        if kind == TreeKind::Dir && previous != Some(TreeKind::Dir) {
            // A new folder is empty until a listing says otherwise; without this it
            // would sit pending in the front end for ever.
            let child = join(dir, &name);
            if !self.dirs.contains_key(&child) {
                self.dirs.insert(child.clone(), DirState::default());
                ops.push(TreeOp::Listing {
                    dir: child,
                    entries: Vec::new(),
                });
            }
        }
        ops
    }

    /// An entry was deleted (or is the old name of a rename).
    pub fn remove(&mut self, dir: &str, name: &str) -> Vec<TreeOp> {
        self.record(dir, name, None);
        self.drop_subtree(&join(dir, name));
        self.change(dir, name, |state| state.remove(name))
    }

    fn record(&mut self, dir: &str, name: &str, kind: Option<TreeKind>) {
        self.generation += 1;
        if self.outstanding > 0 {
            self.overlay
                .insert((dir.to_owned(), name.to_owned()), (self.generation, kind));
        }
    }

    fn drop_subtree(&mut self, path: &str) {
        let prefix = format!("{path}/");
        self.dirs
            .retain(|dir, _| dir != path && !dir.starts_with(&prefix));
    }

    /// Apply `mutate` to a listed directory and say what changed on screen; nothing
    /// if the directory has not been listed.
    ///
    /// A change to one name can change how two others display: an asset appearing
    /// folds its `.md` sidecar away, and a sidecar appearing marks its asset. So the
    /// messages are computed over every name the change can reach, by comparing how
    /// each displayed before and after. Removes go first and upserts in display order,
    /// so the sibling each upsert follows is already in the front end's list.
    fn change(&mut self, dir: &str, name: &str, mutate: impl FnOnce(&mut DirState)) -> Vec<TreeOp> {
        let Some(state) = self.dirs.get_mut(dir) else {
            return Vec::new();
        };
        let mut reach = vec![name.to_owned(), format!("{name}.md")];
        if let Some(stem) = name.strip_suffix(".md") {
            reach.push(stem.to_owned());
        }
        let before: Vec<Option<TreeEntry>> = reach.iter().map(|n| state.display(n)).collect();
        mutate(state);
        let mut ops = Vec::new();
        let mut upserts = Vec::new();
        for (name, before) in reach.iter().zip(before) {
            let remove = || TreeOp::Remove {
                dir: dir.to_owned(),
                name: name.clone(),
            };
            match (before, state.display(name)) {
                (Some(_), None) => ops.push(remove()),
                (Some(old), Some(new)) if old != new => {
                    if old.kind != new.kind {
                        ops.push(remove());
                    }
                    upserts.push(new);
                }
                (None, Some(new)) => upserts.push(new),
                _ => {}
            }
        }
        upserts.sort_by_key(|entry| SortKey::new(&entry.name, entry.kind));
        for entry in upserts {
            let after = state.after(&entry.name);
            ops.push(TreeOp::Upsert {
                dir: dir.to_owned(),
                entry,
                after,
            });
        }
        ops
    }
}

/// Group `(vault-relative path, kind)` rows (kind is `Note` or `Asset`) into one
/// listing's raw entries per directory, **root first, then breadth-first** (ADR-0015
/// §2's warm path). Directories appear as the ancestors of the files given; a
/// directory holding no file anywhere beneath it is absent, by design.
#[must_use]
pub fn group_listings<'a>(
    rows: impl IntoIterator<Item = (&'a str, TreeKind)>,
) -> Vec<(String, Vec<RawEntry>)> {
    let mut map: HashMap<&str, Vec<RawEntry>> = HashMap::new();
    map.insert("", Vec::new());
    for (path, kind) in rows {
        let (dir, name) = split(path);
        let mut created = Vec::new();
        let mut current = dir;
        while !map.contains_key(current) {
            map.insert(current, Vec::new());
            created.push(current);
            current = split(current).0;
        }
        for new_dir in created {
            let (parent, name) = split(new_dir);
            if let Some(siblings) = map.get_mut(parent) {
                siblings.push(RawEntry::new(name, TreeKind::Dir));
            }
        }
        if let Some(siblings) = map.get_mut(dir) {
            siblings.push(RawEntry::new(name, kind));
        }
    }
    let mut listings: Vec<(String, Vec<RawEntry>)> = map
        .into_iter()
        .map(|(dir, entries)| (dir.to_owned(), entries))
        .collect();
    listings.sort_by_cached_key(|(dir, _)| {
        let depth = if dir.is_empty() {
            0
        } else {
            dir.matches('/').count() + 1
        };
        (depth, dir.clone())
    });
    listings
}
