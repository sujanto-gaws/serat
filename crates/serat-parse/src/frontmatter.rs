//! YAML frontmatter, the subset Serat indexes.
//!
//! **Scope, stated rather than discovered by a reader** (ADR-0003 fixes how fields
//! are *queried*, not which YAML is read; this module doc is where that subset is
//! written down):
//!
//! * flat `key: value` pairs, scalars only;
//! * one level of list per key, either flow (`[a, b]`) or block:
//!
//!   ```yaml
//!   aliases:
//!     - plain scalar
//!     - "quoted scalar"   # trailing comment
//!   ```
//!
//!   The items of a block list must all sit at one indentation (any depth, zero
//!   included), may be separated by blank or comment lines, and become one
//!   [`Field`] whose value is a `List` -- the index writes one row per item. A
//!   quoted item is always text (`"2026"` is the string); a plain one is typed like
//!   any other scalar. Quotes are removed, not unescaped. An empty item (`-`) is
//!   YAML null and is dropped. The field's `value_range` runs from the first `-`
//!   marker to the end of the last item's text.
//! * **Refused, whole field, no rows:** a block list whose items are nested
//!   sequences (`- - a`, or an item at a different indent from the first), maps
//!   (`- name: x`), flow collections (`- [a]`, `- {a: b}`, which includes an
//!   unquoted `- [[Wikilink]]`: quote it), block scalars (`|`, `>`), anchors, aliases
//!   or tags (`&`, `*`, `!`), a quoted item with text after the closing quote, or an
//!   unterminated quote. A refused field is a skipped field, never an error and
//!   never a partial list: half an `aliases` list would index as if it were whole.
//! * **Refused the same way:** a nested map under a key (`parent:` followed by
//!   indented `child: x`). Its children are consumed with it, so they no longer
//!   surface as top-level fields of their own (before #57 they did: a type note's
//!   `    type: date` line set the note's `type`).
//! * A value that starts with `#` is a YAML comment, so `tags: #project` has no value
//!   and no field (it used to index as the text `#project`; Obsidian agrees with YAML).
//! * Indentation is whatever whitespace `trim_start` removes, tabs included. YAML
//!   forbids tab indentation; the subset does not police it, because refusing would
//!   lose lists that real vaults do contain.
//! * Flow lists split on commas outside quotes, and each item is read exactly like a
//!   block item, so `[a, "b, c"]` and the block form give the same values and rows.
//! * No multi-line scalars, anchors or multi-document files -- a real YAML parser is
//!   a P2-sized dependency decision, and pulling one in for a subset this small
//!   would cost more in surface area than it returns.
//!
//! A key whose value does not fit is skipped rather than failing the note (design
//! goal 5): the frontmatter's *other* keys, and the whole rest of the file, still
//! index.

use serat_core::{ByteRange, Field, FieldSource, FieldValue};

use crate::inline_fields::{infer_scalar, infer_value};

/// What frontmatter parsing found, beyond the field rows themselves.
pub(crate) struct Frontmatter {
    pub(crate) fields: Vec<Field>,
    /// The `title` key, if present and a plain scalar.
    pub(crate) title: Option<String>,
    /// The `type` key, if present and a plain scalar — FR-STR-04's typed-note name.
    pub(crate) note_type: Option<String>,
}

