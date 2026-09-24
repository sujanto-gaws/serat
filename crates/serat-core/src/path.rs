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

/// The filesystem convention a [`VaultPath`] is validated against.
///
/// A plain enum rather than `#[cfg(windows)]` sprinkled through the validation
/// logic below, so the Windows-only rules stay unit-testable on every platform CI
/// runs on. Task #34 shipped a first fix that made those rules universal, and a
/// file legal on Linux and macOS — `aux.md`, `Meeting 10:30.md`, a directory named
/// `Misc.` — silently vanished from the tree, search and backlinks on those
/// platforms, because `VaultPath::new` refused a name Windows alone cannot have.
/// [`Platform::current`] is the only place `cfg!` appears in this file; every rule
/// below is a plain function of a [`Platform`] value, so the same test binary can
/// assert both "refused on Windows" and "accepted everywhere else" without a
/// second CI runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Windows: reserved device names, a handful of forbidden characters, and
    /// `PathBuf::push` semantics that make a segment containing `:` dangerous
    /// (task #34).
    Windows,
    /// Every other target Serat ships on. A POSIX filesystem enforces far less
    /// than Windows does, and what it does enforce (no `/` or NUL byte in a
    /// name) is already excluded by the segment split above this enum.
    Other,
}

impl Platform {
    /// The platform this binary is actually running on.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(windows) { Self::Windows } else { Self::Other }
    }
}

impl VaultPath {
    /// Normalise and validate a vault-relative path against the platform this
    /// binary runs on ([`Platform::current`]).
    ///
    /// Backslashes become forward slashes, `./` segments are dropped, and repeated
    /// separators collapse.
    ///
    /// # Errors
    ///
    /// See [`VaultPath::new_for_platform`].
    pub fn new(path: impl AsRef<str>) -> Result<Self, VaultError> {
        Self::new_for_platform(path, Platform::current())
    }

