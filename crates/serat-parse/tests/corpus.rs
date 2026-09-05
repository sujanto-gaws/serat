//! The corpus digest guard (F-185/F-188).
//!
//! `serat-index` derives rows from what this crate extracts, and `INDEX_VERSION`
//! (serat-index) is the only signal that tells an existing index its derivation has
//! changed — the schema can be untouched while the *meaning* of a row changes
//! underneath it (F-134/F-171: a parser that starts recognising a construct it used
//! to ignore leaves every schema-level check passing over an index whose content is
//! stale). This test is what makes that kind of change impossible to land silently:
//! it pins a hash of this parser's whole output for one fixed document, and any
//! change to what gets extracted — a new field recognised, a block reclassified, a
//! byte range shifted — changes the hash.
//!
//! Three conditions decide whether this actually works (F-188), and each is called
//! out at its enforcement point below:
//!
//! 1. The failure message must demand *both* updating the digest *and* bumping
//!    `INDEX_VERSION` — see the assertion message. Updating only the digest silently
//!    reintroduces exactly the failure this test exists to catch, one layer in.
//! 2. The digest must be canonical (deterministic across runs and platforms) — see
//!    the note on `format!("{:#?}", ...)` below.
//! 3. The corpus must contain at least one construct this parser currently drops —
//!    see the footnote definition, and the assertion that names it.

use serat_core::{NoteParser, VaultPath};
use serat_parse::CommonMarkParser;

/// Exercises every construct this parser currently handles, plus one it does not
/// (condition 3): a footnote definition, which has no `BlockKind` to become
/// (serat-parse's own module doc names this scope decision) and must stay absent
/// from the corpus's blocks — the day it stops being absent, this test's digest
/// changes and says so.
const CORPUS: &str = "---\n\
title: Corpus\n\
type: fixture\n\
tags: [parsing, corpus]\n\
---\n\
\n\
# Corpus\n\
\n\
## Section One\n\
\n\
A paragraph with **bold**, *emphasis*, `code`, a [[Wikilink]], a [[Target|Alias]], \
an embed ![[diagram.png]], a tag #important, and a [markdown link](https://example.com). ^para-ref\n\
\n\
status:: active\n\
due:: 2026-01-01\n\
\n\
- A plain list item\n\
- [ ] An open task with an inline field owner:: sujan\n\
- [x] A completed task\n\
  - A nested item one level deeper\n\
\n\
> A block quote spanning\n\
> two lines.\n\
\n\
```rust\n\
fn main() {}\n\
```\n\
\n\
| A | B |\n\
|---|---|\n\
| 1 | 2 |\n\
\n\
---\n\
\n\
<div>raw html block</div>\n\
\n\
## Section Two\n\
\n\
Text with a footnote reference[^1], which this parser does not resolve.\n\
\n\
[^1]: A footnote definition — the construct this corpus deliberately still drops.\n\
";

/// Pinned hash of `CommonMarkParser`'s output for [`CORPUS`]. Update it, in the same
/// commit that bumps `INDEX_VERSION`, whenever a deliberate change to this parser's
/// output makes this test fail — never as a change on its own.
const CORPUS_DIGEST: &str = "58178c9d63207c46a6833a3dff08594fbfb0a557e21d6d41d67aa88d00db42ba";

#[test]
fn the_corpus_digest_matches_what_this_parser_currently_produces() {
    let parsed = CommonMarkParser::new()
        .parse(&VaultPath::new("corpus.md").expect("path"), CORPUS.as_bytes())
        .expect("parse");

    // Condition 3: the footnote reference/definition pair must still be absent from
    // the blocks. If this assertion is what fails, someone taught the parser to
    // recognise footnotes — which is exactly the class of change this whole test
    // exists to make loud rather than silent, and the fix is in the module doc's
    // scope list and the schema, not here.
    assert!(
        !parsed
            .blocks
            .iter()
            .any(|block| block.text.contains("footnote definition")),
        "the footnote definition was indexed as a block — this parser is documented \
         to drop footnotes (no BlockKind exists for them); if that changed \
         deliberately, update the module doc's scope list, not just this test"
    );

    // Condition 2: deterministic across runs. `format!("{:#?}", ...)` walks the
    // structs and their Vec fields in the order the parser produced them — document
    // order, by construction (Block::ord is `blocks.len()` at push time) — and
    // nothing in ParsedNote is a HashMap, so this has no run-to-run or
    // platform-to-platform source of nondeterminism to canonicalise away.
    let rendered = format!("{parsed:#?}");
    let digest = blake3::hash(rendered.as_bytes()).to_hex().to_string();

    // Condition 1: the message demands both actions.
    assert_eq!(
        digest, CORPUS_DIGEST,
        "serat-parse's output for the fixed corpus has changed (rendered below). If \
         this is a deliberate change to what serat-parse extracts, TWO things must \
         happen in the same commit: update CORPUS_DIGEST to {digest:?}, AND bump \
         serat_index::INDEX_VERSION (crates/serat-index/src/schema.rs) — an index \
         built by the old derivation and one built by the new one disagree on what a \
         note contains, with no schema change for the cold-start check to notice \
         (F-134/F-171). Updating the digest alone reintroduces the exact failure this \
         test exists to prevent, one layer in (F-188). If this change was not \
         deliberate, the rendering below is the diff to read.\n\n{rendered}"
    );
}
