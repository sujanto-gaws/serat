//! NFR-MNT-01, F-444, F-255: the `check-conflict-markers` binary itself fails
//! on a planted marker and passes on a clean tree, and it reads the tree it is
//! *run* in (`--root`), never the compiled-in checkout (task #45).

use std::fs;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_check-conflict-markers");

fn repo(doc: &str) -> tempfile::TempDir {
    let t = tempfile::tempdir().expect("tempdir");
    let r = t.path();
    fs::write(r.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
    fs::write(r.join("CLAUDE.md"), "# stub\n").unwrap();
    fs::create_dir_all(r.join("docs")).unwrap();
    fs::write(r.join("docs/a.md"), doc).unwrap();
    t
}

fn run(root: &std::path::Path) -> (Output, String) {
    let out = Command::new(BIN)
        .arg("--root")
        .arg(root)
        .env_remove("SERAT_REPO_ROOT")
        .output()
        .expect("run check-conflict-markers");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    (out, text)
}

#[test]
fn a_planted_marker_fails_the_binary_and_is_named() {
    let t = repo("# t\n\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> other\n");
    let (out, text) = run(t.path());
    assert!(!out.status.success(), "must fail:\n{text}");
    assert!(text.contains("3 unresolved conflict marker(s)"), "{text}");
    assert!(text.contains("docs/a.md:3:"), "{text}");
    assert!(text.contains("root: "), "root is announced first:\n{text}");
}

#[test]
fn a_clean_tree_passes_and_says_how_much_it_scanned() {
    let t = repo("# t\n\nProse.\n");
    let (out, text) = run(t.path());
    assert!(out.status.success(), "must pass:\n{text}");
    assert!(text.contains("(2 file(s) scanned)"), "{text}");
}
