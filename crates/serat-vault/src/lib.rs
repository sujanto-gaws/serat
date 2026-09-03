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

pub mod echo;
pub mod hash;
pub mod ignore;
pub mod queue;
pub mod walk;
pub mod watch;

use std::fs;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serat_core::{ContentHash, NoteKind, NoteMeta, VaultError, VaultPath};

pub use echo::EchoSuppressor;
pub use hash::hash_bytes;
pub use ignore::{IgnoreRules, is_note_file};
pub use queue::{QueueStats, WorkQueue};
pub use walk::VaultEntry;
pub use watch::{VaultEvent, Watcher, WatcherConfig};

/// A file's bytes together with the identity of exactly those bytes.
///
/// The two travel together because a byte range is only meaningful against the bytes
/// it was computed from. Handing a caller bytes without the hash is how a parse
/// result ends up attributed to a file that has since changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedFile {
    /// Identity, hash and stat data.
    pub meta: NoteMeta,
    /// The bytes, exactly as they are on disk. No decoding, no line-ending
    /// translation: the parser records byte offsets into this, and normalising here
    /// would shift every one of them.
    pub bytes: Vec<u8>,
}

/// An open vault: a root directory, and the rules for what inside it counts.
#[derive(Debug, Clone)]
pub struct Vault {
    root: PathBuf,
    ignore: IgnoreRules,
    /// Hashes this vault is about to write, so the watcher can recognise our own
    /// events and drop them (architecture §3.2). Shared rather than owned: the
    /// watcher and the writer must consult the same set, and the writer is whoever
    /// holds a clone of this `Vault`.
    echo: Arc<EchoSuppressor>,
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
        Ok(Self {
            root: strip_verbatim(&canonical),
            ignore,
            echo: Arc::new(EchoSuppressor::new()),
        })
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

    /// The echo suppressor this vault writes through.
    ///
    /// The indexing consumer asks it whether an event carries bytes we just wrote:
    /// `if vault.echo().take_echo(loaded.meta.hash) { skip }` after the hash gate.
    #[must_use]
    pub fn echo(&self) -> &Arc<EchoSuppressor> {
        &self.echo
    }

    /// The absolute path of a vault-relative one.
    #[must_use]
    pub fn resolve(&self, path: &VaultPath) -> PathBuf {
        self.root.join(path.as_str())
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
        VaultPath::new(display(relative))
    }

    /// Every indexable file in the vault, in a deterministic order.
    ///
    /// # Errors
    ///
    /// [`VaultError::Io`] if the root itself cannot be read. A directory that cannot
    /// be read part-way through is skipped rather than fatal: one unreadable folder
    /// must not stop a vault from indexing (design goal 5).
    pub fn walk(&self) -> Result<Vec<VaultEntry>, VaultError> {
        walk::walk(self)
    }

