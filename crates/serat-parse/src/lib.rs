//! Markdown bytes in, structures out.
//!
//! This crate is pure: it takes bytes and returns
//! [`ParsedNote`](serat_core::ParsedNote), and it opens nothing (architecture §2.1,
//! §3.1 step 4). That is what lets the parse pool run it across cores minus one with
//! no coordination at all, and what makes a parser test a string literal rather than
//! a fixture directory.
//!
//! # Scope (task I3)
//!
//! [`CommonMarkParser`] handles: YAML frontmatter (the flat subset — see
//! [`frontmatter`]); headings, paragraphs, list items and tasks, code blocks, block
//! quotes, tables, thematic breaks and raw HTML, each with a byte range (comrak's
//! own AST, via [`line_index`]); block references (`^anchor`); inline `key:: value`
//! fields (see [`inline_fields`]); wikilinks, embeds and tags (see [`wikilinks`],
//! hand-scanned because none of the three are CommonMark); and ordinary Markdown
//! links, from comrak's own link nodes.
//!
//! Not handled, by deliberate scope rather than oversight: footnotes, math,
//! description lists, alerts, directives and multiline block quotes have no
//! [`BlockKind`](serat_core::BlockKind) to become and are skipped along with
//! whatever they contain — design goal 5 says a note degrades rather than fails, and
//! these are rare enough at P0 that widening `BlockKind` for them is a design
//! question for whoever hits one, not a blocker here.

mod frontmatter;
mod inline_fields;
mod line_index;
mod text;
mod wikilinks;

use comrak::nodes::{Node, NodeValue};
use comrak::{Arena, Options, parse_document};
use serat_core::{
    Block, BlockKind, ByteRange, Link, LinkKind, LinkTarget, NoteParser, ParseError, ParsedNote,
    Task, VaultPath,
};

use line_index::LineIndex;

/// Parses plain Markdown, with the Obsidian-compatible extensions C-01 permits
/// (frontmatter, wikilinks, inline fields, tags) because they are ordinary Markdown
/// to every other tool — degrading to plain text rather than erroring, which is
/// exactly what a proprietary syntax could not do.
#[derive(Debug, Default)]
pub struct CommonMarkParser;

impl CommonMarkParser {
    /// A parser with no configuration to hold: the whole crate's scope, listed
    /// above, is fixed rather than tunable.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl NoteParser for CommonMarkParser {
    fn parse(&self, path: &VaultPath, bytes: &[u8]) -> Result<ParsedNote, ParseError> {
        let text = std::str::from_utf8(bytes).map_err(|err| ParseError::NotUtf8 {
            path: path.as_str().to_owned(),
            offset: err.valid_up_to(),
        })?;

        let lines = LineIndex::new(bytes);
        let arena = Arena::new();
        let root = parse_document(&arena, text, &options());

        let mut walker = Walker {
            bytes,
            lines: &lines,
            blocks: Vec::new(),
            markdown_links: Vec::new(),
            tasks: Vec::new(),
            heading_stack: Vec::new(),
            excluded: Vec::new(),
        };

        let mut fm_fields = Vec::new();
        let mut fm_range = None;
        let mut title = None;
        let mut note_type = None;

        for child in root.children() {
            let (value, sourcepos) = node_kind(child);
            if let NodeValue::FrontMatter(_) = value {
                let range = lines.range(bytes, sourcepos);
                walker.excluded.push(range);
                fm_range = Some(range);
                let parsed = frontmatter::parse(bytes, range);
                fm_fields = parsed.fields;
                title = parsed.title;
                note_type = parsed.note_type;
            } else {
                walker.walk_block(child, 0);
            }
        }

        let mut fields = fm_fields;
        for block in &walker.blocks {
            fields.extend(inline_fields::scan(bytes, block));
        }

        let mut links = walker.markdown_links;
        links.extend(wikilinks::scan(text, &walker.excluded));
        // F-316: a wikilink in a frontmatter value is a link. `owner: "[[Ada]]"`
        // and an `attendees:` list render as links in Obsidian and appear in its
        // backlinks, so NFR-CMP-01 and FR-NAV-04 both want them; the 20k
        // benchmark vault alone carries 7,989, and without this a note whose only
        // inbound reference is a property is invisible in that note's backlinks.
        //
        // Done as a second scoped pass through the SAME scanner rather than a
        // second scanner, so there stays exactly one definition of what a
        // wikilink is. The scope is inverted: `scan`'s first pass excludes the
        // frontmatter, this one excludes everything else, which is one range
        // because frontmatter can only be the first thing in the file.
        //
        // Tags are dropped rather than scanned for. `#` opens a comment in YAML,
        // and frontmatter tags are declared with a `tags:` key rather than
        // `#tag`, so treating a `#` here as a tag would invent links the file
        // does not contain.
        if let Some(fm) = fm_range {
            let outside_frontmatter = [ByteRange::new(fm.end, bytes.len())];
            links.extend(
                wikilinks::scan(text, &outside_frontmatter)
                    .into_iter()
                    .filter(|link| link.kind != LinkKind::Tag),
            );
        }
        for link in &mut links {
            if link.block_ord.is_none() {
                link.block_ord = containing_block(&walker.blocks, link.range);
            }
        }

        if title.is_none() {
            title = walker
                .blocks
                .iter()
                .find(|block| matches!(block.kind, BlockKind::Heading { level: 1 }))
                .map(|block| block.text.clone());
        }
        if title.is_none() {
            title = Some(path.stem().to_owned());
        }

        Ok(ParsedNote {
            title,
            note_type,
            blocks: walker.blocks,
            fields,
            links,
            tasks: walker.tasks,
        })
    }
}

