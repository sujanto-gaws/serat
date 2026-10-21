//! The `.serat/` ignore offer (NFR-CMP-04, T-06).
//!
//! `append_gitignore_entry` is **the single named exception to T-06** (only Markdown is
//! writable): it takes no path, and writes exactly one line, `/.serat/`, to the
//! `.gitignore` at the vault root. The write itself lives in `serat-vault`
//! (`Vault::append_gitignore_entry` and `Vault::update_prefs`), which owns the one
//! atomic-write implementation; this module reads, and maps the answer onto the IPC
//! types.
//!
//! # What counts as a repository, and the gap
//!
//! A repository is a `.git` entry **at the vault root, file or directory**: a worktree or
//! a submodule has a `.git` *file*, and an offer that ignored those would be wrong for
//! exactly the people who use them. **Stated gap:** only the vault root is looked at. A
//! vault that is a subfolder of a larger repository (the repository root is above it), or
//! one that contains nested repositories, reports [`GitignoreOffer::NotAGitRepo`], and the
//! `.gitignore` consulted is the vault root's own, not one higher up. Walking upward would
//! mean reading and writing outside the vault, which T-07 forbids.
//!
//! # The write
//!
//! Done by `serat-vault`: atomic (temp file, fsync, rename), the file's existing bytes
//! untouched, and the added line uses **the file's own line ending**: `\r\n` if the file's line breaks are mostly
//! `\r\n`, else `\n` (also the choice for a file with no line break, and for a new file).
//! A final line with no terminator gets one first. A `.gitignore` that is a symlink is
//! refused rather than replaced: a rename over a symlink would swap the user's link for a
//! regular file.
//!
//! "Don't ask again" lives in `.serat/prefs.json`, safe under C-02: losing it only shows
//! the offer again.

use std::path::Path;

use serat_vault::{Vault, gitignore_ignores_serat};

use crate::error::from_vault_error;
use crate::ipc::{GitignoreAppended, GitignoreOffer, IpcError};

const IGNORE_FILE: &str = ".gitignore";
const PREFS_FILE: &str = "prefs.json";
const DECLINED_KEY: &str = "gitignore_offer_declined";

/// Whether to show the offer for the vault rooted at `root`.
#[must_use]
pub fn status(root: &Path) -> GitignoreOffer {
    if std::fs::symlink_metadata(root.join(".git")).is_err() {
        return GitignoreOffer::NotAGitRepo;
    }
    if let Ok(bytes) = std::fs::read(root.join(IGNORE_FILE))
        && gitignore_ignores_serat(&bytes)
    {
        return GitignoreOffer::AlreadyIgnored;
    }
    if declined(root) {
        return GitignoreOffer::Declined;
    }
    GitignoreOffer::Offer
}

/// Append `/.serat/` to the vault root's `.gitignore`.
///
/// # Errors
///
/// [`IpcError::Io`] if the file is a symlink, cannot be read, or cannot be written.
pub fn append(vault: &Vault) -> Result<GitignoreAppended, IpcError> {
    let done = vault.append_gitignore_entry().map_err(from_vault_error)?;
    Ok(GitignoreAppended {
        created: done.created,
        already_present: done.already_present,
    })
}

/// Record "Don't ask again" in `.serat/prefs.json`.
///
/// # Errors
///
/// [`IpcError::Io`] if `.serat` is a symlink or the file cannot be written.
pub fn decline(vault: &Vault) -> Result<(), IpcError> {
    vault
        .update_prefs(|existing| {
            // Preserve any other keys a later build wrote.
            let mut prefs = existing.map(parse_prefs).unwrap_or_default();
            prefs.insert(DECLINED_KEY.to_owned(), serde_json::Value::Bool(true));
            serde_json::to_vec_pretty(&prefs).ok()
        })
        .map_err(from_vault_error)
}

