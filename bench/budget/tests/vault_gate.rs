//! The vault gate, against a real vault on disk.
//!
//! Verifies **NFR-PRF-08**, **NFR-MNT-02** and P0 finding **F-26**: a number is
//! only attributable if the vault it was taken against is the canonical one.
//!
//! These are integration tests rather than unit tests because a mutation sweep
//! found that replacing `tree_digest(&path)?` with the digest copied out of the
//! manifest left every unit test green — the refusals in `check_vault` were
//! covered, and the wiring that feeds them was not. A guard is only real at the
//! level it actually runs.
//!
//! The smoke vault is used because it is 400 notes and generates in about a
//! tenth of a second. Its digest is location-independent — the tree hash is over
//! paths relative to the vault root — so a vault generated into a temporary
//! directory hashes to the same canonical value as one in `bench/vaults/`.

use std::path::{Path, PathBuf};

use budget::{preset, verify_vault_at};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> TempDir {
        let path =
            std::env::temp_dir().join(format!("budget-vault-gate-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn generate_smoke(into: &Path) {
    let p = preset("smoke").expect("the smoke preset exists");
    vaultgen::generate(&vaultgen::Options {
        seed: vaultgen::DEFAULT_SEED,
        notes: p.notes,
        assets: p.assets,
        out: into.to_path_buf(),
        manifest: None,
        jobs: None,
        force: true,
        quiet: true,
    })
    .expect("generate the smoke vault");
}

/// Any `.md` file in the vault, for tampering with.
fn some_note(root: &Path) -> PathBuf {
    fn walk(dir: &Path, found: &mut Option<PathBuf>) {
        if found.is_some() {
            return;
        }
        for entry in std::fs::read_dir(dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|e| e == "md") && found.is_none() {
                *found = Some(path);
                return;
            }
        }
    }
    let mut found = None;
    walk(root, &mut found);
    found.expect("the vault contains a note")
}

#[test]
fn the_canonical_vault_passes_the_gate() {
    let dir = TempDir::new("canonical");
    let vault = dir.path().join("vault-smoke");
    generate_smoke(&vault);

    let p = preset("smoke").expect("smoke");
    let checked = verify_vault_at(p, &vault).expect("a freshly generated vault is canonical");
    assert_eq!(checked.digest, p.digest, "digest is location-independent");
    assert_eq!(checked.notes, p.notes);
    assert_eq!(checked.format_version, vaultgen::VAULT_FORMAT_VERSION);
}

#[test]
fn one_edited_byte_stops_the_clock() {
    let dir = TempDir::new("tampered");
    let vault = dir.path().join("vault-smoke");
    generate_smoke(&vault);

    let note = some_note(&vault);
    let mut bytes = std::fs::read(&note).expect("read a note");
    bytes.push(b'!');
    std::fs::write(&note, &bytes).expect("tamper");

    let err = verify_vault_at(preset("smoke").expect("smoke"), &vault)
        .expect_err("a hand-edited vault must not be measured");
    assert!(err.contains("does not match its manifest"), "{err}");
    assert!(err.contains("--preset smoke --force"), "{err}");
}

#[test]
fn a_coherent_vault_from_another_seed_stops_the_clock() {
    // F-26 exactly: the vault and its manifest agree perfectly, because the
    // manifest was regenerated alongside it. Self-consistency is not
    // canonicality, and this is the easy mistake rather than the hard one.
    let dir = TempDir::new("wrong-seed");
    let vault = dir.path().join("vault-smoke");
    let p = preset("smoke").expect("smoke");
    vaultgen::generate(&vaultgen::Options {
        seed: 424_242,
        notes: p.notes,
        assets: p.assets,
        out: vault.clone(),
        manifest: None,
        jobs: None,
        force: true,
        quiet: true,
    })
    .expect("generate");

    let err = verify_vault_at(p, &vault).expect_err("a vault nobody meant to measure");
    assert!(err.contains("not the canonical"), "{err}");
    assert!(
        !err.contains("does not match its manifest"),
        "it does match its manifest — that is the whole point: {err}"
    );
}

#[test]
fn a_vault_with_the_wrong_note_count_stops_the_clock() {
    let dir = TempDir::new("wrong-count");
    let vault = dir.path().join("vault-smoke");
    let p = preset("smoke").expect("smoke");
    vaultgen::generate(&vaultgen::Options {
        seed: vaultgen::DEFAULT_SEED,
        notes: 300,
        assets: p.assets,
        out: vault.clone(),
        manifest: None,
        jobs: None,
        force: true,
        quiet: true,
    })
    .expect("generate");

    let err = verify_vault_at(p, &vault).expect_err("300 notes is not the smoke vault");
    assert!(err.contains("not the canonical"), "{err}");
}

#[test]
fn a_missing_manifest_stops_the_clock() {
    let dir = TempDir::new("no-manifest");
    let vault = dir.path().join("vault-smoke");
    generate_smoke(&vault);
    std::fs::remove_file(vaultgen::manifest_path_for(&vault)).expect("remove the manifest");

    let err = verify_vault_at(preset("smoke").expect("smoke"), &vault)
        .expect_err("no manifest, no attributable number");
    assert!(err.contains("reading"), "{err}");
}
