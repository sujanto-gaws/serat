//! The cross-process writer lock (task #154; NFR-REL-01, NFR-REL-02; ADR-0001).
//!
//! ADR-0001 makes one writer per vault the design, but "one" was only enforced inside a
//! process (serat-app's detached-writer registry, task #96). A second Serat instance, or
//! a benchmark run beside the app, on the same vault would open its own writer, and a
//! rebuild by either unlinks `index.db`, `-wal` and `-shm` while the other still has
//! them open. Across processes that leaves the first writing to an unlinked file while
//! its readers see the new database (silent drift), or two database inodes sharing one
//! `-wal`/`-shm` pair (corruption); on Windows the `remove_file` on an open `index.db`
//! fails. Inside one process the worse case is the SIGBUS test-engineer found in
//! `walIndexAppend` (`docs/05-testing/sigbus-105.md`): unlinking alone does not fault a
//! mapping, but truncating the shared `-shm` does, and that needs two database inodes
//! sharing one `-shm` in one process, where SQLite's own locks (per process on POSIX) do
//! not separate them.
//!
//! So an advisory lock on `.serat/index.lock` is taken **before any rebuild or writer
//! open** and held for as long as the writer lives. It is `File::try_lock`: `flock` on
//! Unix, `LockFileEx` on Windows, no `unsafe`. The operating system drops it when the
//! process dies, so a crash leaves nothing stale on a local disk. A holder elsewhere
//! makes the open fail with [`IndexError::HeldByAnotherProcess`]; it never waits,
//! because a writer that blocked an open would block the window (design goal 2). A
//! failure to create or take the lock for another reason is [`IndexError::Io`].
//!
//! Locks of two separate opens conflict even inside one process, so this also refuses a
//! second writer in the same process, whatever the registry saw.
//!
//! # Limits
//!
//! * **Deleting `.serat/` while the app runs** unlinks `index.lock`: a second instance
//!   then creates a new file and wins, so there are two writers again (no SIGBUS from
//!   this lock's absence, but the old hazard). The lock is a backstop, not a guard
//!   against a user removing the directory under a running app.
//! * **NFS:** `flock` is emulated as a whole-file `fcntl` lock on Linux, which is per
//!   process, so two opens in one process do not conflict there; it also needs a lock
//!   manager, and on a `nolock` mount it is local to the client. Cross-machine
//!   exclusion is not promised.
//! * **SMB:** not verified.
//! * **Windows after a crash:** the lock is released when the process's handles are
//!   closed, which can lag the exit; a quick relaunch may be refused for a moment.
//!
//! Cost: one `open` and one `flock` per index open, measured in the open-time benches.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use serat_core::IndexError;

/// The lock file's name inside the index directory.
pub const LOCK_FILE: &str = "index.lock";

/// Held while a writer (or a rebuild) owns the index in `dir`. Released on drop.
#[derive(Debug)]
pub(crate) struct IndexLock {
    _file: File,
}

impl IndexLock {
    /// Take the lock for `index_dir`, creating the directory and the lock file.
    ///
    /// # Errors
    ///
    /// [`IndexError::HeldByAnotherProcess`] if another holder has it;
    /// [`IndexError::Io`] if the lock file cannot be created or locked for any other
    /// reason.
    pub(crate) fn acquire(index_dir: &Path) -> Result<Self, IndexError> {
        let path: PathBuf = index_dir.join(LOCK_FILE);
        let io = |what: &str, err: &std::io::Error| IndexError::Io {
            message: format!(
                "could not {what} {}: {err}. The index could not be locked here",
                path.display()
            ),
        };
        std::fs::create_dir_all(index_dir).map_err(|err| io("create the directory of", &err))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|err| io("open", &err))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(IndexError::HeldByAnotherProcess {
                path: path.display().to_string(),
            }),
            Err(TryLockError::Error(err)) => Err(io("lock", &err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_acquire_is_refused_until_the_first_is_dropped() {
        let dir = tempfile::tempdir().expect("dir");
        let index = dir.path().join(".serat");
        let first = IndexLock::acquire(&index).expect("first");
        let second = IndexLock::acquire(&index);
        assert!(
            matches!(second, Err(IndexError::HeldByAnotherProcess { .. })),
            "{second:?}"
        );
        drop(first);
        IndexLock::acquire(&index).expect("free again after the holder is gone");
    }

    #[test]
    fn different_directories_do_not_contend() {
        let a = tempfile::tempdir().expect("a");
        let b = tempfile::tempdir().expect("b");
        let _held = IndexLock::acquire(a.path()).expect("a");
        IndexLock::acquire(b.path()).expect("b");
    }
}
