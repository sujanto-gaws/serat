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
    /// [`VaultError::InvalidPath`] if the path is empty, absolute, or contains a
    /// `..` segment. [`VaultError::PathEscapesVault`] is reserved for the case where
    /// a resolved absolute path lands outside the root, which only `serat-vault` can
    /// determine.
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
        if unified.starts_with('/') || segments[0].ends_with(':') {
            return Err(invalid(
                "is absolute; pass a path relative to the vault root",
            ));
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
