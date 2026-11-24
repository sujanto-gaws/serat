//! The editor-buffer commands, wired: `save_patch`, `request_external_diff`,
//! `rebase_buffer`, `note_opened`, `note_closed` and the `note://external-change`
//! event (ADR-0016; FR-VLT-03, FR-VLT-05, NFR-REL-01, C-04).
//!
//! No domain logic. Applying a patch set under the hash lock is
//! [`Vault::write_patch`]; the edit script behind an external change is
//! [`Vault::diff_against_disk`] and `serat_vault::diff::edit_script`. What this module
//! owns is the **base registry** ADR-0016 §3 asks serat-app to keep: for every note an
//! editor pane holds, the hash and bytes of the text that pane is based on, so a disk
//! change can be answered with an edit script in the pane's own coordinates.
//!
//! # The registry keeps the last few bases, not one
//!
//! `note://external-change` is not ordered against command responses (ADR-0016 §2). A
//! pane can therefore ask `request_external_diff(path, B)` after this registry has
//! already moved on to `B'` because it emitted a change the pane held back. Keeping the
//! last [`KEPT_BASES`] bases per open note means that request is answered with a diff
//! from `B`, which is what the pane needs, instead of with `StaleHash` and a conflict
//! the user never caused. A base this registry does not hold is still `StaleHash`
//! carrying the current disk hash, never a guess.
//!
//! # A change is announced only when its bytes are held
//!
//! Every `ExternalChange::Changed` moves the registry to `new_hash`, because the pane
//! will move its own base there when it applies the edits. That needs the bytes of
//! `new_hash`, so the disk is read once more and the change is announced only if that
//! read hashes to `new_hash`; otherwise the disk moved again, the watcher will report
//! it, and announcing now would hand the pane a base the registry cannot diff from.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use serat_core::{ContentHash, VaultError, VaultPath};
use serat_vault::patch::{Edit, apply_edits};
use serat_vault::{Position, Vault, hash_bytes};
use serat_vault::{diff, phases};

use crate::error::from_vault_error;
use crate::ipc::{
    ExternalChange, FileWritten, IpcError, NoteBufferArgs, Patch, Pos, RebaseBufferArgs,
    SavePatchArgs,
};
use crate::state::Events;

/// Bases kept per open note (see the module doc).
pub const KEPT_BASES: usize = 3;

/// How many times a diff is retried when the disk moves between its two reads.
const DIFF_ATTEMPTS: usize = 3;

/// The text a pane is based on.
#[derive(Debug, Clone)]
pub struct Base {
    /// Its hash.
    pub hash: ContentHash,
    /// Its text. Shared, so keeping several costs the notes' size once each.
    pub text: Arc<str>,
}

#[derive(Debug, Default)]
struct Entry {
    opens: u32,
    /// Oldest first; the last is the newest.
    bases: Vec<Base>,
}

/// The base registry. One lock, held for a map operation and nothing else (no I/O, no
/// other lock), so it is a leaf.
#[derive(Debug, Default)]
pub struct Buffers {
    open: Mutex<HashMap<VaultPath, Entry>>,
}

impl Buffers {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<VaultPath, Entry>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A pane holds `path` at `base`. A second pane on the same note with the same
    /// base shares the entry; a different base becomes the newest.
    pub fn opened(&self, path: &VaultPath, base: Base) {
        let mut open = self.lock();
        let entry = open.entry(path.clone()).or_default();
        entry.opens += 1;
        Self::push_base(entry, base);
    }

    /// A pane released `path`. The last release forgets the note's bases.
    pub fn closed(&self, path: &VaultPath) {
        let mut open = self.lock();
        if let Some(entry) = open.get_mut(path) {
            entry.opens = entry.opens.saturating_sub(1);
            if entry.opens == 0 {
                open.remove(path);
            }
        }
    }

    /// The newest base of an open note.
    #[must_use]
    pub fn newest(&self, path: &VaultPath) -> Option<Base> {
        self.lock().get(path)?.bases.last().cloned()
    }

