//! `{line, col}` positions in a document, mapped to and from byte offsets.
//!
//! ADR-0016 §1: **one pure function**, used by both directions — the save path maps
//! a patch's positions to byte offsets to apply it, and the reconciliation diff maps
//! its byte offsets back to positions to describe it — so saves and reconciliation
//! cannot disagree about where a position is.
//!
//! Lines split on `\r\n`, `\n` and a lone `\r`, exactly as CodeMirror 6 splits them.
//! `col` counts **UTF-16 code units** within the line, CodeMirror's native unit. The
//! line terminator itself is never part of a line, so no position can fall inside a
//! `\r\n`, and a BOM is part of line 0's text, exactly as it is in CodeMirror's
//! document, provided the caller built that document from a `String` that kept it.

/// A position in a document: a 0-based line and a UTF-16 column within it.
///
/// Not serialisable on purpose: the wire type is `contracts::Pos` (ADR-0007), and a
/// second serde shape here would be a second contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
    /// 0-based line index.
    pub line: u32,
    /// UTF-16 code units into the line.
    pub col: u32,
}

impl Position {
    /// A position at `line`, `col`.
    #[must_use]
    pub const fn new(line: u32, col: u32) -> Self {
        Self { line, col }
    }
}

/// The byte offset at which each line of `text` begins, in CodeMirror's splitting
/// rule: `\r\n`, `\n` and a lone `\r` all terminate a line, and the terminator
/// belongs to neither the line before it nor the line after it.
///
/// Always has at least one entry (`0`, the start of line 0), even for an empty
/// document. Byte-indexed scanning is safe here because `\r` and `\n` are both
/// single-byte ASCII code points that can never appear as part of a multi-byte
/// UTF-8 sequence — neither as a continuation byte (`0x80..=0xBF`) nor as a leading
/// one (`0xC0..`) — so splitting on them never lands mid-character.
#[must_use]
pub fn line_starts(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut starts = vec![0usize];
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                i += if bytes.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                starts.push(i);
            }
            b'\n' => {
                i += 1;
                starts.push(i);
            }
            _ => i += 1,
        }
    }
    starts
}

/// Line boundaries of one text, computed once.
///
/// Mapping N positions with a per-call scan is O(N * size); a patch set of many
/// edits against a large note builds this once instead.
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// `(start, content_end)` byte offsets of each line; the terminator lies in
    /// `content_end..next start`.
    lines: Vec<(usize, usize)>,
}

impl LineIndex {
    /// Index `text`'s lines under CM6's splitting rule.
    #[must_use]
    pub fn new(text: &str) -> Self {
        let bytes = text.as_bytes();
        let mut lines = Vec::new();
        let (mut start, mut i) = (0usize, 0usize);
        while i < bytes.len() {
            match bytes[i] {
                b'\r' => {
                    lines.push((start, i));
                    i += if bytes.get(i + 1) == Some(&b'\n') {
                        2
                    } else {
                        1
                    };
                    start = i;
                }
                b'\n' => {
                    lines.push((start, i));
                    i += 1;
                    start = i;
                }
                _ => i += 1,
            }
        }
        lines.push((start, bytes.len()));
        Self { lines }
    }

    /// Number of lines (at least 1).
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The byte range of line `line`'s content, excluding its terminator.
    #[must_use]
    pub fn content_range(&self, line: u32) -> Option<(usize, usize)> {
        self.lines.get(line as usize).copied()
    }

    /// Map a position to a byte offset in `text` (which must be the text this
    /// index was built from). `None` if the line does not exist, the column is past
    /// the end of the line's content, or the column falls inside a surrogate pair.
    #[must_use]
    pub fn offset(&self, text: &str, pos: Position) -> Option<usize> {
        let (content_start, content_end) = self.content_range(pos.line)?;
        if pos.col == 0 {
            return Some(content_start);
        }
        let mut units = 0u32;
        let mut offset = content_start;
        for ch in text[content_start..content_end].chars() {
            units += ch.len_utf16() as u32;
            offset += ch.len_utf8();
            if units == pos.col {
                return Some(offset);
            }
            if units > pos.col {
                return None;
            }
        }
        None
    }
}

/// The byte range of line `line`'s content, excluding its terminator.
#[cfg(test)]
fn line_content_range(text: &str, line: u32) -> Option<(usize, usize)> {
    LineIndex::new(text).content_range(line)
}

