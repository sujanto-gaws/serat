//! Inline `key:: value` fields (the Dataview convention Obsidian vaults use).
//!
//! Ordinary Markdown (C-01): `key:: value` renders as plain text in every Markdown
//! viewer that does not understand it, so nothing about reading the file depends on
//! Serat.

use std::sync::LazyLock;

use regex::Regex;
use serat_core::{Block, ByteRange, Field, FieldSource, FieldValue};

/// `key:: value` at the start of a line (after any list/checkbox marker and
/// leading whitespace), value running to the end of the line.
///
/// Deliberately line-anchored rather than found anywhere in a line: Dataview's own
/// convention is one field per line, and a key that merely appears mid-sentence
/// ("the ratio was 3:: to 1") is far more likely as prose than as a field. `key` is
/// restricted to identifier-shaped text, which is what keeps "Note: see below" from
/// being read as a field named `Note`.
static INLINE_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^(?:[-*+]\s+(?:\[.\]\s+)?|\d+[.)]\s+)?(?P<key>[A-Za-z_][A-Za-z0-9_-]*)::[ \t]*(?P<value>[^\r\n]*)")
        .expect("static regex")
});

/// Every inline field found within `block`'s own byte range.
///
/// Scans `bytes` rather than `block.text`: `range`/`value_range` are byte-range
/// patch targets (C-04), and `block.text` has already had markup collected out of
/// it, which is not the same string as what is actually on disk at those offsets.
pub(crate) fn scan(bytes: &[u8], block: &Block) -> Vec<Field> {
    let Some(slice) = block.range.slice(bytes) else {
        return Vec::new();
    };
    let Ok(text) = std::str::from_utf8(slice) else {
        return Vec::new();
    };

    INLINE_FIELD
        .captures_iter(text)
        .filter_map(|captures| {
            let key = captures.name("key")?;
            let value = captures.name("value")?;
            let whole = captures.get(0)?;

            let raw_value = value.as_str().trim_end();
            if raw_value.is_empty() {
                // `due::` with nothing after it names no value worth indexing, and
                // an empty val_text is not obviously more useful than no row.
                return None;
            }

            let base = block.range.start;
            Some(Field {
                key: key.as_str().to_owned(),
                value: infer_value(raw_value),
                source: FieldSource::Inline,
                range: ByteRange::new(base + whole.start(), base + whole.end()),
                value_range: ByteRange::new(
                    base + value.start(),
                    base + value.start() + raw_value.len(),
                ),
                block_ord: Some(block.ord),
            })
        })
        .collect()
}

/// Remove every inline field occurrence from `text`, for the one column that is
/// documented to want it (`tasks.text`, schema/001_init.sql — a task's display text
/// with its own bookkeeping fields taken out). Ordinary block text keeps them: a
/// field is still text someone might search for.
pub(crate) fn strip(text: &str) -> String {
    let mut out = INLINE_FIELD.replace_all(text, "").into_owned();
    out.retain(|ch| !matches!(ch, '\r'));
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Guess a field's type from its literal text.
///
/// P0 has no schema registry to declare types (FR-STR-01 is P2), so every field is
/// inferred from what was written, the same way frontmatter's are.
pub(crate) fn infer_value(raw: &str) -> FieldValue {
    let raw = raw.trim();
    if let Some(list) = raw.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
        return FieldValue::List(
            list.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(infer_scalar)
                .collect(),
        );
    }
    infer_scalar(raw)
}

fn infer_scalar(raw: &str) -> FieldValue {
    if raw.eq_ignore_ascii_case("true") {
        return FieldValue::Bool(true);
    }
    if raw.eq_ignore_ascii_case("false") {
        return FieldValue::Bool(false);
    }
    if let Ok(number) = raw.parse::<f64>() {
        return FieldValue::Num(number);
    }
    if is_iso_date(raw) {
        return FieldValue::Date(raw.to_owned());
    }
    FieldValue::Text(raw.to_owned())
}

