//! Vault-relative paths.

use serde::{Deserialize, Serialize};

use crate::error::VaultError;

/// A path to a file inside the vault, relative to the vault root.
///
/// One normal form, everywhere: forward slashes, no leading `./`, no `..`, no
/// leading or trailing separator. The index stores this string, the UI displays it,
/// and links resolve against it — so two spellings of the same file would become two
/// notes, and a Windows backslash reaching the database would make the same vault
/// index differently on different platforms.
///
/// This type validates and normalises spelling only. Resolving a path against a
/// real vault root — following symlinks, rejecting escapes, matching ignore rules —
/// is `serat-vault`'s job (architecture §2.1), because it requires touching a
/// filesystem and this crate touches none.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VaultPath(String);

impl VaultPath {
    /// Normalise and validate a vault-relative path.
    ///
    /// Backslashes become forward slashes, `./` segments are dropped, and repeated
    /// separators collapse.
    ///
    /// # Errors
    ///
    /// [`VaultError::InvalidPath`] if the path is empty, absolute, contains a `..`
    /// segment, contains a `:` in any segment (a Windows drive prefix or an NTFS
    /// alternate data stream — NFR-SEC-06, T-07, T-10), names a reserved Windows
    /// device (`CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`, with or
    /// without an extension), or has a segment with a trailing dot or space (both
    /// are silently stripped by the Windows filesystem APIs, so `"note. "` and
    /// `"note"` would otherwise name the same file on one platform and two
    /// different files on another). [`VaultError::PathEscapesVault`] is reserved
    /// for the case where a resolved absolute path lands outside the root, which
    /// only `serat-vault` can determine.
    pub fn new(path: impl AsRef<str>) -> Result<Self, VaultError> {
        let raw = path.as_ref();
        let invalid = |reason: &str| VaultError::InvalidPath {
            path: raw.to_owned(),
            reason: reason.to_owned(),
        };

        let unified = raw.replace('\\', "/");
        let mut segments = Vec::new();
        for segment in unified.split('/') {
            match segment {
                "" | "." => {}
                ".." => return Err(invalid("contains a `..` segment")),
                other => segments.push(other),
            }
        }
        if segments.is_empty() {
            return Err(invalid("is empty"));
        }
        // A Windows drive prefix or a UNC root reaching this constructor means an
        // absolute path was passed where a vault-relative one was required.
        if unified.starts_with('/') {
            return Err(invalid(
                "is absolute; pass a path relative to the vault root",
            ));
        }
        for segment in &segments {
            // `:` anywhere in a segment, not just a trailing drive letter on the
            // first one. `PathBuf::push` of a rootless, prefixed segment like
            // `"C:Users"` *replaces* the accumulated path on Windows instead of
            // appending to it — that is what let `"a/C:/Windows/x"` escape the
            // vault root with no symlink involved (task #34) — and `:` mid-segment
            // also names an NTFS alternate data stream (`note.md:hidden`), a second
            // way to read or write bytes the vault never walked. Refusing `:`
            // anywhere closes both with one rule (NFR-SEC-06, T-07, T-10).
            if segment.contains(':') {
                return Err(invalid(
                    "contains a `:`, which is a drive prefix or an NTFS alternate \
                     data stream on Windows",
                ));
            }
            // Windows strips a trailing dot or space from a segment before it ever
            // reaches the filesystem, so "note." and "note" name the same file
            // there but two different rows here — and the OS-level stripping is
            // itself a path first written on POSIX might not anticipate, which is
            // a second door out of the vault-as-identity guarantee VaultPath
            // exists to give (NFR-SEC-06).
            if segment.ends_with('.') || segment.ends_with(' ') {
                return Err(invalid(
                    "has a segment with a trailing dot or space, which Windows \
                     silently strips",
                ));
            }
            if is_reserved_windows_name(segment) {
                return Err(invalid(
                    "names a reserved Windows device (CON, PRN, AUX, NUL, COM1-9, LPT1-9)",
                ));
            }
        }
        Ok(Self(segments.join("/")))
    }

    /// The normalised path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The final segment, including any extension.
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// The final segment without its extension, which is a note's default title.
    #[must_use]
    pub fn stem(&self) -> &str {
        let name = self.file_name();
        match name.rfind('.') {
            Some(0) | None => name,
            Some(dot) => &name[..dot],
        }
    }

    /// The lowercased extension, without the dot.
    #[must_use]
    pub fn extension(&self) -> Option<String> {
        let name = self.file_name();
        let dot = name.rfind('.')?;
        if dot == 0 {
            return None;
        }
        Some(name[dot + 1..].to_ascii_lowercase())
    }

    /// The containing directory, or `None` for a file at the vault root.
    #[must_use]
    pub fn parent(&self) -> Option<&str> {
        self.0.rfind('/').map(|slash| &self.0[..slash])
    }

    /// Whether this is a Markdown note rather than an asset.
    ///
    /// The distinction the pipeline cares about: a note is parsed, an asset is
    /// extracted (architecture §7).
    #[must_use]
    pub fn is_markdown(&self) -> bool {
        matches!(self.extension().as_deref(), Some("md" | "markdown"))
    }
}