/// Map a `{line, col}` position to a byte offset into `text`.
///
/// `None` if `line` does not exist, `col` lands past the end of that line's
/// content, or inside a surrogate pair -- all client bugs, per ADR-0016 §2
/// step 2, which the caller reports as [`serat_core::VaultError::MalformedPatch`]
/// rather than clamping silently. For many positions in one text use [`LineIndex`].
#[must_use]
pub fn position_to_byte_offset(text: &str, pos: Position) -> Option<usize> {
    LineIndex::new(text).offset(text, pos)
}

/// Map a byte offset in `text` back to a `{line, col}` position.
///
/// # Panics
///
/// Panics if `offset` is not on a UTF-8 character boundary of `text`, or exceeds
/// its length. Every caller in this crate derives `offset` from `text` itself (a
/// diff's own match boundaries), so an offset that violates this is our own bug,
/// not an input to validate.
#[must_use]
pub fn byte_offset_to_position(text: &str, offset: usize) -> Position {
    assert!(
        offset <= text.len() && text.is_char_boundary(offset),
        "byte_offset_to_position: {offset} is not a char boundary of a {}-byte string",
        text.len()
    );
    let starts = line_starts(text);
    // The line containing `offset` is the last line whose start is <= offset.
    let line = starts
        .partition_point(|&start| start <= offset)
        .saturating_sub(1);
    let content_start = starts[line];
    let units: u32 = text[content_start..offset]
        .chars()
        .map(|ch| ch.len_utf16() as u32)
        .sum();
    Position::new(line as u32, units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_starts_splits_on_all_three_cm6_terminators() {
        assert_eq!(line_starts("a\nb\r\nc\rd"), vec![0, 2, 5, 7]);
        assert_eq!(line_starts(""), vec![0]);
        assert_eq!(line_starts("no terminator"), vec![0]);
        assert_eq!(line_starts("trailing\n"), vec![0, 9]);
    }

    #[test]
    fn position_to_byte_offset_round_trips_through_ascii_lines() {
        let text = "first\nsecond\r\nthird\rfourth";
        for (pos, expected) in [
            (Position::new(0, 0), 0),
            (Position::new(0, 5), 5),  // end of "first", before the \n
            (Position::new(1, 0), 6),  // start of "second"
            (Position::new(1, 6), 12), // end of "second", before \r\n
            (Position::new(2, 0), 14), // start of "third"
            (Position::new(3, 0), 20), // start of "fourth"
        ] {
            assert_eq!(
                position_to_byte_offset(text, pos),
                Some(expected),
                "{pos:?}"
            );
        }
    }

    #[test]
    fn a_column_inside_or_past_a_terminator_is_none() {
        let text = "ab\ncd";
        // Line 0's content is "ab", 2 UTF-16 units long: col 3 has nowhere to land.
        assert_eq!(position_to_byte_offset(text, Position::new(0, 3)), None);
        // A line that does not exist.
        assert_eq!(position_to_byte_offset(text, Position::new(5, 0)), None);
    }

    #[test]
    fn col_counts_utf16_code_units_not_bytes_or_scalars() {
        // "é" is one UTF-16 unit but two UTF-8 bytes; the emoji is a UTF-16
        // surrogate pair (2 units) but one scalar value and four UTF-8 bytes.
        let text = "é\u{1F600}x";
        assert_eq!(position_to_byte_offset(text, Position::new(0, 0)), Some(0));
        assert_eq!(position_to_byte_offset(text, Position::new(0, 1)), Some(2));
        assert_eq!(position_to_byte_offset(text, Position::new(0, 3)), Some(6));
        assert_eq!(byte_offset_to_position(text, 0), Position::new(0, 0));
        assert_eq!(byte_offset_to_position(text, 2), Position::new(0, 1));
        assert_eq!(byte_offset_to_position(text, 6), Position::new(0, 3));
    }

    #[test]
    fn a_bom_counts_as_one_utf16_unit_of_line_zero() {
        // ADR-0016 §1: the BOM is part of line 0's text, as it is in CodeMirror's
        // document, and every column after it is offset by one unit accordingly.
        let text = "\u{FEFF}status:: active";
        assert_eq!(position_to_byte_offset(text, Position::new(0, 0)), Some(0));
        // One UTF-16 unit (3 UTF-8 bytes) past the BOM is the start of "status".
        assert_eq!(position_to_byte_offset(text, Position::new(0, 1)), Some(3));
    }

    #[test]
    fn byte_offset_to_position_is_the_inverse_of_position_to_byte_offset() {
        let text = "first\nsecond\r\nthird\rfourth line here";
        for line in 0..4u32 {
            let (start, end) = line_content_range(text, line).expect("line exists");
            let mut offset = start;
            while offset <= end {
                let pos = byte_offset_to_position(text, offset);
                assert_eq!(
                    position_to_byte_offset(text, pos),
                    Some(offset),
                    "line {line} offset {offset} round-trips through {pos:?}"
                );
                if offset == end {
                    break;
                }
                offset += text[offset..].chars().next().map_or(1, char::len_utf8);
            }
        }
    }
}

/// The shared table test ADR-0016 §"Risks" calls for: the same fixture strings and
/// expected `{line, col}` <-> byte-offset mappings as the front end's
/// `app/src/editor/patches.test.ts` (`textFromString` / `posAt` / `offsetAt`
/// describe block, task #21, `b87755d`), so a drift between CodeMirror's line
/// model and this module's is a failing test on both sides rather than a note
/// where positions silently disagree.
///
/// `byte_offset_to_position` here is `posAt` there; `position_to_byte_offset` is
/// `offsetAt`. The front end counts UTF-16 code units in JS string indices, which
/// are already UTF-16 code units; this module counts them explicitly over UTF-8
/// bytes, which is the entire reason the mapping is a function and not an
/// assumption.
#[cfg(test)]
mod shared_table_tests {
    use super::*;

    #[test]
    fn splits_on_lf() {
        let doc = "one\ntwo\nthree";
        assert_eq!(byte_offset_to_position(doc, 0), Position::new(0, 0));
        assert_eq!(byte_offset_to_position(doc, 4), Position::new(1, 0));
        assert_eq!(byte_offset_to_position(doc, 5), Position::new(1, 1));
    }

    #[test]
    fn splits_on_crlf_and_the_terminator_is_never_part_of_either_line() {
        let doc = "one\r\ntwo\r\nthree";
        // Offset 3 is the position right after "one", the end of line 0.
        assert_eq!(byte_offset_to_position(doc, 3), Position::new(0, 3));
        // Line 1 ("two") starts at byte offset 5, after the two-byte \r\n.
        let line_1_from = 5;
        assert_eq!(
            position_to_byte_offset(doc, Position::new(1, 0)),
            Some(line_1_from)
        );
        assert_eq!(
            byte_offset_to_position(doc, line_1_from),
            Position::new(1, 0)
        );
    }

    #[test]
    fn splits_on_a_lone_cr() {
        let doc = "one\rtwo\rthree";
        assert_eq!(byte_offset_to_position(doc, 4), Position::new(1, 0));
    }

    #[test]
    fn keeps_a_bom_as_the_first_character_of_line_zeros_text() {
        let doc = "\u{FEFF}title\nbody";
        assert_eq!(byte_offset_to_position(doc, 3), Position::new(0, 1));
        assert_eq!(position_to_byte_offset(doc, Position::new(0, 1)), Some(3));
    }

    #[test]
    fn counts_utf16_code_units_not_code_points_for_astral_characters() {
        // U+1F600 (an emoji) is a surrogate pair: two UTF-16 code units, and
        // four UTF-8 bytes.
        let doc = "a\u{1F600}b";
        assert_eq!(byte_offset_to_position(doc, 1), Position::new(0, 1)); // right after "a"
        assert_eq!(byte_offset_to_position(doc, 5), Position::new(0, 3)); // after the pair, before "b"
        assert_eq!(position_to_byte_offset(doc, Position::new(0, 3)), Some(5));
    }

    #[test]
    fn offset_and_position_are_exact_inverses_across_a_whole_document_mixed_terminators_included() {
        // The fixture the amendment specifically asked for: CRLF, a lone CR and
        // LF, all three, in one document — exactly the front end's
        // "offsetAt is the exact inverse of posAt" fixture.
        //
        // Only *addressable* byte offsets round-trip, and the walk below visits
        // exactly those: every offset inside a line's content, plus each line's
        // own start, but never the second byte of a `\r\n` — offset 6 in this
        // fixture, the `\n` half of "alpha\r\n"'s terminator. ADR-0016 §1 states
        // that byte as unaddressable ("no position can fall inside a `\r\n`"),
        // and CodeMirror's own line model agrees: `doc.line(2).from` is the
        // *only* position between line 0 and line 1, not two. Walking every raw
        // byte instead of every addressable one is precisely the bug this test
        // first caught: `byte_offset_to_position(doc, 6)` computed *some*
        // position, because it does not know it has landed inside a
        // terminator, and that position did not map back to 6 — proving the
        // fixture needs the same "walk content, skip terminators" shape the
        // narrower `first\nsecond\r\nthird\rfourth` test above already uses,
        // not a blind `for offset in 0..=len`.
        let doc = "alpha\r\nbeta\rgamma\ndelta";
        for line in 0..4u32 {
            let (start, end) = line_content_range(doc, line).expect("line exists");
            let mut offset = start;
            loop {
                let pos = byte_offset_to_position(doc, offset);
                assert_eq!(
                    position_to_byte_offset(doc, pos),
                    Some(offset),
                    "line {line} offset {offset} round-trips through {pos:?}"
                );
                if offset == end {
                    break;
                }
                offset += doc[offset..].chars().next().map_or(1, char::len_utf8);
            }
        }
    }
}

/// The CodeMirror offset model, restated: a document offset counts UTF-16 units of
/// line text plus **one per separator, whatever its width in the file**. The
/// fixtures are the ones in `app/src/editor/patches.test.ts`, with the `posAt`
/// results written out, so the two halves are checked against the same numbers.
#[cfg(test)]
mod cm_offset_table {
    use super::*;

    /// `(file text, CM doc offset, expected {line, col})` from patches.test.ts.
    const TABLE: &[(&str, usize, (u32, u32))] = &[
        ("one\ntwo\nthree", 0, (0, 0)),
        ("one\ntwo\nthree", 4, (1, 0)),
        ("one\ntwo\nthree", 5, (1, 1)),
        ("one\r\ntwo\r\nthree", 3, (0, 3)),
        ("one\r\ntwo\r\nthree", 4, (1, 0)), // line(2).from: ONE offset per CRLF
        ("one\r\ntwo\r\nthree", 8, (2, 0)),
        ("one\rtwo\rthree", 4, (1, 0)),
        ("\u{FEFF}title\nbody", 1, (0, 1)),
        ("a\u{1F600}b", 1, (0, 1)),
        ("a\u{1F600}b", 3, (0, 3)),
        ("alpha\r\nbeta\rgamma\ndelta", 6, (1, 0)),
        ("alpha\r\nbeta\rgamma\ndelta", 11, (2, 0)),
        ("alpha\r\nbeta\rgamma\ndelta", 17, (3, 0)),
    ];

    /// CM's `posAt`, from an independent line split.
    fn pos_at(text: &str, cm_offset: usize) -> Position {
        let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut remaining = cm_offset;
        for (line, content) in normalised.split('\n').enumerate() {
            let len = content.encode_utf16().count();
            if remaining <= len {
                return Position::new(line as u32, remaining as u32);
            }
            remaining -= len + 1;
        }
        panic!("offset {cm_offset} past the end of {text:?}");
    }

    #[test]
    fn the_front_end_fixtures_give_the_same_line_and_column() {
        for &(text, offset, (line, col)) in TABLE {
            assert_eq!(
                pos_at(text, offset),
                Position::new(line, col),
                "{text:?} @ {offset}"
            );
            // And the Rust mapping lands on the byte where that line/col is.
            let byte = position_to_byte_offset(text, Position::new(line, col))
                .unwrap_or_else(|| panic!("{text:?} {line}:{col}"));
            assert_eq!(
                byte_offset_to_position(text, byte),
                Position::new(line, col)
            );
        }
    }

    #[test]
    fn every_cm_offset_of_every_fixture_maps_to_a_byte_and_back() {
        for &(text, ..) in TABLE {
            let total = text
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .encode_utf16()
                .count();
            for offset in 0..=total {
                let pos = pos_at(text, offset);
                let Some(byte) = position_to_byte_offset(text, pos) else {
                    // The only offsets without a byte are inside a surrogate pair,
                    // which CM addresses and the patch protocol must reject.
                    assert!(text.contains('\u{1F600}'), "{text:?} @ {offset}");
                    continue;
                };
                assert_eq!(
                    byte_offset_to_position(text, byte),
                    pos,
                    "{text:?} @ {offset}"
                );
            }
        }
    }
}
