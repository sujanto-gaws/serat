//! Watching, reading, atomic writes, ignore rules, path canonicalisation.
//!
//! Everything in Serat that touches a filesystem is here, and nothing here knows what
//! Markdown is (architecture §2.1). This crate hands out bytes and paths; what those
//! bytes mean is `serat-parse`'s problem, and the separation is what lets the parser
//! be tested with string literals instead of fixture directories.
//!
//! # The pipeline this crate owns
//!
//! Steps 1 to 3 of architecture §3.1, in order:
//!
//! 1. [`Watcher`] coalesces filesystem events per path over a 150 ms window. Editors
//!    that save via temp-and-rename emit several events per save; they collapse to
//!    one.
//! 2. [`WorkQueue`] deduplicates by path and is bounded. A git checkout touching
//!    4,000 files enqueues 4,000 entries, not 4,000 parses.
//! 3. [`Vault::read_if_changed`] is the hash gate: it reads the file and compares
//!    BLAKE3 against the hash the index holds. Sync tools touch mtime constantly
//!    without changing content, and this discards those for the cost of a hash.
//!
//! Plus the write half: [`Vault::write_atomic`] is temp-file-and-rename, and
//! [`EchoSuppressor`] is how the watcher recognises our own write and drops it
//! (§3.2) instead of reindexing a file we just finished writing.
//!
//! Nothing here sits on the keystroke path, which is this pipeline's actual design
//! constraint — not throughput (design goal 2).

mod atomic;
pub mod diff;
pub mod echo;
mod gitignore;
pub mod hash;
pub mod ignore;
#[cfg(test)]
mod ignore_file_tests;
pub mod patch;
#[cfg(test)]
mod patch_props;
pub mod position;
pub mod queue;
pub mod refusal;
pub mod tree;
pub mod walk;
#[cfg(all(test, unix))]
mod walk_refusal_tests;
pub mod watch;
#[cfg(test)]
mod write_race_tests;

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serat_core::verbatim::strip_verbatim;
use serat_core::{
    ContentHash, EchoGuard, LoadedNote, NoteKind, NoteMeta, Platform, VaultError, VaultPath,
};

pub use echo::EchoSuppressor;
pub use gitignore::{GitignoreAppended, gitignore_ignores_serat};
pub use hash::hash_bytes;
pub use ignore::{IGNORE_FILE, IgnoreRules, Patterns, is_note_file};
pub use patch::Edit;
pub use position::Position;
pub use queue::{QueueStats, WorkQueue};
pub use refusal::{Refusal, RefusalLog};
pub use tree::{RescanNeeded, TreeOut, TreeProgress, TreeSession, TreeSource, WalkSummary};

pub use watch::{VaultEvent, Watcher, WatcherConfig};

/// What [`Vault::write_atomic_if`] requires of the file as it is on disk, checked under
/// the per-path lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// The file must not exist (a create).
    Absent,
    /// The file must exist with this content hash (an overwrite of a known base).
    Hash(ContentHash),
}

/// An open vault: a root directory, and the rules for what inside it counts.
#[derive(Debug, Clone)]
pub struct Vault {
    root: PathBuf,
    ignore: IgnoreRules,
    /// Hashes an indexed write should tell the watcher to drop as its own echo
    /// (architecture §3.2). Shared rather than owned: the watcher and whoever records
    /// an echo must consult the same set, and `write_atomic` no longer touches this at
    /// all — see [`Vault::write_atomic`] and the [`EchoGuard`] impl below for where
    /// recording now happens and why (F-224, F-226).
    echo: Arc<EchoSuppressor>,
    /// Paths the walk or the watcher could not turn into a `VaultPath`, reported
    /// rather than dropped (task #34's amendment). Shared for the same reason
    /// `echo` is: the walk and the watcher must both be able to add to it.
    refusals: RefusalLog,
    /// One mutex per path being written, shared by every clone (ADR-0016 §2): held
    /// from the first read to the rename in every in-process content write.
    locks: Arc<atomic::PathLocks>,
}

