//! Walking the vault.

use std::fs::{self, Metadata};
use std::time::UNIX_EPOCH;

use serat_core::{NoteKind, VaultError, VaultPath};

use crate::{Vault, ignore::is_note_file};

/// One indexable file, as the walk found it.
///
/// No hash: hashing means reading every byte of every file, and the walk's job is to
/// enumerate. The rebuild reads and hashes as it goes ([`Vault::read`]); the
/// incremental path reads only what changed. Hashing here would make listing a vault
/// as expensive as indexing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultEntry {
    /// Vault-relative path.
    pub path: VaultPath,
    /// Whether this file is parsed or extracted.
    pub kind: NoteKind,
    /// Size in bytes.
    pub size: u64,
    /// Modification time, milliseconds since the Unix epoch.
    pub mtime_ms: i64,
}

/// Every indexable file under the vault root, sorted by path.
///
/// Sorted because a rebuild that visits files in directory order produces a different
/// set of rowids on every run, and "delete the index, rebuild, get the same answers"
/// (FR-VLT-02, and the T1 rebuild-equivalence test) is easier to assert when the only
/// thing that varies is the content.
///
/// Directory symlinks are not followed. A vault containing a link to its own parent
/// would otherwise walk forever, and Serat indexes the folder the user pointed it at
/// rather than everything reachable from it.
pub(crate) fn walk(vault: &Vault) -> Result<Vec<VaultEntry>, VaultError> {
    let mut entries = Vec::new();
    let mut pending = vec![vault.root().to_path_buf()];

    while let Some(directory) = pending.pop() {
        let listing = match fs::read_dir(&directory) {
            Ok(listing) => listing,
            // One unreadable directory must not stop the vault indexing (design goal
            // 5). The root is the exception: if that is unreadable there is no vault.
            Err(err) if directory == vault.root() => {
                return Err(VaultError::io(crate::display(&directory), &err));
            }
            Err(_) => continue,
        };

        for entry in listing.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            // symlink_metadata, not metadata: a symlink to a directory must be
            // recognised as a symlink rather than followed into a cycle.
            let Ok(stat) = entry.metadata() else { continue };
            let Ok(link_stat) = fs::symlink_metadata(entry.path()) else {
                continue;
            };

            if link_stat.is_symlink() {
                continue;
            }

            if stat.is_dir() {
                if !vault.ignore_rules().is_ignored_dir(&name) {
                    pending.push(entry.path());
                }
                continue;
            }

            if vault.ignore_rules().is_ignored_file(&name) {
                continue;
            }

            let path = match vault.relativise(&entry.path()) {
                Ok(path) => path,
                Err(_) => continue,
            };
            entries.push(VaultEntry {
                kind: if is_note_file(&name) {
                    NoteKind::Note
                } else {
                    NoteKind::Asset
                },
                size: stat.len(),
                mtime_ms: mtime_ms(&stat),
                path,
            });
        }
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// Modification time in milliseconds since the Unix epoch.
///
/// Zero when the platform will not say, which is a value and not a failure: mtime is
/// recorded for display and ordering and is never used for change detection — that is
/// what the hash gate is for, precisely because mtime is unreliable across sync tools
/// and filesystems.
pub(crate) fn mtime_ms(stat: &Metadata) -> i64 {
    stat.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|since| i64::try_from(since.as_millis()).ok())
        .unwrap_or(0)
}

/// Paths that exist on disk right now, as a set, for reconciling against the index.
#[must_use]
pub fn paths_of(entries: &[VaultEntry]) -> Vec<VaultPath> {
    entries.iter().map(|entry| entry.path.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IgnoreRules, Vault};

    fn write(root: &std::path::Path, relative: &str, bytes: &[u8]) {
        let target = root.join(relative);
        fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
        fs::write(target, bytes).expect("write");
    }

    fn fixture() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();

        write(root, "10-projects/serat.md", b"# Serat\n");
        write(root, "10-projects/notes/meeting.md", b"# Meeting\n");
        write(root, "40-refs/contract.pdf", b"%PDF-1.7\n");
        write(root, "README.markdown", b"# Read me\n");

        // None of these may appear in the walk.
        write(root, ".serat/index.db", b"not a note");
        write(root, ".obsidian/workspace.json", b"{}");
        write(root, ".git/config", b"[core]");
        write(root, "node_modules/pkg/index.js", b"module.exports = {}");
        write(root, ".DS_Store", b"junk");
        write(root, ".serat-tmp-serat.md.1.0", b"half a write");
        write(root, "10-projects/serat.md~", b"editor backup");

        let vault = Vault::open(root).expect("open");
        (dir, vault)
    }

    #[test]
    fn the_walk_finds_notes_and_assets_and_nothing_else() {
        let (_dir, vault) = fixture();
        let found: Vec<String> = vault
            .walk()
            .expect("walk")
            .into_iter()
            .map(|entry| entry.path.to_string())
            .collect();

        assert_eq!(
            found,
            vec![
                "10-projects/notes/meeting.md",
                "10-projects/serat.md",
                "40-refs/contract.pdf",
                "README.markdown",
            ]
        );
    }

    #[test]
    fn the_walk_is_sorted_so_a_rebuild_is_reproducible() {
        let (_dir, vault) = fixture();
        let once = vault.walk().expect("walk");
        let twice = vault.walk().expect("walk");
        assert_eq!(once, twice);

        let mut sorted = once.clone();
        sorted.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(once, sorted);
    }

    #[test]
    fn assets_and_notes_are_distinguished() {
        let (_dir, vault) = fixture();
        let entries = vault.walk().expect("walk");
        let kinds: Vec<(String, NoteKind)> = entries
            .iter()
            .map(|entry| (entry.path.to_string(), entry.kind))
            .collect();

        assert_eq!(
            kinds,
            vec![
                ("10-projects/notes/meeting.md".to_owned(), NoteKind::Note),
                ("10-projects/serat.md".to_owned(), NoteKind::Note),
                ("40-refs/contract.pdf".to_owned(), NoteKind::Asset),
                ("README.markdown".to_owned(), NoteKind::Note),
            ]
        );
    }

    #[test]
    fn sizes_are_real_and_mtimes_are_populated() {
        let (_dir, vault) = fixture();
        let entries = vault.walk().expect("walk");
        let serat = entries
            .iter()
            .find(|entry| entry.path.as_str() == "10-projects/serat.md")
            .expect("found");
        assert_eq!(serat.size, 8);
        assert!(serat.mtime_ms > 0, "mtime was not populated");
    }

    #[test]
    fn extra_ignore_rules_are_honoured_by_the_walk() {
        let dir = tempfile::tempdir().expect("temp dir");
        write(dir.path(), "keep/note.md", b"keep");
        write(dir.path(), "archive/old.md", b"drop");

        let vault = Vault::with_rules(dir.path(), IgnoreRules::new().ignoring_dir("archive"))
            .expect("open");
        let found: Vec<String> = vault
            .walk()
            .expect("walk")
            .into_iter()
            .map(|entry| entry.path.to_string())
            .collect();
        assert_eq!(found, vec!["keep/note.md"]);
    }

    #[test]
    fn an_empty_vault_walks_to_nothing_rather_than_failing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::open(dir.path()).expect("open");
        assert!(vault.walk().expect("walk").is_empty());
    }
}