/// Parse the frontmatter block's content.
///
/// `content_range` is the frontmatter block's byte range as comrak reported it,
/// which for `NodeValue::FrontMatter` includes the `---` delimiter lines on both
/// sides — this function is what strips those to find the lines actually worth
/// scanning as YAML.
pub(crate) fn parse(bytes: &[u8], content_range: ByteRange) -> Frontmatter {
    let mut fields = Vec::new();
    let mut title = None;
    let mut note_type = None;

    let Some(slice) = content_range.slice(bytes) else {
        return Frontmatter {
            fields,
            title,
            note_type,
        };
    };
    let Ok(text) = std::str::from_utf8(slice) else {
        return Frontmatter {
            fields,
            title,
            note_type,
        };
    };

    // body_lines yields offsets already absolute within `text` (it starts counting
    // from 0, the start of the opening delimiter line, and skips past it before
    // yielding anything) -- so the file-absolute position only needs content_range's
    // own start added once, not the body's start added a second time on top of an
    // offset that already includes it.
    let lines: Vec<(usize, &str)> = body_lines(text)
        .map(|(offset, line)| (content_range.start + offset, line))
        .collect();

    let mut at = 0;
    while at < lines.len() {
        let (line_start, raw_line) = lines[at];
        at += 1;
        let Some((key, value_text, value_offset)) = split_key_value(raw_line) else {
            continue;
        };
        if value_text.is_empty() || value_text.starts_with('#') {
            // Nothing inline: a block list, a nested map, or a null. Everything
            // indented under it (and any `- item` at the key's own indent) belongs
            // to it, so none of it is read as a key of its own.
            let key_indent = raw_line.len() - raw_line.trim_start().len();
            let end = continuation_end(&lines, at, key_indent);
            if let Some(field) = block_list(key, line_start, &lines[at..end]) {
                fields.push(field);
            }
            at = end;
            continue;
        }

        let value_start = line_start + value_offset;
        let value_end = value_start + value_text.len();
        let value_range = ByteRange::new(value_start, value_end);
        let key_range = ByteRange::new(line_start, value_end);

        if key == "title" {
            title = Some(strip_quotes(value_text).to_owned());
        }
        if key == "type" {
            note_type = Some(strip_quotes(value_text).to_owned());
        }

        fields.push(Field {
            key: key.to_owned(),
            value: infer_value(strip_quotes(value_text)),
            source: FieldSource::Frontmatter,
            range: key_range,
            value_range,
            block_ord: None,
        });
    }

    Frontmatter {
        fields,
        title,
        note_type,
    }
}

/// Every line of the frontmatter body, skipping the opening and closing delimiter
/// lines, paired with its byte offset relative to the start of the body (the byte
/// right after the opening `---` line's newline).
fn body_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut lines = text.split('\n');
    lines.next(); // the opening "---"
    let mut offset = text
        .split_once('\n')
        .map_or(text.len(), |(first, _)| first.len() + 1);

    std::iter::from_fn(move || {
        let line = lines.next()?;
        // The closing delimiter, or the trailing empty split after the final
        // newline: both end the body.
        if line.trim_end_matches('\r') == "---" {
            return None;
        }
        let this_offset = offset;
        offset += line.len() + 1;
        Some((this_offset, line))
    })
}

/// Split one YAML-subset line into `(key, value, value_offset_in_line)`, or `None`
/// for anything this subset does not cover — a comment, a blank line, a `- item`
/// line (block lists are read by [`parse`] when it sees a key with no inline value,
/// see [`block_list`]).
fn split_key_value(line: &str) -> Option<(&str, &str, usize)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
        return None;
    }
    let (key, rest) = trimmed.split_once(':')?;
    let key = key.trim();
    if key.is_empty()
        || !key
            .chars()
            .all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '-')
    {
        return None;
    }
    let value_in_rest = rest.len() - rest.trim_start().len();
    let value = rest.trim_start().trim_end_matches('\r').trim_end();
    let value_offset = indent + key.len() + 1 + value_in_rest;
    Some((key, value, value_offset))
}

/// The index one past the last line that belongs to the key whose value starts on
/// `from`: blank and comment lines, lines indented deeper than the key, and `-`
/// items at the key's own indent. A line at the key's indent that is not an item is
/// the next sibling key, never a child.
fn continuation_end(lines: &[(usize, &str)], from: usize, key_indent: usize) -> usize {
    let mut end = from;
    for (at, (_, raw)) in lines.iter().enumerate().skip(from) {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim_start();
        let belongs = trimmed.is_empty()
            || trimmed.starts_with('#')
            || line.len() - trimmed.len() > key_indent
            || (line.len() - trimmed.len() == key_indent && is_item_marker(trimmed));
        if !belongs {
            break;
        }
        // Trailing blank/comment lines are not part of the value; only advance
        // past a line that carries something.
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            end = at + 1;
        }
    }
    end
}

/// `-` alone or followed by a space/tab: a sequence entry marker.
fn is_item_marker(trimmed: &str) -> bool {
    let mut chars = trimmed.chars();
    chars.next() == Some('-') && matches!(chars.next(), None | Some(' ' | '\t'))
}

