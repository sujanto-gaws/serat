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
pub(crate) fn write_exclusive(temp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)?;
    file.write_all(bytes)?;
    // Flush before the rename. Without this the rename can be durable while the
    // contents are not, which turns a power cut into a note full of zeroes.
    file.sync_all()
}

/// Write `bytes` to a new temp file in `parent` and return its path.
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
) -> std::io::Result<PathBuf> {
    let mut last = None;
    for _ in 0..NAME_ATTEMPTS {
        let temp = temp_name(parent, file_name);
        match write_exclusive(&temp, bytes) {
            Ok(()) => {
                if let Some(permissions) = permissions
                    && let Err(err) = fs::set_permissions(&temp, permissions)
                {
                    let _ = fs::remove_file(&temp);
                    return Err(err);
                }
                return Ok(temp);
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

/// fsync the directory so the rename itself survives a power cut. Best effort: the
/// rename has happened and is visible, so a failure here is not a failed write, and
/// Windows has no directory handle to sync.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = fs::File::open(dir) {
        let _ = handle.sync_all();
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
