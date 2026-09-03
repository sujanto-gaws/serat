//! Byte ranges into a note's bytes.

use serde::{Deserialize, Serialize};

/// A half-open byte range `[start, end)` into the raw bytes of a file.
///
/// Byte offsets, not character offsets and not line/column: the write-back patcher
/// replaces exactly these bytes and touches nothing else (C-04, ADR-0008), and a
/// range expressed in anything but bytes would have to be re-derived — which means
/// re-parsing — before it could be used.
///
/// A range is only meaningful together with the [`ContentHash`](crate::ContentHash)
/// of the bytes it was computed from. That pairing is the anchor: the patcher
/// re-reads the file, checks the hash, and only then trusts the range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ByteRange {
    /// First byte of the range.
    pub start: usize,
    /// One past the last byte of the range.
    pub end: usize,
}

impl ByteRange {
    /// A range from `start` to `end`.
    ///
    /// # Panics
    ///
    /// Panics if `end < start`. An inverted range is a bug in a parser, not an
    /// input error, and every construction site here is our own code.
    #[must_use]
    pub const fn new(start: usize, end: usize) -> Self {
        assert!(end >= start, "byte range end precedes start");
        Self { start, end }
    }

    /// The empty range at `offset`, used for a field that does not exist yet.
    #[must_use]
    pub const fn empty_at(offset: usize) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }

    /// Number of bytes covered.
    #[must_use]
    pub const fn len(self) -> usize {
        self.end - self.start
    }

    /// Whether the range covers no bytes.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Whether `other` lies entirely within this range.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        other.start >= self.start && other.end <= self.end
    }

    /// Whether the two ranges share at least one byte.
    #[must_use]
    pub const fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }

    /// The range as a slice index.
    #[must_use]
    pub const fn as_range(self) -> std::ops::Range<usize> {
        self.start..self.end
    }

    /// The bytes this range covers, or `None` if it does not fit `bytes`.
    ///
    /// Returning `None` rather than panicking is deliberate: a range that no longer
    /// fits its file is the ordinary outcome of an external edit, which the write
    /// path answers with `Refused{RangeMoved}` (architecture §6.1 step 3).
    #[must_use]
    pub fn slice(self, bytes: &[u8]) -> Option<&[u8]> {
        bytes.get(self.as_range())
    }
}

impl From<ByteRange> for std::ops::Range<usize> {
    fn from(value: ByteRange) -> Self {
        value.as_range()
    }
}

impl From<std::ops::Range<usize>> for ByteRange {
    fn from(value: std::ops::Range<usize>) -> Self {
        Self::new(value.start, value.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slicing_out_of_bounds_is_none_not_a_panic() {
        let bytes = b"status:: active";
        assert_eq!(ByteRange::new(9, 15).slice(bytes), Some(&b"active"[..]));
        assert_eq!(ByteRange::new(9, 99).slice(bytes), None);
    }

    #[test]
    fn containment_and_overlap() {
        let outer = ByteRange::new(0, 10);
        assert!(outer.contains(ByteRange::new(2, 5)));
        assert!(!outer.contains(ByteRange::new(9, 11)));
        assert!(outer.overlaps(ByteRange::new(9, 11)));
        assert!(!outer.overlaps(ByteRange::new(10, 12)));
        assert!(ByteRange::empty_at(4).is_empty());
    }
}
