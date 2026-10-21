//! What the walk and the watcher refuse to look at.

/// Directory names that are never descended into.
///
/// `.obsidian` is the interesting one. The charter's zero-switching-cost promise is
/// that Serat reads an existing Obsidian vault unmodified, with `.obsidian/` "ignored
/// but preserved" — so it must not be indexed, and it must not be touched. Ignoring
/// it here does both, because nothing outside a walk ever names a file.
const IGNORED_DIRS: &[&str] = &[
    ".serat",    // our own index. Indexing it would index a description of itself.
    ".obsidian", // Obsidian's configuration. Preserved, never read, never written.
    ".git",      // and its packfiles, which are large and binary and meaningless here
    ".trash",    // Obsidian's soft-delete directory
    ".stfolder", // Syncthing
    ".stversions",
    "node_modules",
];

/// File extensions treated as Markdown notes.
///
/// Everything else in the vault is an asset: recorded, given a sidecar, extracted in
/// P2 (architecture §7). Nothing is skipped for being unrecognised — design goal 5
/// says capability degrades and nothing breaks, so an unknown file type is still a
/// row, still linkable, still queryable.
const NOTE_EXTENSIONS: &[&str] = &["md", "markdown"];

/// The rules the walk and the watcher share.
///
/// One type, used by both, because a file the walk skips and the watcher accepts
/// would be indexed on edit and vanish on rebuild — a difference that only shows up
/// as "my vault has different contents after a rebuild", which is the one thing
/// C-02 promises cannot happen.
#[derive(Debug, Clone, Default)]
pub struct IgnoreRules {
    extra_dirs: Vec<String>,
}

impl IgnoreRules {
    /// The default rules.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Also ignore a directory name, anywhere in the vault.
    #[must_use]
    pub fn ignoring_dir(mut self, name: impl Into<String>) -> Self {
        self.extra_dirs.push(name.into());
        self
    }

    /// Whether a directory should be descended into.
    #[must_use]
    pub fn is_ignored_dir(&self, name: &str) -> bool {
        // Case-folded (task #34, NFR-SEC-06): Windows and macOS filesystems are
        // case-insensitive by default, so ".Obsidian" or ".GIT" names the same
        // directory a case-sensitive comparison would walk straight into —
        // indexing a vault's own index, or a packfile, on those platforms only.
        IGNORED_DIRS
            .iter()
            .any(|ignored| ignored.eq_ignore_ascii_case(name))
            || self
                .extra_dirs
                .iter()
                .any(|extra| extra.eq_ignore_ascii_case(name))
            // Every remaining dotted directory. A vault is a folder of files someone
            // reads; a hidden directory in it belongs to a tool, not to them.
            || name.starts_with('.')
    }

    /// Whether a file should be indexed at all.
    #[must_use]
    pub fn is_ignored_file(&self, name: &str) -> bool {
        // Dotfiles are configuration. .gitignore, .DS_Store and .editorconfig are not
        // notes, and indexing them would put them in search results forever.
        name.starts_with('.') || is_temp_name(name)
    }
}

/// The prefix of the temporary files [`crate::Vault::write_atomic`] creates.
///
/// Recognisable on sight and skipped by the walk, so a crash mid-write leaves
/// litter rather than a phantom note. Naming them after us also means a user who
/// finds one knows who to blame.
pub(crate) const TEMP_PREFIX: &str = ".serat-tmp-";

/// Whether a filename is one of our in-flight writes, or another tool's.
fn is_temp_name(name: &str) -> bool {
    name.starts_with(TEMP_PREFIX)
        || name.ends_with('~')       // emacs, gedit
        || name.ends_with(".swp")    // vim
        || name.ends_with(".tmp")
}

/// Whether a filename is a Markdown note rather than an asset.
#[must_use]
pub fn is_note_file(name: &str) -> bool {
    name.rsplit_once('.')
        .map(|(_, extension)| NOTE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_index_and_obsidians_config_are_never_descended_into() {
        let rules = IgnoreRules::new();
        for ignored in [".serat", ".obsidian", ".git", ".trash", "node_modules"] {
            assert!(rules.is_ignored_dir(ignored), "{ignored} was not ignored");
        }
        for kept in ["10-projects", "20-systems", "attachments", "Daily Notes"] {
            assert!(!rules.is_ignored_dir(kept), "{kept} was ignored");
        }
    }

    #[test]
    fn extra_ignores_compose() {
        let rules = IgnoreRules::new().ignoring_dir("archive");
        assert!(rules.is_ignored_dir("archive"));
        assert!(rules.is_ignored_dir(".serat"));
        assert!(!rules.is_ignored_dir("10-projects"));
    }

    #[test]
    fn ignored_dir_matching_folds_case() {
        // Task #34, NFR-SEC-06. Windows and macOS default to case-insensitive
        // filesystems, so a case-sensitive comparison here would walk straight
        // into ".Obsidian", ".GIT" or "Node_Modules" on those platforms while
        // refusing to on Linux — the exact "my vault has different contents
        // depending on the machine" outcome C-02 promises cannot happen.
        let rules = IgnoreRules::new().ignoring_dir("archive");
        for ignored in [
            ".Serat",
            ".OBSIDIAN",
            ".Git",
            ".Trash",
            "Node_Modules",
            "NODE_MODULES",
            "ARCHIVE",
            "Archive",
        ] {
            assert!(rules.is_ignored_dir(ignored), "{ignored} was not ignored");
        }
    }

    #[test]
    fn our_own_temp_files_are_not_notes() {
        let rules = IgnoreRules::new();
        assert!(rules.is_ignored_file(".serat-tmp-note.md.1"));
        assert!(rules.is_ignored_file(".DS_Store"));
        assert!(rules.is_ignored_file("note.md~"));
        assert!(rules.is_ignored_file(".note.md.swp"));
        assert!(!rules.is_ignored_file("note.md"));
        assert!(!rules.is_ignored_file("contract.pdf"));
    }

    #[test]
    fn note_extensions_are_case_insensitive_and_assets_are_not_notes() {
        assert!(is_note_file("note.md"));
        assert!(is_note_file("NOTE.MD"));
        assert!(is_note_file("note.markdown"));
        assert!(!is_note_file("contract.pdf"));
        assert!(!is_note_file("README"));
        assert!(!is_note_file("archive.md.zip"));
    }
}
