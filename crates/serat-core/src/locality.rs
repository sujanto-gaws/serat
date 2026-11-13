//! Whether a file's bytes are on this machine (ADR-0018 §1; NFR-REL-01, NFR-CMP-01).
//!
//! Cloud clients leave placeholders: OneDrive Files-On-Demand on Windows, File Provider
//! "dataless" files on macOS. Reading one downloads it, so a caller that wants to avoid
//! that has to ask first. This is a pure function of metadata the caller already has;
//! nothing here opens, reads or stats anything.
//!
//! Limits, as the ADR lists them:
//!
//! * **Linux has no signal.** FUSE clients (rclone, onedriver) download inside `read`
//!   and mark nothing, so [`is_local`] returns `None`: unknown, not "local".
//! * **Windows placeholder-compatibility ("disguise") mode** may hide these attributes
//!   from this process. Detection then fails open: the file reads as local, and the
//!   caller behaves as it does without this check (a read that downloads, and an `Err`
//!   if the download fails, never zero bytes).
//! * **Files only.** A dataless or placeholder *directory* is not judged here.
//! * **Racy by nature.** A file can be evicted or hydrated right after the answer.

/// `FILE_ATTRIBUTE_OFFLINE`: the data is not immediately available.
pub const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
/// `FILE_ATTRIBUTE_RECALL_ON_OPEN`: opening the file recalls it.
pub const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
/// `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`: reading the file recalls it.
pub const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
/// `SF_DATALESS`: the macOS `st_flags` bit for a file whose data is not local.
pub const SF_DATALESS: u32 = 0x4000_0000;

/// Windows: local unless any of the three recall attributes is set. Runs on every
/// platform so the cases are unit-testable with hand-built values.
#[must_use]
pub const fn is_local_attrs(file_attributes: u32) -> bool {
    file_attributes
        & (FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
            | FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_RECALL_ON_OPEN)
        == 0
}

/// macOS: local unless `SF_DATALESS` is set in `st_flags`. Runs on every platform.
#[must_use]
pub const fn is_local_flags(st_flags: u32) -> bool {
    st_flags & SF_DATALESS == 0
}

/// Whether the file `meta` describes has its bytes on this machine: `Some(true)` or
/// `Some(false)` where the platform gives a signal (Windows, macOS), `None` where it
/// does not (Linux and anything else). `meta` should come from `symlink_metadata` or
/// an open handle, so a link is not followed to a placeholder.
#[must_use]
pub fn is_local(meta: &std::fs::Metadata) -> Option<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        Some(is_local_attrs(meta.file_attributes()))
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt as _;
        Some(is_local_flags(meta.st_flags()))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = meta;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_windows_recall_bit_alone_makes_a_file_not_local() {
        for bit in [
            FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
            FILE_ATTRIBUTE_OFFLINE,
            FILE_ATTRIBUTE_RECALL_ON_OPEN,
        ] {
            assert!(!is_local_attrs(bit), "{bit:#x}");
            // With the ordinary attributes a real file carries (ARCHIVE | REPARSE_POINT).
            assert!(!is_local_attrs(bit | 0x20 | 0x400), "{bit:#x}");
        }
    }

    #[test]
    fn combined_windows_recall_bits_are_not_local() {
        let all = FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
            | FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_RECALL_ON_OPEN;
        assert!(!is_local_attrs(all));
        assert!(!is_local_attrs(
            FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN
        ));
        assert!(!is_local_attrs(FILE_ATTRIBUTE_OFFLINE | 0x20));
    }

    #[test]
    fn ordinary_and_neighbouring_windows_attributes_are_local() {
        assert!(is_local_attrs(0));
        // ARCHIVE, HIDDEN, READONLY, REPARSE_POINT (a symlink or a hydrated placeholder),
        // SPARSE_FILE, and the bits adjacent to each recall bit.
        for attrs in [
            0x20,
            0x2,
            0x1,
            0x400,
            0x200,
            0x0800,
            0x2000,
            0x0002_0000,
            0x0080_0000,
            0x0020_0000,
        ] {
            assert!(is_local_attrs(attrs), "{attrs:#x}");
        }
    }

    #[test]
    fn sf_dataless_alone_makes_a_macos_file_not_local() {
        assert!(!is_local_flags(SF_DATALESS));
        // With UF_HIDDEN (0x8000) and SF_ARCHIVED (0x0001_0000) beside it.
        assert!(!is_local_flags(SF_DATALESS | 0x8000 | 0x0001_0000));
    }

    #[test]
    fn other_macos_flags_are_local() {
        for flags in [
            0,
            0x8000,
            0x0001_0000,
            0x2000_0000,
            0x8000_0000,
            0x0000_0002,
        ] {
            assert!(is_local_flags(flags), "{flags:#x}");
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn linux_has_no_signal() {
        let meta = std::fs::metadata(".").expect("stat");
        assert_eq!(is_local(&meta), None);
    }
}