/// comrak's options for this crate's whole scope.
fn options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.front_matter_delimiter = Some("---".to_owned());
    options.extension.table = true;
    options.extension.tasklist = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    // Recognised and then dropped (module doc): without this, "[^1]: text" is not a
    // footnote at all, just an ordinary paragraph that reads as though footnotes
    // were supported by accident. Enabling it is what makes "no BlockKind exists for
    // footnotes" a real, checked scope decision rather than the absence of one.
    options.extension.footnotes = true;
    options.parse.relaxed_tasklist_matching = true;
    // Wikilinks are handled by this crate's own scanner (module doc, wikilinks.rs):
    // comrak's built-in extension has no notion of `![[embed]]` and does not split
    // `Note#Heading`/`Note#^ref` fragments, so leaving it disabled here avoids the
    // two mechanisms disagreeing about what a `[[...]]` occurrence means.
    options
}

/// A node's value and sourcepos, cloned out from behind its `RefCell` so the borrow
/// does not need to stay alive across a match arm that recurses into the tree.
fn node_kind<'a>(node: Node<'a>) -> (NodeValue, comrak::nodes::Sourcepos) {
    let ast = node.data();
    (ast.value.clone(), ast.sourcepos)
}

/// The innermost block whose range contains `range`, if any — used to attach a
/// wikilink/embed/tag (found by scanning raw text, not the AST) to the block it
/// occurred in, the same way an inline field is attached.
fn containing_block(blocks: &[Block], range: ByteRange) -> Option<u32> {
    blocks
        .iter()
        .filter(|block| block.range.contains(range))
        .min_by_key(|block| block.range.len())
        .map(|block| block.ord)
}

/// Walks comrak's block tree, producing [`Block`]s, [`Task`]s and Markdown
/// [`Link`]s in document order.
struct Walker<'b> {
    bytes: &'b [u8],
    lines: &'b LineIndex,
    blocks: Vec<Block>,
    markdown_links: Vec<Link>,
    tasks: Vec<Task>,
    /// Level and text of every open heading, outermost first — popped down to the
    /// level of a new heading before it is pushed, which is what turns a flat
    /// sequence of headings into a path.
    heading_stack: Vec<(u8, String)>,
    /// Frontmatter's range, plus every code/HTML block's and inline code span's range: wikilinks.rs's scan
    /// runs over the whole file's text and needs to know what to skip.
    excluded: Vec<ByteRange>,
}

