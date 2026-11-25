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

/// The inverse of [`strip_verbatim_str`]: the `\\?\` spelling of an absolute Windows
/// path, for I/O on names Win32 would normalise away (`note.`, `note `, `AUX.md`).
///
/// - `C:\x` -> `\\?\C:\x`
/// - `\\srv\share\x` -> `\\?\UNC\srv\share\x`
/// - already `\\?\` -> unchanged
/// - not a Windows absolute path (relative, POSIX) -> the input, unchanged
///
/// Verbatim paths are not normalised by the OS, so `/` becomes `\`. Resolving `.` or
/// `..` lexically is hazardous (`C:\..` would name the volume device, `\\srv\share\..`
/// would pop the share), so `None` is returned for any `.`, `..` or empty segment, and
/// for a `\\.\` device path, which has no verbatim spelling. The caller must treat
/// `None` as "cannot judge", never as "absent".
#[must_use]
pub fn to_verbatim_str(text: &str) -> Option<String> {
    if text.starts_with(r"\\?\") {
        return Some(text.to_owned());
    }
    if text.starts_with(r"\\.\") {
        return None;
    }
    let slashed = text.replace('/', r"\");
    let (prefix, rest) = if let Some(unc) = slashed.strip_prefix(r"\\") {
        (r"\\?\UNC\", unc)
    } else if slashed.as_bytes().get(1) == Some(&b':')
        && slashed
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
        && slashed.as_bytes().get(2) == Some(&b'\\')
    {
        (r"\\?\", slashed.as_str())
    } else {
        return Some(text.to_owned());
    };
    if rest
        .split('\\')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }
    Some(format!("{prefix}{rest}"))
}

/// [`to_verbatim_str`] over a [`Path`]. `None` as there, and also when the path is not
/// valid Unicode: a lossy conversion would stat a different name.
#[must_use]
pub fn to_verbatim(path: &Path) -> Option<PathBuf> {
    to_verbatim_str(path.to_str()?).map(PathBuf::from)
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

    #[test]
    fn to_verbatim_has_three_forms_and_is_the_inverse_of_strip() {
        let v = |text| to_verbatim_str(text).expect("some");
        assert_eq!(v(r"C:\v\note."), r"\\?\C:\v\note.");
        assert_eq!(v(r"\\srv\sh\x"), r"\\?\UNC\srv\sh\x");
        assert_eq!(v(r"\\?\Volume{1}\v"), r"\\?\Volume{1}\v");
        assert_eq!(v(r"\\?\C:\v"), r"\\?\C:\v");
        assert_eq!(v("C:/v/note "), r"\\?\C:\v\note ");
        for plain in [r"C:\v\x", r"\\srv\sh\x"] {
            assert_eq!(strip_verbatim_str(&v(plain)), plain);
        }
    }

    #[test]
    fn to_verbatim_refuses_what_it_cannot_spell_safely() {
        for text in [
            r"C:\v\.\x",
            r"C:\v\a\..\b",
            r"C:\..",
            r"\\srv\sh\..",
            r"C:\v\\x",
            r"\\.\COM1",
        ] {
            assert_eq!(to_verbatim_str(text), None, "{text}");
        }
        // Not a Windows path: unchanged, slashes included.
        assert_eq!(to_verbatim_str("a/b").as_deref(), Some("a/b"));
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_unicode_has_no_verbatim_form() {
        use std::os::unix::ffi::OsStrExt as _;
        let path = Path::new(std::ffi::OsStr::from_bytes(b"x\xff"));
        assert_eq!(to_verbatim(path), None);
    }
}
