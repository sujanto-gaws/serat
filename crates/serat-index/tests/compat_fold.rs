//! Obsidian-compatibility corpus, fold cases (task #12; NFR-CMP-01; architecture
//! §4.5 "Matching is case-insensitive" and the §10 NFC/NFD open question).
//!
//! Fixtures live in `bench/compat/corpus/fold/` (see its README). Each test copies a
//! case into a temp vault, rebuilds the index with the real `fold_case` and the real
//! resolver, and reads back which note each link landed on.
//!
//! Links are extracted by the test `LineParser` (`tests/support`: one `[[target]]`
//! per line), **not** by the CommonMark parser, so these tests pin the *resolver*
//! and `fold_case`, not link syntax. Whether `serat-parse` extracts the same
//! targets from the same bytes is a separate question these cases do not answer.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serat_index::{index_path, rebuild, schema};
use support::{DiskVault, LineParser};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/compat/corpus/fold")
}

fn copy_case(case: &str, into: &Path) {
    for entry in fs::read_dir(fixtures().join(case)).expect("fixture dir") {
        let entry = entry.expect("entry");
        fs::copy(entry.path(), into.join(entry.file_name())).expect("copy fixture");
    }
}

/// Rebuild `vault` and return `link note path -> resolved target path (or None)`.
fn resolutions(vault: &Path) -> BTreeMap<String, Option<String>> {
    let index = tempfile::tempdir().expect("temp");
    rebuild(
        &DiskVault::new(vault),
        &LineParser::default(),
        vault,
        index.path(),
    )
    .expect("rebuild");
    let conn = schema::open(&index_path(index.path())).expect("open");
    let mut stmt = conn
        .prepare(
            "SELECT src.path, dst.path FROM links l
               JOIN notes src ON src.id = l.src_note
               LEFT JOIN notes dst ON dst.id = l.dst_note",
        )
        .expect("prepare");
    stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })
    .expect("query")
    .map(|r| r.expect("row"))
    .collect()
}

fn tmp_case(case: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp");
    copy_case(case, dir.path());
    dir
}

/// NFR-CMP-01: an all-caps Greek name ending in capital sigma is found by both its
/// upper-case and its lower-case spelling. `str::to_lowercase` would turn the final
/// capital sigma into `ς` in one and `σ` in the other; `fold_case` must not.
#[test]
fn greek_final_sigma_resolves_from_upper_and_lower_case_links() {
    let vault = tmp_case("greek");
    let got = resolutions(vault.path());
    for link in ["link-upper.md", "link-lower.md", "link-medial-sigma.md"] {
        assert_eq!(
            got.get(link),
            Some(&Some("ΟΔΟΣ.md".to_owned())),
            "{link} must resolve to ΟΔΟΣ.md: {got:?}"
        );
    }
}

/// NFR-CMP-01: `[[istanbul]]` does NOT find `İstanbul.md`, matching Obsidian.
/// U+0130 lowercases to `i` + U+0307 (JavaScript's `toLowerCase`, Obsidian's base
/// mapping, and Rust's), so the dotted capital never folds to plain `i`. The dotless
/// `ı` is its own letter and folds to itself.
#[test]
fn turkish_dotted_capital_i_is_not_found_by_ascii_or_dotless_links() {
    let vault = tmp_case("turkish");
    let got = resolutions(vault.path());
    assert_eq!(
        got.get("link-dotted.md"),
        Some(&Some("İstanbul.md".to_owned())),
        "{got:?}"
    );
    assert_eq!(got.get("link-ascii-lower.md"), Some(&None), "{got:?}");
    assert_eq!(got.get("link-dotless.md"), Some(&None), "{got:?}");
}

/// `Café` with the accent as U+0301 after a plain `e`: the NFD spelling.
#[cfg(target_os = "linux")]
const CAFE_NFD: &str = "Cafe\u{301}";
/// The same word with the precomposed U+00E9: the NFC spelling.
#[cfg(target_os = "linux")]
const CAFE_NFC: &str = "Caf\u{e9}";

/// Built at test time, not committed: APFS cannot hold both spellings side by side
/// and git on macOS recomposes NFD names (fixtures README). Linux only.
#[cfg(target_os = "linux")]
fn nfd_vault() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path();
    fs::write(root.join(format!("{CAFE_NFD}.md")), "# Cafe\n").expect("nfd note");
    fs::write(root.join("link-nfd.md"), format!("[[{CAFE_NFD}]]\n")).expect("nfd link");
    fs::write(root.join("link-nfc.md"), format!("[[{CAFE_NFC}]]\n")).expect("nfc link");
    dir
}