impl<'b> Walker<'b> {
    /// Walk one level of siblings (the document's top level, or one list's items),
    /// `depth` being how many lists deep this level is nested.
    fn walk_block<'a>(&mut self, node: Node<'a>, depth: u8) {
        let (value, sourcepos) = node_kind(node);
        let range = self.lines.range(self.bytes, sourcepos);

        match value {
            NodeValue::FrontMatter(_) => {} // only ever a top-level node; handled by the caller
            NodeValue::Heading(heading) => {
                let text = text::collect(node);
                self.heading_stack
                    .retain(|(level, _)| *level < heading.level);
                let heading_path = self.current_heading_path();
                self.heading_stack.push((heading.level, text.clone()));
                self.collect_markdown_links(node);
                self.push_block(
                    BlockKind::Heading {
                        level: heading.level,
                    },
                    range,
                    text,
                    heading_path,
                );
            }
            NodeValue::Paragraph => {
                self.collect_markdown_links(node);
                let (text, block_ref) = strip_block_ref(text::collect(node));
                let heading_path = self.current_heading_path();
                self.push_block(BlockKind::Paragraph, range, text, heading_path)
                    .block_ref = block_ref;
            }
            NodeValue::BlockQuote => {
                self.collect_markdown_links(node);
                let (text, block_ref) = strip_block_ref(text::collect(node));
                let heading_path = self.current_heading_path();
                self.push_block(BlockKind::Quote, range, text, heading_path)
                    .block_ref = block_ref;
            }
            NodeValue::Table(_) => {
                self.collect_markdown_links(node);
                let (text, block_ref) = strip_block_ref(text::collect(node));
                let heading_path = self.current_heading_path();
                self.push_block(BlockKind::Table, range, text, heading_path)
                    .block_ref = block_ref;
            }
            NodeValue::ThematicBreak => {
                let heading_path = self.current_heading_path();
                self.push_block(BlockKind::Rule, range, String::new(), heading_path);
            }
            NodeValue::CodeBlock(code) => {
                self.excluded.push(range);
                let heading_path = self.current_heading_path();
                let language = normalise_language(&code.info);
                self.push_block(
                    BlockKind::Code { language },
                    range,
                    code.literal.trim_end_matches('\n').to_owned(),
                    heading_path,
                );
            }
            NodeValue::HtmlBlock(html) => {
                self.excluded.push(range);
                let heading_path = self.current_heading_path();
                self.push_block(
                    BlockKind::Html,
                    range,
                    html.literal.trim_end_matches('\n').to_owned(),
                    heading_path,
                );
            }
            NodeValue::List(_) => {
                self.walk_children(node, depth);
            }
            NodeValue::Item(_) => {
                self.collect_markdown_links(node);
                let (text, block_ref) = strip_block_ref(self.own_text(node));
                let heading_path = self.current_heading_path();
                self.push_block(BlockKind::ListItem { depth }, range, text, heading_path)
                    .block_ref = block_ref;
                self.walk_nested_lists(node, depth);
            }
            NodeValue::TaskItem(task) => {
                self.collect_markdown_links(node);
                let (raw_text, block_ref) = strip_block_ref(self.own_text(node));
                let marker = task.symbol.unwrap_or(' ');
                let done = Task::marker_is_done(marker);
                let marker_range = self.lines.range(self.bytes, task.symbol_sourcepos);
                let heading_path = self.current_heading_path();
                let block = self.push_block(
                    BlockKind::Task { depth },
                    range,
                    raw_text.clone(),
                    heading_path,
                );
                block.block_ref = block_ref;
                let ord = block.ord;
                self.tasks.push(Task {
                    block_ord: ord,
                    done,
                    marker,
                    text: inline_fields::strip(&raw_text),
                    range,
                    marker_range,
                });
                self.walk_nested_lists(node, depth);
            }
            // No BlockKind exists for these (module doc): dropped along with
            // whatever they contain, rather than guessing at a mapping.
            _ => {}
        }
    }

    fn walk_children<'a>(&mut self, node: Node<'a>, depth: u8) {
        for child in node.children() {
            self.walk_block(child, depth);
        }
    }

    /// A list item's own paragraph content, excluding any nested list — a nested
    /// list's items are indexed as their own blocks (at `depth + 1`), not folded
    /// into this item's text.
    fn own_text<'a>(&self, node: Node<'a>) -> String {
        let mut out = String::new();
        for child in node.children() {
            if matches!(node_kind(child).0, NodeValue::List(_)) {
                continue;
            }
            text::collect_into(child, &mut out);
        }
        out
    }

    fn walk_nested_lists<'a>(&mut self, node: Node<'a>, depth: u8) {
        for child in node.children() {
            if matches!(node_kind(child).0, NodeValue::List(_)) {
                self.walk_children(child, depth + 1);
            }
        }
    }

    fn current_heading_path(&self) -> Vec<String> {
        self.heading_stack
            .iter()
            .map(|(_, text)| text.clone())
            .collect()
    }

    fn push_block(
        &mut self,
        kind: BlockKind,
        range: ByteRange,
        text: String,
        heading_path: Vec<String>,
    ) -> &mut Block {
        let ord = self.blocks.len() as u32;
        self.blocks.push(Block {
            ord,
            kind,
            range,
            text: normalise_whitespace(&text),
            heading_path,
            block_ref: None,
        });
        self.blocks.last_mut().expect("just pushed")
    }

    /// Ordinary `[text](url)` links, from comrak's own `Link` nodes — not `Image`,
    /// which architecture treats as an embed-equivalent handled by its own
    /// `![[...]]` scan when written the wikilink way; a Markdown-style `![alt](src)`
    /// naming an external image is not a link between notes and is left as text.
    fn collect_markdown_links<'a>(&mut self, node: Node<'a>) {
        for descendant in node.descendants() {
            let (value, sourcepos) = node_kind(descendant);
            // An inline code span (backticks included) is no place for a tag or a
            // wikilink: the scan skips it (task #145).
            if let NodeValue::Code(_) = value {
                let range = self.lines.range(self.bytes, sourcepos);
                self.excluded.push(range);
                continue;
            }
            if let NodeValue::Link(link) = value {
                let range = self.lines.range(self.bytes, sourcepos);
                let alias = text::collect(descendant);
                let (note, heading, block_ref) = split_link_target(&link.url);
                self.markdown_links.push(Link {
                    kind: LinkKind::Markdown,
                    target: LinkTarget {
                        note,
                        heading,
                        block_ref,
                        alias: (!alias.is_empty()).then_some(alias),
                    },
                    range,
                    block_ord: None,
                });
            }
        }
    }
}

