//! Determinism — the one property NFR-MNT-02 turns on.
//!
//! Verifies: **NFR-MNT-02** ("one seed produces byte-identical output"),
//! **US-35**, **C-06**. If any test here fails, every recorded number in
//! `docs/05-testing/reports/` is comparing two different vaults and is
//! therefore meaningless.

mod common;

use common::{TempDir, walk};
use vaultgen::{Options, generate, manifest_path_for};

fn opts(out: &TempDir, seed: u64, jobs: Option<usize>) -> Options {
    Options {
        seed,
        notes: 300,
        assets: 20,
        out: out.path().to_path_buf(),
        manifest: None,
        jobs,
        force: true,
        quiet: true,
    }
}

#[test]
fn same_seed_produces_byte_identical_output() {
    let a = TempDir::new("det-a");
    let b = TempDir::new("det-b");
    let ma = generate(&opts(&a, 20_260_903, None)).expect("generate a");
    let mb = generate(&opts(&b, 20_260_903, None)).expect("generate b");

    assert_eq!(
        ma.tree_digest, mb.tree_digest,
        "same seed produced different tree digests"
    );

    let fa = walk(a.path());
    let fb = walk(b.path());
    assert_eq!(
        fa.iter().map(|f| &f.0).collect::<Vec<_>>(),
        fb.iter().map(|f| &f.0).collect::<Vec<_>>(),
        "same seed produced a different file list"
    );
    for ((path, bytes_a), (_, bytes_b)) in fa.iter().zip(fb.iter()) {
        assert_eq!(bytes_a, bytes_b, "{path} differs between two runs");
    }
    assert!(fa.len() > 300, "suspiciously few files: {}", fa.len());
}

#[test]
fn thread_count_does_not_change_the_bytes() {
    // Notes are written in parallel; if content depended on write order this
    // would fail, and a benchmark run on a different machine would silently
    // measure a different vault.
    let a = TempDir::new("jobs-1");
    let b = TempDir::new("jobs-7");
    let ma = generate(&opts(&a, 4_242, Some(1))).expect("generate 1 job");
    let mb = generate(&opts(&b, 4_242, Some(7))).expect("generate 7 jobs");
    assert_eq!(ma.tree_digest, mb.tree_digest);
    assert_eq!(walk(a.path()), walk(b.path()));
}

#[test]
fn a_different_seed_produces_a_different_vault() {
    let a = TempDir::new("seed-1");
    let b = TempDir::new("seed-2");
    let ma = generate(&opts(&a, 1, None)).expect("generate seed 1");
    let mb = generate(&opts(&b, 2, None)).expect("generate seed 2");
    assert_ne!(
        ma.tree_digest, mb.tree_digest,
        "two seeds produced the same vault; the seed is being ignored"
    );
}

#[test]
fn the_manifest_lands_outside_the_vault() {
    // C-02 in spirit: the vault holds nothing a real vault would not. The
    // manifest is scaffolding, so it lives beside the vault, not inside it.
    let parent = TempDir::new("manifest");
    let out = parent.join("vault");
    let o = Options {
        seed: 9,
        notes: 200,
        assets: 8,
        out: out.clone(),
        manifest: None,
        jobs: None,
        force: true,
        quiet: true,
    };
    generate(&o).expect("generate");
    let expected = manifest_path_for(&out);
    assert!(expected.exists(), "{} was not written", expected.display());
    assert!(!out.join("vault.manifest.json").exists());

    let json = std::fs::read_to_string(&expected).expect("read manifest");
    assert!(json.contains("\"seed\": 9"));
    assert!(json.contains("\"tree_digest_sha256\""));
    assert!(json.contains("NFR-MNT-02"));
}

#[test]
fn the_manifest_digest_matches_the_files_on_disk() {
    let dir = TempDir::new("digest");
    let m = generate(&opts(&dir, 77, None)).expect("generate");
    // Recompute the digest the same way the generator does, from what is
    // actually on disk rather than from what it believed it wrote.
    let mut tree = vaultgen::hash::Sha256::new();
    for (path, bytes) in walk(dir.path()) {
        tree.update(path.as_bytes());
        tree.update(b"\0");
        tree.update(&vaultgen::hash::Sha256::digest(&bytes));
        tree.update(b"\n");
    }
    assert_eq!(m.tree_digest, vaultgen::hash::hex(&tree.finish()));
    assert_eq!(m.files, walk(dir.path()).len());
}

#[test]
fn force_refuses_to_delete_a_directory_it_did_not_create() {
    // `--force` on a mistyped path must not be able to remove a real vault.
    let dir = TempDir::new("guard");
    std::fs::write(dir.join("Important note.md"), b"not generated\n").expect("write");

    let mut o = opts(&dir, 1, None);
    o.force = true;
    let err = generate(&o).expect_err("should refuse");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(
        dir.join("Important note.md").exists(),
        "the guard deleted the file it was supposed to protect"
    );
}

#[test]
fn a_non_empty_target_needs_force() {
    let dir = TempDir::new("noforce");
    let mut o = opts(&dir, 1, None);
    o.force = true;
    generate(&o).expect("first generate");

    o.force = false;
    let err = generate(&o).expect_err("should refuse without --force");
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);

    // With the stamp present, --force is allowed to regenerate.
    o.force = true;
    generate(&o).expect("regenerate with force");
}
