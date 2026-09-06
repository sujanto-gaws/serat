//! Walking the vault.

use std::fs::{self, Metadata};
use std::path::Path;
use std::time::UNIX_EPOCH;

use serat_core::{NoteKind, SourceEntry, VaultError, VaultPath};

use crate::{Vault, ignore::is_note_file};

/// Every indexable file under the vault root, sorted by path.
///
/// Sorted because a rebuild that visits files in directory order produces a different
/// set of rowids on every run, and "delete the index, rebuild, get the same answers"
/// (FR-VLT-02, and the T1 rebuild-equivalence test) is easier to assert when the only
/// thing that varies is the content.
///
/// No symlink is followed, directory or file (`stat.is_symlink()` below skips both,
/// not only directories as an earlier version of this comment claimed). A vault
/// containing a link to its own parent would otherwise walk forever, and Serat indexes
/// the folder the user pointed it at rather than everything reachable from it — the
/// same rule [`Vault::resolve`] enforces for a path that arrives without going through
/// this walk at all (T-07).
pub(crate) fn walk(vault: &Vault) -> Result<Vec<SourceEntry>, VaultError> {
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
            // One stat, not two. DirEntry::metadata does not follow symlinks on either
            // platform, so is_symlink here is the same answer fs::symlink_metadata
            // would have given — and the second call was a syscall per file on the
            // enumeration path.
            let Ok(stat) = entry.metadata() else { continue };

            if stat.is_symlink() {
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

            // F-12. relativise canonicalises, which costs a syscall per entry and
            // measured 1.42 s of the 1.85 s this walk took on the 20k vault — and
            // tauri-app-dev then measured what that thread does to a cold start: 1410 ms
            // against 821 ms with the walk moved, because it contends for the disk and
            // the cores WebView2 is starting on.
            //
            // There is nothing left for canonicalisation to resolve. This loop built
            // every path itself by joining onto an already-canonical root, and it has
            // already skipped symlinks above, so stripping the known prefix lexically
            // gives the same answer. relativise stays for paths arriving from outside
            // the process — watcher events, arguments, drag-and-drop — which is what
            // its documentation says it is for.
            let path = match relativise_walked(vault, &entry.path()) {
                Some(path) => path,
                None => continue,
            };
            entries.push(SourceEntry {
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

/// Strip the vault root lexically from a path this walk built.
///
/// Only sound for paths the walk constructed by joining onto `vault.root()`, which is
/// already canonical, and only after symlinks have been excluded. Anything arriving
/// from outside the process goes through [`Vault::relativise`], which canonicalises.
fn relativise_walked(vault: &Vault, path: &Path) -> Option<VaultPath> {
    let relative = path.strip_prefix(vault.root()).ok()?;
    VaultPath::new(relative.to_string_lossy()).ok()
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
pub fn paths_of(entries: &[SourceEntry]) -> Vec<VaultPath> {
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

#[cfg(test)]
mod lexical_tests {
    use super::*;
    use crate::Vault;

    fn write(root: &Path, relative: &str, bytes: &[u8]) {
        let target = root.join(relative);
        fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
        fs::write(target, bytes).expect("write");
    }

    #[test]
    fn the_lexical_strip_agrees_with_canonicalising_relativise() {
        // F-12's correctness condition. The walk stopped canonicalising per entry
        // because it builds every path itself by joining onto an already-canonical
        // root and has already excluded symlinks — so the two must give the same
        // answer for every path the walk produces. If they ever diverge, the walk and
        // the watcher would disagree about a file's identity and the same note would
        // index twice under two spellings.
        let dir = tempfile::tempdir().expect("temp");
        let root = dir.path();
        for relative in [
            "note.md",
            "10-projects/serat.md",
            "10-projects/notes/deep/meeting.md",
            "40-refs/contract.pdf",
            "spaces in the name/and here.md",
            "Mixed-Case/README.markdown",
        ] {
            write(root, relative, b"body");
        }

        let vault = Vault::open(root).expect("open");
        let entries = vault.walk().expect("walk");
        assert_eq!(entries.len(), 6);

        for entry in &entries {
            let absolute = vault.resolve(&entry.path).expect("resolve a path the walk produced");
            let canonical = vault
                .relativise(&absolute)
                .expect("relativise a path the walk produced");
            assert_eq!(
                entry.path, canonical,
                "the lexical strip and relativise disagree about {}",
                entry.path
            );
        }
    }

    #[test]
    fn the_walk_still_refuses_to_leave_the_vault() {
        // Stripping a prefix lexically is only safe because the walk never descends
        // into a symlink. This pins the second half of that: a path that does not
        // start with the root produces nothing rather than a path built from whatever
        // strip_prefix left behind.
        let dir = tempfile::tempdir().expect("temp");
        let vault = Vault::open(dir.path()).expect("open");
        let outside = dir.path().parent().expect("parent").join("elsewhere.md");
        assert!(relativise_walked(&vault, &outside).is_none());
    }
}