fn split_link_target(url: &str) -> (String, Option<String>, Option<String>) {
    match url.split_once('#') {
        Some((note, fragment)) if fragment.starts_with('^') => {
            (note.to_owned(), None, Some(fragment[1..].to_owned()))
        }
        Some((note, fragment)) => (note.to_owned(), Some(fragment.to_owned()), None),
        None => (url.to_owned(), None, None),
    }
}

/// The info string of a fenced code block, lowercased and trimmed to the language
/// name — GFM allows trailing attributes after the first word (`` ```rust,ignore ``
/// in some dialects); only the first word is a language anyone queries on.
fn normalise_language(info: &str) -> Option<String> {
    info.split_whitespace()
        .next()
        .map(|word| word.to_lowercase())
        .filter(|word| !word.is_empty())
}

/// Collapse runs of whitespace (including the newlines `text::collect` leaves from
/// block-level children) to single spaces, and trim the ends. What goes to
/// full-text search is meant to be read, not to preserve the source's line wrapping.
fn normalise_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Detect and strip a trailing `^block-ref` from a block's already-flattened text.
///
/// Working on the flattened string rather than raw bytes because the anchor is
/// meant to trail the block's *content*, and content is what `text::collect` (or
/// `Walker::own_text`) has already assembled — including across soft breaks, which
/// this runs after so a wrapped last line is still recognised.
fn strip_block_ref(text: String) -> (String, Option<String>) {
    let trimmed = text.trim_end();
    if let Some(caret) = trimmed.rfind('^') {
        let candidate = &trimmed[caret + 1..];
        let is_anchor = !candidate.is_empty()
            && candidate
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
        let preceded_by_boundary = caret == 0
            || trimmed[..caret]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if is_anchor && preceded_by_boundary {
            let before = trimmed[..caret].trim_end().to_owned();
            return (before, Some(candidate.to_owned()));
        }
    }
    (text, None)
}