    /// The base of an open note with this hash.
    #[must_use]
    pub fn find(&self, path: &VaultPath, hash: ContentHash) -> Option<Base> {
        self.lock()
            .get(path)?
            .bases
            .iter()
            .find(|b| b.hash == hash)
            .cloned()
    }

    /// Record a new newest base for a note that is open; a no-op for one that is not.
    pub fn advance(&self, path: &VaultPath, base: Base) {
        if let Some(entry) = self.lock().get_mut(path) {
            Self::push_base(entry, base);
        }
    }

    fn push_base(entry: &mut Entry, base: Base) {
        entry.bases.retain(|b| b.hash != base.hash);
        entry.bases.push(base);
        if entry.bases.len() > KEPT_BASES {
            entry.bases.remove(0);
        }
    }

    /// How many notes are open.
    #[must_use]
    pub fn open_count(&self) -> usize {
        self.lock().len()
    }
}

/// The buffer commands for one open vault.
pub struct Editor {
    vault: Arc<Vault>,
    vault_id: String,
    events: Arc<Events>,
    buffers: Buffers,
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor")
            .field("vault_id", &self.vault_id)
            .finish_non_exhaustive()
    }
}

fn to_edit(patch: &Patch) -> Edit {
    Edit::new(
        Position::new(patch.from.line, patch.from.col),
        Position::new(patch.to.line, patch.to.col),
        patch.insert.clone(),
    )
}

fn to_patch(edit: Edit) -> Patch {
    Patch {
        from: Pos {
            line: edit.from.line,
            col: edit.from.col,
        },
        to: Pos {
            line: edit.to.line,
            col: edit.to.col,
        },
        insert: edit.insert,
    }
}

fn parse_hash(raw: &str) -> Result<ContentHash, IpcError> {
    ContentHash::from_hex(raw).map_err(from_vault_error)
}

fn markdown(path: &str) -> Result<VaultPath, IpcError> {
    let path = VaultPath::new(path).map_err(from_vault_error)?;
    if !path.is_markdown() {
        return Err(IpcError::NotWritable {
            path: path.to_string(),
        });
    }
    Ok(path)
}

impl Editor {
    /// The editor for `vault`, announcing on `events`.
    #[must_use]
    pub fn new(vault: Arc<Vault>, vault_id: String, events: Arc<Events>) -> Arc<Self> {
        Arc::new(Self {
            vault,
            vault_id,
            events,
            buffers: Buffers::default(),
        })
    }

    /// The registry, for tests and for the live bridge.
    #[must_use]
    pub fn buffers(&self) -> &Buffers {
        &self.buffers
    }

    /// `note_opened`. The pane read `args.base_hash`; the registry needs its bytes, so
    /// the file is read now and must still hash to it.
    ///
    /// # Errors
    ///
    /// [`IpcError::StaleHash`] (with the disk's hash) if the file changed since the
    /// pane read it, so the pane re-reads rather than being based on bytes nobody
    /// holds; [`IpcError::NotWritable`] for a non-Markdown path; [`IpcError::NotUtf8`].
    pub fn note_opened(&self, args: &NoteBufferArgs) -> Result<(), IpcError> {
        let path = markdown(&args.path)?;
        let hash = parse_hash(&args.base_hash)?;
        let loaded = self.vault.read(&path).map_err(from_vault_error)?;
        if loaded.meta.hash != hash {
            return Err(IpcError::StaleHash {
                path: path.to_string(),
                current_hash: loaded.meta.hash.to_hex(),
            });
        }
        let text = String::from_utf8(loaded.bytes).map_err(|_| IpcError::NotUtf8 {
            path: path.to_string(),
        })?;
        self.buffers.opened(
            &path,
            Base {
                hash,
                text: text.into(),
            },
        );
        Ok(())
    }

    /// `note_closed`. A path that is not valid, or not open, is nothing to release.
    pub fn note_closed(&self, args: &NoteBufferArgs) {
        if let Ok(path) = VaultPath::new(&args.path) {
            self.buffers.closed(&path);
        }
    }

