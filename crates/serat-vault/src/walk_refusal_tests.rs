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