    /// Read a file and hash it.
    ///
    /// # Errors
    ///
    /// [`VaultError::NotFound`] if it is gone, [`VaultError::Io`] otherwise.
    pub fn read(&self, path: &VaultPath) -> Result<LoadedFile, VaultError> {
        let absolute = self.resolve(path);
        let bytes = fs::read(&absolute).map_err(|err| VaultError::io(path.as_str(), &err))?;
        let stat = fs::metadata(&absolute).map_err(|err| VaultError::io(path.as_str(), &err))?;

        Ok(LoadedFile {
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
    ) -> Result<Option<LoadedFile>, VaultError> {
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
    /// [`VaultError::AtomicWriteFailed`] if any step fails, in which case the
    /// original file is untouched.
    pub fn write_atomic(&self, path: &VaultPath, bytes: &[u8]) -> Result<ContentHash, VaultError> {
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
        // Before the write, not after (architecture §3.2, and echo.rs is explicit
        // about it): on a fast filesystem the watcher can deliver the event before a
        // later call would have returned, and an entry recorded too late suppresses
        // nothing.
        self.echo.expect(hash);

        let target = self.resolve(path);
        let parent = target.parent().ok_or_else(|| VaultError::InvalidPath {
            path: path.as_str().to_owned(),
            reason: "has no parent directory".to_owned(),
        })?;
        let failed = |message: String| VaultError::AtomicWriteFailed {
            path: path.as_str().to_owned(),
            message,
        };

        fs::create_dir_all(parent).map_err(|err| failed(err.to_string()))?;

        let temp = unique_temp_path(parent, path.file_name());
        let write_temp = || -> std::io::Result<()> {
            let mut file = fs::File::create(&temp)?;
            file.write_all(bytes)?;
            // Flush before the rename. Without this the rename can be durable while
            // the contents are not, which turns a power cut into a note full of
            // zeroes — the exact outcome the temp file exists to prevent.
            file.sync_all()
        };
        if let Err(err) = write_temp() {
            let _ = fs::remove_file(&temp);
            return Err(failed(err.to_string()));
        }

        if let Err(err) = fs::rename(&temp, &target) {
            let _ = fs::remove_file(&temp);
            return Err(failed(err.to_string()));
        }

        Ok(hash)
    }
}

/// A temporary path beside the target, unique enough that two concurrent writes to
/// the same note cannot collide.
fn unique_temp_path(parent: &Path, file_name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    parent.join(format!("{}{file_name}.{pid}.{nonce}", ignore::TEMP_PREFIX))
}

/// Windows canonicalisation returns `\\?\D:\...`; every path we compare against and
/// every path we show a user is the plain form.
fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\")
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

/// A path for an error message. Lossy on purpose: a path we cannot render is
/// precisely the one an error needs to name.
fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
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
            let absolute = vault.resolve(&path);
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
                !vault.resolve(&path).exists(),
                "{binary} was created despite the refusal"
            );
        }
    }

    #[test]
    fn our_own_write_is_recorded_for_echo_suppression_before_it_lands() {
        // §3.2, and the half that was missing: EchoSuppressor was correct but nothing
        // called it, so the branch did not implement echo suppression at all. Recording
        // must happen before the write — on a fast filesystem the watcher can deliver
        // the event before a later call would have returned.
        let (_dir, vault) = vault();
        let path = VaultPath::new("note.md").expect("path");
        let hash = vault
            .write_atomic(
                &path, b"# Ours
",
            )
            .expect("write");

        assert!(
            vault.echo().take_echo(hash),
            "the writer did not record the hash it was about to produce"
        );
        assert!(
            !vault.echo().take_echo(hash),
            "one write suppressed two events"
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
            !vault.echo().take_echo(theirs.meta.hash),
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
    fn an_absolute_path_inside_the_vault_relativises_to_one_spelling() {
        let (_dir, vault) = vault();
        let path = VaultPath::new("20-systems/indexing.md").expect("path");
        vault.write_atomic(&path, b"body").expect("write");

        let absolute = vault.resolve(&path);
        assert_eq!(vault.relativise(&absolute).expect("relativise"), path);
    }

    #[test]
    fn a_deleted_file_still_relativises() {
        // Delete events arrive after the file is gone, so canonicalisation fails and
        // the lexical path has to be enough. If this stopped working, deletes would
        // never reach the index and it would keep rows for files that no longer
        // exist.
        let (_dir, vault) = vault();
        let path = VaultPath::new("gone.md").expect("path");
        assert_eq!(
            vault.relativise(&vault.resolve(&path)).expect("relativise"),
            path
        );
    }

    #[test]
    fn an_asset_is_read_as_an_asset() {
        // Built with fs::write rather than write_atomic: our own write path refuses a
        // non-Markdown target (T-06), and a fixture that established the opposite as
        // normal usage would undermine the invariant it sits beside.
        let (_dir, vault) = vault();
        let path = VaultPath::new("40-refs/contract.pdf").expect("path");
        let absolute = vault.resolve(&path);
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