#[cfg(test)]
pub(crate) mod test_support {
    use comrak::Arena;
    use comrak::nodes::AstNode;

    /// Parse `text` against this crate's own options, in a leaked arena. Only ever
    /// used from tests: leaking is how a `&'static AstNode` can be handed back
    /// without every call site managing an `Arena`'s lifetime itself, and a test
    /// process exits long before the leak would matter.
    pub(crate) fn parse(text: &str) -> &'static AstNode<'static> {
        let arena: &'static Arena<'static> = Box::leak(Box::new(Arena::new()));
        let leaked_text: &'static str = Box::leak(text.to_owned().into_boxed_str());
        comrak::parse_document(arena, leaked_text, &super::options())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serat_core::{Field, VaultPath};

    fn parse(text: &str) -> ParsedNote {
        let parser = CommonMarkParser::new();
        parser
            .parse(&VaultPath::new("note.md").expect("path"), text.as_bytes())
            .expect("parse")
    }

    fn field<'a>(parsed: &'a ParsedNote, key: &str) -> &'a Field {
        parsed
            .fields
            .iter()
            .find(|field| field.key == key)
            .unwrap_or_else(|| panic!("no field {key:?} among {:?}", parsed.fields))
    }

    #[test]
    fn a_wikilink_in_a_frontmatter_value_is_a_link() {
        // F-316. `owner:` is a scalar property and `attendees:` a block list;
        // both render as links in Obsidian and both appear in its backlinks, so
        // NFR-CMP-01 and FR-NAV-04 want them indexed. The block-list form matters
        // on its own: those lines are not fields today (the YAML subset skips
        // `-` items), so before this there was no trace of an attendee at all.
        let parsed = parse(
            "---\ntype: meeting\nowner: \"[[Juno Halvorsen]]\"\nattendees:\n  - \"[[Ada Lovelace]]\"\n  - \"[[Alan Turing]]\"\n---\n\n# Title\n\nBody with [[Body Link]].\n",
        );
        let mut targets: Vec<&str> = parsed
            .links
            .iter()
            .map(|l| l.target.note.as_str())
            .collect();
        targets.sort_unstable();
        assert_eq!(
            targets,
            vec!["Ada Lovelace", "Alan Turing", "Body Link", "Juno Halvorsen"],
            "every frontmatter wikilink is a link, alongside the body one"
        );

        // The ranges must be file-absolute, not relative to a frontmatter slice:
        // a link whose range is wrong is worse than one that is absent, because
        // P2's patcher trusts recorded ranges (C-04, ADR-0008).
        let src = "---\ntype: meeting\nowner: \"[[Juno Halvorsen]]\"\nattendees:\n  - \"[[Ada Lovelace]]\"\n  - \"[[Alan Turing]]\"\n---\n\n# Title\n\nBody with [[Body Link]].\n";
        let owner = parsed
            .links
            .iter()
            .find(|l| l.target.note == "Juno Halvorsen")
            .expect("the owner link");
        assert_eq!(
            &src[owner.range.start..owner.range.end],
            "[[Juno Halvorsen]]"
        );
    }

    #[test]
    fn a_hash_in_frontmatter_is_not_a_tag() {
        // `#` opens a comment in YAML, and frontmatter tags are declared with a
        // `tags:` key rather than `#tag`, so scanning frontmatter for tags would
        // invent links the file does not contain. The body's tag still counts.
        let parsed = parse(
            "---\ntitle: Note\n# a comment mentioning #notatag\n---\n\nBody #realtag here.\n",
        );
        let tags: Vec<&str> = parsed
            .links
            .iter()
            .filter(|l| l.kind == LinkKind::Tag)
            .map(|l| l.target.note.as_str())
            .collect();
        assert_eq!(tags, vec!["realtag"], "only the body tag is a tag");
    }

    #[test]
    fn frontmatter_supplies_title_and_type_and_fields() {
        let parsed = parse("---\ntitle: Serat\ntype: project\nstatus: active\n---\n\nBody.\n");
        assert_eq!(parsed.title.as_deref(), Some("Serat"));
        assert_eq!(parsed.note_type.as_deref(), Some("project"));
        assert_eq!(
            field(&parsed, "status").source,
            serat_core::FieldSource::Frontmatter
        );
    }

    #[test]
    fn title_falls_back_to_the_first_h1_then_the_filename() {
        let parsed = parse("# My Title\n\nBody.\n");
        assert_eq!(parsed.title.as_deref(), Some("My Title"));

        let parsed = parse("Just a paragraph, no heading.\n");
        assert_eq!(parsed.title.as_deref(), Some("note"));
    }

    #[test]
    fn headings_build_a_path_for_the_blocks_beneath_them() {
        let parsed = parse("# One\n## Two\nBody under Two.\n# Three\nBody under Three.\n");
        let body_under_two = parsed
            .blocks
            .iter()
            .find(|block| block.text == "Body under Two.")
            .expect("block");
        assert_eq!(body_under_two.heading_path, vec!["One", "Two"]);

        let body_under_three = parsed
            .blocks
            .iter()
            .find(|block| block.text == "Body under Three.")
            .expect("block");
        assert_eq!(body_under_three.heading_path, vec!["Three"]);
    }

    #[test]
    fn byte_ranges_point_at_the_blocks_own_bytes() {
        let source = "# Heading\n\nA paragraph.\n";
        let parsed = parse(source);
        let bytes = source.as_bytes();
        for block in &parsed.blocks {
            let slice = block.range.slice(bytes).expect("range in bounds");
            let text = String::from_utf8_lossy(slice);
            assert!(
                text.contains(block.text.trim()) || block.text.is_empty(),
                "block text {:?} not found in its own byte range {:?}",
                block.text,
                text
            );
        }
    }

    #[test]
    fn a_task_records_done_and_the_marker_character() {
        let parsed = parse("- [x] Ship it\n- [ ] Not yet\n");
        assert_eq!(parsed.tasks.len(), 2);
        assert!(parsed.tasks[0].done);
        assert_eq!(parsed.tasks[0].marker, 'x');
        assert!(!parsed.tasks[1].done);
        assert_eq!(parsed.tasks[1].marker, ' ');
    }

    #[test]
    fn a_task_marker_range_points_at_exactly_the_bracket_character() {
        let source = "- [x] Ship it\n";
        let parsed = CommonMarkParser::new()
            .parse(&VaultPath::new("note.md").expect("path"), source.as_bytes())
            .expect("parse");
        let task = &parsed.tasks[0];
        assert_eq!(task.marker_range.slice(source.as_bytes()), Some(&b"x"[..]));
    }

    #[test]
    fn nested_list_items_get_increasing_depth_and_their_own_blocks() {
        let parsed = parse("- top\n  - nested\n    - double nested\n");
        let depths: Vec<u8> = parsed
            .blocks
            .iter()
            .filter_map(|block| match block.kind {
                BlockKind::ListItem { depth } => Some(depth),
                _ => None,
            })
            .collect();
        assert_eq!(depths, vec![0, 1, 2]);
    }

    #[test]
    fn a_nested_items_text_does_not_duplicate_into_its_parent() {
        let parsed = parse("- top\n  - nested\n");
        let top = parsed
            .blocks
            .iter()
            .find(|block| matches!(block.kind, BlockKind::ListItem { depth: 0 }))
            .expect("top item");
        assert_eq!(top.text, "top");
    }

    #[test]
    fn a_block_reference_is_split_from_the_paragraph_text() {
        let parsed = parse("This is a cited paragraph. ^cite1\n");
        let block = &parsed.blocks[0];
        assert_eq!(block.text, "This is a cited paragraph.");
        assert_eq!(block.block_ref.as_deref(), Some("cite1"));
    }

    #[test]
    fn a_code_block_keeps_its_literal_content_and_language() {
        let parsed = parse("```rust\nfn main() {}\n```\n");
        let block = &parsed.blocks[0];
        assert_eq!(
            block.kind,
            BlockKind::Code {
                language: Some("rust".to_owned())
            }
        );
        assert_eq!(block.text, "fn main() {}");
    }

    #[test]
    fn a_wikilink_and_a_tag_inside_the_body_are_found_and_attached_to_their_block() {
        let parsed = parse("See [[Other Note]] and #important.\n");
        assert_eq!(parsed.links.len(), 2);
        let wiki = parsed
            .links
            .iter()
            .find(|link| link.kind == LinkKind::Wikilink)
            .expect("wikilink");
        assert_eq!(wiki.target.note, "Other Note");
        assert_eq!(wiki.block_ord, Some(0));
    }

    #[test]
    fn a_wikilink_inside_a_fenced_code_block_is_not_extracted() {
        let parsed = parse("```\n[[not a link]]\n```\n");
        assert!(parsed.links.is_empty(), "{:?}", parsed.links);
    }

    /// Every tag and wikilink target `text` yields, in order (task #145).
    fn tags_and_links(text: &str) -> Vec<(LinkKind, String)> {
        parse(text)
            .links
            .into_iter()
            .map(|link| (link.kind, link.target.note))
            .collect()
    }

    #[test]
    fn a_tag_right_after_a_closing_backtick_is_still_a_tag() {
        // The span's range ends at the closing backtick, exclusive: `#b` follows it.
        assert_eq!(
            tags_and_links("`#a`#b\n"),
            vec![(LinkKind::Tag, "b".to_owned())]
        );
    }

    #[test]
    fn a_code_span_at_offset_zero_and_one_ending_at_eof_hold_no_tag() {
        assert!(tags_and_links("`#a` text\n").is_empty());
        assert!(
            tags_and_links("text `#a`").is_empty(),
            "no trailing newline"
        );
        assert!(tags_and_links("`#a`").is_empty(), "the whole file");
        assert!(
            tags_and_links("``#a ` b``\n").is_empty(),
            "a double-backtick span"
        );
    }

    #[test]
    fn an_unterminated_backtick_opens_no_span_so_the_tag_after_it_is_found() {
        assert_eq!(
            tags_and_links("a ` b #c\n"),
            vec![(LinkKind::Tag, "c".to_owned())]
        );
    }

    #[test]
    fn a_code_span_in_a_heading_a_quote_a_list_item_and_a_table_holds_no_tag() {
        for text in [
            "# h `#a`\n",
            "> q `#a`\n",
            "- i `#a`\n",
            "- [ ] t `#a`\n",
            "| h |\n|---|\n| `#a` |\n",
        ] {
            assert!(tags_and_links(text).is_empty(), "{text:?}");
        }
    }

    #[test]
    fn a_markdown_link_is_recorded_with_its_display_text_as_alias() {
        let parsed = parse("See [the docs](https://example.com/docs).\n");
        let link = &parsed.links[0];
        assert_eq!(link.kind, LinkKind::Markdown);
        assert_eq!(link.target.note, "https://example.com/docs");
        assert_eq!(link.target.alias.as_deref(), Some("the docs"));
    }

    #[test]
    fn an_inline_field_on_its_own_line_is_indexed_against_its_block() {
        let parsed = parse("status:: active\n");
        let f = field(&parsed, "status");
        assert_eq!(f.block_ord, Some(0));
    }

    #[test]
    fn a_dropped_construct_does_not_fail_the_whole_note() {
        // Footnotes have no BlockKind (module doc). The note must still index
        // everything around the dropped construct.
        let parsed = parse("Before.\n\n[^1]: A footnote definition.\n\nAfter.\n");
        let texts: Vec<&str> = parsed.blocks.iter().map(|b| b.text.as_str()).collect();
        assert!(texts.contains(&"Before."));
        assert!(texts.contains(&"After."));
    }
}