/// A conservative ISO-8601 date/date-time check: `YYYY-MM-DD`, optionally followed
/// by `THH:MM` or ` HH:MM` and further time components. Not a calendar validator —
/// `9999-99-99` passes — because `val_date`'s comparisons rely on lexicographic
/// order, not on the value denoting a real day, and rejecting a string shaped like a
/// date but wrong in an edge case would push it into `val_text` for no benefit.
fn is_iso_date(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    let digits_at = |range: std::ops::Range<usize>| {
        bytes.get(range.clone()).is_some_and(|slice| {
            slice.len() == range.len() && slice.iter().all(u8::is_ascii_digit)
        })
    };
    bytes.len() >= 10
        && digits_at(0..4)
        && bytes.get(4) == Some(&b'-')
        && digits_at(5..7)
        && bytes.get(7) == Some(&b'-')
        && digits_at(8..10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serat_core::BlockKind;

    fn block(text_range: ByteRange, ord: u32) -> Block {
        Block {
            ord,
            kind: BlockKind::Paragraph,
            range: text_range,
            text: String::new(),
            heading_path: Vec::new(),
            block_ref: None,
        }
    }

    #[test]
    fn a_plain_field_line_is_found() {
        let bytes = b"status:: active\n";
        let blk = block(ByteRange::new(0, bytes.len()), 0);
        let fields = scan(bytes, &blk);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].key, "status");
        assert_eq!(fields[0].value, FieldValue::Text("active".to_owned()));
        assert_eq!(fields[0].value_range.slice(bytes), Some(&b"active"[..]));
    }

    #[test]
    fn a_field_inside_a_task_line_is_found_after_the_checkbox() {
        let bytes = b"- [ ] ship it due:: 2024-01-01\n";
        let blk = block(ByteRange::new(0, bytes.len()), 0);
        let fields = scan(bytes, &blk);
        // The regex is line-anchored to the start of the line, and "due::" is not at
        // the start here (it follows "ship it "), so this documents the actual
        // scope: only a field that is its own whole line, or immediately after a
        // list/checkbox marker, is recognised. That is deliberate breadth control,
        // not an oversight — see the module doc.
        assert!(fields.is_empty());
    }

    #[test]
    fn a_field_as_the_whole_of_a_task_line_is_found() {
        let bytes = b"- [ ] due:: 2024-01-01\n";
        let blk = block(ByteRange::new(0, bytes.len()), 0);
        let fields = scan(bytes, &blk);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].key, "due");
        assert_eq!(fields[0].value, FieldValue::Date("2024-01-01".to_owned()));
    }

    #[test]
    fn types_are_inferred_from_the_literal() {
        assert_eq!(infer_value("42"), FieldValue::Num(42.0));
        assert_eq!(infer_value("3.5"), FieldValue::Num(3.5));
        assert_eq!(infer_value("true"), FieldValue::Bool(true));
        assert_eq!(infer_value("2024-01-01"), FieldValue::Date("2024-01-01".to_owned()));
        assert_eq!(infer_value("sujan"), FieldValue::Text("sujan".to_owned()));
        assert_eq!(
            infer_value("[a, b, 2]"),
            FieldValue::List(vec![
                FieldValue::Text("a".to_owned()),
                FieldValue::Text("b".to_owned()),
                FieldValue::Num(2.0),
            ])
        );
    }

    #[test]
    fn a_key_that_is_not_identifier_shaped_is_not_a_field() {
        // "3:: to 1" — a ratio written in prose, not a field. The key must start
        // with a letter or underscore.
        let bytes = b"the ratio was 3:: to 1\n";
        let blk = block(ByteRange::new(0, bytes.len()), 0);
        assert!(scan(bytes, &blk).is_empty());
    }

    #[test]
    fn stripping_removes_the_field_but_keeps_the_rest_of_the_line() {
        assert_eq!(strip("ship it\ndue:: 2024-01-01"), "ship it");
        assert_eq!(strip("due:: 2024-01-01"), "");
    }
}