    /// `save_patch`, up to and including the write. The caller submits the note to the
    /// index afterwards, as for any write.
    ///
    /// **`request_external_diff` is the only diff path for a refused save.** A
    /// `StaleHash` returns the error and nothing else: the pane always calls
    /// `request_external_diff` and does not wait for an event (ADR-0016 §2), so an
    /// event here would only duplicate the answer.
    ///
    /// On success the registry's newest base for the note becomes the new hash. The new
    /// base's bytes are computed from the held base and the patches with the same
    /// `apply_edits` the write used, not read back: an external write landing between
    /// the rename and a read-back must not leave the registry without bytes Serat
    /// itself wrote. They are advanced only if they hash to what `write_patch`
    /// returned.
    ///
    /// # Errors
    ///
    /// [`IpcError::StaleHash`], [`IpcError::InvalidPatch`],
    /// [`IpcError::LineEndingsWouldFuse`], [`IpcError::NotWritable`], and the rest of
    /// [`from_vault_error`].
    pub fn save_patch(&self, args: &SavePatchArgs) -> Result<(VaultPath, FileWritten), IpcError> {
        let path = markdown(&args.path)?;
        let base_hash = parse_hash(&args.base_hash)?;
        let edits: Vec<Edit> = args.patches.iter().map(to_edit).collect();
        // Taken before the write: the registry may be moved by a watcher event while
        // the write runs.
        let held = self.buffers.find(&path, base_hash);
        phases::mark("app.prepare");

        let wrote = self
            .vault
            .write_patch(&path, base_hash, &edits)
            .map_err(from_vault_error)?;
        let new_hash = wrote.hash;
        phases::mark("app.write_patch_return");

        if let Some(base) = held {
            let applied = apply_edits(&base.text, &edits, path.as_str());
            phases::mark("app.apply_edits_again");
            if let Ok(text) = applied {
                let matches = hash_bytes(text.as_bytes()) == new_hash;
                phases::mark("app.hash_again");
                if matches {
                    self.buffers.advance(
                        &path,
                        Base {
                            hash: new_hash,
                            text: text.into(),
                        },
                    );
                }
            }
            phases::mark("app.advance_base");
        }
        // Hash, mtime and size all come from the write itself (ADR-0015 §2): the
        // mtime was statted under the path lock right after the rename, so it is never
        // newer than the bytes the hash describes. No second read or stat here.
        let written = FileWritten {
            path: path.to_string(),
            hash: new_hash.to_hex(),
            mtime_ms: wrote.mtime_ms,
            bytes: wrote.size,
        };
        Ok((path, written))
    }

    /// `request_external_diff`.
    ///
    /// # Errors
    ///
    /// [`IpcError::StaleHash`] with the current disk hash when no base with
    /// `args.base_hash` is held (the note was closed and reopened, or a newer base
    /// replaced it) or the disk kept moving while it was diffed; never a guess.
    pub fn request_external_diff(
        &self,
        args: &NoteBufferArgs,
    ) -> Result<Option<ExternalChange>, IpcError> {
        let path = markdown(&args.path)?;
        let hash = parse_hash(&args.base_hash)?;
        match self.buffers.find(&path, hash) {
            Some(base) => self.external_change(&path, &base),
            None => Err(self.stale_against_disk(&path)),
        }
    }

    fn stale_against_disk(&self, path: &VaultPath) -> IpcError {
        match self.vault.read(path) {
            Ok(loaded) => IpcError::StaleHash {
                path: path.to_string(),
                current_hash: loaded.meta.hash.to_hex(),
            },
            Err(err) => from_vault_error(err),
        }
    }

