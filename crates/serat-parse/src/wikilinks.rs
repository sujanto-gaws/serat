//! Wikilinks (`[[Note]]`), embeds (`![[Note]]`) and tags (`#tag`).
//!
//! None of these three are CommonMark, and comrak's own wikilink extension does not
//! cover the shape Obsidian actually writes: it has no notion of `![[embed]]`, and
//! it does not split `Note#Heading` or `Note#^block-ref` into a target and a
//! fragment — the whole bracket content becomes one opaque URL. So all three are
//! found here, by scanning the raw bytes directly, which is also what lets one
//! scanner produce byte-accurate `range`s for all of them without a second pass
//! through comrak's inline tree to re-locate what it already saw.
//!
//! Scoped deliberately: `exclude` keeps this out of frontmatter, out of fenced code /
//! HTML blocks and out of *inline* code spans (`` `#not a tag` ``), whose ranges the
//! caller takes from comrak's inline tree (task #145; Obsidian indexes nothing inside
//! a code span). The exclusion applies at a match's *start*: `[[a `b` c]]` still links,
//! with the span inside the link text, because the link begins outside any span.

use serat_core::{ByteRange, Link, LinkKind, LinkTarget};

/// Every wikilink, embed and tag in `text`, outside of `exclude`d ranges.
pub(crate) fn scan(text: &str, exclude: &[ByteRange]) -> Vec<Link> {
    let exclude = &merged(exclude);
    let bytes = text.as_bytes();
    let mut links = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if excluded(exclude, i) {
            i += 1;
            continue;
        }
        if bytes[i] == b'#'
            && let Some((tag, end)) = tag_at(bytes, i)
        {
            links.push(Link {
                kind: LinkKind::Tag,
                target: LinkTarget::note(tag),
                range: ByteRange::new(i, end),
                block_ord: None,
            });
            i = end;
            continue;
        }
        if bytes[i] == b'['
            && bytes.get(i + 1) == Some(&b'[')
            && let Some((link, end)) = bracket_link(text, i, false)
        {
            links.push(link);
            i = end;
            continue;
        }
        if bytes[i] == b'!'
            && bytes.get(i + 1) == Some(&b'[')
            && bytes.get(i + 2) == Some(&b'[')
            && let Some((link, end)) = bracket_link(text, i + 1, true)
        {
            links.push(Link {
                range: ByteRange::new(i, link.range.end),
                ..link
            });
            i = end;
            continue;
        }
        i += 1;
    }
    links
}

fn excluded(exclude: &[ByteRange], offset: usize) -> bool {
    // `exclude` is sorted and disjoint (see `merged`): the only candidate is the last
    // range starting at or before `offset`.
    let after = exclude.partition_point(|range| range.start <= offset);
    after > 0 && offset < exclude[after - 1].end
}

/// `exclude` sorted by start with overlapping or touching ranges joined, so
/// [`excluded`] is a binary search rather than a scan of every range per byte (a note
/// with thousands of code spans would otherwise be quadratic).
fn merged(exclude: &[ByteRange]) -> Vec<ByteRange> {
    let mut sorted = exclude.to_vec();
    sorted.sort_by_key(|range| range.start);
    let mut out: Vec<ByteRange> = Vec::with_capacity(sorted.len());
    for range in sorted {
        match out.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => out.push(range),
        }
    }
    out
}

/// A tag starting at `text[start]` (which is `#`), or `None` if what follows is not
/// tag-shaped — most commonly an ATX heading (`# Heading`, a literal space right
/// after `#`) or a bare `#` followed by a digit (Obsidian tags may not be entirely
/// numeric; requiring a leading letter or underscore is the simple version of that
/// rule).
fn tag_at(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    let first = *bytes.get(start + 1)?;
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    let mut end = start + 1;
    while let Some(&byte) = bytes.get(end) {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'/') {
            end += 1;
        } else {
            break;
        }
    }
    let tag = std::str::from_utf8(&bytes[start + 1..end]).ok()?.to_owned();
    Some((tag, end))
}

/// A `[[...]]` occurrence starting at `text[start]` (the first `[`), or `None` if it
/// is never closed on the same... anywhere in `text` — Obsidian wikilinks do not
/// span paragraph breaks in practice, but nothing here assumes single-line, only
/// that a closing `]]` exists at all.
fn bracket_link(text: &str, start: usize, embed: bool) -> Option<(Link, usize)> {
    let bytes = text.as_bytes();
    let inner_start = start + 2;
    let close = find_close(bytes, inner_start)?;
    let inner = &text[inner_start..close];
    if inner.is_empty() || inner.contains('[') {
        // An empty [[]] names nothing; a stray "[" inside means this was not a
        // wikilink to begin with (most often two adjacent single-bracket links,
        // "[a][[b]]", which this scanner is not the parser for — comrak's own Link
        // handling owns single-bracket links).
        return None;
    }

    let (target_part, alias) = match inner.split_once('|') {
        Some((target, alias)) => (target, Some(alias.trim().to_owned())),
        None => (inner, None),
    };

    let (note, heading, block_ref) = split_target(target_part.trim());

    let end = close + 2;
    Some((
        Link {
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
            range: ByteRange::new(start, end),
            block_ord: None,
        },
        end,
    ))
}

