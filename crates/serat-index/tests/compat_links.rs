//! Obsidian-compatibility corpus, link targets (task #139; NFR-CMP-01, FR-NAV-04,
//! FR-EDT-03, FR-NAV-01; ADR-0003; architecture §9.2 subpaths, §4.5 resolution).
//!
//! Fixtures live in `bench/compat/corpus/links/`. Each test copies them into a temp
//! vault, rebuilds the index with the real resolver, and reads `links.dst_note` and
//! `links.dst_block_ref` back.
//!
//! **What this pins, and what it does not.** `serat-index` never links `serat-parse`
//! (the dependency rule), so the links come from [`WikiParser`] below, which splits a
//! target the way `serat-parse`'s `split_target` does. That makes these tests pin the
//! *index and resolver* given a split target. That the real parser emits the same
//! split from the same bytes is pinned separately by
//! `crates/serat-parse/tests/compat_links.rs` against the same fixtures; the two
//! tables must be edited together.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use serat_core::{
    ByteRange, Link, LinkKind, LinkTarget, NoteParser, ParseError, ParsedNote, VaultPath,
};
use serat_index::{index_path, rebuild, schema};
use support::DiskVault;

/// Extracts `[[...]]` and `![[...]]` from the text, splitting `|alias`, `#heading`
/// and `#^block` the way `serat-parse` does. Nothing else about Markdown.
struct WikiParser;

impl NoteParser for WikiParser {
    fn parse(&self, path: &VaultPath, bytes: &[u8]) -> Result<ParsedNote, ParseError> {
        let mut parsed = ParsedNote {
            title: Some(path.stem().to_owned()),
            ..ParsedNote::default()
        };
        if !path.is_markdown() {
            return Ok(parsed);
        }
        let text = String::from_utf8_lossy(bytes);
        let mut from = 0;
        while let Some(open) = text[from..].find("[[").map(|i| i + from) {
            let Some(close) = text[open..].find("]]").map(|i| i + open) else {
                break;
            };
            let embed = open > 0 && text.as_bytes()[open - 1] == b'!';
            let inner = &text[open + 2..close];
            let (target_part, alias) = match inner.split_once('|') {
                Some((t, a)) => (t, Some(a.to_owned())),
                None => (inner, None),
            };
            let (note, heading, block_ref) = match target_part.split_once('#') {
                Some((n, frag)) if frag.starts_with('^') => {
                    (n.to_owned(), None, Some(frag[1..].to_owned()))
                }
                Some((n, frag)) => (n.to_owned(), Some(frag.to_owned()), None),
                None => (target_part.to_owned(), None, None),
            };
            parsed.links.push(Link {
                kind: if embed {
                    LinkKind::Embed
                } else {
                    LinkKind::Wikilink
                },
                target: LinkTarget {
                    note,
                    heading,
                    block_ref,
                    alias,
                },
                range: ByteRange::new(open, close + 2),
                block_ord: None,
            });
            from = close + 2;
        }
        Ok(parsed)
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/compat/corpus/links")
}

/// One link row as the index stores it.
#[derive(Debug, PartialEq, Eq)]
struct Row {
    src: String,
    kind: String,
    dst_path: String,
    block_ref: Option<String>,
    resolved_to: Option<String>,
}

fn rows_of(vault: &Path) -> Vec<Row> {
    let index = tempfile::tempdir().expect("temp");
    rebuild(&DiskVault::new(vault), &WikiParser, vault, index.path()).expect("rebuild");
    let conn = schema::open(&index_path(index.path())).expect("open");
    let mut stmt = conn
        .prepare(
            "SELECT src.path, l.kind, l.dst_path, l.dst_block_ref, dst.path
               FROM links l
               JOIN notes src ON src.id = l.src_note
               LEFT JOIN notes dst ON dst.id = l.dst_note
              ORDER BY src.path, l.id",
        )
        .expect("prepare");
    stmt.query_map([], |r| {
        Ok(Row {
            src: r.get(0)?,
            kind: r.get(1)?,
            dst_path: r.get(2)?,
            block_ref: r.get(3)?,
            resolved_to: r.get(4)?,
        })
    })
    .expect("query")
    .map(|r| r.expect("row"))
    .collect()
}

fn corpus_vault() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp");
    for entry in fs::read_dir(fixtures()).expect("fixture dir") {
        let entry = entry.expect("entry");
        // The README documents the corpus and quotes link syntax in backticks; both
        // parsers would pull links from it, so it is not part of the vault.
        if entry.file_name() == "README.md" {
            continue;
        }
        fs::copy(entry.path(), dir.path().join(entry.file_name())).expect("copy fixture");
    }
    dir
}