impl Vault {
    /// Open the vault rooted at `root`.
    ///
    /// The root is canonicalised once, here, and every path in the crate is derived
    /// from it. That is what makes [`VaultPath`] a reliable identity: without it, the
    /// same file reached through a symlink, a relative path and a differently-cased
    /// drive letter would be three notes.
    ///
    /// # Errors
    ///
    /// [`VaultError::NotFound`] if the directory does not exist,
    /// [`VaultError::InvalidPath`] if it is not a directory.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, VaultError> {
        Self::with_rules(root, IgnoreRules::new())
    }

    /// Open the vault with non-default ignore rules.
    ///
    /// # Errors
    ///
    /// As [`Vault::open`].
    pub fn with_rules(root: impl AsRef<Path>, ignore: IgnoreRules) -> Result<Self, VaultError> {
        let root = root.as_ref();
        let canonical =
            fs::canonicalize(root).map_err(|err| VaultError::io(display(root), &err))?;
        if !canonical.is_dir() {
            return Err(VaultError::InvalidPath {
                path: display(root),
                reason: "is not a directory".to_owned(),
            });
        }
        let vault = Self {
            root: strip_verbatim(&canonical),
            ignore,
            echo: Arc::new(EchoSuppressor::new()),
            refusals: RefusalLog::new(),
            locks: Arc::default(),
        };
        vault.reload_ignore_file();
        Ok(vault)
    }

    /// Re-read the ignore file ([`IGNORE_FILE`], at the root) into the rules in
    /// force. Returns whether the effective patterns changed, in which case what the
    /// vault contains has changed too and the caller must walk it again (the watcher
    /// raises [`VaultEvent::RescanRequired`] for exactly that).
    ///
    /// A missing, unreadable, symlinked or oversized file means no patterns. It is
    /// never an error: the file is the user's and Serat only reads it (FR-VLT-07).
    pub fn reload_ignore_file(&self) -> bool {
        let path = self.root.join(IGNORE_FILE);
        let bytes = match fs::symlink_metadata(&path) {
            Ok(stat) if stat.is_file() => fs::read(&path).ok(),
            _ => None,
        };
        self.ignore.set_ignore_file(bytes.as_deref())
    }

    /// Every indexable file at or below the vault-relative directory `dir`, sorted,
    /// under the same rules as [`Vault::walk`]. A directory that is gone, is a
    /// symlink or is itself ignored lists nothing. For reconciling one directory after
    /// a [`VaultEvent::DirChanged`] without walking the vault.
    ///
    /// # Errors
    ///
    /// [`VaultError::Io`] only if the vault root cannot be read.
    pub fn walk_dir(&self, dir: &VaultPath) -> Result<Vec<serat_core::SourceEntry>, VaultError> {
        walk::walk_dir(self, dir)
    }

    pub(crate) fn path_locks(&self) -> &atomic::PathLocks {
        &self.locks
    }

    /// The canonical root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The ignore rules in force.
    #[must_use]
    pub fn ignore_rules(&self) -> &IgnoreRules {
        &self.ignore
    }

    /// The echo suppressor the watcher consults.
    ///
    /// The watcher asks it whether an event carries bytes we just indexed:
    /// `if vault.echo().take_echo(&path, loaded.meta.hash) { skip }` after the hash gate.
    /// Recording an entry is [`EchoGuard::record_echo`], which `Vault` implements
    /// below — callers that write through the index's save path use that trait object
    /// rather than reaching in here directly.
    #[must_use]
    pub fn echo(&self) -> &Arc<EchoSuppressor> {
        &self.echo
    }

    /// Paths the walk or the watcher refused to index, because their name is
    /// illegal on the platform this binary runs on (task #34's amendment,
    /// FR-VLT-01, design goal 5).
    ///
    /// Empty on an ordinary vault. When it is not, a caller has a status fact
    /// worth surfacing — "N files could not be indexed" plus the list — rather
    /// than a gap in the tree nothing explains.
    #[must_use]
    pub fn refusals(&self) -> &RefusalLog {
        &self.refusals
    }

    /// The absolute path of a vault-relative one, refusing to resolve through a
    /// symlink.
    ///
    /// This is the narrowest point every filesystem access in this crate passes
    /// through — [`Vault::read`] and [`Vault::write_atomic`] both call it, and nothing
    /// bypasses it to reach the disk. That is deliberate (T-07, found by code-reviewer's
    /// Tauri surface audit, task #18 finding 4): [`walk::walk`] refuses every symlink
    /// as it discovers files, but a path that arrives some other way — the IPC
    /// boundary's `read_file`/`write_file`, most concretely — never went through the
    /// walk, and `root.join(path)` alone follows a symlinked component exactly as
    /// happily as a real one. "A symlink is not a vault file" needs to be one rule
    /// with one enforcement point, not the walk's rule and a second, absent one here —
    /// two mechanisms that happen to agree are two mechanisms that can drift apart,
    /// which is exactly what had already happened.
    ///
    /// Checked component by component from the root down with `symlink_metadata`
    /// (never `metadata`, which follows the very thing being checked for), the same
    /// primitive the walk already uses — deliberately not a full `canonicalize`, which
    /// does more syscalls than this needs and returns a path that then has to be
    /// re-compared against the root, a second mechanism for the same question. A
    /// component that does not exist yet is not a symlink, so this refuses an existing
    /// symlinked ancestor without also refusing a legitimate new file in a new
    /// subdirectory that has not been created yet.
    ///
    /// This sits on `NoteSource::load`'s path, so a rebuild pays it once per file
    /// (NFR-PRF-04, working agreement 4) — a small, bounded number of `lstat` calls per
    /// path rather than one, but each is cheap, and design goal 1 (the vault survives
    /// the application) outranks the budget if the two are ever actually in tension.
    ///
    /// # Errors
    ///
    /// [`VaultError::PathEscapesVault`] if any path component between the root and the
    /// target, inclusive, is a symlink, or if the accumulated path does not end up
    /// inside the canonical root at all.
    pub fn resolve(&self, path: &VaultPath) -> Result<PathBuf, VaultError> {
        self.resolve_segments(path.as_str().split('/'), path.as_str())
    }

    /// The body of [`Vault::resolve`], parameterised over raw segments rather
    /// than a [`VaultPath`].
    ///
    /// A seam for tests (task #34's amendment): `VaultPath::new` refuses every
    /// segment shape that would trip the containment check below — a `:` on
    /// Windows, a leading `/` — which is correct, but it also means no test that
    /// goes through `resolve` can reach that check's own line, and a check no
    /// test can reach is not proven to do anything. Calling this directly with a
    /// segment a `VaultPath` could never hold (`"/etc"`, which `PathBuf::push`
    /// treats as absolute and uses to *replace* the accumulated path — on every
    /// platform, not only Windows) exercises the containment check on its own
    /// terms, independent of whichever upstream guard currently keeps that shape
    /// from arising in practice.
    fn resolve_segments<'a>(
        &self,
        segments: impl Iterator<Item = &'a str>,
        reported_path: &str,
    ) -> Result<PathBuf, VaultError> {
        let mut current = self.root.clone();
        for component in segments {
            current.push(component);
            let is_symlink = fs::symlink_metadata(&current)
                .map(|meta| meta.is_symlink())
                .unwrap_or(false);
            if is_symlink {
                return Err(VaultError::PathEscapesVault {
                    path: reported_path.to_owned(),
                });
            }
        }
        // Defence in depth (task #34, NFR-SEC-06, T-07, T-10): `VaultPath::new`
        // already refuses the segment shapes that let a `PathBuf::push` escape the
        // root — a `:` anywhere on Windows, in particular, which lets a rootless
        // drive-prefixed segment like "C:Users" *replace* the accumulated path
        // instead of extending it — but a guard that lives only at construction is
        // one mechanism, and one mechanism that regresses (a future segment shape
        // nobody thought of, a change to `VaultPath` that loosens a check) fails
        // open with nothing here to catch it. This is the narrowest point every
        // resolution passes through, so it is where a second, independent check
        // belongs: whatever `current` ended up being, it must still be inside
        // `self.root`. Factored out as `is_contained` so the containment rule
        // itself is unit-testable independently of how `current` was built, and
        // reachable end-to-end through `resolve_segments` above.
        if !is_contained(&self.root, &current) {
            return Err(VaultError::PathEscapesVault {
                path: reported_path.to_owned(),
            });
        }
        Ok(current)
    }

    /// Turn an absolute path into a vault-relative one.
    ///
    /// Refuses anything outside the root. This is the only door into [`VaultPath`]
    /// for paths that came from outside — a watcher event, a command-line argument, a
    /// drag-and-drop — and it is where a `..` that survived everything else stops.
    ///
    /// # Errors
    ///
    /// [`VaultError::PathEscapesVault`] if the path is not inside the root, and
    /// whatever [`VaultPath::new`] rejects.
    pub fn relativise(&self, path: &Path) -> Result<VaultPath, VaultError> {
        let escaped = || VaultError::PathEscapesVault {
            path: display(path),
        };

        // Reject traversal before comparing prefixes: `root/../elsewhere` starts with
        // `root` as a string and is not in the vault.
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(escaped());
        }

        // The file may already be gone — a delete event arrives after the fact — so
        // canonicalisation is best-effort and the lexical path is the fallback.
        let absolute = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let absolute = strip_verbatim(&absolute);

        let relative = absolute.strip_prefix(&self.root).map_err(|_| escaped())?;
        vault_path_from_relative(Platform::current(), relative)
    }

    /// Every indexable file in the vault, in a deterministic order.
    ///
    /// # Errors
    ///
    /// [`VaultError::Io`] if the root itself cannot be read. A directory that cannot
    /// be read part-way through is skipped rather than fatal: one unreadable folder
    /// must not stop a vault from indexing (design goal 5).
    pub fn walk(&self) -> Result<Vec<serat_core::SourceEntry>, VaultError> {
        walk::walk(self)
    }

    /// Read a file and hash it. The mtime in the result is the handle's, taken before
    /// the bytes are read (see the body for why the order matters).
    ///
    /// # Errors
    ///
    /// [`VaultError::NotFound`] if it is gone, [`VaultError::PathEscapesVault`] if it
    /// resolves through a symlink (T-07), [`VaultError::Io`] otherwise.
    pub fn read(&self, path: &VaultPath) -> Result<LoadedNote, VaultError> {
        self.read_hooked(path, || {})
    }

    /// [`Vault::read`] with a seam between the stat and the read, so a test can land a
    /// write exactly there (as `write_atomic_hooked` does for the rename).
    fn read_hooked(
        &self,
        path: &VaultPath,
        after_stat: impl FnOnce(),
    ) -> Result<LoadedNote, VaultError> {
        let absolute = self.resolve(path)?;
        // One open, stat the handle, then read from it. The mtime must be taken
        // *before* the bytes: a write landing between the two then leaves an old
        // mtime beside the new hash, which the next mtime comparison catches. The
        // other order records the new mtime beside the old hash and hides the change.
        let io = |err: std::io::Error| VaultError::io(path.as_str(), &err);
        let mut file = fs::File::open(&absolute).map_err(io)?;
        let stat = file.metadata().map_err(io)?;
        after_stat();
        let mut bytes = Vec::with_capacity(usize::try_from(stat.len()).unwrap_or(0));
        std::io::Read::read_to_end(&mut file, &mut bytes).map_err(io)?;

        Ok(LoadedNote {
            meta: NoteMeta {
                hash: hash_bytes(&bytes),
                kind: if is_note_file(path.file_name()) {
                    NoteKind::Note
                } else {
                    NoteKind::Asset
                },
                size: bytes.len() as u64,
                mtime_ms: walk::mtime_ms(&stat),
                path: path.clone(),
            },
            bytes,
        })
    }

    /// The hash gate: read the file, and return it only if its content differs from
    /// `previous`.
    ///
    /// Architecture §3.1 step 3. `previous` is the hash the index holds for this
    /// path, or `None` if it holds no row. Passing the index's hash in rather than
    /// looking it up here is what keeps this crate from knowing the index exists.
    ///
    /// A discarded event costs one read and one hash — no parse, no transaction, no
    /// view invalidation. That is the point: a sync tool that rewrites mtime on
    /// 20,000 files costs a scan, not a rebuild.
    ///
    /// # Errors
    ///
    /// As [`Vault::read`].
    pub fn read_if_changed(
        &self,
        path: &VaultPath,
        previous: Option<ContentHash>,
    ) -> Result<Option<LoadedNote>, VaultError> {
        let loaded = self.read(path)?;
        if Some(loaded.meta.hash) == previous {
            return Ok(None);
        }
        Ok(Some(loaded))
    }

    /// Replace a file's contents atomically, and return the hash of what was written.
    ///
    /// Write to a temporary file in the same directory, flush it, then rename over
    /// the target (architecture §6.1 step 5). Same directory because a rename is only
    /// atomic within a filesystem, and the system temp directory is routinely on
    /// another one.
    ///
    /// A reader therefore sees either every byte of the old file or every byte of the
    /// new one, never a prefix of the new — design goal 1, in the place it is
    /// cheapest to get wrong. Interrupting this leaves a `.serat-tmp-` file that the
    /// walk ignores, not a truncated note.
    ///
    /// The caller supplies the complete new bytes. This function does not know what a
    /// field or a byte range is: byte-range patching (C-04, ADR-0008) happens above it
    /// and hands down the result.
    ///
    /// # Errors
    ///
    /// [`VaultError::PathEscapesVault`] if the target resolves through a symlink
    /// (T-07) — refused before the parent directory is even created, so a symlinked
    /// ancestor is never written through, only ever discovered.
    /// [`VaultError::AtomicWriteFailed`] if any step fails, in which case the
    /// original file is untouched.
    pub fn write_atomic(&self, path: &VaultPath, bytes: &[u8]) -> Result<ContentHash, VaultError> {
        self.write_atomic_hooked(path, bytes, None, || {})
    }

    /// [`Vault::write_atomic`] guarded by what the caller believes is on disk, checked
    /// **under the per-path lock** (ADR-0016 §2: the lock is held from step 1's
    /// re-read to the rename in both write paths). This is `write_file`'s path: the
    /// optimistic lock of architecture §6.1 and the create-only case, so that a
    /// concurrent `write_patch` or a second create cannot slip in between the check
    /// and the rename.
    ///
    /// # Errors
    ///
    /// As [`Vault::write_atomic`], plus [`VaultError::StaleHash`] if the file exists
    /// and `expect` is [`Expect::Absent`] or a different hash (`found` is what is on
    /// disk), and [`VaultError::NotFound`] if `expect` is a hash and the file is gone.
    pub fn write_atomic_if(
        &self,
        path: &VaultPath,
        expect: Expect,
        bytes: &[u8],
    ) -> Result<ContentHash, VaultError> {
        self.write_atomic_hooked(path, bytes, Some(expect), || {})
    }

    /// The shared body, with a hook that runs after the temporary file is complete and
    /// before the rename: the window the race tests need to reach deterministically.
    fn write_atomic_hooked(
        &self,
        path: &VaultPath,
        bytes: &[u8],
        expect: Option<Expect>,
        before_rename: impl FnOnce(),
    ) -> Result<ContentHash, VaultError> {
        // T-06 / FR-BIN-19: binaries are read, extracted, annotated and queried, never
        // rewritten. The threat model's stated mechanism is that no write path accepts
        // a non-Markdown target, so the refusal belongs here — in the one function that
        // writes — rather than in each caller that might remember to check.
        if !path.is_markdown() {
            return Err(VaultError::NotAMarkdownTarget {
                path: path.as_str().to_owned(),
            });
        }

        let hash = hash_bytes(bytes);
        // No echo recording here (architecture §3.2, amended by F-226). It used to
        // happen at this point, before the write — and that placement is why F-196 and
        // F-222 were findable at all: it recorded regardless of whether anything ever
        // indexed this write, so a caller with no submit path (F-222) or a submit
        // later refused by the writer (F-196) permanently suppressed the only event
        // that would ever have indexed the note. `write_atomic` is a write and nothing
        // else now; recording is [`EchoGuard::record_echo`], called by the index's save
        // path only after a submit has actually been accepted (F-224).

        let target = self.resolve(path)?;
        let parent = target.parent().ok_or_else(|| VaultError::InvalidPath {
            path: path.as_str().to_owned(),
            reason: "has no parent directory".to_owned(),
        })?;
        let failed = |message: String| VaultError::AtomicWriteFailed {
            path: path.as_str().to_owned(),
            message,
        };

        // ADR-0016 §2: no other in-process writer of this path between here and the
        // rename.
        let lock = self.locks.lock_for(path.as_str());
        let _held = lock.hold();

        // Step 1 of ADR-0016 §2, under the lock: what the caller believes is on disk.
        if let Some(expect) = expect {
            match (fs::read(&target), expect) {
                (Ok(current), Expect::Absent) => {
                    return Err(VaultError::StaleHash {
                        path: path.as_str().to_owned(),
                        expected: "absent".to_owned(),
                        found: hash_bytes(&current).to_hex(),
                    });
                }
                (Ok(current), Expect::Hash(want)) => {
                    let found = hash_bytes(&current);
                    if found != want {
                        return Err(VaultError::StaleHash {
                            path: path.as_str().to_owned(),
                            expected: want.to_hex(),
                            found: found.to_hex(),
                        });
                    }
                }
                (Err(err), Expect::Hash(_)) if err.kind() == std::io::ErrorKind::NotFound => {
                    return Err(VaultError::NotFound {
                        path: path.as_str().to_owned(),
                    });
                }
                (Err(err), _) if err.kind() == std::io::ErrorKind::NotFound => {}
                (Err(err), _) => return Err(VaultError::io(path.as_str(), &err)),
            }
        }

        fs::create_dir_all(parent).map_err(|err| failed(err.to_string()))?;

        // F-33. The replacement inherits the original's permissions, because a
        // temp-and-rename write creates a *new* file and a new file gets the process
        // default. Without this, replacing a note the user chmod'd 600 silently
        // publishes it at 644 — an application quietly widening access to the user's
        // own data, which is design goal 1 failing in the one direction nobody checks.
        //
        // Cross-platform, though what it carries differs: on Unix this is the full
        // mode; on Windows only the read-only flag exists to carry.
        let existing_permissions = fs::metadata(&target).ok().map(|stat| stat.permissions());

        let temp = atomic::write_temp(parent, path.file_name(), bytes, existing_permissions)
            .map_err(|err| failed(err.to_string()))?;
        before_rename();
        atomic::commit(&temp, &target).map_err(|err| failed(err.to_string()))?;

        Ok(hash)
    }

    /// Apply a patch set against `base_hash` and write the result, atomically
    /// (ADR-0016 §2, C-04).
    ///
    /// This is the editor save path — the one this crate offers as an
    /// alternative to [`Vault::write_atomic`] when the caller holds an edit
    /// script rather than a whole new document. Unlike `write_atomic`, it never
    /// creates a file: a save is always against a base that was read, so a
    /// missing target is [`VaultError::NotFound`], and `write_file`'s
    /// `base_hash: None` creation case stays `write_atomic`'s alone (ADR-0016
    /// §2, closing paragraph).
    ///
    /// The steps are ADR-0016 §2's, in order:
    ///
    /// 1. Re-read the file and refuse with [`VaultError::StaleHash`] if its hash
    ///    is not `base_hash` — the optimistic lock.
    /// 2. Map each edit's positions onto byte offsets and check the ranges are
    ///    ordered and disjoint ([`patch::apply_edits`]).
    /// 3. Convert every `\n` in an insert to the note's own line ending.
    /// 4. Splice and write the temporary file, then **re-hash the target
    ///    immediately before the rename** and refuse with `StaleHash` — removing
    ///    the temporary file — if it moved. The remaining window (one hash plus
    ///    one `rename`) is stated as a bound, not hidden: an external write
    ///    landing inside it is overwritten, silently, exactly as ADR-0016 §2
    ///    records.
    /// 5. Rename and return the new hash.
    ///
    /// # Errors
    ///
    /// [`VaultError::NotAMarkdownTarget`] for a non-Markdown target (T-06,
    /// mirroring `write_atomic`). [`VaultError::NotFound`] if the file does not
    /// exist. [`VaultError::PathEscapesVault`] if it resolves through a symlink
    /// (T-07). [`VaultError::StaleHash`] per steps 1 and 4 above.
    /// [`VaultError::MalformedPatch`] if the edits do not apply to the base
    /// cleanly. [`VaultError::AtomicWriteFailed`] if the write itself fails, in
    /// which case the original file is untouched.
    pub fn write_patch(
        &self,
        path: &VaultPath,
        base_hash: ContentHash,
        edits: &[patch::Edit],
    ) -> Result<ContentHash, VaultError> {
        self.write_patch_hooked(path, base_hash, edits, || {}, || {})
    }

    /// [`Vault::write_patch`] with a hook that runs after the temporary file is
    /// complete and before the re-hash: the window the hostile tests (an external
    /// write, a crash) need to reach deterministically.
    fn write_patch_hooked(
        &self,
        path: &VaultPath,
        base_hash: ContentHash,
        edits: &[patch::Edit],
        before_rehash: impl FnOnce(),
        before_rename: impl FnOnce(),
    ) -> Result<ContentHash, VaultError> {
        if !path.is_markdown() {
            return Err(VaultError::NotAMarkdownTarget {
                path: path.as_str().to_owned(),
            });
        }

        let target = self.resolve(path)?;
        let stale = |found: ContentHash| VaultError::StaleHash {
            path: path.as_str().to_owned(),
            expected: base_hash.to_hex(),
            found: found.to_hex(),
        };

        // ADR-0016 §2: the per-path lock, from step 1's read to the rename. Without it
        // two of Serat's own writers can both pass step 4's re-hash before either
        // renames, and one returns `Ok` with its bytes already gone.
        let lock = self.locks.lock_for(path.as_str());
        let _held = lock.hold();

        // Step 1: the hash lock. A delete or a rewrite between the caller's read
        // and this call surfaces here rather than silently patching the wrong
        // bytes.
        let current_bytes = fs::read(&target).map_err(|err| VaultError::io(path.as_str(), &err))?;
        let current_hash = hash_bytes(&current_bytes);
        if current_hash != base_hash {
            return Err(stale(current_hash));
        }
        let base_text =
            std::str::from_utf8(&current_bytes).map_err(|_| VaultError::MalformedPatch {
                path: path.as_str().to_owned(),
                reason: "base is not valid utf-8; a non-utf-8 file is never opened for editing"
                    .to_owned(),
            })?;

        // Steps 2 and 3: map, validate and splice. Nothing outside a patched
        // range is touched — see `patch::apply_edits`'s own guarantee.
        let new_text = patch::apply_edits(base_text, edits, path.as_str())?;
        let new_bytes = new_text.into_bytes();
        let new_hash = hash_bytes(&new_bytes);

        let parent = target.parent().ok_or_else(|| VaultError::InvalidPath {
            path: path.as_str().to_owned(),
            reason: "has no parent directory".to_owned(),
        })?;
        let failed = |message: String| VaultError::AtomicWriteFailed {
            path: path.as_str().to_owned(),
            message,
        };

        // F-33's rule applies here too: the replacement inherits the original's
        // permissions.
        let existing_permissions = fs::metadata(&target).ok().map(|stat| stat.permissions());

        let temp = atomic::write_temp(parent, path.file_name(), &new_bytes, existing_permissions)
            .map_err(|err| failed(err.to_string()))?;

        before_rehash();

        // Step 4's re-hash, immediately before the rename (ADR-0016 §2). This is
        // what "immediately before" means in code: nothing between this read and
        // the `fs::rename` call below touches the disk.
        let reread = match fs::read(&target) {
            Ok(bytes) => bytes,
            Err(err) => {
                let _ = fs::remove_file(&temp);
                return Err(VaultError::io(path.as_str(), &err));
            }
        };
        let reread_hash = hash_bytes(&reread);
        if reread_hash != base_hash {
            let _ = fs::remove_file(&temp);
            return Err(stale(reread_hash));
        }

        before_rename();

        atomic::commit(&temp, &target).map_err(|err| failed(err.to_string()))?;

        Ok(new_hash)
    }

    /// The edit script from `base` to the file as it is on disk now, in `base`'s
    /// coordinates (ADR-0016 §3; FR-VLT-05). The answer to a stale-base refusal and
    /// to `request_external_diff`: the caller holds the base text, the vault holds
    /// the disk.
    ///
    /// Returns the disk hash with the script, or `None` when the disk bytes hash to
    /// `base_hash` (nothing to report). A disk change that is only a line-ending
    /// flip returns `Some` with an empty script and the new hash.
    ///
    /// # Errors
    ///
    /// [`VaultError::NotFound`] / [`VaultError::Io`] as for any read;
    /// [`VaultError::MalformedPatch`] if the disk bytes are not UTF-8 (such a note
    /// is never open for editing).
    pub fn diff_against_disk(
        &self,
        path: &VaultPath,
        base_hash: ContentHash,
        base: &str,
    ) -> Result<Option<(ContentHash, diff::EditScript)>, VaultError> {
        let target = self.resolve(path)?;
        let bytes = fs::read(&target).map_err(|err| VaultError::io(path.as_str(), &err))?;
        let hash = hash_bytes(&bytes);
        if hash == base_hash {
            return Ok(None);
        }
        let other = std::str::from_utf8(&bytes).map_err(|_| VaultError::MalformedPatch {
            path: path.as_str().to_owned(),
            reason: "the file on disk is not valid utf-8".to_owned(),
        })?;
        Ok(Some((hash, diff::edit_script(base, other))))
    }
}