/// The byte offset of the `]]` closing a wikilink opened at `inner_start`, if one
/// exists before the text ends. Does not cross a blank line: two consecutive
/// newlines end a wikilink search the same way they end a paragraph, so an
/// unrelated `[[` early in a file cannot swallow everything after it looking for a
/// `]]` that was never meant to close it.
fn find_close(bytes: &[u8], inner_start: usize) -> Option<usize> {
    let mut i = inner_start;
    let mut consecutive_newlines = 0;
    while i + 1 < bytes.len() {
        match bytes[i] {
            b']' if bytes[i + 1] == b']' => return Some(i),
            b'\n' => {
                consecutive_newlines += 1;
                if consecutive_newlines >= 2 {
                    return None;
                }
            }
            _ => consecutive_newlines = 0,
        }
        i += 1;
    }
    None
}

/// Split `Note#Heading` or `Note#^block-ref` into their parts. A bare `#^` with
/// nothing before it names the current note, which is written here as an empty
/// `note` — `serat-index`'s resolution is unaffected either way, since an empty
/// path resolves to nothing today (self-references are a later phase).
fn split_target(target: &str) -> (String, Option<String>, Option<String>) {
    match target.split_once('#') {
        Some((note, fragment)) if fragment.starts_with('^') => {
            (note.to_owned(), None, Some(fragment[1..].to_owned()))
        }
        Some((note, fragment)) => (note.to_owned(), Some(fragment.to_owned()), None),
        None => (target.to_owned(), None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_wikilink_names_a_note() {
        let links = scan("See [[Meeting Notes]] for details.", &[]);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].kind, LinkKind::Wikilink);
        assert_eq!(links[0].target.note, "Meeting Notes");
        assert_eq!(links[0].target.alias, None);
    }

    #[test]
    fn an_embed_is_distinguished_from_a_wikilink() {
        let links = scan("![[diagram.png]]", &[]);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].kind, LinkKind::Embed);
        assert_eq!(links[0].target.note, "diagram.png");
        // The range covers the leading "!" too, and "![[diagram.png]]" is 16 bytes.
        assert_eq!(links[0].range, ByteRange::new(0, 16));
    }

    #[test]
    fn an_alias_is_split_from_the_target() {
        let links = scan("[[Meeting Notes|last week's meeting]]", &[]);
        assert_eq!(links[0].target.note, "Meeting Notes");
        assert_eq!(
            links[0].target.alias.as_deref(),
            Some("last week's meeting")
        );
    }

    #[test]
    fn a_heading_fragment_is_split_from_the_note() {
        let links = scan("[[Project#Status]]", &[]);
        assert_eq!(links[0].target.note, "Project");
        assert_eq!(links[0].target.heading.as_deref(), Some("Status"));
        assert_eq!(links[0].target.block_ref, None);
    }

    #[test]
    fn a_block_ref_fragment_is_split_from_the_note() {
        let links = scan("[[Project#^abc123]]", &[]);
        assert_eq!(links[0].target.note, "Project");
        assert_eq!(links[0].target.block_ref.as_deref(), Some("abc123"));
        assert_eq!(links[0].target.heading, None);
    }

    #[test]
    fn a_tag_is_found() {
        let links = scan("This is #important and #project/serat work.", &[]);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].kind, LinkKind::Tag);
        assert_eq!(links[0].target.note, "important");
        assert_eq!(links[1].target.note, "project/serat");
    }

    #[test]
    fn an_atx_heading_marker_is_not_a_tag() {
        let links = scan("# Heading\n## Another\ncontent #real-tag\n", &[]);
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links[0].target.note, "real-tag");
    }

    #[test]
    fn a_purely_numeric_hash_is_not_a_tag() {
        let links = scan("issue #1234 needs a #fix\n", &[]);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target.note, "fix");
    }

    fn range(start: usize, end: usize) -> ByteRange {
        ByteRange::new(start, end)
    }

    #[test]
    fn merged_sorts_and_joins_touching_and_overlapping_ranges_and_keeps_empty_ones_harmless() {
        let out = merged(&[
            range(20, 25),
            range(0, 5),
            range(5, 8),   // touches the first
            range(22, 30), // overlaps the 20..25
            range(40, 40), // zero-length
            range(2, 3),   // inside the first
        ]);
        assert_eq!(out, vec![range(0, 8), range(20, 30), range(40, 40)]);
        assert!(excluded(&out, 0) && excluded(&out, 7) && !excluded(&out, 8));
        assert!(excluded(&out, 29) && !excluded(&out, 30));
        assert!(!excluded(&out, 40), "a zero-length range excludes nothing");
        assert!(merged(&[]).is_empty() && !excluded(&[], 0));
    }

    #[test]
    fn a_wikilink_whose_text_holds_a_code_span_still_links() {
        // The span excludes only matches that *start* inside it.
        let links = scan("[[a `b` c]]", &[range(4, 7)]);
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links[0].target.note, "a `b` c");
    }

    #[test]
    fn an_excluded_range_is_skipped() {
        let text = "```\n#not-a-tag [[not a link]]\n```\n#real-tag";
        let excluded = ByteRange::new(0, 34); // the fenced block, roughly
        let links = scan(text, &[excluded]);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target.note, "real-tag");
    }

    #[test]
    fn an_unclosed_wikilink_is_not_reported() {
        let links = scan("Some [[unfinished", &[]);
        assert!(links.is_empty());
    }

    #[test]
    fn a_wikilink_does_not_search_across_a_blank_line() {
        let links = scan("[[note\n\nmore text]] and [[a real one]]", &[]);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target.note, "a real one");
    }
}
