//! The one temp-file, fsync and rename implementation, and the per-path write lock
//! (ADR-0008, ADR-0016 §2; NFR-REL-01, NFR-SEC-06, FR-VLT-03, T-07).
//!
//! Every byte this crate writes goes through [`write_temp`] and [`commit`]: the note
//! write paths in `lib.rs` and the two named T-06 exceptions in `gitignore.rs`. There
//! used to be a second copy of this logic in `serat-app`; two copies drift.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher as _, Hasher as _};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::ignore::TEMP_PREFIX;

/// How many fresh names to try before giving up on `AlreadyExists`.
const NAME_ATTEMPTS: u32 = 8;

/// A name beside the target that nobody can have planted in advance.
///
/// The nonce is 64 bits from `RandomState`, which std seeds from the OS per process, so
/// it is not a function of the pid, the clock or a counter. Combined with
/// [`write_exclusive`] this is T-07's rule applied to temp files: a symlink planted in a
/// synced vault at a guessed temp name cannot redirect the write.
fn temp_name(parent: &Path, file_name: &str) -> PathBuf {
    let nonce = RandomState::new().build_hasher().finish();
    parent.join(format!("{TEMP_PREFIX}{file_name}.{nonce:016x}"))
}

/// Create `temp` exclusively (`O_EXCL`, which does not follow a symlink) and write it
/// durably. Fails with `AlreadyExists` if anything, a symlink included, is at that name.
/// Returns the mtime (unix ms) of the open handle after the final sync: the mtime these
/// bytes carry, which a rename preserves (ADR-0015 section 2).
pub(crate) fn write_exclusive(temp: &Path, bytes: &[u8]) -> std::io::Result<i64> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)?;
    file.write_all(bytes)?;
    crate::save_phases::mark(crate::save_phases::SavePhase::Write);
    // Flush before the rename. Without this the rename can be durable while the
    // contents are not, which turns a power cut into a note full of zeroes.
    sync_all(&file, crate::save_phases::Fsync::File)?;
    crate::save_phases::mark(crate::save_phases::SavePhase::Sync);
    Ok(crate::walk::mtime_ms(&file.metadata()?))
}

