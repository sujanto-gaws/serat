//! Block-style frontmatter lists reach the index (task #57; FR-STR-01, FR-NAV-01,
//! NFR-CMP-01).
//!
//! Found by #16: vaultgen writes `aliases:` as a block list on about 18% of notes and
//! the P0 parser skipped block lists, so the index held no alias row at all. The
//! oracle is the generator's own manifest count, not a number read back from the
//! index, so a parser that silently drops items cannot agree with it by accident.
//!
//! The 20k vault is generated into a temp directory with the canonical seed rather
//! than read from `bench/vaults/`: the manifest on disk predates the `aliases` count.

use std::path::{Path, PathBuf};

use budget::preset;
use serat_index::{index_path, rebuild, schema};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> TempDir {
        let path =
            std::env::temp_dir().join(format!("budget-fm-lists-{label}-{}", std::process::id()));
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

fn alias_rows_equal_the_manifest(preset_name: &str) {
    let p = preset(preset_name).expect("preset");
    let work = TempDir::new(preset_name);
    let vault = work.path().join("vault");
    let manifest = vaultgen::generate(&vaultgen::Options {
        seed: vaultgen::DEFAULT_SEED,
        notes: p.notes,
        assets: p.assets,
        out: vault.clone(),
        manifest: None,
        jobs: None,
        force: true,
        quiet: true,
    })
    .expect("generate");
    let expected = i64::from(manifest.content.aliases);
    assert!(expected > 0, "the generator wrote no aliases");

    let source = serat_vault::Vault::open(&vault).expect("open vault");
    let index = work.path().join("index");
    rebuild(&source, &serat_parse::CommonMarkParser, &vault, &index).expect("rebuild");
    let conn = schema::open(&index_path(&index)).expect("open index");
    let rows: i64 = conn
        .query_row(
            "SELECT count(*) FROM fields WHERE key = 'aliases' AND source = 'frontmatter'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert!(rows > 0, "no alias rows in the index");
    assert_eq!(
        rows, expected,
        "alias rows in the index vs aliases the generator wrote"
    );
}

#[test]
fn smoke_vault_alias_rows_equal_the_generators_count() {
    alias_rows_equal_the_manifest("smoke");
}

#[test]
fn the_20k_vault_alias_rows_equal_the_generators_count() {
    alias_rows_equal_the_manifest("20k");
}
