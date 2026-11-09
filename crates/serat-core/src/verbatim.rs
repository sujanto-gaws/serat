//! Windows verbatim-prefix normalisation, shared by `serat-vault` and `serat-app`
//! (ADR-0017 step 7: the prefix is stripped from both the real path and the root).
//!
//! Pure string work, no I/O, so it lives here and runs in Linux CI.
//!
//! NOTE: the Windows behaviour cannot be exercised on this platform. The string
//! tests below cover it; a Windows CI run against a real UNC share is owed.

use std::path::{Path, PathBuf};

/// Map a `canonicalize`-style verbatim path to its plain form.
///
/// - `\\?\UNC\srv\share\x` -> `\\srv\share\x`
/// - `\\?\C:\x` -> `C:\x` (a trailing separator is kept)
/// - anything else is returned unchanged: plain `C:\x`, `\\srv\share\x`, POSIX
///   paths, `\\.\` device paths (left as-is, not refused: canonicalisation does
///   not yield them and rewriting would change their meaning) and other verbatim
///   forms such as `\\?\Volume{..}\`, which have no plain spelling. A bare
///   `\\?\C:` or `\\?\C:foo` is also left alone: without a separator the plain form
///   would be drive-relative, a different meaning.
///
/// Forward slashes are not separators inside verbatim paths, and `canonicalize`
/// never emits them. The `Path` wrapper goes through `to_string_lossy`; once
/// Windows CI exists, a `cfg(windows)` body over `Path::components()` is the
/// lossless option.
#[must_use]
pub fn strip_verbatim_str(text: &str) -> String {
    let Some(rest) = text.strip_prefix(r"\\?\") else {
        return text.to_owned();
    };
    if let Some(head) = rest.get(..4)
        && head.eq_ignore_ascii_case(r"UNC\")
    {
        return format!(r"\\{}", &rest[4..]);
    }
    let bytes = rest.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        return rest.to_owned();
    }
    text.to_owned()
}

/// [`strip_verbatim_str`] over a [`Path`].
#[must_use]
pub fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let stripped = strip_verbatim_str(&text);
    if stripped == text {
        path.to_path_buf()
    } else {
        PathBuf::from(stripped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_forms() {
        assert_eq!(strip_verbatim_str(r"\\?\UNC\srv\share\v"), r"\\srv\share\v");
        assert_eq!(strip_verbatim_str(r"\\?\C:\x"), r"C:\x");
        assert_eq!(strip_verbatim_str(r"C:\x"), r"C:\x");
        assert_eq!(strip_verbatim_str(r"\\srv\share\x"), r"\\srv\share\x");
    }

    #[test]
    fn trailing_separator_kept() {
        assert_eq!(strip_verbatim_str(r"\\?\C:\"), r"C:\");
        assert_eq!(strip_verbatim_str(r"\\?\UNC\srv\share\"), r"\\srv\share\");
    }

    #[test]
    fn bare_drive_is_not_made_drive_relative() {
        assert_eq!(strip_verbatim_str(r"\\?\C:"), r"\\?\C:");
        assert_eq!(strip_verbatim_str(r"\\?\C:foo"), r"\\?\C:foo");
    }

    #[test]
    fn device_and_other_verbatim_left_alone() {
        assert_eq!(strip_verbatim_str(r"\\.\C:\x"), r"\\.\C:\x");
        assert_eq!(
            strip_verbatim_str(r"\\.\PhysicalDrive0"),
            r"\\.\PhysicalDrive0"
        );
        assert_eq!(strip_verbatim_str(r"\\?\Volume{1}\x"), r"\\?\Volume{1}\x");
        assert_eq!(strip_verbatim_str(r"\\?\UNC"), r"\\?\UNC");
    }

    #[test]
    fn posix_is_a_no_op() {
        for p in ["/home/u/vault", "/", "rel/x.md", "", "/a/\\\\?\\b"] {
            assert_eq!(strip_verbatim_str(p), p);
            assert_eq!(strip_verbatim(Path::new(p)), Path::new(p));
        }
    }
}