impl EchoGuard for Vault {
    /// Delegates to the shared [`EchoSuppressor`], so the save path and the watcher
    /// consult the same set regardless of which clone of `Vault` each holds.
    fn record_echo(&self, path: &VaultPath, hash: ContentHash) {
        self.echo.record(path, hash);
    }
}

/// A path for an error message. Lossy on purpose: a path we cannot render is
/// precisely the one an error needs to name.
fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Turn a path already relative to the vault root into a [`VaultPath`], refusing
/// a real path component that would otherwise be silently misread.
///
/// `VaultPath::new` splits on both `/` and `\`, which is right for a path a user
/// typed or pasted — a Windows spelling reaching a text field — but wrong for a
/// path the filesystem itself just handed back: on a platform where `\` is an
/// ordinary filename character, a real file named `"a\b.md"` would otherwise
/// silently alias to the two-segment path `"a/b.md"` (task #34's amendment, nit
/// (b)). This checks every real [`Component`] for a literal `\` before joining
/// them — on Windows the check is skipped, because `\` there is the separator
/// and cannot appear inside a single component's text at all.
pub(crate) fn vault_path_from_relative(
    platform: Platform,
    relative: &Path,
) -> Result<VaultPath, VaultError> {
    let joined = display(relative);
    if platform != Platform::Windows {
        for component in relative.components() {
            let Some(text) = component.as_os_str().to_str() else {
                // Lossy rendering would map two different non-UTF-8 names onto one
                // string, and `VaultPath` is UTF-8 by construction.
                return Err(VaultError::InvalidPath {
                    path: joined,
                    reason: "file name is not valid UTF-8".to_owned(),
                });
            };
            if text.contains('\\') {
                return Err(VaultError::InvalidPath {
                    path: joined,
                    reason: format!(
                        "file name {text:?} contains a literal '\\', which this platform \
                         allows in a name but which would otherwise be re-read as a path \
                         separator"
                    ),
                });
            }
        }
    }
    VaultPath::new(joined)
}

