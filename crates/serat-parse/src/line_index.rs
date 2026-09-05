//! Converting comrak's line/column source positions into byte offsets.
//!
//! comrak reports `Sourcepos` as 1-based `(line, column)` pairs, with `column`
//! measured in UTF-8 bytes by default (comrak's own doc on `LineColumn`) — which is
//! exactly what [`serat_core::ByteRange`] needs, once the line number is turned into
//! a byte offset. Building that table once per note and reusing it for every node is
//! cheaper than re-scanning from the start of the file for every sourcepos.

use serat_core::ByteRange;

use comrak::nodes::Sourcepos;

/// Byte offset of the start of each line, 0-indexed internally, built once per note.
pub(crate) struct LineIndex {
    /// `starts[i]` is the byte offset where line `i + 1` begins.
    starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    pub(crate) fn new(bytes: &[u8]) -> Self {
        let mut starts = vec![0];
        for (offset, &byte) in bytes.iter().enumerate() {
            if byte == b'\n' {
                starts.push(offset + 1);
            }
        }
        Self {
            starts,
            len: bytes.len(),
        }
    }

    /// The byte offset of `(line, column)`, both 1-based. Clamped to the end of the
    /// file rather than panicking: a line comrak reports past what we indexed would
    /// be a comrak/LineIndex disagreement, and a clamp turns that into a slightly
    /// wrong range instead of a crash on a note that would otherwise have indexed.
    fn offset(&self, line: usize, column: usize) -> usize {
        let line_start = self
            .starts
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or(self.len);
        (line_start + column.saturating_sub(1)).min(self.len)
    }

    /// The exclusive end offset one past the last byte of the UTF-8 character
    /// starting at `offset`. comrak's `end` position names the *last* character of a
    /// node (inclusive), so this is what turns that into the exclusive bound
    /// [`ByteRange`] uses.
    fn char_end(&self, bytes: &[u8], offset: usize) -> usize {
        let width = match bytes.get(offset) {
            Some(byte) if byte & 0b1000_0000 == 0 => 1,
            Some(byte) if byte & 0b1110_0000 == 0b1100_0000 => 2,
            Some(byte) if byte & 0b1111_0000 == 0b1110_0000 => 3,
            Some(byte) if byte & 0b1111_1000 == 0b1111_0000 => 4,
            // Not a UTF-8 lead byte where one was expected. Cannot happen against a
            // str's own byte indices, but a width of 1 is the safe fallback rather
            // than panicking on a range this is only ever used to widen slightly.
            _ => 1,
        };
        (offset + width).min(self.len)
    }

    /// Convert a comrak sourcepos into a [`ByteRange`], inclusive-end position and
    /// all, into an exclusive-end one.
    pub(crate) fn range(&self, bytes: &[u8], sourcepos: Sourcepos) -> ByteRange {
        let start = self.offset(sourcepos.start.line, sourcepos.start.column);
        let end_start = self.offset(sourcepos.end.line, sourcepos.end.column);
        let end = self.char_end(bytes, end_start);
        ByteRange::new(start, end.max(start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_line_document_resolves_to_itself() {
        let bytes = b"hello";
        let index = LineIndex::new(bytes);
        let range = index.range(bytes, Sourcepos::from((1, 1, 1, 5)));
        assert_eq!(range, ByteRange::new(0, 5));
    }

    #[test]
    fn the_second_line_starts_after_the_first_newline() {
        let bytes = b"first\nsecond\n";
        let index = LineIndex::new(bytes);
        // "second" is line 2, columns 1..6 (six bytes).
        let range = index.range(bytes, Sourcepos::from((2, 1, 2, 6)));
        assert_eq!(&bytes[range.start..range.end], b"second");
    }

    #[test]
    fn a_multibyte_character_widens_the_end_by_its_own_byte_length() {
        // "héllo": h(1) é(2 bytes) l l o, so byte offsets are h=0, é=1..3, l=3, l=4,
        // o=5. comrak's column for é is 2 (1-based, byte-counted), and the range
        // "1:1-1:6" (start col 1, end col 6, which is the byte-column of the final
        // "o") must include o's own single byte.
        let text = "héllo";
        let bytes = text.as_bytes();
        assert_eq!(bytes.len(), 6);
        let index = LineIndex::new(bytes);
        let range = index.range(bytes, Sourcepos::from((1, 1, 1, 6)));
        assert_eq!(range, ByteRange::new(0, 6));
        assert_eq!(&bytes[range.start..range.end], text.as_bytes());
    }
}