fn from<'a>(rows: &'a [Row], src: &str) -> Vec<&'a Row> {
    rows.iter().filter(|r| r.src == src).collect()
}

const TARGET: &str = "heading-and-block-target.md";

/// FR-NAV-04, FR-EDT-03, NFR-CMP-01, architecture §9.2: a `#Heading`, `#^block` or
/// `#A#B` subpath leaves the note part in `dst_path`, resolves to the base note, and
/// only the block form fills `dst_block_ref`.
#[test]
fn subpath_links_resolve_to_the_base_note_and_record_the_block_ref() {
    let vault = corpus_vault();
    let rows = rows_of(vault.path());
    let links = from(&rows, "heading-block-link.md");
    assert_eq!(links.len(), 4, "{links:?}");
    let base = |block: Option<&str>| Row {
        src: "heading-block-link.md".into(),
        kind: "wiki".into(),
        dst_path: "heading-and-block-target".into(),
        block_ref: block.map(str::to_owned),
        resolved_to: Some(TARGET.into()),
    };
    // `#Heading`, `#^block1`, `#A#B`, bare control; in source order.
    assert_eq!(*links[0], base(None), "#Heading");
    assert_eq!(*links[1], base(Some("block1")), "#^block1");
    assert_eq!(*links[2], base(None), "#A#B");
    assert_eq!(*links[3], base(None), "bare control");
}

/// FR-NAV-01, NFR-CMP-01, ADR-0003: embeds of an asset, with and without a display
/// width, and of a note, with and without a heading scope. `|300` is display text
/// only, so it must not change what the link resolves to.
#[test]
fn embeds_resolve_to_the_asset_or_the_note_and_the_width_is_display_only() {
    let vault = corpus_vault();
    let rows = rows_of(vault.path());
    let links = from(&rows, "embeds.md");
    assert_eq!(links.len(), 4, "{links:?}");
    let embed = |dst: &str, to: &str| Row {
        src: "embeds.md".into(),
        kind: "embed".into(),
        dst_path: dst.into(),
        block_ref: None,
        resolved_to: Some(to.into()),
    };
    assert_eq!(
        *links[0],
        embed("asset-image.png", "asset-image.png"),
        "![[x.png]]"
    );
    assert_eq!(
        *links[1],
        embed("asset-image.png", "asset-image.png"),
        "![[x.png|300]]"
    );
    assert_eq!(*links[2], embed("heading-and-block-target", TARGET));
    assert_eq!(
        *links[3],
        embed("heading-and-block-target", TARGET),
        "![[note#Heading]]"
    );
}

/// FR-NAV-01, architecture §9.2: aliases are quick-switcher candidates only. A link
/// written with an alias stays unresolved (and is recorded, not dropped); the same
/// note reached by its file name resolves. If aliases ever become link targets this
/// test must change with the resolver and the architecture text together.
#[test]
fn an_alias_is_not_a_link_target_only_a_switcher_candidate() {
    let vault = corpus_vault();
    let rows = rows_of(vault.path());
    let links = from(&rows, "alias-link.md");
    let by = |dst: &str| {
        links
            .iter()
            .find(|r| r.dst_path == dst)
            .unwrap_or_else(|| panic!("no link to {dst}: {links:?}"))
    };
    assert_eq!(by("Ali").resolved_to, None, "[[Ali]]");
    assert_eq!(by("Target Alias").resolved_to, None, "[[Target Alias]]");
    assert_eq!(
        by("aliases-target").resolved_to.as_deref(),
        Some("aliases-target.md")
    );
}
