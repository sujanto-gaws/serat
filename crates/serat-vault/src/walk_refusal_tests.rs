//! Walk-level tests for the refusal log (task #34).

use std::fs;
use std::os::unix::ffi::OsStrExt as _;

use crate::Vault;

#[test]
fn a_non_utf8_name_is_reported_and_a_rewalk_does_not_duplicate_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let odd = dir.path().join(std::ffi::OsStr::from_bytes(b"caf\xe9.md"));
    fs::write(&odd, b"x").expect("write");
    let vault = Vault::open(dir.path()).expect("open");
    for _ in 0..10 {
        assert!(vault.walk().expect("walk").is_empty());
    }
    assert_eq!(
        vault.refusals().len(),
        1,
        "{:?}",
        vault.refusals().snapshot()
    );
    fs::remove_file(&odd).expect("rm");
    vault.walk().expect("walk");
    assert!(
        vault.refusals().is_empty(),
        "a deleted file is still reported"
    );
}

fn odd_dir(root: &std::path::Path) -> std::path::PathBuf {
    root.join(std::ffi::OsStr::from_bytes(b"bad\\dir"))
}

#[test]
fn a_refused_directory_is_one_entry_in_both_walks_and_is_not_entered() {
    // #13 N1. `walk_tree` reported the directory and `walk` every file beneath it, so
    // the count depended on which walk ran last.
    let dir = tempfile::tempdir().expect("temp dir");
    fs::create_dir(odd_dir(dir.path())).expect("mkdir");
    for name in ["a.md", "b.md", "c.md"] {
        fs::write(odd_dir(dir.path()).join(name), b"x").expect("write");
    }
    fs::write(dir.path().join("fine.md"), b"x").expect("write");
    let vault = Vault::open(dir.path()).expect("open");

    let found = vault.walk().expect("walk");
    assert_eq!(found.len(), 1, "{found:?}");
    let by_walk = vault.refusals().snapshot();
    assert_eq!(by_walk.len(), 1, "{by_walk:?}");

    vault.walk_tree("", |_, _| {}).expect("tree walk");
    let by_tree = vault.refusals().snapshot();
    assert_eq!(
        by_walk, by_tree,
        "the two walks disagree about the refusals"
    );
}

#[test]
fn refusals_leave_at_the_next_walk_and_a_directory_walk_only_touches_its_subtree() {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::create_dir(dir.path().join("sub")).expect("mkdir");
    let odd = |parent: &std::path::Path| parent.join(std::ffi::OsStr::from_bytes(b"x\\y.md"));
    fs::write(odd(dir.path()), b"x").expect("write");
    fs::write(odd(&dir.path().join("sub")), b"x").expect("write");
    let vault = Vault::open(dir.path()).expect("open");
    vault.walk_tree("", |_, _| {}).expect("tree walk");
    assert_eq!(
        vault.refusals().len(),
        2,
        "{:?}",
        vault.refusals().snapshot()
    );

    // Deleting the one in sub/ and re-listing only sub/ forgets that one alone.
    fs::remove_file(odd(&dir.path().join("sub"))).expect("rm");
    vault
        .walk_dir(&serat_core::VaultPath::new("sub").expect("path"))
        .expect("walk_dir");
    let left = vault.refusals().snapshot();
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(!left[0].path.starts_with("sub/"), "{left:?}");

    // The next walk of the root, of either kind, forgets the rest.
    fs::remove_file(odd(dir.path())).expect("rm");
    vault.walk_tree("", |_, _| {}).expect("tree walk");
    assert!(
        vault.refusals().is_empty(),
        "{:?}",
        vault.refusals().snapshot()
    );
}
