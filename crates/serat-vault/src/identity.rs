//! The identity of a directory as the filesystem knows it (task #96).
//!
//! Two paths that reach one directory -- a case-insensitive mount, a bind mount, a
//! symlink -- have the same [`FileIdentity`]; the application keys "one index writer per
//! vault" by it. It is a plain value read once with no handle kept, so holding it never
//! pins the directory (a removable volume can still be ejected).

use std::path::Path;

/// `(volume, file)`: device and inode on Unix, volume serial number and file index on
/// Windows. Opaque; compare for equality only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileIdentity(u64, u64);

impl FileIdentity {
    /// Read the identity of the directory at `path`, or `None` if it cannot be read.
    #[must_use]
    pub fn of_dir(path: &Path) -> Option<Self> {
        imp::of_dir(path)
    }
}

#[cfg(unix)]
mod imp {
    use super::FileIdentity;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    pub(super) fn of_dir(path: &Path) -> Option<FileIdentity> {
        let meta = std::fs::metadata(path).ok()?;
        Some(FileIdentity(meta.dev(), meta.ino()))
    }
}

#[cfg(windows)]
mod imp {
    use super::FileIdentity;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Path;

    /// `FILE_FLAG_BACKUP_SEMANTICS`: required to open a directory by handle.
    const BACKUP_SEMANTICS: u32 = 0x0200_0000;

    pub(super) fn of_dir(path: &Path) -> Option<FileIdentity> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(BACKUP_SEMANTICS)
            .open(path)
            .ok()?;
        // The handle is dropped at the end of this function: only the numbers stay.
        let info = winapi_util::file::information(&file).ok()?;
        Some(FileIdentity(info.volume_serial_number(), info.file_index()))
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::FileIdentity;
    use std::path::Path;

    pub(super) fn of_dir(_path: &Path) -> Option<FileIdentity> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_directory_has_one_identity_and_two_have_two() {
        let dir = tempfile::tempdir().expect("tmp");
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir(&a).expect("a");
        std::fs::create_dir(&b).expect("b");
        let id = FileIdentity::of_dir(&a).expect("identity");
        assert_eq!(
            Some(id),
            FileIdentity::of_dir(&a.join(".").join("..").join("a"))
        );
        assert_ne!(Some(id), FileIdentity::of_dir(&b));
        assert_eq!(FileIdentity::of_dir(&dir.path().join("missing")), None);
    }
}
