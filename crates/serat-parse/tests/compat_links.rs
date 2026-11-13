//! The real parser's reading of the link fixtures (task #139; NFR-CMP-01,
//! FR-EDT-03, FR-NAV-04; ADR-0003).
//!
//! `crates/serat-index/tests/compat_links.rs` runs the same fixtures through the
//! index with a stub link extractor (the index never links `serat-parse`). This is
//! the other half: it pins what `CommonMarkParser` actually emits for those bytes, so
//! the stub's split cannot drift from the real one unseen. Edit the two together.

use std::fs;
use std::path::Path;

use serat_core::{LinkKind, NoteParser, VaultPath};
use serat_parse::CommonMarkParser;

/// `(kind, note, heading, block_ref, alias)` for every link in a fixture, in order.
type Shape = (
    LinkKind,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn links_of(name: &str) -> Vec<Shape> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../bench/compat/corpus/links")
        .join(name);
    let bytes = fs::read(path).expect("fixture");
    CommonMarkParser::new()
        .parse(&VaultPath::new(name).expect("path"), &bytes)
        .expect("a fixture never fails the note")
        .links
        .into_iter()
        .map(|l| {
            (
                l.kind,
                l.target.note,
                l.target.heading,
                l.target.block_ref,
                l.target.alias,
            )
        })
        .collect()
}

fn s(x: &str) -> String {
    x.to_owned()
}

#[test]
fn subpath_fixture_splits_heading_block_and_nested_heading() {
    let t = s("heading-and-block-target");
    assert_eq!(
        links_of("heading-block-link.md"),
        vec![
            (
                LinkKind::Wikilink,
                t.clone(),
                Some(s("Some Heading")),
                None,
                None
            ),
            (LinkKind::Wikilink, t.clone(), None, Some(s("block1")), None),
            // `#A#B` is kept whole in `heading`; the note part is what the index uses.
            (
                LinkKind::Wikilink,
                t.clone(),
                Some(s("Some Heading#Another Heading")),
                None,
                None
            ),
            (LinkKind::Wikilink, t, None, None, None),
        ]
    );
}

#[test]
fn alias_fixtures_give_plain_wikilinks_and_the_alias_is_not_a_link_part() {
    // The index test `an_alias_is_not_a_link_target_only_a_switcher_candidate` uses
    // these two files; this pins what the real parser hands the index for them:
    // plain wikilinks (no heading, block or alias part), so the stub cannot drift.
    let plain = |note: &str| (LinkKind::Wikilink, s(note), None, None, None);
    assert_eq!(
        links_of("alias-link.md"),
        vec![plain("Ali"), plain("Target Alias"), plain("aliases-target")]
    );
    assert_eq!(links_of("aliases-target.md"), vec![plain("alias-link")]);
}

#[test]
fn embed_fixture_keeps_the_width_as_alias_and_the_heading_scope() {
    let t = s("heading-and-block-target");
    assert_eq!(
        links_of("embeds.md"),
        vec![
            (LinkKind::Embed, s("asset-image.png"), None, None, None),
            (
                LinkKind::Embed,
                s("asset-image.png"),
                None,
                None,
                Some(s("300"))
            ),
            (LinkKind::Embed, t.clone(), None, None, None),
            (LinkKind::Embed, t, Some(s("Some Heading")), None, None),
        ]
    );
}

#[test]
fn an_inline_code_span_holds_no_tag_and_no_wikilink() {
    // Task #145, NFR-CMP-01: Obsidian indexes nothing inside a code span. The fenced
    // block was already skipped; the inline span used to yield a Tag for `#abc`.
    let links = links_of("code-span-tag.md");
    assert_eq!(
        links,
        vec![
            (LinkKind::Tag, s("real"), None, None, None),
            (
                LinkKind::Wikilink,
                s("heading-and-block-target"),
                None,
                None,
                None
            ),
        ],
        "{links:?}"
    );
}
