//! The ignore file (FR-VLT-07) through the walks: one rule, three enumerations.

use std::fs;
use std::path::Path;

use serat_core::{SourceEntry, VaultPath};

use crate::{IGNORE_FILE, Vault};

fn write(root: &Path, relative: &str, bytes: &[u8]) {
    let target = root.join(relative);
    fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
    fs::write(target, bytes).expect("write");
}

fn fixture(ignore: &str) -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    write(root, "keep.md", b"# keep\n");
    write(root, "archive/old.md", b"# old\n");
    write(root, "archive/deep/older.md", b"# older\n");
    write(root, "notes/archive/inner.md", b"# inner\n");
    write(root, "notes/draft.tmpl.md", b"# tmpl\n");
    write(root, "notes/live.md", b"# live\n");
    write(root, IGNORE_FILE, ignore.as_bytes());
    let vault = Vault::open(root).expect("open");
    (dir, vault)
}

fn paths(entries: Vec<SourceEntry>) -> Vec<String> {
    entries.into_iter().map(|e| e.path.to_string()).collect()
}

#[test]
fn an_entry_for_archive_removes_it_from_the_walk_and_nothing_else() {
    // FR-VLT-07's acceptance example.
    let (_dir, vault) = fixture("/archive\n*.tmpl.md\n");
    assert_eq!(
        paths(vault.walk().expect("walk")),
        ["keep.md", "notes/archive/inner.md", "notes/live.md"]
    );
}

#[test]
fn the_tree_walk_and_the_directory_walk_apply_the_same_rules() {
    let (_dir, vault) = fixture("/archive\n*.tmpl.md\n");
    let mut seen = Vec::new();
    vault
        .walk_tree("", |dir, entries| {
            for entry in entries {
                seen.push(format!("{dir}/{}", entry.name));
            }
        })
        .expect("tree walk");
    assert!(
        !seen.iter().any(|p| p == "/archive"),
        "the anchored entry did not hide the root's archive: {seen:?}"
    );
    assert!(!seen.iter().any(|p| p.contains("tmpl")), "{seen:?}");
    assert!(
        seen.iter().any(|p| p == "notes/archive"),
        "the anchored entry hid a nested archive: {seen:?}"
    );

    let notes = VaultPath::new("notes").expect("path");
    assert_eq!(
        paths(vault.walk_dir(&notes).expect("walk_dir")),
        ["notes/archive/inner.md", "notes/live.md"]
    );
    // An ignored directory lists nothing when asked for directly, nor does one
    // beneath it.
    let deep = VaultPath::new("archive/deep").expect("path");
    assert!(vault.walk_dir(&deep).expect("walk_dir").is_empty());
}

#[test]
fn a_reload_changes_the_next_walk() {
    let (dir, vault) = fixture("/archive\n");
    assert_eq!(vault.walk().expect("walk").len(), 4);
    fs::write(dir.path().join(IGNORE_FILE), b"").expect("write");
    assert!(vault.reload_ignore_file());
    assert_eq!(vault.walk().expect("walk").len(), 6);
    fs::remove_file(dir.path().join(IGNORE_FILE)).expect("rm");
    assert!(!vault.reload_ignore_file(), "empty to absent is no change");
}

#[test]
fn the_ignore_file_is_not_itself_indexed() {
    let (_dir, vault) = fixture("/archive\n");
    assert!(
        !paths(vault.walk().expect("walk"))
            .iter()
            .any(|p| p.contains("ignore"))
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_ignore_file_is_not_read() {
    let (dir, vault) = fixture("/archive\n");
    let outside = tempfile::tempdir().expect("outside");
    fs::write(outside.path().join("rules"), b"*.md\n").expect("write");
    fs::remove_file(dir.path().join(IGNORE_FILE)).expect("rm");
    std::os::unix::fs::symlink(outside.path().join("rules"), dir.path().join(IGNORE_FILE))
        .expect("symlink");
    vault.reload_ignore_file();
    assert_eq!(vault.walk().expect("walk").len(), 6, "a symlink was read");
}