/// Read the lines under `key:` as a block sequence of scalars, or `None` if they
/// are not one (module doc: what is refused). `lines` carries file-absolute offsets.
fn block_list(key: &str, key_line_start: usize, lines: &[(usize, &str)]) -> Option<Field> {
    let mut items = Vec::new();
    let mut indent = None;
    let mut first_marker = None;
    let mut last_end = None;

    for &(line_start, raw) in lines {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let this_indent = line.len() - trimmed.len();
        if !is_item_marker(trimmed) || *indent.get_or_insert(this_indent) != this_indent {
            return None;
        }
        let after_marker = &trimmed[1..];
        let body = after_marker.trim_start();
        let body_offset = this_indent + 1 + (after_marker.len() - body.len());
        first_marker.get_or_insert(line_start + this_indent);

        let Some((value, length)) = item_scalar(body)? else {
            continue; // null item
        };
        last_end = Some(line_start + body_offset + length);
        items.push(value);
    }

    let (first, last) = (first_marker?, last_end?);
    Some(Field {
        key: key.to_owned(),
        value: FieldValue::List(items),
        source: FieldSource::Frontmatter,
        range: ByteRange::new(key_line_start, last),
        value_range: ByteRange::new(first, last),
        block_ord: None,
    })
}

/// One block-list item's text. The outer `None` is a refusal; `Some(None)` is a null
/// (empty) item; `Some(Some((value, len)))` is a scalar and how many bytes of `body`
/// it spans (quotes included, trailing comment excluded).
pub(crate) fn item_scalar(body: &str) -> Option<Option<(FieldValue, usize)>> {
    let quote = match body.chars().next() {
        None | Some('#') => return Some(None),
        Some(q @ ('"' | '\'')) => q as u8,
        Some('[' | '{' | '&' | '*' | '!' | '|' | '>' | '%' | '@' | '`' | '?') => return None,
        Some('-') if is_item_marker(body) => return None,
        Some(_) => 0,
    };

    if quote != 0 {
        let bytes = body.as_bytes();
        let mut at = 1;
        let close = loop {
            match *bytes.get(at)? {
                b'\\' if quote == b'"' => at += 2,
                b'\'' if quote == b'\'' && bytes.get(at + 1) == Some(&b'\'') => at += 2,
                b if b == quote => break at,
                _ => at += 1,
            }
        };
        let rest = body[close + 1..].trim_start();
        if !rest.is_empty() && !rest.starts_with('#') {
            return None;
        }
        return Some(Some((
            FieldValue::Text(body[1..close].to_owned()),
            close + 1,
        )));
    }

    // Plain scalar: ends at ` #` (a comment) or the end of the line.
    let mut text = body;
    if let Some(at) = text.find(" #").or_else(|| text.find("\t#")) {
        text = &text[..at];
    }
    let text = text.trim_end();
    // `key: value` is a map item.
    if text.ends_with(':') || text.contains(": ") || text.contains(":\t") {
        return None;
    }
    Some(Some((infer_scalar(text), text.len())))
}