    /// The change from `base` to the disk as it is now, moving the registry to the new
    /// hash; `None` when they are equal; `Deleted` when the file is gone.
    fn external_change(
        &self,
        path: &VaultPath,
        base: &Base,
    ) -> Result<Option<ExternalChange>, IpcError> {
        for _ in 0..DIFF_ATTEMPTS {
            match self.vault.diff_against_disk(path, base.hash, &base.text) {
                Ok(None) => return Ok(None),
                Ok(Some((new_hash, script))) => {
                    let Ok(loaded) = self.vault.read(path) else {
                        continue;
                    };
                    if loaded.meta.hash != new_hash {
                        continue;
                    }
                    let Ok(text) = String::from_utf8(loaded.bytes) else {
                        continue;
                    };
                    self.buffers.advance(
                        path,
                        Base {
                            hash: new_hash,
                            text: text.into(),
                        },
                    );
                    return Ok(Some(ExternalChange::Changed {
                        vault_id: self.vault_id.clone(),
                        path: path.to_string(),
                        base_hash: base.hash.to_hex(),
                        new_hash: new_hash.to_hex(),
                        edits: script.edits.into_iter().map(to_patch).collect(),
                        whole: script.whole,
                    }));
                }
                Err(VaultError::NotFound { .. }) => {
                    return Ok(Some(ExternalChange::Deleted {
                        vault_id: self.vault_id.clone(),
                        path: path.to_string(),
                    }));
                }
                Err(err) => return Err(from_vault_error(err)),
            }
        }
        Err(self.stale_against_disk(path))
    }

    /// `rebase_buffer`: ADR-0016 §4's *Keep mine*. The patches that turn the disk text
    /// (which must still hash to `args.disk_hash`) into `args.buffer`, in disk
    /// coordinates. The disk text becomes the registry's newest base, because the pane
    /// makes it its own.
    ///
    /// # Errors
    ///
    /// [`IpcError::StaleHash`] if the disk is no longer `args.disk_hash`.
    pub fn rebase_buffer(&self, args: &RebaseBufferArgs) -> Result<Vec<Patch>, IpcError> {
        let path = markdown(&args.path)?;
        let disk_hash = parse_hash(&args.disk_hash)?;
        let loaded = self.vault.read(&path).map_err(from_vault_error)?;
        if loaded.meta.hash != disk_hash {
            return Err(IpcError::StaleHash {
                path: path.to_string(),
                current_hash: loaded.meta.hash.to_hex(),
            });
        }
        let disk = String::from_utf8(loaded.bytes).map_err(|_| IpcError::NotUtf8 {
            path: path.to_string(),
        })?;
        let script = diff::edit_script(&disk, &args.buffer);
        self.buffers.advance(
            &path,
            Base {
                hash: disk_hash,
                text: disk.into(),
            },
        );
        Ok(script.edits.into_iter().map(to_patch).collect())
    }