/// Write `bytes` to a new temp file in `parent` and return its path and the mtime (unix
/// ms) of the bytes, taken from the open handle before any rename. Rename preserves
/// mtime (POSIX `rename`, Windows `MoveFileEx`), so this mtime always predates the
/// moment our bytes became visible under the target name: a later external write can
/// only carry a later-or-equal mtime, never be mistaken for ours (ADR-0015 section 2).
///
/// When `permissions` is given the temp file gets them (F-33): a temp-and-rename write
/// creates a new file, and a new file would otherwise publish a note the user made
/// private at the process default. On failure the temp file is removed, but only if
/// this call created it: a name that already existed is never touched.
pub(crate) fn write_temp(
    parent: &Path,
    file_name: &str,
    bytes: &[u8],
    permissions: Option<fs::Permissions>,
) -> std::io::Result<(PathBuf, i64)> {
    let mut last = None;
    for _ in 0..NAME_ATTEMPTS {
        let temp = temp_name(parent, file_name);
        match write_exclusive(&temp, bytes) {
            Ok(mtime_ms) => {
                if let Some(permissions) = permissions
                    && let Err(err) = fs::set_permissions(&temp, permissions)
                {
                    let _ = fs::remove_file(&temp);
                    return Err(err);
                }
                return Ok((temp, mtime_ms));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => last = Some(err),
            Err(err) => {
                let _ = fs::remove_file(&temp);
                return Err(err);
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no free temporary name")))
}

/// Rename `temp` over `target` and make the rename durable. On failure `temp` is
/// removed and the original is untouched.
pub(crate) fn commit(temp: &Path, target: &Path) -> std::io::Result<()> {
    if let Err(err) = fs::rename(temp, target) {
        let _ = fs::remove_file(temp);
        return Err(err);
    }
    if let Some(parent) = target.parent() {
        sync_dir(parent);
    }
    Ok(())
}

/// The one place a durability flush is issued. In a release build this is `sync_all`;
/// under `cfg(test)` it also appends to the save-phase event log, so a test can pin how
/// many fsyncs a save issues and in which phase they fall (NFR-PRF-03, ADR-0019 second
/// amendment, TC-185). It changes no byte written.
#[cfg_attr(not(test), allow(unused_variables))]
fn sync_all(file: &fs::File, what: crate::save_phases::Fsync) -> std::io::Result<()> {
    #[cfg(test)]
    crate::save_phases::log_fsync(what);
    file.sync_all()
}

/// fsync the directory so the rename itself survives a power cut. Best effort: the
/// rename has happened and is visible, so a failure here is not a failed write, and
/// Windows has no directory handle to sync.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = fs::File::open(dir) {
        let _ = sync_all(&handle, crate::save_phases::Fsync::Dir);
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// The in-process write locks: one mutex per path, created on demand and removed when
/// the last holder or waiter lets go, so the map is bounded by concurrent writes
/// (ADR-0016 §2).
#[derive(Debug, Default)]
pub(crate) struct PathLocks {
    map: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

/// A handle on one path's mutex. Call [`PathLock::hold`] for the guard; declare the
/// `PathLock` first so the guard drops before the entry is considered for removal.
pub(crate) struct PathLock<'a> {
    locks: &'a PathLocks,
    key: String,
    mutex: Arc<Mutex<()>>,
}

impl PathLocks {
    /// The lock for `path`. The key is case-folded with `serat_core::fold_case`, the same fold link resolution uses: on a case-insensitive filesystem
    /// `A.md` and `a.md` are one file, and over-serialising on a case-sensitive one
    /// costs nothing.
    pub(crate) fn lock_for(&self, path: &str) -> PathLock<'_> {
        let key = serat_core::fold_case(path);
        let mut map = self
            .map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mutex = Arc::clone(map.entry(key.clone()).or_default());
        PathLock {
            locks: self,
            key,
            mutex,
        }
    }

    /// Entries currently in the map.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

impl PathLock<'_> {
    /// Block until this path has no other in-process writer. A writer that panicked
    /// while holding it does not wedge the path: there is no state behind the mutex.
    pub(crate) fn hold(&self) -> MutexGuard<'_, ()> {
        self.mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for PathLock<'_> {
    fn drop(&mut self) {
        let mut map = self
            .locks
            .map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The map's own reference and this one: nobody else holds or waits.
        if Arc::strong_count(&self.mutex) == 2 {
            map.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::VaultPath;
    use crate::save_phases::{Event, Fsync, SavePhase, drain_log};

    /// NFR-PRF-03, ADR-0019 second amendment (task #211), TC-185: the vault-I/O excusal
    /// counts `write + sync + rename` as the vault's file I/O, on the premise that `sync`
    /// holds the one file fsync and `rename` the one directory fsync (unix only). This
    /// pins that: a second fsync before the Write mark, or a dropped directory sync,
    /// changes what the phases mean and fails here.
    #[test]
    fn a_save_fsyncs_the_file_in_sync_and_on_unix_the_directory_in_rename_and_nothing_else() {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = crate::Vault::open(dir.path()).expect("open vault");
        let path = VaultPath::new("note.md").expect("path");
        drain_log();
        vault.write_atomic(&path, b"# one\n").expect("write");
        let log = drain_log();
        #[cfg(unix)]
        let expected = vec![
            Event::Mark(SavePhase::Hash),
            Event::Mark(SavePhase::Write),
            Event::Fsync(Fsync::File),
            Event::Mark(SavePhase::Sync),
            Event::Fsync(Fsync::Dir),
            Event::Mark(SavePhase::Rename),
        ];
        #[cfg(not(unix))]
        let expected = vec![
            Event::Mark(SavePhase::Hash),
            Event::Mark(SavePhase::Write),
            Event::Fsync(Fsync::File),
            Event::Mark(SavePhase::Sync),
            Event::Mark(SavePhase::Rename),
        ];
        assert_eq!(log, expected);
    }

    /// Test files that are `#[cfg(test)]` modules in their entirety (declared so in lib.rs).
    const TEST_ONLY_FILES: [&str; 3] = [
        "ignore_file_tests.rs",
        "patch_props.rs",
        "write_race_tests.rs",
    ];

    /// The code of one source file as the scan sees it: line comments stripped (a `//`
    /// at the start of a line or after whitespace, so a URL in a string survives) and
    /// every column-0 test module (`mod x {` whose `#[cfg(..)]` line mentions `test`)
    /// skipped up to its closing column-0 `}`. Scanning continues after it, and a module
    /// cfg'd on anything else (a platform, a feature) is scanned as ordinary code.
    fn production_code(text: &str) -> String {
        let lines: Vec<&str> = text.lines().collect();
        let mut out = String::new();
        let mut in_test_mod = false;
        for (i, line) in lines.iter().enumerate() {
            if in_test_mod {
                in_test_mod = *line != "}";
                continue;
            }
            if line.starts_with("mod ")
                && line.trim_end().ends_with('{')
                && i > 0
                && lines[i - 1].trim_start().starts_with("#[cfg(")
                && lines[i - 1].contains("test")
            {
                in_test_mod = true;
                continue;
            }
            let code = match line.find("//") {
                Some(at) if at == 0 || line[..at].ends_with(char::is_whitespace) => &line[..at],
                _ => line,
            };
            out.push_str(code);
            out.push('\n');
        }
        out
    }

    /// NFR-PRF-03, ADR-0019 second amendment guard (i), TC-185: the fsync pin only sees
    /// flushes routed through [`super::sync_all`], so that helper must be the only place
    /// in the crate's production code that flushes. A method call (`.sync_all(`), a
    /// qualified one (`File::sync_data(`), `fdatasync(`, `fsync(` or `FlushFileBuffers(`
    /// anywhere else, or a second flush in `atomic.rs`, changes what the `sync` and
    /// `rename` phases mean without the sequence test noticing; this fails it instead.
    /// A bare `sync_all(` call is the helper itself, allowed only in `atomic.rs`.
    #[test]
    fn the_logging_helper_is_the_only_place_the_vault_flushes_to_disk() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&src).expect("src dir").flatten() {
            let path = entry.path();
            // `read_dir` is not recursive: a subdirectory would be unscanned, so refuse it.
            assert!(
                !path.is_dir(),
                "{} is a directory; make this scan recursive",
                path.display()
            );
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.extension().is_none_or(|e| e != "rs")
                || TEST_ONLY_FILES.contains(&name.as_str())
            {
                continue;
            }
            let mut code = production_code(&std::fs::read_to_string(&path).expect("readable"));
            if name == "atomic.rs" {
                // The helper's own body is the one allowed call.
                let at = code.find("fn sync_all(").expect("the helper");
                let end = at + code[at..].find("\n}\n").expect("its end");
                code.replace_range(at..end, "");
            }
            for (n, line) in code.lines().enumerate() {
                for word in [
                    "sync_all",
                    "sync_data",
                    "fdatasync",
                    "fsync",
                    "FlushFileBuffers",
                ] {
                    let call = format!("{word}(");
                    for (at, _) in line.match_indices(&call) {
                        let before = line[..at].chars().next_back();
                        let bare_helper_call =
                            name == "atomic.rs" && word == "sync_all" && before != Some('.');
                        let part_of_a_name =
                            before.is_some_and(|c| c.is_alphanumeric() || c == '_');
                        if !part_of_a_name && !bare_helper_call {
                            found.push(format!("{name}:{}: {}", n + 1, line.trim()));
                        }
                    }
                }
            }
        }
        assert!(
            found.is_empty(),
            "a flush outside atomic::sync_all: {found:#?}"
        );
    }
}