/// Strip one layer of matching quotes, the way a YAML scanner would for a quoted
/// scalar. Not unescaping — `\"` inside a double-quoted string is out of scope for
/// the same reason nested maps are.
fn strip_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serat_core::FieldValue;

    #[test]
    fn flat_scalars_are_read_with_their_types() {
        let bytes = b"---\ntitle: Serat\ntype: project\ndone: true\ncount: 3\n---\n";
        let range = ByteRange::new(0, bytes.len());
        let fm = parse(bytes, range);

        assert_eq!(fm.title.as_deref(), Some("Serat"));
        assert_eq!(fm.note_type.as_deref(), Some("project"));

        let by_key = |key: &str| fm.fields.iter().find(|field| field.key == key);
        assert_eq!(
            by_key("done").map(|field| &field.value),
            Some(&FieldValue::Bool(true))
        );
        assert_eq!(
            by_key("count").map(|field| &field.value),
            Some(&FieldValue::Num(3.0))
        );
    }

    #[test]
    fn a_flow_list_becomes_a_list_field() {
        let bytes = b"---\ntags: [a, b, c]\n---\n";
        let range = ByteRange::new(0, bytes.len());
        let fm = parse(bytes, range);
        assert_eq!(
            fm.fields[0].value,
            FieldValue::List(vec![
                FieldValue::Text("a".to_owned()),
                FieldValue::Text("b".to_owned()),
                FieldValue::Text("c".to_owned()),
            ])
        );
    }

    #[test]
    fn quoted_scalars_have_their_quotes_removed_but_not_unescaped() {
        let bytes = b"---\ntitle: \"Serat: a Vault\"\n---\n";
        let range = ByteRange::new(0, bytes.len());
        let fm = parse(bytes, range);
        // "Serat: a Vault" contains a colon, which split_key_value's naive
        // split_once(':') would misread as key="title", value="\"Serat"  if it
        // were not for split_once taking only the FIRST colon — value is
        // "\"Serat: a Vault\"" whole, and stripping quotes removes only the outer
        // pair.
        assert_eq!(fm.title.as_deref(), Some("Serat: a Vault"));
    }

    #[test]
    fn a_value_range_points_at_exactly_the_value_bytes() {
        let bytes = b"---\nstatus: active\n---\n";
        let range = ByteRange::new(0, bytes.len());
        let fm = parse(bytes, range);
        let field = &fm.fields[0];
        assert_eq!(field.value_range.slice(bytes), Some(&b"active"[..]));
    }

    #[test]
    fn a_line_that_is_not_key_value_shaped_is_skipped_not_fatal() {
        // A block list under a key — out of this subset's scope, documented in the
        // module doc — must not stop the OTHER keys in the same frontmatter from
        // being read.
        let bytes = b"---\ntags:\n  - a\n  - b\ntitle: Still Here\n---\n";
        let range = ByteRange::new(0, bytes.len());
        let fm = parse(bytes, range);
        assert_eq!(fm.title.as_deref(), Some("Still Here"));
    }

    #[test]
    fn a_block_list_becomes_a_list_field_and_neighbours_survive() {
        let bytes = b"---\ntags:\n  - a\n  - \"b c\"\ntitle: Still Here\n---\n";
        let fm = parse(bytes, ByteRange::new(0, bytes.len()));
        assert_eq!(fm.title.as_deref(), Some("Still Here"));
        assert_eq!(
            fm.fields[0].value,
            FieldValue::List(vec![
                FieldValue::Text("a".to_owned()),
                FieldValue::Text("b c".to_owned())
            ])
        );
    }

    #[test]
    fn a_nested_map_does_not_leak_its_children_as_top_level_keys() {
        let bytes = b"---\nparent:\n  type: date\n  title: nope\n---\n";
        let fm = parse(bytes, ByteRange::new(0, bytes.len()));
        assert!(fm.fields.is_empty());
        assert_eq!(fm.note_type, None);
        assert_eq!(fm.title, None);
    }

    #[test]
    fn no_short_frontmatter_over_a_hostile_alphabet_panics_and_every_range_is_in_bounds() {
        // Every string of up to five characters from the characters that steer the
        // block-list scanner. The assertion is "returns, with ranges inside the
        // input and on char boundaries"; what it returns is covered above.
        let alphabet = [
            "k:", "\n", " ", "-", "\"", "'", "#", "\\", "[", "\r", "\u{e9}", "\t",
        ];
        let mut stack: Vec<Vec<usize>> = vec![vec![]];
        while let Some(word) = stack.pop() {
            let body: String = word.iter().map(|&i| alphabet[i]).collect();
            for doc in [
                format!("---\nkey:\n{body}\n---\n"),
                format!("---\nkey: [{body}\n{body}"), // flow-ish, unclosed, no newline
            ] {
                let bytes = doc.as_bytes();
                let fm = parse(bytes, ByteRange::new(0, bytes.len()));
                for field in &fm.fields {
                    for range in [field.range, field.value_range] {
                        assert!(
                            range.start <= range.end && range.end <= bytes.len(),
                            "{doc:?}"
                        );
                        assert!(range.slice(bytes).is_some(), "{doc:?}");
                        assert!(
                            doc.is_char_boundary(range.start) && doc.is_char_boundary(range.end)
                        );
                    }
                }
            }
            if word.len() < 5 {
                for next in 0..alphabet.len() {
                    let mut longer = word.clone();
                    longer.push(next);
                    stack.push(longer);
                }
            }
        }
    }

    #[test]
    fn a_child_at_the_keys_own_nonzero_indent_is_a_sibling_not_swallowed() {
        let bytes = b"---\n  parent:\n  child: x\n---\n";
        let fm = parse(bytes, ByteRange::new(0, bytes.len()));
        assert_eq!(fm.fields.len(), 1);
        assert_eq!(fm.fields[0].key, "child");
    }

    #[test]
    fn a_value_that_starts_with_a_hash_is_a_comment() {
        let bytes = b"---\ntags: #project\n---\n";
        let fm = parse(bytes, ByteRange::new(0, bytes.len()));
        assert!(fm.fields.is_empty());
    }
}