    /// The watcher reported `path` changed. If a pane holds it, announce the change.
    /// Our own save normally produces no event: the registry holds the saved hash. If the
    /// watcher wins the race and reports first, the pane drops that change on its base
    /// check (ADR-0016 §2) and the next save or `request_external_diff` settles it.
    pub fn path_changed(&self, path: &VaultPath) {
        let Some(base) = self.buffers.newest(path) else {
            return;
        };
        if let Ok(Some(change)) = self.external_change(path, &base) {
            self.events.emit_external(&change);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    struct Fixture {
        dir: tempfile::TempDir,
        editor: Arc<Editor>,
        events: Arc<Mutex<Vec<ExternalChange>>>,
    }

    fn fixture(files: &[(&str, &[u8])]) -> Fixture {
        let dir = tempfile::tempdir().expect("dir");
        for (name, bytes) in files {
            fs::write(dir.path().join(name), bytes).expect("write");
        }
        let vault = Arc::new(Vault::open(dir.path()).expect("open"));
        let sinks = Arc::new(Events::default());
        let events: Arc<Mutex<Vec<ExternalChange>>> = Arc::default();
        let log = Arc::clone(&events);
        sinks.install_external(move |change| log.lock().expect("log").push(change.clone()));
        Fixture {
            dir,
            editor: Editor::new(vault, "v1".to_owned(), sinks),
            events,
        }
    }

    fn hash_of(dir: &Path, name: &str) -> String {
        serat_vault::hash_bytes(&fs::read(dir.join(name)).expect("read")).to_hex()
    }

    fn args(name: &str, hash: &str) -> NoteBufferArgs {
        NoteBufferArgs {
            path: name.to_owned(),
            base_hash: hash.to_owned(),
        }
    }

    fn patch(from: (u32, u32), to: (u32, u32), insert: &str) -> Patch {
        Patch {
            from: Pos {
                line: from.0,
                col: from.1,
            },
            to: Pos {
                line: to.0,
                col: to.1,
            },
            insert: insert.to_owned(),
        }
    }

    fn save(name: &str, hash: &str, patches: Vec<Patch>) -> SavePatchArgs {
        SavePatchArgs {
            path: name.to_owned(),
            base_hash: hash.to_owned(),
            patches,
        }
    }

    fn open(fx: &Fixture, name: &str) -> String {
        let hash = hash_of(fx.dir.path(), name);
        fx.editor.note_opened(&args(name, &hash)).expect("opened");
        hash
    }

    #[test]
    fn file_written_carries_the_vaults_own_hash_mtime_and_size_without_a_reread() {
        // ADR-0015 section 2 / #71. The note was never opened, so no held base: size
        // must still come from the write, not from a read-back. The mtime must be the
        // one the file has right after the write (nothing touches it in between).
        let fx = fixture(&[("a.md", b"abc\nrest\n")]);
        let base = hash_of(fx.dir.path(), "a.md");
        let (_, written) = fx
            .editor
            .save_patch(&save("a.md", &base, vec![patch((0, 0), (0, 3), "WXYZ")]))
            .expect("saved");
        let on_disk = fs::read(fx.dir.path().join("a.md")).expect("read");
        let stat = fs::metadata(fx.dir.path().join("a.md")).expect("stat");
        let mtime = stat
            .modified()
            .expect("mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("epoch")
            .as_millis() as i64;
        assert_eq!(written.hash, serat_vault::hash_bytes(&on_disk).to_hex());
        assert_eq!(written.bytes, on_disk.len() as u64);
        assert_eq!(written.mtime_ms, mtime);
    }

    #[test]
    fn a_save_patches_the_bytes_moves_the_base_and_its_own_echo_says_nothing() {
        let fx = fixture(&[("a.md", b"# one\r\ntwo\r\n")]);
        let base = open(&fx, "a.md");

        let (path, written) = fx
            .editor
            .save_patch(&save("a.md", &base, vec![patch((1, 0), (1, 3), "2\nmore")]))
            .expect("saved");
        assert_eq!(path.as_str(), "a.md");
        // C-04: the CRLF endings are the note's own, and nothing else moved.
        assert_eq!(
            fs::read(fx.dir.path().join("a.md")).expect("read"),
            b"# one\r\n2\r\nmore\r\n"
        );
        assert_eq!(written.hash, hash_of(fx.dir.path(), "a.md"));
        assert_eq!(written.bytes, 16);

        // The watcher's event for our own write: the registry already holds the hash.
        fx.editor.path_changed(&path);
        assert!(fx.events.lock().expect("log").is_empty());

        // And the next save is against the new base, not a stale one.
        fx.editor
            .save_patch(&save(
                "a.md",
                &written.hash,
                vec![patch((0, 0), (0, 0), ">")],
            ))
            .expect("second save");
    }

    #[test]
    fn a_stale_save_is_only_an_error_and_request_external_diff_is_the_one_diff_path() {
        let fx = fixture(&[("a.md", b"one\ntwo\nthree\n")]);
        let base = open(&fx, "a.md");
        fs::write(fx.dir.path().join("a.md"), b"one\nTWO\nthree\n").expect("external");

        let err = fx
            .editor
            .save_patch(&save("a.md", &base, vec![patch((2, 0), (2, 5), "3")]))
            .expect_err("stale");
        assert!(matches!(err, IpcError::StaleHash { .. }), "{err:?}");
        assert_eq!(
            fs::read(fx.dir.path().join("a.md")).expect("read"),
            b"one\nTWO\nthree\n",
            "a refused save touches nothing"
        );
        assert!(
            fx.events.lock().expect("log").is_empty(),
            "no duplicate event: the pane always asks"
        );

        let answer = fx
            .editor
            .request_external_diff(&args("a.md", &base))
            .expect("diff")
            .expect("changed");
        let ExternalChange::Changed {
            base_hash,
            new_hash,
            edits,
            whole,
            ..
        } = answer
        else {
            panic!("expected Changed");
        };
        assert_eq!(base_hash, base);
        assert_eq!(new_hash, hash_of(fx.dir.path(), "a.md"));
        assert!(!whole);
        assert_eq!(edits, vec![patch((1, 0), (1, 3), "TWO")]);
    }

    #[test]
    fn the_new_base_is_ours_even_if_the_file_moves_again_right_after_the_save() {
        let fx = fixture(&[("a.md", b"one\ntwo\n")]);
        let base = open(&fx, "a.md");
        let (_, written) = fx
            .editor
            .save_patch(&save("a.md", &base, vec![patch((0, 0), (0, 3), "ONE")]))
            .expect("saved");
        // An external write lands after the rename. The registry still holds the bytes
        // Serat wrote, so the diff from the saved hash is a real one, not a conflict.
        fs::write(fx.dir.path().join("a.md"), b"ONE\nTWO\n").expect("external");
        let change = fx
            .editor
            .request_external_diff(&args("a.md", &written.hash))
            .expect("held")
            .expect("changed");
        assert!(matches!(
            change,
            ExternalChange::Changed { ref edits, .. } if edits == &vec![patch((1, 0), (1, 3), "TWO")]
        ));
    }

    #[test]
    fn a_base_nobody_holds_is_stale_with_the_disk_hash_never_a_guess() {
        let fx = fixture(&[("a.md", b"x\n")]);
        let held = open(&fx, "a.md");
        let disk = hash_of(fx.dir.path(), "a.md");
        let unknown = "0".repeat(64);
        match fx.editor.request_external_diff(&args("a.md", &unknown)) {
            Err(IpcError::StaleHash { current_hash, .. }) => assert_eq!(current_hash, disk),
            other => panic!("expected StaleHash, got {other:?}"),
        }
        // Closed: the bytes are released, so even the once-held hash is now unknown.
        fx.editor.note_closed(&args("a.md", &held));
        assert!(matches!(
            fx.editor.request_external_diff(&args("a.md", &held)),
            Err(IpcError::StaleHash { .. })
        ));
        assert_eq!(fx.editor.buffers().open_count(), 0);
    }

    #[test]
    fn an_external_edit_is_announced_once_in_base_coordinates() {
        let fx = fixture(&[("a.md", b"l0\nl1\nl2\nl3\nl4\n")]);
        let base = open(&fx, "a.md");
        fs::write(fx.dir.path().join("a.md"), b"L0\nl1\nl2\nl3\nl4\n").expect("external");
        let path = VaultPath::new("a.md").expect("path");
        fx.editor.path_changed(&path);
        fx.editor.path_changed(&path);

        let events = fx.events.lock().expect("log");
        assert_eq!(events.len(), 1, "the second report finds nothing new");
        assert!(matches!(
            &events[0],
            ExternalChange::Changed { base_hash, edits, .. }
                if base_hash == &base && edits == &vec![patch((0, 0), (0, 1), "L")]
        ));
    }

    #[test]
    fn a_line_ending_only_change_is_emitted_with_no_edits() {
        let fx = fixture(&[("a.md", b"a\nb\n")]);
        open(&fx, "a.md");
        fs::write(fx.dir.path().join("a.md"), b"a\r\nb\r\n").expect("external");
        fx.editor
            .path_changed(&VaultPath::new("a.md").expect("path"));
        let events = fx.events.lock().expect("log");
        assert!(matches!(
            events.as_slice(),
            [ExternalChange::Changed { edits, new_hash, .. }]
                if edits.is_empty() && new_hash == &hash_of(fx.dir.path(), "a.md")
        ));
    }

    #[test]
    fn a_deleted_note_is_announced_and_a_note_nobody_holds_is_not_read() {
        let fx = fixture(&[("a.md", b"x\n"), ("b.md", b"y\n")]);
        open(&fx, "a.md");
        fs::remove_file(fx.dir.path().join("a.md")).expect("rm");
        fs::remove_file(fx.dir.path().join("b.md")).expect("rm");
        fx.editor
            .path_changed(&VaultPath::new("a.md").expect("path"));
        fx.editor
            .path_changed(&VaultPath::new("b.md").expect("path"));
        let events = fx.events.lock().expect("log");
        assert!(matches!(
            events.as_slice(),
            [ExternalChange::Deleted { path, .. }] if path == "a.md"
        ));
    }

    #[test]
    fn a_note_that_changed_since_it_was_read_cannot_be_opened_on_that_base() {
        let fx = fixture(&[("a.md", b"x\n")]);
        let read = hash_of(fx.dir.path(), "a.md");
        fs::write(fx.dir.path().join("a.md"), b"x changed\n").expect("external");
        match fx.editor.note_opened(&args("a.md", &read)) {
            Err(IpcError::StaleHash { current_hash, .. }) => {
                assert_eq!(current_hash, hash_of(fx.dir.path(), "a.md"));
            }
            other => panic!("expected StaleHash, got {other:?}"),
        }
        assert_eq!(fx.editor.buffers().open_count(), 0);
    }

    #[test]
    fn keep_mine_rebases_onto_the_disk_and_returns_patches_in_disk_coordinates() {
        let fx = fixture(&[("a.md", b"one\ntwo\nthree\n")]);
        let _ = open(&fx, "a.md");
        fs::write(fx.dir.path().join("a.md"), b"one\nTWO\nthree\n").expect("external");
        let disk = hash_of(fx.dir.path(), "a.md");

        let patches = fx
            .editor
            .rebase_buffer(&RebaseBufferArgs {
                path: "a.md".to_owned(),
                disk_hash: disk.clone(),
                buffer: "one\nmine\nthree\n".to_owned(),
            })
            .expect("rebased");
        assert_eq!(patches, vec![patch((1, 0), (1, 3), "mine")]);

        // The next save turns the disk into exactly the buffer.
        fx.editor
            .save_patch(&save("a.md", &disk, patches))
            .expect("saved");
        assert_eq!(
            fs::read(fx.dir.path().join("a.md")).expect("read"),
            b"one\nmine\nthree\n"
        );

        // A disk that moved again is a StaleHash, not a diff against the wrong text.
        fs::write(fx.dir.path().join("a.md"), b"moved\n").expect("external");
        assert!(matches!(
            fx.editor.rebase_buffer(&RebaseBufferArgs {
                path: "a.md".to_owned(),
                disk_hash: disk,
                buffer: String::new(),
            }),
            Err(IpcError::StaleHash { .. })
        ));
    }

    #[test]
    fn the_boundary_rules_hold_for_every_buffer_command() {
        let fx = fixture(&[("a.md", b"x\n"), ("b.pdf", b"%PDF")]);
        let base = hash_of(fx.dir.path(), "a.md");
        // Rule 3: only Markdown is writable (T-06), refused before any read.
        assert!(matches!(
            fx.editor.save_patch(&save("b.pdf", &base, vec![])),
            Err(IpcError::NotWritable { .. })
        ));
        // Rule 2: a path leaving the vault is refused by name.
        assert!(matches!(
            fx.editor.save_patch(&save("../a.md", &base, vec![])),
            Err(IpcError::PathEscapesVault { .. })
        ));
        // A client bug is its own variant and the file is untouched.
        assert!(matches!(
            fx.editor
                .save_patch(&save("a.md", &base, vec![patch((9, 0), (9, 1), "x")])),
            Err(IpcError::InvalidPatch { .. })
        ));
        assert!(matches!(
            fx.editor.save_patch(&save("a.md", "not-hex", vec![])),
            Err(IpcError::MalformedHash { .. })
        ));
        assert_eq!(fs::read(fx.dir.path().join("a.md")).expect("read"), b"x\n");
    }

    #[test]
    fn only_the_last_few_bases_are_kept_per_note() {
        let buffers = Buffers::default();
        let path = VaultPath::new("a.md").expect("path");
        let base = |n: u8| Base {
            hash: serat_vault::hash_bytes(&[n]),
            text: String::from("t").into(),
        };
        buffers.opened(&path, base(0));
        for n in 1..=5 {
            buffers.advance(&path, base(n));
        }
        assert!(buffers.find(&path, base(0).hash).is_none());
        assert!(buffers.find(&path, base(2).hash).is_none());
        assert!(buffers.find(&path, base(3).hash).is_some());
        assert_eq!(buffers.newest(&path).map(|b| b.hash), Some(base(5).hash));
        // A note nobody holds is not remembered by advancing it.
        let other = VaultPath::new("z.md").expect("path");
        buffers.advance(&other, base(1));
        assert!(buffers.newest(&other).is_none());
    }

    // Task #172: per-phase timing of `save_patch` (NFR-PRF-03 evidence, US-01). The phases
    // observe the write and change nothing in it (C-04).

    const SAVE_PHASES: [&str; 15] = [
        "app.prepare",
        "vault.resolve",
        "vault.path_lock",
        "vault.read_base",
        "vault.hash_before",
        "vault.apply_edits",
        "vault.hash_after",
        "vault.stat_permissions",
        "vault.write_temp_fsync",
        "vault.reread",
        "vault.hash_reread",
        "vault.rename_syncdir",
        "app.write_patch_return",
        "app.apply_edits_again",
        "app.hash_again",
    ];

    #[test]
    fn a_timed_save_reports_every_phase_in_call_order_and_they_sum_to_the_call() {
        const {
            assert!(
                phases::PHASE_TIMING,
                "serat-app's dev-dependencies turn it on"
            )
        };
        let fx = fixture(&[("a.md", b"# one\ntwo\n")]);
        let base = open(&fx, "a.md");
        let started = std::time::Instant::now();
        phases::begin();
        let (_, written) = fx
            .editor
            .save_patch(&save("a.md", &base, vec![patch((1, 0), (1, 3), "2")]))
            .expect("saved");
        let total = started.elapsed();
        let phases = phases::finish();
        let names: Vec<&str> = phases.iter().map(|p| p.name).collect();
        let mut expected = SAVE_PHASES.to_vec();
        expected.push("app.advance_base");
        assert_eq!(names, expected);
        let sum: std::time::Duration = phases.iter().map(|p| p.elapsed).sum();
        assert!(sum <= total, "{sum:?} of {total:?}");
        // What is outside every phase is the two `Instant::now` calls and the return.
        assert!(
            total - sum < std::time::Duration::from_millis(5),
            "{sum:?} of {total:?}"
        );
        // The timing watched the write; it did not change it.
        assert_eq!(
            fs::read(fx.dir.path().join("a.md")).expect("read"),
            b"# one\n2\n"
        );
        assert_eq!(written.hash, hash_of(fx.dir.path(), "a.md"));
    }

    #[test]
    fn a_save_with_no_held_base_has_no_second_apply_and_an_untimed_save_records_nothing() {
        let fx = fixture(&[("a.md", b"abc\n")]);
        let base = hash_of(fx.dir.path(), "a.md");
        phases::begin();
        let (_, written) = fx
            .editor
            .save_patch(&save("a.md", &base, vec![patch((0, 0), (0, 3), "x")]))
            .expect("saved");
        let names: Vec<&str> = phases::finish().iter().map(|p| p.name).collect();
        let mut expected = SAVE_PHASES[..12].to_vec();
        expected.push("app.write_patch_return");
        assert_eq!(names, expected);
        // Without `begin`, the same call leaves nothing behind for a later `finish`.
        fx.editor
            .save_patch(&save(
                "a.md",
                &written.hash,
                vec![patch((0, 0), (0, 1), "y")],
            ))
            .expect("saved again");
        assert!(phases::finish().is_empty());
    }
}