/// Whether `candidate` is `root` itself, or lies inside it.
///
/// [`Vault::resolve`]'s defence-in-depth check (task #34, NFR-SEC-06, T-07, T-10):
/// `Path::starts_with` compares components, not raw text, so a sibling directory
/// that merely shares `root`'s string prefix (`/vault-old` against `/vault`) is
/// correctly not contained — that distinction is `std`'s, not reimplemented here,
/// which is the point of using it rather than a string comparison.
fn is_contained(root: &Path, candidate: &Path) -> bool {
    candidate.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::open(dir.path()).expect("open vault");
        (dir, vault)
    }

    #[test]
    fn a_write_round_trips_through_a_read() {
        let (_dir, vault) = vault();
        let path = VaultPath::new("10-projects/serat.md").expect("path");

        let hash = vault
            .write_atomic(&path, b"# Serat\n\nstatus:: active\n")
            .expect("write");
        let loaded = vault.read(&path).expect("read");
        assert_eq!(loaded.bytes, b"# Serat\n\nstatus:: active\n");
        assert_eq!(loaded.meta.hash, hash);
        assert_eq!(loaded.meta.kind, NoteKind::Note);
        assert_eq!(loaded.meta.size, 25);
        assert_eq!(loaded.meta.path, path);
    }

    #[test]
    fn a_write_between_the_stat_and_the_read_leaves_the_old_mtime_beside_the_new_bytes() {
        let (dir, vault) = vault();
        let path = VaultPath::new("note.md").expect("path");
        vault.write_atomic(&path, b"old\n").expect("seed");
        let before = vault.read(&path).expect("read").meta.mtime_ms;
        let file = dir.path().join("note.md");

        let loaded = vault
            .read_hooked(&path, || {
                // In place, so the open handle sees it; mtime moved well forward.
                fs::write(&file, b"new bytes\n").expect("rewrite");
                let handle = fs::File::options().write(true).open(&file).expect("open");
                handle
                    .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
                    .expect("mtime");
            })
            .expect("read");

        assert_eq!(loaded.bytes, b"new bytes\n");
        assert_eq!(
            loaded.meta.mtime_ms, before,
            "the mtime was taken after the bytes"
        );
        let now = vault.read(&path).expect("read").meta.mtime_ms;
        assert_ne!(now, before, "the file's own mtime did move");
    }

    #[test]
    fn write_patch_touches_only_the_patched_range() {
        let (_dir, vault) = vault();
        let path = VaultPath::new("note.md").expect("path");
        let base_hash = vault
            .write_atomic(&path, b"status:: draft\ntitle:: Serat\n")
            .expect("seed");

        let edits = [patch::Edit::new(
            position::Position::new(0, 9),
            position::Position::new(0, 14),
            "active".to_owned(),
        )];
        vault.write_patch(&path, base_hash, &edits).expect("patch");

        assert_eq!(
            vault.read(&path).expect("read").bytes,
            b"status:: active\ntitle:: Serat\n"
        );
    }

    #[test]
    fn write_patch_preserves_crlf_and_a_bom_it_did_not_touch() {
        let (_dir, vault) = vault();
        let path = VaultPath::new("note.md").expect("path");
        let base_hash = vault
            .write_atomic(&path, "\u{FEFF}first\r\nsecond\r\n".as_bytes())
            .expect("seed");

        // Edit only the second line; the BOM, the first line and every CRLF must
        // survive unchanged.
        let edits = [patch::Edit::new(
            position::Position::new(1, 0),
            position::Position::new(1, 6),
            "SECOND".to_owned(),
        )];
        vault.write_patch(&path, base_hash, &edits).expect("patch");

        assert_eq!(
            vault.read(&path).expect("read").bytes,
            "\u{FEFF}first\r\nSECOND\r\n".as_bytes()
        );
    }

    #[test]
    fn write_patch_refuses_a_stale_base() {
        let (_dir, vault) = vault();
        let path = VaultPath::new("note.md").expect("path");
        let base_hash = vault.write_atomic(&path, b"original").expect("seed");
        // Someone else writes before our patch lands.
        vault
            .write_atomic(&path, b"changed by someone else")
            .expect("write");

        let edits = [patch::Edit::new(
            position::Position::new(0, 0),
            position::Position::new(0, 8),
            "patched".to_owned(),
        )];
        let result = vault.write_patch(&path, base_hash, &edits);
        assert!(
            matches!(result, Err(VaultError::StaleHash { .. })),
            "{result:?}"
        );
        assert_eq!(
            vault.read(&path).expect("read").bytes,
            b"changed by someone else",
            "a refused patch must leave the file exactly as the other writer left it"
        );
    }

    #[test]
    fn write_patch_leaves_no_temporary_file_behind_on_a_stale_base() {
        let (dir, vault) = vault();
        let path = VaultPath::new("note.md").expect("path");
        let base_hash = vault.write_atomic(&path, b"original").expect("seed");
        vault.write_atomic(&path, b"changed").expect("write");

        let edits = [patch::Edit::new(
            position::Position::new(0, 0),
            position::Position::new(0, 8),
            "patched".to_owned(),
        )];
        assert!(vault.write_patch(&path, base_hash, &edits).is_err());

        let strays: Vec<String> = fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(ignore::TEMP_PREFIX))
            .collect();
        assert!(strays.is_empty(), "left behind {strays:?}");
    }

    #[test]
    fn write_patch_refuses_a_malformed_patch_without_touching_the_file() {
        let (_dir, vault) = vault();
        let path = VaultPath::new("note.md").expect("path");
        let base_hash = vault.write_atomic(&path, b"short").expect("seed");

        // A position past the end of the only line.
        let edits = [patch::Edit::new(
            position::Position::new(0, 99),
            position::Position::new(0, 99),
            "x".to_owned(),
        )];
        let result = vault.write_patch(&path, base_hash, &edits);
        assert!(
            matches!(result, Err(VaultError::MalformedPatch { .. })),
            "{result:?}"
        );
        assert_eq!(vault.read(&path).expect("read").bytes, b"short");
    }

    #[test]
    fn write_patch_refuses_a_non_markdown_target() {
        let (_dir, vault) = vault();
        let path = VaultPath::new("40-refs/contract.pdf").expect("path");
        // No file exists, but the markdown check runs before any read.
        let bogus_hash = hash_bytes(b"anything");
        let result = vault.write_patch(&path, bogus_hash, &[]);
        assert!(matches!(result, Err(VaultError::NotAMarkdownTarget { .. })));
    }

    #[test]
    fn write_patch_on_a_file_the_property_test_style_random_patch_leaves_everything_else_alone() {
        // NFR-MNT-03's property, exercised directly rather than only through
        // `patch::apply_edits`'s unit tests: many small, non-overlapping edits
        // against a realistic multi-line, mixed-content base, applied through
        // the full `write_patch` path (hash lock, atomic write, re-hash before
        // rename), and every byte outside a patched range must survive.
        let (_dir, vault) = vault();
        let path = VaultPath::new("note.md").expect("path");
        let base = "status:: draft\ntitle:: Serat\nbody line one\nbody line two\n";
        let base_hash = vault.write_atomic(&path, base.as_bytes()).expect("seed");

        // Two disjoint, ascending edits: one on line 0, one on line 2.
        let edits = [
            patch::Edit::new(
                position::Position::new(0, 9),
                position::Position::new(0, 14),
                "active".to_owned(),
            ),
            patch::Edit::new(
                position::Position::new(2, 0),
                position::Position::new(2, 0),
                "ONE ".to_owned(),
            ),
        ];
        vault.write_patch(&path, base_hash, &edits).expect("patch");

        assert_eq!(
            vault.read(&path).expect("read").bytes,
            b"status:: active\ntitle:: Serat\nONE body line one\nbody line two\n"
        );
    }

    #[test]
    fn writing_leaves_no_temporary_files_behind() {
        let (dir, vault) = vault();
        let path = VaultPath::new("note.md").expect("path");
        vault.write_atomic(&path, b"one").expect("write");
        vault.write_atomic(&path, b"two").expect("write");

        let strays: Vec<String> = fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(ignore::TEMP_PREFIX))
            .collect();
        assert!(strays.is_empty(), "left behind {strays:?}");
        assert_eq!(vault.read(&path).expect("read").bytes, b"two");
    }

    #[test]
    fn the_hash_gate_discards_a_touched_but_unchanged_file() {
        let (_dir, vault) = vault();
        let path = VaultPath::new("note.md").expect("path");
        let hash = vault.write_atomic(&path, b"unchanged").expect("write");

        // What a sync tool does: same bytes, new mtime.
        assert!(
            vault
                .read_if_changed(&path, Some(hash))
                .expect("read")
                .is_none()
        );

        // What an editor does.
        vault.write_atomic(&path, b"changed").expect("write");
        let changed = vault
            .read_if_changed(&path, Some(hash))
            .expect("read")
            .expect("content differs");
        assert_eq!(changed.bytes, b"changed");

        // And a path the index has never seen is always a change.
        assert!(vault.read_if_changed(&path, None).expect("read").is_some());
    }

    #[test]
    fn a_reader_never_observes_a_partially_written_note() {
        // S2 from the I2 review. Both previous atomic-write tests passed unchanged
        // against a plain truncate-in-place fs::write — the "no temp files left
        // behind" one especially, since an implementation with no temp files
        // trivially leaves none. So nothing asserted the property temp-and-rename
        // exists for.
        //
        // A reader spins on the file while a writer replaces 4 MiB of it. Every byte
        // string it observes must be exactly the old one or exactly the new one, never
        // a prefix, a mixture or a truncation. Under truncate-in-place this fails
        // within a handful of iterations.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let (_dir, vault) = vault();
        let path = VaultPath::new("big.md").expect("path");
        let old_bytes = vec![b'o'; 4 * 1024 * 1024];
        let new_bytes = vec![b'n'; 4 * 1024 * 1024];
        vault.write_atomic(&path, &old_bytes).expect("seed");

        let stop = Arc::new(AtomicBool::new(false));
        let torn = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));

        let reader = {
            let absolute = vault.resolve(&path).expect("resolve");
            let stop = Arc::clone(&stop);
            let torn = Arc::clone(&torn);
            let reads = Arc::clone(&reads);
            let old_bytes = old_bytes.clone();
            let new_bytes = new_bytes.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(seen) = fs::read(&absolute) {
                        reads.fetch_add(1, Ordering::Relaxed);
                        if seen != old_bytes && seen != new_bytes {
                            torn.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            })
        };

        for round in 0..20 {
            let bytes = if round % 2 == 0 {
                &new_bytes
            } else {
                &old_bytes
            };
            vault.write_atomic(&path, bytes).expect("write");
        }
        stop.store(true, Ordering::Relaxed);
        reader.join().expect("reader");

        assert!(
            reads.load(Ordering::Relaxed) > 0,
            "the reader never managed a read, so this proved nothing"
        );
        assert_eq!(
            torn.load(Ordering::Relaxed),
            0,
            "a reader saw a file that was neither the old contents nor the new"
        );
    }

    #[test]
    fn write_patch_never_leaves_a_reader_with_a_torn_file() {
        // FR-VLT-03's old-or-new property, for `write_patch` rather than
        // `write_atomic`: a reader spinning on the file while a patch lands must
        // see exactly the pre-patch bytes or exactly the post-patch bytes, never
        // a mixture — `write_patch` shares `write_atomic`'s temp-and-rename tail,
        // but it is its own code path (a hash lock and a re-hash sit in front of
        // it), so the property is asserted against it directly rather than
        // inferred from the other test.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let (_dir, vault) = vault();
        let path = VaultPath::new("big.md").expect("path");
        let old_line = "o".repeat(4 * 1024 * 1024);
        let old_bytes = old_line.clone().into_bytes();
        let mut hash = vault.write_atomic(&path, &old_bytes).expect("seed");

        let stop = Arc::new(AtomicBool::new(false));
        let torn = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));

        let reader = {
            let absolute = vault.resolve(&path).expect("resolve");
            let stop = Arc::clone(&stop);
            let torn = Arc::clone(&torn);
            let reads = Arc::clone(&reads);
            let old_bytes = old_bytes.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(seen) = fs::read(&absolute) {
                        reads.fetch_add(1, Ordering::Relaxed);
                        let all_o = seen.iter().all(|&b| b == b'o');
                        let all_n = seen.iter().all(|&b| b == b'n');
                        if seen.len() != old_bytes.len() || !(all_o || all_n) {
                            torn.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            })
        };

        // Each round replaces the whole line via one patch, alternating the
        // character so a torn read is distinguishable from either valid state.
        for round in 0..20 {
            let replacement = if round % 2 == 0 { 'n' } else { 'o' };
            let edits = [patch::Edit::new(
                position::Position::new(0, 0),
                position::Position::new(0, 4 * 1024 * 1024),
                replacement.to_string().repeat(4 * 1024 * 1024),
            )];
            hash = vault.write_patch(&path, hash, &edits).expect("patch");
        }
        stop.store(true, Ordering::Relaxed);
        reader.join().expect("reader");

        assert!(
            reads.load(Ordering::Relaxed) > 0,
            "the reader never managed a read, so this proved nothing"
        );
        assert_eq!(
            torn.load(Ordering::Relaxed),
            0,
            "a reader saw a file that was neither the pre-patch nor the post-patch contents"
        );
    }

    #[test]
    fn a_binary_target_is_refused() {
        // T-06 and FR-BIN-19: binaries are read, extracted, annotated and queried,
        // never rewritten. The threat model's mechanism is that no write path accepts
        // a non-Markdown target, so the refusal lives in the only function that writes.
        let (_dir, vault) = vault();
        for binary in [
            "40-refs/contract.pdf",
            "assets/diagram.png",
            "data/rows.xlsx",
        ] {
            let path = VaultPath::new(binary).expect("path");
            assert!(
                matches!(
                    vault.write_atomic(&path, b"anything"),
                    Err(VaultError::NotAMarkdownTarget { .. })
                ),
                "{binary} was accepted as a write target"
            );
            assert!(
                !vault.resolve(&path).expect("resolve").exists(),
                "{binary} was created despite the refusal"
            );
        }
    }

    #[test]
    fn write_atomic_no_longer_touches_the_echo_suppressor() {
        // F-226: recording moved out of write_atomic and into EchoGuard::record_echo,
        // called by the index's save path only after a submit is accepted — not by the
        // write itself. A write with nothing downstream to call record_echo (F-222's
        // shape, S1's write_file before this fix) must not leave an entry behind: an
        // entry nobody consumes just sits there until it expires, which is harmless,
        // but a caller relying on write_atomic to have recorded anything is relying on
        // behaviour this function no longer has.
        let (_dir, vault) = vault();
        let path = VaultPath::new("note.md").expect("path");
        let hash = vault
            .write_atomic(
                &path, b"# Ours
",
            )
            .expect("write");

        assert!(
            !vault.echo().take_echo(&path, hash),
            "write_atomic recorded an echo entry; it must be a plain write (F-224)"
        );
    }

    #[test]
    fn record_echo_after_a_write_suppresses_the_watchers_own_event() {
        // The other half: EchoGuard::record_echo, called the way the real save path
        // calls it — after write_atomic, standing in for "after a successful submit" —
        // still suppresses exactly one event, keyed by hash.
        let (_dir, vault) = vault();
        let path = VaultPath::new("note.md").expect("path");
        let hash = vault
            .write_atomic(
                &path, b"# Ours
",
            )
            .expect("write");

        vault.record_echo(&path, hash);

        assert!(
            vault.echo().take_echo(&path, hash),
            "record_echo did not register the hash for suppression"
        );
        assert!(
            !vault.echo().take_echo(&path, hash),
            "one recorded echo suppressed two events"
        );
    }

    #[test]
    fn someone_elses_write_is_not_suppressed() {
        let (dir, vault) = vault();
        vault
            .write_atomic(&VaultPath::new("ours.md").expect("path"), b"ours")
            .expect("write");

        // Another tool writes, bypassing us entirely.
        fs::write(dir.path().join("theirs.md"), b"theirs").expect("write");
        let theirs = vault
            .read(&VaultPath::new("theirs.md").expect("path"))
            .expect("read");

        assert!(
            !vault.echo().take_echo(&theirs.meta.path, theirs.meta.hash),
            "an external edit was mistaken for our own echo"
        );
    }

    #[test]
    fn paths_outside_the_vault_are_refused() {
        let (dir, vault) = vault();
        let outside = dir.path().parent().expect("parent").join("elsewhere.md");
        assert!(matches!(
            vault.relativise(&outside),
            Err(VaultError::PathEscapesVault { .. })
        ));
        assert!(matches!(
            vault.relativise(&dir.path().join("..").join("elsewhere.md")),
            Err(VaultError::PathEscapesVault { .. })
        ));
    }

    #[test]
    fn a_colon_bearing_vault_path_is_refused_before_it_ever_reaches_resolve_on_windows() {
        // Task #34, amended after code-reviewer-2's regression finding. The
        // public door is closed at `VaultPath::new` on `Platform::Windows`
        // (NFR-SEC-06, T-07, T-10): a segment with a `:` anywhere — not just a
        // leading drive prefix — must never become a `VaultPath` there, so it
        // can never reach `resolve` to test `PathBuf::push`'s Windows
        // replace-instead-of-extend behaviour in the first place. On every other
        // platform the same names are ordinary and must be accepted — a colon
        // has no special meaning to `PathBuf::push` there — which is exactly
        // what made the first version of this fix a regression: it refused
        // these names everywhere, dropping real Linux and macOS files.
        for bad in ["a/C:/Windows/x", "a/C:Users/x", "note.md:hidden"] {
            assert!(
                serat_core::VaultPath::new_for_platform(bad, serat_core::Platform::Windows)
                    .is_err(),
                "accepted on Windows: {bad:?}"
            );
            assert!(
                serat_core::VaultPath::new_for_platform(bad, serat_core::Platform::Other).is_ok(),
                "refused on a platform where a colon is an ordinary character: {bad:?}"
            );
        }
    }

    #[test]
    fn is_contained_refuses_a_candidate_outside_the_root() {
        // The containment check `resolve()` applies after the symlink loop
        // (task #34, NFR-SEC-06), tested directly as a pure function of two
        // paths, independent of how the candidate was built. See
        // `resolve_segments_containment_check_is_reachable_directly` below for
        // the same check exercised end-to-end through `Vault::resolve_segments`.
        assert!(!is_contained(Path::new("/vault"), Path::new("/etc/passwd")));
        assert!(!is_contained(Path::new("/vault"), Path::new("C:Users")));
        // A sibling directory that merely shares the root's text prefix is not
        // contained either — `Path::starts_with` is component-wise, and a naive
        // string-prefix reimplementation of this check would get this wrong.
        assert!(!is_contained(
            Path::new("/vault"),
            Path::new("/vault-old/secret.md")
        ));
        assert!(is_contained(
            Path::new("/vault"),
            Path::new("/vault/10-projects/note.md")
        ));
        assert!(is_contained(Path::new("/vault"), Path::new("/vault")));
    }

    #[test]
    fn resolve_segments_containment_check_is_reachable_directly() {
        // Task #34's amendment. `VaultPath::new` refuses every segment shape
        // that would trip `is_contained` inside `resolve` — a `:` on Windows, a
        // leading `/` — which is correct, but it also means nothing that goes
        // through the public `resolve(&VaultPath)` can reach that check's own
        // line on any platform, and a check nothing can reach is not proven to
        // do anything. `resolve_segments` is the seam: called directly with a
        // raw segment a `VaultPath` could never hold, it proves the check fires
        // for real, not only in the `is_contained` unit test above.
        //
        // "/etc" is the segment: `PathBuf::push` of a string that is itself
        // absolute *replaces* the accumulated path rather than extending it —
        // this is ordinary, documented `std` behaviour on every platform
        // (`PathBuf::push`'s own docs give exactly this example), not the
        // Windows-only drive-letter case `VaultPath::new`'s `:` rule defends
        // against. So this test needs no `#[cfg]` at all.
        let (_dir, vault) = vault();
        let result = vault.resolve_segments(["a", "/etc"].into_iter(), "a//etc");
        assert!(
            matches!(result, Err(VaultError::PathEscapesVault { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn an_absolute_path_inside_the_vault_relativises_to_one_spelling() {
        let (_dir, vault) = vault();
        let path = VaultPath::new("20-systems/indexing.md").expect("path");
        vault.write_atomic(&path, b"body").expect("write");

        let absolute = vault.resolve(&path).expect("resolve");
        assert_eq!(vault.relativise(&absolute).expect("relativise"), path);
    }

    #[cfg(unix)]
    #[test]
    fn relativise_refuses_a_real_name_with_a_literal_backslash() {
        // `a\b.md` is one file name on Unix; `VaultPath::new` would re-read it as
        // `a/b.md`, aliasing a real file to a path nothing on disk has.
        let (dir, vault) = vault();
        let odd = dir.path().join("a\\b.md");
        std::fs::write(&odd, b"x").expect("write");
        let result = vault.relativise(&odd);
        assert!(
            matches!(result, Err(VaultError::InvalidPath { .. })),
            "{result:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn relativise_refuses_a_name_that_is_not_utf8_rather_than_aliasing_it() {
        use std::os::unix::ffi::OsStrExt as _;
        let (dir, vault) = vault();
        let odd = dir.path().join(std::ffi::OsStr::from_bytes(b"caf\xe9.md"));
        std::fs::write(&odd, b"x").expect("write");
        let result = vault.relativise(&odd);
        assert!(
            matches!(result, Err(VaultError::InvalidPath { .. })),
            "{result:?}"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn relativise_accepts_names_only_windows_would_refuse() {
        // Task #34's regression, at `relativise` directly: `aux.md`, a colon in
        // a filename and a directory named with a trailing dot are all
        // ordinary, legal names on this platform, and `relativise` — the one
        // door every path arriving from outside the process goes through
        // (watcher events, IPC arguments, drag-and-drop) — must accept them
        // rather than refuse a real file (FR-VLT-01, design goal 5).
        let (dir, vault) = vault();
        for legal in ["aux.md", "Meeting 10:30.md", "Misc./n.md"] {
            let path = VaultPath::new(legal).expect("path");
            vault.write_atomic(&path, b"body").expect("write");
            let absolute = dir.path().join(legal);
            assert_eq!(
                vault.relativise(&absolute).expect("relativise"),
                path,
                "{legal} was refused"
            );
        }
    }

    #[test]
    fn a_deleted_file_still_relativises() {
        // Delete events arrive after the file is gone, so canonicalisation fails and
        // the lexical path has to be enough. If this stopped working, deletes would
        // never reach the index and it would keep rows for files that no longer
        // exist.
        let (_dir, vault) = vault();
        let path = VaultPath::new("gone.md").expect("path");
        let absolute = vault.resolve(&path).expect("resolve");
        assert_eq!(vault.relativise(&absolute).expect("relativise"), path);
    }

    #[test]
    fn an_asset_is_read_as_an_asset() {
        // Built with fs::write rather than write_atomic: our own write path refuses a
        // non-Markdown target (T-06), and a fixture that established the opposite as
        // normal usage would undermine the invariant it sits beside.
        let (_dir, vault) = vault();
        let path = VaultPath::new("40-refs/contract.pdf").expect("path");
        let absolute = vault.resolve(&path).expect("resolve");
        fs::create_dir_all(absolute.parent().expect("parent")).expect("mkdir");
        fs::write(&absolute, b"%PDF-1.7 header only").expect("write");
        assert_eq!(vault.read(&path).expect("read").meta.kind, NoteKind::Asset);
    }

    #[test]
    fn a_missing_file_is_not_found_rather_than_a_generic_io_error() {
        let (_dir, vault) = vault();
        let path = VaultPath::new("nope.md").expect("path");
        assert!(matches!(
            vault.read(&path),
            Err(VaultError::NotFound { .. })
        ));
    }
}

/// The vault, as the index sees it.
///
/// This is the seam architecture §2.1 prescribes: `serat-index` needs to walk and read
/// in order to rebuild, and may not depend on this crate, so it takes a
/// [`NoteSource`] and `serat-app` supplies the `Vault`. It is also what lets the
/// rebuild be tested against a `Vec` with no filesystem in sight.
impl serat_core::NoteSource for Vault {
    fn entries(&self) -> Result<Vec<serat_core::SourceEntry>, VaultError> {
        self.walk()
    }

    fn load(&self, path: &VaultPath) -> Result<LoadedNote, VaultError> {
        // The index never holds what the ignore file hides. A change accepted under the
        // old rules during the watcher's window, or a path from a stale queue, is
        // answered as gone, so the consumer removes the row rather than re-adding it.
        // `read` itself is not restricted: the editor may open an ignored note.
        if self.ignore.user_ignores_path(path.as_str()) {
            return Err(VaultError::NotFound {
                path: path.as_str().to_owned(),
            });
        }
        self.read(path)
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use serat_core::NoteSource as _;

    #[test]
    fn the_vault_satisfies_the_trait_the_index_rebuilds_through() {
        // The seam, exercised rather than assumed. If this compiles and passes, a
        // rebuild driven by serat-index over a real vault works without serat-index
        // ever linking this crate.
        let dir = tempfile::tempdir().expect("temp");
        let vault = Vault::open(dir.path()).expect("open");
        vault
            .write_atomic(&VaultPath::new("a.md").expect("path"), b"# A\n")
            .expect("write");
        fs::write(dir.path().join("b.pdf"), b"%PDF binary").expect("write");

        let entries = vault.entries().expect("entries");
        assert_eq!(entries.len(), 2);

        // F-29b: the entry carries what the walk already knew, so the rebuild does not
        // re-stat every file to rediscover it.
        let note = entries
            .iter()
            .find(|entry| entry.path.as_str() == "a.md")
            .expect("note");
        assert_eq!(note.kind, NoteKind::Note);
        assert_eq!(note.size, 4);
        assert!(note.mtime_ms > 0);

        let asset = entries
            .iter()
            .find(|entry| entry.path.as_str() == "b.pdf")
            .expect("asset");
        assert_eq!(asset.kind, NoteKind::Asset);

        let loaded = vault.load(&note.path).expect("load");
        assert_eq!(loaded.bytes, b"# A\n");
        assert_eq!(loaded.meta.kind, NoteKind::Note);
        assert_eq!(loaded.meta.size, note.size);
    }
}

#[cfg(test)]
mod permission_tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::open(dir.path()).expect("open vault");
        (dir, vault)
    }

    #[test]
    fn replacing_a_note_does_not_change_its_permissions() {
        // F-33, design goal 1. A temp-and-rename write creates a new file, and a new
        // file gets the process default — so without care, saving a note the user
        // restricted quietly publishes it.
        let (_dir, vault) = vault();
        let path = VaultPath::new("private.md").expect("path");
        vault.write_atomic(&path, b"# Before\n").expect("write");

        let absolute = vault.resolve(&path).expect("resolve");
        let before = fs::metadata(&absolute).expect("stat").permissions();

        vault.write_atomic(&path, b"# After\n").expect("write");

        let after = fs::metadata(&absolute).expect("stat").permissions();
        assert_eq!(
            before, after,
            "the replacement did not inherit the original's permissions"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_note_the_user_restricted_stays_restricted() {
        // The case the cross-platform test cannot make bite: on Windows only the
        // read-only flag exists to carry, so 600-stays-600 is Unix-only.
        use std::os::unix::fs::PermissionsExt as _;

        let (_dir, vault) = vault();
        let path = VaultPath::new("secrets.md").expect("path");
        vault.write_atomic(&path, b"# Secrets\n").expect("write");

        let absolute = vault.resolve(&path).expect("resolve");
        fs::set_permissions(&absolute, fs::Permissions::from_mode(0o600)).expect("chmod");

        vault
            .write_atomic(&path, b"# Still secret\n")
            .expect("write");

        let mode = fs::metadata(&absolute).expect("stat").permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "a note the user chmod'd 600 was published at {mode:o}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_read_only_note_is_not_silently_replaced() {
        // Windows-only, and the reviewer was right that it needed gating. On Unix
        // set_readonly(true) clears the write bits on the file, but rename(2) checks
        // the permissions of the containing directory rather than of the target, so
        // the rename succeeds and this would fail for a reason that says nothing
        // about write_atomic. a_note_the_user_restricted_stays_restricted is the Unix
        // half.
        //
        // What it asserts here: the read-only flag is the whole of what permissions
        // carry on Windows, renaming over a read-only file is refused by the
        // platform, and the caller sees a refusal rather than a silent no-op.
        let (_dir, vault) = vault();
        let path = VaultPath::new("locked.md").expect("path");
        vault.write_atomic(&path, b"# Locked").expect("write");

        let absolute = vault.resolve(&path).expect("resolve");
        let original = fs::metadata(&absolute).expect("stat").permissions();

        let mut locked = original.clone();
        locked.set_readonly(true);
        fs::set_permissions(&absolute, locked).expect("set read-only");

        let refused = vault.write_atomic(&path, b"# Overwritten");

        // Restore by putting back exactly what was there rather than by clearing the
        // read-only flag: set_readonly(false) is world-writable on Unix, which clippy
        // rejects and which would be wrong even here.
        fs::set_permissions(&absolute, original).expect("restore");

        assert!(
            refused.is_err(),
            "a note the user marked read-only was replaced anyway"
        );
        assert_eq!(
            fs::read(&absolute).expect("read"),
            b"# Locked",
            "the original contents did not survive the refused write"
        );
    }

    #[test]
    fn a_new_note_is_created_without_inheriting_anything() {
        // The other side: there is nothing to inherit from, so this must not fail or
        // reach for a default that is not the platform's.
        let (_dir, vault) = vault();
        let path = VaultPath::new("brand-new.md").expect("path");
        vault.write_atomic(&path, b"# New\n").expect("write");
        assert_eq!(vault.read(&path).expect("read").bytes, b"# New\n");
    }

    #[test]
    fn resolve_does_not_mistake_a_new_subdirectory_for_a_symlink() {
        // T-07's control, cross-platform because it needs no real symlink: resolve()
        // checks every path component with symlink_metadata, and a component that
        // does not exist yet must read as "not a symlink" rather than as a refusal,
        // or a write into any subdirectory that has not been created yet would fail.
        let (_dir, vault) = vault();
        let path = VaultPath::new("10-projects/new-subdir/note.md").expect("path");
        vault.write_atomic(&path, b"# New\n").expect("write");
        assert_eq!(vault.read(&path).expect("read").bytes, b"# New\n");
    }

    #[cfg(windows)]
    #[test]
    fn pathbuf_push_of_a_rootless_drive_prefixed_segment_replaces_the_path() {
        // Task #34's actual mechanism, confirmed against the real platform rather
        // than assumed: `PathBuf::push` of a segment like "C:Users" — a drive
        // letter with no root, as opposed to "C:\Users" — discards whatever was
        // accumulated so far instead of extending it. This is exactly what let
        // "a/C:/Windows/x" escape the vault root with no symlink involved before
        // `VaultPath::new` refused the `:` outright. The test documents the `std`
        // behaviour the fix depends on continuing to hold; it does not call any
        // Serat code, because `VaultPath::new`'s guard now makes the vulnerable
        // call into `resolve()` unreachable through the public API.
        let mut current = PathBuf::from(r"C:\vault\a");
        current.push("C:Users");
        assert_eq!(
            current,
            PathBuf::from("C:Users"),
            "PathBuf::push no longer replaces on a rootless drive-prefixed push; \
             re-examine whether the `:` guard in VaultPath::new is still load-bearing"
        );
    }

    /// Creates an NTFS directory junction with `mklink /J`.
    ///
    /// Not `std::os::windows::fs::symlink_dir`: a symlink needs Developer Mode or an
    /// elevated process on Windows, which `threat-model.md`'s T-07 section already
    /// records as the reason the Unix symlink suite below cannot run unconditionally
    /// on this platform. Task #76's probe measured `mklink /J` needing neither — so
    /// this, unlike the tests in `symlink_tests`, can run on an ordinary Windows CI
    /// runner without `#[ignore]` standing in for a privilege check nobody wrote.
    #[cfg(windows)]
    fn make_junction(target: &Path, link: &Path) {
        let status = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .status()
            .expect("running mklink");
        assert!(
            status.success(),
            "mklink /J {} {} failed",
            link.display(),
            target.display()
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_junctioned_directory_component_is_refused() {
        // T-07 / task #76. Architect's probe (threat-model.md, task/75-cfg-gating-
        // ruling) measured, on this platform: `mklink /J` needs no elevation,
        // `symlink_metadata().is_symlink()` reports `true` for the mount point, and
        // reading through it succeeds when nothing refuses it -- so this asserts the
        // conclusion that probe reached rather than re-deriving it, and turns an
        // observation into a check that fails on a regression. Setup reaches the
        // path being tested (F-58): without `resolve`'s component-by-component
        // check, this read returns `Ok` carrying "outside", the escape the check
        // exists to close -- not some incidental error.
        let (dir, vault) = vault();
        let outside = tempfile::tempdir().expect("outside");
        fs::create_dir_all(outside.path().join("nested")).expect("mkdir outside");
        fs::write(outside.path().join("nested/secret.md"), b"outside").expect("write outside");

        make_junction(outside.path(), &dir.path().join("synced"));

        let path = VaultPath::new("synced/nested/secret.md").expect("path");
        let result = vault.read(&path);
        assert!(
            matches!(result, Err(VaultError::PathEscapesVault { .. })),
            "a junctioned ancestor directory was followed instead of refused: {result:?}"
        );
    }
}

// Unix-only: creating a real symlink on Windows needs Developer Mode or an
// elevated process, which a test runner cannot assume. The property under test —
// resolve() refuses any path component that is a symlink — has no platform-specific
// content, so one platform's coverage stands for the mechanism.
#[cfg(all(test, unix))]
mod symlink_tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::open(dir.path()).expect("open vault");
        (dir, vault)
    }

    #[test]
    fn a_symlinked_note_is_refused_rather_than_read() {
        // T-07. Setup reaches the path being tested (F-58): without the fix this read
        // succeeds and returns "outside", which is exactly the escape the fix exists
        // to close, so a broken fix would fail this test by returning Ok(_) with the
        // wrong bytes rather than by some incidental error.
        let (dir, vault) = vault();
        let outside = tempfile::tempdir().expect("outside");
        let secret = outside.path().join("secret.md");
        fs::write(&secret, b"outside").expect("write outside the vault");

        symlink(&secret, dir.path().join("escape.md")).expect("symlink");

        let path = VaultPath::new("escape.md").expect("path");
        let result = vault.read(&path);
        assert!(
            matches!(result, Err(VaultError::PathEscapesVault { .. })),
            "a symlinked note was followed instead of refused: {result:?}"
        );
    }

    #[test]
    fn a_symlinked_directory_component_is_refused() {
        // The case a single lstat on the final component would miss: the file itself
        // is ordinary, but an ancestor directory is the symlink, matching a synced
        // folder or a git submodule inside the vault — exactly the ordinary case the
        // task named, not an exotic one.
        let (dir, vault) = vault();
        let outside = tempfile::tempdir().expect("outside");
        fs::create_dir_all(outside.path().join("nested")).expect("mkdir outside");
        fs::write(outside.path().join("nested/secret.md"), b"outside").expect("write outside");

        symlink(outside.path(), dir.path().join("synced")).expect("symlink dir");

        let path = VaultPath::new("synced/nested/secret.md").expect("path");
        let result = vault.read(&path);
        assert!(
            matches!(result, Err(VaultError::PathEscapesVault { .. })),
            "a symlinked ancestor directory was followed instead of refused: {result:?}"
        );
    }

    #[test]
    fn write_atomic_refuses_a_symlinked_ancestor_before_touching_the_disk() {
        // The write side, mirroring the read side: a symlinked ancestor must be
        // refused before create_dir_all or the temp file, not discovered by writing
        // through the link and then noticing.
        let (dir, vault) = vault();
        let outside = tempfile::tempdir().expect("outside");

        symlink(outside.path(), dir.path().join("synced")).expect("symlink dir");

        let path = VaultPath::new("synced/new.md").expect("path");
        let result = vault.write_atomic(&path, b"# New\n");
        assert!(
            matches!(result, Err(VaultError::PathEscapesVault { .. })),
            "a write through a symlinked ancestor was not refused: {result:?}"
        );
        assert!(
            !outside.path().join("new.md").exists(),
            "the write landed outside the vault despite being refused"
        );
    }
}