fn parse_prefs(bytes: &[u8]) -> serde_json::Map<String, serde_json::Value> {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .and_then(|v| match v {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

fn declined(root: &Path) -> bool {
    std::fs::read(root.join(".serat").join(PREFS_FILE))
        .map(|b| parse_prefs(&b))
        .unwrap_or_default()
        .get(DECLINED_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp directory that derefs to its path and is removed on drop.
    struct Root(tempfile::TempDir);

    impl std::ops::Deref for Root {
        type Target = Path;
        fn deref(&self) -> &Path {
            self.0.path()
        }
    }

    fn vault_of(root: &Path) -> Vault {
        Vault::open(root).expect("open")
    }

    fn root(_name: &str) -> Root {
        Root(tempfile::tempdir().expect("fixture"))
    }

    #[test]
    fn no_git_entry_means_no_offer() {
        assert_eq!(status(&root("none")), GitignoreOffer::NotAGitRepo);
    }

    #[test]
    fn a_git_directory_or_a_git_file_both_count() {
        let a = root("gitdir");
        std::fs::create_dir(a.join(".git")).unwrap();
        assert_eq!(status(&a), GitignoreOffer::Offer);
        let b = root("gitfile");
        std::fs::write(b.join(".git"), "gitdir: ../elsewhere\n").unwrap();
        assert_eq!(status(&b), GitignoreOffer::Offer);
    }

    #[test]
    fn the_four_spellings_are_already_ignored_and_near_misses_are_not() {
        for line in [".serat", ".serat/", "/.serat", "/.serat/"] {
            let r = root("spell");
            std::fs::create_dir(r.join(".git")).unwrap();
            std::fs::write(r.join(".gitignore"), format!("target\r\n{line}  \r\n")).unwrap();
            assert_eq!(status(&r), GitignoreOffer::AlreadyIgnored, "{line}");
        }
        let r = root("near");
        std::fs::create_dir(r.join(".git")).unwrap();
        std::fs::write(r.join(".gitignore"), "# /.serat/\n.serat-old\n!/.serat/\n").unwrap();
        assert_eq!(status(&r), GitignoreOffer::Offer);
    }

    #[test]
    fn append_creates_the_file_and_is_idempotent() {
        let r = root("create");
        std::fs::create_dir(r.join(".git")).unwrap();
        let first = append(&vault_of(&r)).unwrap();
        assert!(first.created && !first.already_present);
        assert_eq!(
            std::fs::read_to_string(r.join(".gitignore")).unwrap(),
            "/.serat/\n"
        );
        let second = append(&vault_of(&r)).unwrap();
        assert!(!second.created && second.already_present);
        assert_eq!(
            std::fs::read_to_string(r.join(".gitignore")).unwrap(),
            "/.serat/\n"
        );
        assert_eq!(status(&r), GitignoreOffer::AlreadyIgnored);
    }

    #[test]
    fn append_keeps_existing_bytes_and_uses_the_files_line_ending() {
        let r = root("crlf");
        std::fs::write(r.join(".gitignore"), b"target\r\n*.log").unwrap();
        append(&vault_of(&r)).unwrap();
        assert_eq!(
            std::fs::read(r.join(".gitignore")).unwrap(),
            b"target\r\n*.log\r\n/.serat/\r\n"
        );

        let r = root("lf");
        std::fs::write(r.join(".gitignore"), b"target\n").unwrap();
        append(&vault_of(&r)).unwrap();
        assert_eq!(
            std::fs::read(r.join(".gitignore")).unwrap(),
            b"target\n/.serat/\n"
        );

        // Mixed, mostly LF: LF wins.
        let r = root("mixed");
        std::fs::write(r.join(".gitignore"), b"a\r\nb\nc\nd\n").unwrap();
        append(&vault_of(&r)).unwrap();
        assert!(
            std::fs::read(r.join(".gitignore"))
                .unwrap()
                .ends_with(b"d\n/.serat/\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_gitignore_is_refused_and_left_a_symlink() {
        let r = root("symlink");
        let target = r.join("real-ignore");
        std::fs::write(&target, "x\n").unwrap();
        std::os::unix::fs::symlink(&target, r.join(".gitignore")).unwrap();
        assert!(matches!(append(&vault_of(&r)), Err(IpcError::Io { .. })));
        assert!(
            std::fs::symlink_metadata(r.join(".gitignore"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "x\n");
    }

    #[test]
    fn declining_is_remembered_in_prefs_json_and_writes_nothing_else() {
        let r = root("decline");
        std::fs::create_dir(r.join(".git")).unwrap();
        decline(&vault_of(&r)).unwrap();
        assert_eq!(status(&r), GitignoreOffer::Declined);
        assert!(!r.join(".gitignore").exists());
        // Idempotent, and other keys survive.
        std::fs::write(
            r.join(".serat/prefs.json"),
            r#"{"future":1,"gitignore_offer_declined":true}"#,
        )
        .unwrap();
        decline(&vault_of(&r)).unwrap();
        let text = std::fs::read_to_string(r.join(".serat/prefs.json")).unwrap();
        assert!(text.contains("future"), "{text}");
        // Losing the file only shows the offer again (C-02).
        std::fs::remove_file(r.join(".serat/prefs.json")).unwrap();
        assert_eq!(status(&r), GitignoreOffer::Offer);
    }

    #[test]
    fn already_ignored_wins_over_declined() {
        let r = root("both");
        std::fs::create_dir(r.join(".git")).unwrap();
        decline(&vault_of(&r)).unwrap();
        append(&vault_of(&r)).unwrap();
        assert_eq!(status(&r), GitignoreOffer::AlreadyIgnored);
    }
}
