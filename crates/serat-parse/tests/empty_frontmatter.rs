//! The empty-frontmatter edge of the sidecar template (task #97; FR-BIN-02, ADR-0005).
//!
//! A sidecar may be written with an empty frontmatter block. comrak's front-matter
//! splitter needs a line between the delimiters, so `---\n\n---\n` is an empty
//! frontmatter block (no fields, no error) while `---\n---\n` is two thematic breaks
//! and no frontmatter at all. The parsed note does not expose frontmatter presence,
//! so this asserts the field count and the `Rule` blocks in the body.

use serat_core::{BlockKind, NoteParser, ParsedNote, VaultPath};
use serat_parse::CommonMarkParser;

fn parse(text: &str) -> ParsedNote {
    CommonMarkParser::new()
        .parse(&VaultPath::new("x.pdf.md").expect("path"), text.as_bytes())
        .expect("never an error")
}

fn rules(note: &ParsedNote) -> usize {
    note.blocks
        .iter()
        .filter(|b| b.kind == BlockKind::Rule)
        .count()
}

#[test]
fn a_blank_line_between_the_delimiters_is_an_empty_frontmatter_block() {
    let note = parse("---\n\n---\n");
    assert!(note.fields.is_empty());
    assert_eq!(rules(&note), 0, "the delimiters are frontmatter, not breaks");
}

#[test]
fn adjacent_delimiters_are_two_thematic_breaks_and_no_frontmatter() {
    let note = parse("---\n---\n");
    assert!(note.fields.is_empty());
    assert_eq!(rules(&note), 2);
}
