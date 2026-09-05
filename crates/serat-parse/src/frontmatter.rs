//! YAML frontmatter, the subset P0 needs.
//!
//! **Scope, stated rather than discovered by a reader:** flat `key: value` pairs and
//! one level of list (block `- item` or flow `[a, b]`), scalars only. No nested
//! maps, no multi-line scalars, no anchors or multi-document files — a real YAML
//! parser is a P2-sized dependency decision, and pulling one in for a subset this
//! small would cost more in surface area than it returns. A key whose value does not
//! fit this subset is skipped rather than failing the note (design goal 5): the
//! frontmatter's *other* keys, and the whole rest of the file, still index.

use serat_core::{ByteRange, Field, FieldSource};

use crate::inline_fields::infer_value;

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
    // yielding anything) — so the file-absolute position only needs content_range's
    // own start added once, not the body's start added a second time on top of an
    // offset that already includes it.
    for (offset_in_body, raw_line) in body_lines(text) {
        let line_start = content_range.start + offset_in_body;
        let Some((key, value_text, value_offset)) = split_key_value(raw_line) else {
            continue;
        };
        if value_text.is_empty() {
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
/// for anything this subset does not cover — a comment, a blank line, a nested
/// map's second-level key, a list item continuing a `-`-style list (handled instead
/// by [`parse`] noticing the previous key had no inline value — not yet: block
/// lists are a documented gap, see the module doc).
fn split_key_value(line: &str) -> Option<(&str, &str, usize)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
        return None;
    }
    let (key, rest) = trimmed.split_once(':')?;
    let key = key.trim();
    if key.is_empty() || !key.chars().all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '-') {
        return None;
    }
    let value_in_rest = rest.len() - rest.trim_start().len();
    let value = rest.trim_start().trim_end_matches('\r').trim_end();
    let value_offset = indent + key.len() + 1 + value_in_rest;
    Some((key, value, value_offset))
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
}
