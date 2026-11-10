//! Walking the vault.

use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serat_core::{NoteKind, Platform, SourceEntry, VaultError, VaultPath};

use crate::refusal::Refusal;
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
    scan(vault, vault.root().to_path_buf(), "")
}

/// [`walk`], from one directory: the entries at or below `dir`.
///
/// The refusal log is replaced for that subtree only, so a directory rescan does not
/// forget refusals elsewhere in the vault.
pub(crate) fn walk_dir(vault: &Vault, dir: &VaultPath) -> Result<Vec<SourceEntry>, VaultError> {
    if vault.ignore_rules().is_ignored_dir_chain(dir.as_str()) {
        vault.refusals().replace_scope(dir.as_str(), Vec::new());
        return Ok(Vec::new());
    }
    match vault.resolve(dir) {
        Ok(absolute) if absolute.is_dir() => scan(vault, absolute, dir.as_str()),
        // Gone, a file, or a symlink: nothing is listed there any more.
        _ => {
            vault.refusals().replace_scope(dir.as_str(), Vec::new());
            Ok(Vec::new())
        }
    }
}

/// The walk proper, from `start` (whose vault-relative path is `start_rel`, empty for
/// the root).
fn scan(vault: &Vault, start: PathBuf, start_rel: &str) -> Result<Vec<SourceEntry>, VaultError> {
    let user = vault.ignore_rules().user_patterns();
    let match_user = !user.is_empty();
    // Refusals are collected and swapped in at the end, so the log describes the
    // vault as this walk found it without ever being empty mid-walk, and a walk of
    // one directory cannot disturb the rest (#13 N1; task #14).
    let mut refused: Vec<Refusal> = Vec::new();
    let mut entries = Vec::new();
    let mut pending = vec![(start, start_rel.to_owned())];

    while let Some((directory, rel)) = pending.pop() {
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

            let is_dir = stat.is_dir();
            if is_dir {
                if vault.ignore_rules().is_ignored_dir(&name) {
                    continue;
                }
            } else if vault.ignore_rules().is_ignored_file(&name) {
                continue;
            }
            if match_user && user.is_ignored(&join(&rel, &name), is_dir) {
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
                Ok(path) => path,
                // Task #34's amendment: a name this platform cannot represent as
                // a `VaultPath` — a reserved Windows device found on a
                // vault synced in from elsewhere, say — is reported rather than
                // silently absent from the tree, search and backlinks
                // (FR-VLT-01, design goal 5). Skipping the entry is still
                // correct: there is no row to create for a name the current
                // platform's filesystem could not itself hold.
                //
                // A refused **directory** is one entry and is not entered, exactly as
                // `walk_tree` does it (#13 N1): the two walks used to disagree, this
                // one reporting every file beneath and that one the directory, so the
                // count of refusals depended on which walk ran last.
                Err(VaultError::InvalidPath { path, reason }) => {
                    refused.push(Refusal { path, reason });
                    continue;
                }
                // Anything else (the entry vanished mid-walk, or lies outside a
                // root this loop could not have produced in the first place) is
                // not a name refusal, so it is not reported as one.
                Err(_) => continue,
            };
            if is_dir {
                pending.push((entry.path(), path.as_str().to_owned()));
                continue;
            }
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

    vault.refusals().replace_scope(start_rel, refused);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// `rel/name`, or `name` at the root.
fn join(rel: &str, name: &str) -> String {
    if rel.is_empty() {
        name.to_owned()
    } else {
        format!("{rel}/{name}")
    }
}

/// Strip the vault root lexically from a path this walk built.
///
/// Only sound for paths the walk constructed by joining onto `vault.root()`, which is
/// already canonical, and only after symlinks have been excluded. Anything arriving
/// from outside the process goes through [`Vault::relativise`], which canonicalises.
pub(crate) fn relativise_walked(vault: &Vault, path: &Path) -> Result<VaultPath, VaultError> {
    let relative = path
        .strip_prefix(vault.root())
        .map_err(|_| VaultError::PathEscapesVault {
            path: path.to_string_lossy().into_owned(),
        })?;
    crate::vault_path_from_relative(Platform::current(), relative)
}

/// Modification time in milliseconds since the Unix epoch.
///
/// Zero when the platform will not say, which is a value and not a failure. mtime is
/// never the authority for change detection: the hash gate is, precisely because mtime
/// is unreliable across sync tools and filesystems. The catch-up and the search-hit
/// staleness hint do use it, as a cheap first test whose misses the hash check covers.
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

    #[cfg(not(windows))]
    #[test]
    fn names_only_windows_would_refuse_are_indexed_on_this_platform() {
        // Task #34's regression, pinned at the walk level rather than only at
        // `VaultPath::new`'s: `aux.md`, a colon in a filename and a directory
        // named with a trailing dot are all ordinary, legal names here, and the
        // walk must produce a row for each rather than silently dropping it
        // (FR-VLT-01, design goal 5). code-reviewer-2's probe named the first
        // and third of these exactly.
        let dir = tempfile::tempdir().expect("temp dir");
        write(dir.path(), "aux.md", b"# Aux\n");
        write(dir.path(), "Meeting 10:30.md", b"# Meeting\n");
        write(dir.path(), "Misc./n.md", b"# N\n");

        let vault = Vault::open(dir.path()).expect("open");
        let found: Vec<String> = vault
            .walk()
            .expect("walk")
            .into_iter()
            .map(|entry| entry.path.to_string())
            .collect();

        assert_eq!(
            found,
            vec!["Meeting 10:30.md", "Misc./n.md", "aux.md"],
            "a name only illegal on Windows was dropped on this platform"
        );
        assert!(
            vault.refusals().is_empty(),
            "an ordinary, legal name was reported as refused: {:?}",
            vault.refusals().snapshot()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_literal_backslash_in_a_real_file_name_is_reported_not_silently_aliased() {
        // Task #34's amendment, nit (b). A backslash is an ordinary character in
        // a Unix filename, but `VaultPath::new` also treats it as a separator —
        // deliberately, for a path a user typed or pasted — so passing an OS
        // path component straight through would silently re-read one file
        // `"a\b.md"` as the two-segment path `"a/b.md"`, aliasing a real name to
        // one nothing on disk actually has. `#[cfg(unix)]` because creating a
        // file with a literal backslash in its name needs a filesystem that
        // allows one; Windows cannot have this file to begin with.
        use std::os::unix::ffi::OsStrExt as _;

        let dir = tempfile::tempdir().expect("temp dir");
        let path = std::ffi::OsStr::from_bytes(b"a\\b.md");
        fs::write(dir.path().join(path), b"# Odd name\n").expect("write");

        let vault = Vault::open(dir.path()).expect("open");
        let found = vault.walk().expect("walk");

        assert!(
            found.is_empty(),
            "a literal backslash was silently split into a two-segment path: {found:?}"
        );
        let refusals = vault.refusals().snapshot();
        assert_eq!(
            refusals.len(),
            1,
            "the refusal was not reported: {refusals:?}"
        );
        assert!(
            refusals[0].reason.contains('\\'),
            "the reported reason does not explain the backslash: {refusals:?}"
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
            let absolute = vault
                .resolve(&entry.path)
                .expect("resolve a path the walk produced");
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

    /// Create a symlink, or say why we could not.
    ///
    /// Mirrors `serat-app`'s helper (task #29) rather than the `#[cfg(all(test, unix))]`
    /// gate this crate's own `symlink_tests` module uses, and the reason is specific to
    /// this guard rather than a general objection to that gate. **The claim being tested
    /// is itself cross-platform**: `walk`'s comment states that `DirEntry::metadata`
    /// "does not follow symlinks on either platform", and that is what makes one stat
    /// sufficient where two were used before (task #24). A test compiled on one platform
    /// cannot check a sentence about both.
    ///
    /// On Unix a failure is a real error, not a platform limit (F-306). On Windows the
    /// privilege may genuinely be absent, and then this says so out loud rather than
    /// passing having asserted nothing (F-58).
    fn make_symlink(
        target: &std::path::Path,
        link: &std::path::Path,
        dir: bool,
    ) -> Result<(), String> {
        #[cfg(unix)]
        {
            let _ = dir;
            std::os::unix::fs::symlink(target, link).expect(
                "symlink creation on Unix needs no privilege; a failure here is a real \
                 error, not a reason to skip a containment test",
            );
            Ok::<(), String>(())
        }
        #[cfg(windows)]
        {
            let made = if dir {
                std::os::windows::fs::symlink_dir(target, link)
            } else {
                std::os::windows::fs::symlink_file(target, link)
            };
            made.map_err(|err| {
                format!(
                    "symlink creation failed ({err}) -- Windows needs Developer Mode or \
                     an elevated process, which a test runner cannot assume"
                )
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link, dir);
            Err("no symlink primitive is wired for this platform".to_owned())
        }
    }

    /// `cargo test` has no third outcome for a plain `#[test]`, so this is the loudest
    /// signal short of failing outright -- and failing outright would redden the suite
    /// on every unprivileged Windows machine for a platform limit rather than a defect.
    fn skip_loudly(test_name: &str, reason: &str) {
        eprintln!("SKIPPED {test_name}: {reason}");
    }

    #[test]
    fn the_walk_does_not_enumerate_a_symlink_out_of_the_vault() {
        // The FIRST half of what the test below calls "the second half of that", and
        // until now nothing pinned it: `walk.rs`'s `stat.is_symlink()` skip could be
        // deleted and this crate's whole suite stayed green at 64/64 (F-362). Three
        // artifacts cite that line -- the invariant task #27 landed so a changer would
        // read it, this module's lexical-strip soundness argument, and the threat
        // model's T-07 "walk.rs skips every symlink, so a link is never indexed" -- and
        // none of them executed it.
        //
        // Deleting the skip makes this fail with the link in the output, which is the
        // only thing that makes the three citations mean anything.
        let dir = tempfile::tempdir().expect("temp");
        let outside = tempfile::tempdir().expect("outside");
        let secret = outside.path().join("secret.md");
        fs::write(&secret, b"outside the vault").expect("write outside");

        let vault = Vault::open(dir.path()).expect("open");
        write(dir.path(), "real.md", b"# inside");

        if let Err(reason) = make_symlink(&secret, &dir.path().join("escape.md"), false) {
            skip_loudly(
                "the_walk_does_not_enumerate_a_symlink_out_of_the_vault",
                &reason,
            );
            return;
        }

        let entries = walk(&vault).expect("walk");
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();

        assert!(
            !paths.iter().any(|p| p.contains("escape")),
            "the walk enumerated a symlink pointing outside the vault: {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p.contains("real")),
            "the walk skipped the ordinary note as well, so this proves nothing: {paths:?}"
        );
    }

    #[test]
    fn the_walk_does_not_descend_through_a_symlinked_directory() {
        // The other half of the invariant's own wording -- "no symlink is followed,
        // directory or file" -- and the half that makes a vault containing a link to
        // its own parent terminate rather than walk forever.
        let dir = tempfile::tempdir().expect("temp");
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("hidden.md"), b"outside").expect("write outside");

        let vault = Vault::open(dir.path()).expect("open");
        write(dir.path(), "real.md", b"# inside");

        if let Err(reason) = make_symlink(outside.path(), &dir.path().join("linked"), true) {
            skip_loudly(
                "the_walk_does_not_descend_through_a_symlinked_directory",
                &reason,
            );
            return;
        }

        let entries = walk(&vault).expect("walk");
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();

        // Assert on the LINK, not on what is behind it. An earlier version of this
        // asserted `hidden.md` was absent and passed with the guard DELETED, because a
        // symlinked directory reports `is_symlink() == true` and `is_dir() == false`,
        // so the walk never descends into it either way -- non-descent comes from the
        // `is_dir()` branch rather than from the skip. That made it a near-miss dressed
        // as coverage, the shape F-315 records. What the skip actually decides is
        // whether the link ITSELF is enumerated.
        assert!(
            !paths.iter().any(|p| p.contains("linked")),
            "the walk enumerated a symlinked directory as an entry: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.contains("hidden")),
            "the walk descended through a symlinked directory: {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p.contains("real")),
            "the walk found nothing at all, so this proves nothing: {paths:?}"
        );
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
        assert!(relativise_walked(&vault, &outside).is_err());
    }

    /// F-12 (task #107): the walk strips the root lexically and never canonicalises an
    /// entry. Pinned by count, so load cannot move it; the earlier guard timed the two
    /// ways and could invert on a busy host. The control: `relativise`, which does
    /// canonicalise, is counted once per call by the same hook.
    #[test]
    fn the_walk_does_not_canonicalise_every_entry() {
        use crate::CANONICALISE_CALLS;
        const FILES: usize = 200;
        let dir = tempfile::tempdir().expect("temp");
        for i in 0..FILES {
            write(
                dir.path(),
                &format!("{:02}-folder/note-{i:05}.md", i % 10),
                b"# Note\n",
            );
        }
        let vault = Vault::open(dir.path()).expect("open");

        let before = CANONICALISE_CALLS.with(std::cell::Cell::get);
        let entries = vault.walk().expect("walk");
        let during_walk = CANONICALISE_CALLS.with(std::cell::Cell::get) - before;
        assert_eq!(entries.len(), FILES);
        assert_eq!(
            during_walk, 0,
            "the walk canonicalised {during_walk} times for {FILES} entries"
        );

        for entry in &entries {
            let absolute = vault.resolve(&entry.path).expect("resolve");
            vault.relativise(&absolute).expect("relativise");
        }
        let with_relativise = CANONICALISE_CALLS.with(std::cell::Cell::get) - before;
        assert_eq!(
            with_relativise, FILES,
            "the control: relativise must count one canonicalise per entry"
        );
    }
}