/// Whether a segment names a reserved Windows device, with or without an
/// extension (`"NUL"` and `"NUL.txt"` both address the null device, never a
/// file called that).
///
/// Case-folded: the reservation is case-insensitive on Windows, and a check
/// that only matched `"NUL"` and missed `"nul"` or `"Nul.md"` would leave the
/// door it exists to close open to any writer that happened to pick the
/// wrong case.
fn is_reserved_windows_name(segment: &str) -> bool {
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = segment.split('.').next().unwrap_or(segment);
    RESERVED
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(stem))
}

impl std::fmt::Display for VaultPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<VaultPath> for String {
    fn from(value: VaultPath) -> Self {
        value.0
    }
}

impl TryFrom<String> for VaultPath {
    type Error = VaultError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl AsRef<str> for VaultPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separators_normalise_to_one_spelling() {
        let expected = "10-projects/serat/notes.md";
        for spelling in [
            r"10-projects\serat\notes.md",
            "./10-projects/serat/notes.md",
            "10-projects//serat/notes.md",
            "10-projects/./serat/notes.md",
        ] {
            let path = VaultPath::new(spelling).expect(spelling);
            assert_eq!(path.as_str(), expected, "from {spelling}");
        }
    }

    #[test]
    fn traversal_and_absolute_paths_are_refused() {
        for bad in [
            "",
            "   /../secrets.md",
            "../outside.md",
            "10-projects/../../etc/passwd",
            "/etc/passwd",
            r"C:\Users\sujan\vault\note.md",
        ] {
            assert!(VaultPath::new(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn components() {
        let path = VaultPath::new("20-systems/Indexing Pipeline.md").expect("path");
        assert_eq!(path.file_name(), "Indexing Pipeline.md");
        assert_eq!(path.stem(), "Indexing Pipeline");
        assert_eq!(path.extension().as_deref(), Some("md"));
        assert_eq!(path.parent(), Some("20-systems"));
        assert!(path.is_markdown());

        let asset = VaultPath::new("40-refs/contract.PDF").expect("path");
        assert_eq!(asset.extension().as_deref(), Some("pdf"));
        assert!(!asset.is_markdown());
        assert_eq!(asset.parent(), Some("40-refs"));

        let dotfile = VaultPath::new(".gitignore").expect("path");
        assert_eq!(dotfile.extension(), None);
        assert_eq!(dotfile.stem(), ".gitignore");
        assert_eq!(dotfile.parent(), None);
    }

    #[test]
    fn a_colon_anywhere_in_a_segment_is_refused() {
        // Task #34 / NFR-SEC-06 / T-07 / T-10. `segments[0].ends_with(':')` alone
        // let a non-leading drive prefix through: `Vault::resolve` pushes each
        // segment onto a `PathBuf`, and on Windows `PathBuf::push` of a rootless,
        // prefixed segment like "C:Users" *replaces* the whole accumulated path
        // rather than appending to it — so "a/C:/Windows/x" escaped the vault root
        // with no symlink involved. A trailing colon on a later segment is the
        // same shape one directory down, and `:` mid-segment is a second escape
        // route: an NTFS alternate data stream.
        for bad in [
            "a/C:/Windows/x",
            "a/C:Users/x",
            "notes/b:c.md",
            "note.md:hidden",
            "note.md:hidden:$DATA",
        ] {
            assert!(VaultPath::new(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn reserved_windows_device_names_are_refused_case_insensitively_and_with_an_extension() {
        // NFR-SEC-06. Any of these as a segment addresses a device on Windows, not
        // a file, regardless of case or a trailing extension.
        for bad in [
            "CON", "con", "Con.md", "PRN", "prn.txt", "AUX", "NUL", "nul.md", "COM1", "com3.md",
            "LPT1", "lpt9.md", "notes/CON.md", "notes/con/note.md",
        ] {
            assert!(VaultPath::new(bad).is_err(), "accepted {bad:?}");
        }
        // And the near-misses must not be caught by an over-eager match.
        for ok in ["CONtent.md", "console.md", "NULL.md", "COM10.md", "LPT10.md"] {
            assert!(VaultPath::new(ok).is_ok(), "rejected {ok:?}");
        }
    }

    #[test]
    fn a_trailing_dot_or_space_in_a_segment_is_refused() {
        // NFR-SEC-06. Windows silently strips a trailing dot or space before the
        // segment ever reaches a filesystem call, so "note." and "note" would name
        // the same file there but two different rows here.
        for bad in ["note.md.", "note.md ", "dir./note.md", "dir /note.md"] {
            assert!(VaultPath::new(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn round_trips_through_json_as_a_plain_string() {
        let path = VaultPath::new("00-inbox/today.md").expect("path");
        let json = serde_json::to_string(&path).expect("serialise");
        assert_eq!(json, "\"00-inbox/today.md\"");
        assert_eq!(
            serde_json::from_str::<VaultPath>(&json).expect("deserialise"),
            path
        );
        assert!(serde_json::from_str::<VaultPath>("\"../escape.md\"").is_err());
    }
}