    /// As [`VaultPath::new`], but validated against a specific [`Platform`]
    /// rather than whatever this binary happens to run on.
    ///
    /// The seam `serat-vault`'s walk and watcher use to decide, per §34's
    /// amendment, whether a name the local filesystem is happily holding is one
    /// this *binary's own platform* would also accept, or whether it is a name
    /// only illegal elsewhere and must be reported rather than silently dropped —
    /// and the seam this crate's own tests use to exercise the Windows-only rules
    /// on every CI runner rather than only a Windows one.
    ///
    /// # Errors
    ///
    /// [`VaultError::InvalidPath`] if the path is empty, contains a `..` segment,
    /// or is absolute — a leading `/` or a leading drive letter (`C:`), both of
    /// which mean an absolute path arrived where a vault-relative one was
    /// required, on every platform. On [`Platform::Windows`] specifically, also
    /// if any segment contains a `:` elsewhere than a leading drive letter (task
    /// #34: `PathBuf::push` of a rootless, drive-prefixed segment like
    /// `"C:Users"` *replaces* the accumulated path on Windows instead of
    /// extending it, and `:` mid-segment separately names an NTFS alternate data
    /// stream — NFR-SEC-06, T-07, T-10), names a reserved Windows device (`CON`,
    /// `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, `CLOCK$`, `COM0`-`COM9`,
    /// `LPT0`-`LPT9`, with or without an extension, case-insensitive), or has a
    /// segment with a trailing dot or space (both are silently stripped by the
    /// Windows filesystem APIs, so `"note. "` and `"note"` would otherwise name
    /// the same file there but two different rows here). None of these four is a
    /// real name on Windows at all, so refusing them only there costs nothing and
    /// accepting them everywhere else is not a laxity — it is what those
    /// platforms already do. [`VaultError::PathEscapesVault`] is reserved for the
    /// case where a resolved absolute path lands outside the root, which only
    /// `serat-vault` can determine.
    pub fn new_for_platform(path: impl AsRef<str>, platform: Platform) -> Result<Self, VaultError> {
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
        // Universal, on every platform: a leading drive letter or a UNC root means
        // an absolute path arrived where a vault-relative one was required. A user
        // who pastes "C:\Users\..." made the same mistake regardless of what this
        // binary runs on, so this stays outside the `platform` gate below.
        if unified.starts_with('/') || segments[0].ends_with(':') {
            return Err(invalid(
                "is absolute; pass a path relative to the vault root",
            ));
        }
        if platform == Platform::Windows {
            for segment in &segments {
                // `:` anywhere in a later segment, not just a leading drive
                // letter. `PathBuf::push` of a rootless, prefixed segment like
                // `"C:Users"` *replaces* the accumulated path on Windows instead
                // of appending to it — that is what let `"a/C:/Windows/x"` escape
                // the vault root with no symlink involved (task #34) — and `:`
                // mid-segment also names an NTFS alternate data stream
                // (`note.md:hidden`), a second way to read or write bytes the
                // vault never walked. On any other platform a colon is an
                // ordinary filename character with neither hazard, so this rule
                // is Windows-only rather than universal (task #34's regression).
                if segment.contains(':') {
                    return Err(invalid(
                        "contains a `:`, which is a drive prefix or an NTFS alternate \
                         data stream on Windows",
                    ));
                }
                // Windows strips a trailing dot or space from a segment before it
                // ever reaches the filesystem, so "note." and "note" name the
                // same file there but two different rows here. Elsewhere both are
                // ordinary, distinct filenames, so this is Windows-only too.
                if segment.ends_with('.') || segment.ends_with(' ') {
                    return Err(invalid(
                        "has a segment with a trailing dot or space, which Windows \
                         silently strips",
                    ));
                }
                if is_reserved_windows_name(segment) {
                    return Err(invalid(
                        "names a reserved Windows device (CON, PRN, AUX, NUL, CONIN$, \
                         CONOUT$, CLOCK$, COM0-9, LPT0-9)",
                    ));
                }
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
/// wrong case. The stem's trailing spaces are trimmed before comparing —
/// `"CON .md"` still addresses the device, because Windows itself strips a
/// trailing space from a component before resolving it (the same fact the
/// trailing-dot-or-space rule above states as its own, general rule; this one
/// additionally has to apply *before* the device-name comparison rather than
/// only as a standalone refusal, or `"CON .md"` would be caught by neither).
fn is_reserved_windows_name(segment: &str) -> bool {
    // COM0/LPT0 are real reserved devices alongside COM1-9/LPT1-9 (the docs that
    // list only 1-9 are incomplete); COM¹, COM² and COM³ are the superscript
    // digits Windows itself has historically produced when the console code page
    // renders 1/2/3 that way, and both spellings are reserved so a name copied
    // from a Windows error message is caught too. CONIN$, CONOUT$ and CLOCK$
    // round out the device list.
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "CLOCK$", "COM0", "COM1", "COM2", "COM3",
        "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "COM\u{b9}", "COM\u{b2}", "COM\u{b3}",
        "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT\u{b9}",
        "LPT\u{b2}", "LPT\u{b3}",
    ];
    let stem = segment
        .split('.')
        .next()
        .unwrap_or(segment)
        .trim_end_matches(' ');
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
    fn a_colon_anywhere_in_a_segment_is_refused_on_windows_and_accepted_elsewhere() {
        // Task #34 / NFR-SEC-06 / T-07 / T-10, amended after code-reviewer-2's
        // finding: the universal leading-drive check alone let a non-leading
        // drive prefix through — `Vault::resolve` pushes each segment onto a
        // `PathBuf`, and on Windows `PathBuf::push` of a rootless, prefixed
        // segment like "C:Users" *replaces* the whole accumulated path rather
        // than appending to it, so "a/C:/Windows/x" escaped the vault root with
        // no symlink involved. But that replacement is Windows-specific
        // behaviour: on every other platform a colon is an ordinary filename
        // character (`"b:c.md"` is a ordinary, legal name on ext4 and APFS), and
        // refusing it there dropped real files from the tree silently — the
        // regression this test now pins in both directions with one seam,
        // callable from any CI runner regardless of what it actually boots.
        for bad in [
            "a/C:/Windows/x",
            "a/C:Users/x",
            "notes/b:c.md",
            "note.md:hidden",
            "note.md:hidden:$DATA",
        ] {
            assert!(
                VaultPath::new_for_platform(bad, Platform::Windows).is_err(),
                "accepted on Windows: {bad:?}"
            );
            assert!(
                VaultPath::new_for_platform(bad, Platform::Other).is_ok(),
                "refused on a platform where none of this is dangerous: {bad:?}"
            );
        }
    }

    #[test]
    fn reserved_windows_device_names_are_refused_on_windows_case_insensitively_with_an_extension_and_accepted_elsewhere()
     {
        // NFR-SEC-06. Any of these as a segment addresses a device on Windows, not
        // a file, regardless of case, a trailing extension, or a trailing space on
        // the stem — but "con.md", "aux.md" and friends are entirely ordinary
        // filenames on every other platform (task #34's regression: one of the
        // probe's three dropped files was exactly `aux.md`).
        for bad in [
            "CON",
            "con",
            "Con.md",
            "CON .md",
            "PRN",
            "prn.txt",
            "AUX",
            "aux.md",
            "NUL",
            "nul.md",
            "COM0",
            "COM1",
            "com3.md",
            "LPT0",
            "LPT1",
            "lpt9.md",
            "CONIN$",
            "CONOUT$",
            "CLOCK$",
            "COM\u{b9}",
            "LPT\u{b2}",
            "notes/CON.md",
            "notes/con/note.md",
        ] {
            assert!(
                VaultPath::new_for_platform(bad, Platform::Windows).is_err(),
                "accepted on Windows: {bad:?}"
            );
            assert!(
                VaultPath::new_for_platform(bad, Platform::Other).is_ok(),
                "refused on a platform with no such device: {bad:?}"
            );
        }
        // And the near-misses must not be caught by an over-eager match, on
        // either platform.
        for ok in ["CONtent.md", "console.md", "NULL.md", "COM10.md", "LPT10.md"] {
            assert!(
                VaultPath::new_for_platform(ok, Platform::Windows).is_ok(),
                "rejected on Windows: {ok:?}"
            );
            assert!(
                VaultPath::new_for_platform(ok, Platform::Other).is_ok(),
                "rejected: {ok:?}"
            );
        }
    }

    #[test]
    fn a_trailing_dot_or_space_in_a_segment_is_refused_on_windows_and_accepted_elsewhere() {
        // NFR-SEC-06. Windows silently strips a trailing dot or space before the
        // segment ever reaches a filesystem call, so "note." and "note" would name
        // the same file there but two different rows here. Elsewhere both are
        // ordinary, distinct filenames — a directory literally named "Misc." is
        // legal on Linux and macOS, and was one of the probe's three dropped
        // files under the regression this test now pins against returning.
        for bad in ["note.md.", "note.md ", "dir./note.md", "dir /note.md", "Misc./n.md"] {
            assert!(
                VaultPath::new_for_platform(bad, Platform::Windows).is_err(),
                "accepted on Windows: {bad:?}"
            );
            assert!(
                VaultPath::new_for_platform(bad, Platform::Other).is_ok(),
                "refused on a platform where trailing dots and spaces are ordinary: {bad:?}"
            );
        }
    }

    #[test]
    fn the_probes_three_dropped_files_are_accepted_on_the_platform_that_can_have_them() {
        // The exact three names code-reviewer-2's probe found silently vanishing
        // from the tree, search and backlinks under the regression (FR-VLT-01,
        // design goal 5), asserted through the platform-agnostic `new` rather
        // than `new_for_platform` — this is what a Linux or macOS CI run actually
        // exercises, since `Platform::current()` there is `Other`.
        for legal in ["aux.md", "Meeting 10:30.md", "Misc./n.md"] {
            assert!(VaultPath::new(legal).is_ok(), "dropped: {legal:?}");
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
