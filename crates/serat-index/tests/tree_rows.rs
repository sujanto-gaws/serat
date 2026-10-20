//! The tree's warm-path read (ADR-0015 §2; FR-VLT-01): what the index holds, grouped
//! into listings by the pure function in `serat-core`.

mod support;

use std::path::Path;

use serat_core::{TreeKind, group_listings};
use serat_index::tree::read_tree_rows;
use serat_index::{index_path, rebuild};
use support::{FakeVault, LineParser};

#[test]
fn the_index_rows_group_into_root_first_listings_with_assets_marked() {
    let dir = tempfile::tempdir().expect("temp");
    rebuild(
        &FakeVault::with(&[
            ("a.md", "# A\n"),
            ("projects/b.md", "# B\n"),
            ("projects/deep/c.md", "# C\n"),
            ("refs/contract.pdf", "%PDF"),
        ]),
        &LineParser::default(),
        Path::new("/vault"),
        dir.path(),
    )
    .expect("rebuild");

    let rows = read_tree_rows(&index_path(dir.path())).expect("rows");
    assert_eq!(rows.len(), 4);
    assert!(rows.contains(&("refs/contract.pdf".to_owned(), TreeKind::Asset)));

    let listings = group_listings(rows.iter().map(|(p, k)| (p.as_str(), *k)));
    let dirs: Vec<&str> = listings.iter().map(|(d, _)| d.as_str()).collect();
    assert_eq!(dirs, ["", "projects", "refs", "projects/deep"]);
}