/// The spelling-for-itself half, asserted: a link spelt exactly as the file name is
/// spelt resolves, whatever Obsidian does across spellings.
#[cfg(target_os = "linux")]
#[test]
fn nfd_file_name_is_found_by_its_own_spelling() {
    let vault = nfd_vault();
    let got = resolutions(vault.path());
    assert_eq!(
        got.get("link-nfd.md"),
        Some(&Some(format!("{CAFE_NFD}.md"))),
        "{got:?}"
    );
}

/// The cross-spelling half: does `[[Café]]` (NFC) find the NFD-named file?
/// Ignored because the expected outcome is Obsidian's, and no Obsidian run has been
/// recorded (architecture §10, first half of the NFC/NFD question). The assertion is
/// the "Obsidian normalises" hypothesis; `fold_case` has no NFC step today, so if
/// that hypothesis is confirmed this test fails until `fold_case` gains one (a schema
/// change and a rebuild, ADR-0002), and if it is refuted the assertion flips and the
/// ignore goes.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "expected outcome is Obsidian's and has not been measured (architecture §10 NFC/NFD question)"]
fn nfd_file_name_is_found_by_its_nfc_link_if_obsidian_normalises() {
    let vault = nfd_vault();
    let got = resolutions(vault.path());
    assert_eq!(
        got.get("link-nfc.md"),
        Some(&Some(format!("{CAFE_NFD}.md"))),
        "{got:?}"
    );
}

// `Ǘ` U+01D7 and its lower case `ǘ` U+01D8: a letter with two stacked combining marks
// (diaeresis, then acute), so the NFD spelling is three scalars and case folding and
// normalisation interact.
const U_NFC_UPPER: &str = "\u{1D7}";
#[cfg(target_os = "linux")]
const U_NFC_LOWER: &str = "\u{1D8}";
/// Fully decomposed: `U` + diaeresis + acute.
#[cfg(target_os = "linux")]
const U_NFD_UPPER: &str = "U\u{308}\u{301}";
/// Fully decomposed: `u` + diaeresis + acute.
#[cfg(target_os = "linux")]
const U_NFD_LOWER: &str = "u\u{308}\u{301}";

/// NFR-CMP-01: the committed NFC case. `Ǘ.md` is found by `[[ǘ]]`, both precomposed:
/// case folding alone, no normalisation involved.
#[test]
fn composed_u_diaeresis_acute_is_found_across_case() {
    let vault = tmp_case("u-diaeresis-acute");
    let got = resolutions(vault.path());
    assert_eq!(
        got.get("link-lower.md"),
        Some(&Some(format!("{U_NFC_UPPER}.md"))),
        "{got:?}"
    );
}

/// The NFD side, built at test time for the reason `nfd_vault` is (fixtures README).
#[cfg(target_os = "linux")]
fn u_nfd_vault() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path();
    fs::write(root.join(format!("{U_NFD_UPPER}.md")), "# U\n").expect("nfd note");
    fs::write(
        root.join("link-nfd-lower.md"),
        format!("[[{U_NFD_LOWER}]]\n"),
    )
    .expect("nfd link");
    fs::write(
        root.join("link-nfc-lower.md"),
        format!("[[{U_NFC_LOWER}]]\n"),
    )
    .expect("nfc link");
    dir
}

/// Same-spelling half, asserted: the NFD file is found by its own spelling in lower
/// case, which exercises case folding over a base letter followed by two marks.
#[cfg(target_os = "linux")]
#[test]
fn decomposed_u_diaeresis_acute_is_found_by_its_own_spelling_across_case() {
    let vault = u_nfd_vault();
    let got = resolutions(vault.path());
    assert_eq!(
        got.get("link-nfd-lower.md"),
        Some(&Some(format!("{U_NFD_UPPER}.md"))),
        "{got:?}"
    );
}

/// Cross-spelling half: `[[ǘ]]` (NFC) against the NFD-named `Ǘ`. Ignored for the same
/// reason as the Café case: the expected outcome is Obsidian's and is not measured.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "expected outcome is Obsidian's and has not been measured (architecture §10 NFC/NFD question)"]
fn decomposed_u_diaeresis_acute_is_found_by_its_nfc_link_if_obsidian_normalises() {
    let vault = u_nfd_vault();
    let got = resolutions(vault.path());
    assert_eq!(
        got.get("link-nfc-lower.md"),
        Some(&Some(format!("{U_NFD_UPPER}.md"))),
        "{got:?}"
    );
}
