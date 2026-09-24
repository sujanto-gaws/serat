//! Applying a patch set to a base document (ADR-0016 §2).
//!
//! An editor save is a list of `{from, to, insert}` edits against the *base* — the
//! text last loaded or saved, identified by its hash — not a whole document. This
//! module turns such a list into new bytes with one property enforced by
//! construction rather than by care: **every byte outside a patched range is copied
//! verbatim.** There is no code path that re-encodes a line ending, drops a BOM or
//! touches anything the caller did not name.
//!
//! [`crate::Vault::write_patch`] is the write half: hash-gated, atomic, re-hashed
//! immediately before the rename (ADR-0016 §2 steps 1–5).

use serat_core::VaultError;

use crate::position::{self, Position};

/// One replace-range edit against a base document, in `{line, col}` coordinates.
///
/// The one shape ADR-0016 §1 uses in both directions: a save sends a list of these
/// to be applied, and reconciliation's diff produces the same shape to describe an
/// external change (`ExternalChange.edits`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// Start of the replaced range, inclusive.
    pub from: Position,
    /// End of the replaced range, exclusive.
    pub to: Position,
    /// Text to put in its place. May be empty (a pure deletion) or non-empty with
    /// `from == to` (a pure insertion). `\n` inside it is converted to the base
    /// document's own line ending before it is spliced in (ADR-0016 §2 step 3).
    pub insert: String,
}

impl Edit {
    /// An edit replacing `from..to` with `insert`.
    #[must_use]
    pub const fn new(from: Position, to: Position, insert: String) -> Self {
        Self { from, to, insert }
    }
}

/// The line ending [`apply_edits`] uses for every `\n` inside an inserted string:
/// the base document's own first terminator, or `\n` when it has none.
///
/// ADR-0016 §2 step 3. CodeMirror always joins with `\n` internally, so an insert
/// arriving from the front end is `\n`-delimited regardless of what the file on
/// disk uses; converting it back to the file's own ending is what keeps a CRLF or
/// a mixed-ending note from acquiring a stray LF the one time a word on one of its
/// lines changes.
fn line_ending(text: &str) -> &'static str {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => return "\r\n",
            b'\r' => return "\r",
            b'\n' => return "\n",
            _ => i += 1,
        }
    }
    "\n"
}

/// Apply `edits` to `base`, returning the new document text.
///
/// # Errors
///
/// [`VaultError::MalformedPatch`] if any position does not map to a byte offset in
/// `base` ([`position::position_to_byte_offset`]), or if the edits are not given in
/// ascending, non-overlapping order — ADR-0016 §2 step 2's "the ranges are ordered
/// and disjoint". A client bug, not a race (the hash lock is what catches a race),
/// so it is reported rather than silently reordered or clamped.
///
/// # Guarantee
///
/// Every byte of `base` outside a patched range appears unchanged, at the same
/// relative position, in the output — by construction: the implementation below is
/// exactly "copy the gap before each edit, copy the converted insert, repeat, copy
/// the tail", with no step that touches a byte the caller did not name.
pub fn apply_edits(base: &str, edits: &[Edit], path: &str) -> Result<String, VaultError> {
    let malformed = |reason: String| VaultError::MalformedPatch {
        path: path.to_owned(),
        reason,
    };
    let ending = line_ending(base);

    let mut offsets = Vec::with_capacity(edits.len());
    for edit in edits {
        let from = position::position_to_byte_offset(base, edit.from).ok_or_else(|| {
            malformed(format!(
                "edit `from` position {:?} does not map to a byte offset in the base",
                edit.from
            ))
        })?;
        let to = position::position_to_byte_offset(base, edit.to).ok_or_else(|| {
            malformed(format!(
                "edit `to` position {:?} does not map to a byte offset in the base",
                edit.to
            ))
        })?;
        if to < from {
            return Err(malformed(format!(
                "edit range is inverted: `to` {:?} precedes `from` {:?}",
                edit.to, edit.from
            )));
        }
        offsets.push((from, to));
    }
    for pair in offsets.windows(2) {
        let (_, prev_to) = pair[0];
        let (next_from, _) = pair[1];
        if next_from < prev_to {
            return Err(malformed(
                "edits overlap, or are not given in ascending order".to_owned(),
            ));
        }
    }

    let mut out = String::with_capacity(base.len());
    let mut cursor = 0usize;
    for (edit, (from, to)) in edits.iter().zip(&offsets) {
        // The gap since the last edit, copied verbatim — this is the invariant:
        // nothing between patched ranges is ever re-derived.
        out.push_str(&base[cursor..*from]);
        for ch in edit.insert.chars() {
            if ch == '\n' {
                out.push_str(ending);
            } else {
                out.push(ch);
            }
        }
        cursor = *to;
    }
    out.push_str(&base[cursor..]);

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, col: u32) -> Position {
        Position::new(line, col)
    }

    #[test]
    fn a_single_word_edit_touches_nothing_else() {
        let base = "status:: draft\ntitle:: Serat\n";
        let edits = [Edit::new(
            pos(0, 9),
            pos(0, 14),
            "active".to_owned(),
        )];
        let out = apply_edits(base, &edits, "note.md").expect("apply");
        assert_eq!(out, "status:: active\ntitle:: Serat\n");
    }

    #[test]
    fn an_insert_and_a_deletion_compose_left_to_right() {
        let base = "one two three";
        let edits = [
            Edit::new(pos(0, 0), pos(0, 0), "ZERO ".to_owned()),
            Edit::new(pos(0, 4), pos(0, 8), String::new()),
        ];
        let out = apply_edits(base, &edits, "note.md").expect("apply");
        assert_eq!(out, "ZERO one three");
    }

    #[test]
    fn an_inserted_newline_becomes_the_bases_own_line_ending() {
        // ADR-0016 §2 step 3, CRLF.
        let base = "one\r\ntwo\r\n";
        let edits = [Edit::new(pos(0, 3), pos(0, 3), "\nINSERTED".to_owned())];
        let out = apply_edits(base, &edits, "note.md").expect("apply");
        assert_eq!(out, "one\r\nINSERTED\r\ntwo\r\n");
    }

    #[test]
    fn a_bom_and_everything_after_it_survives_an_unrelated_edit() {
        let base = "\u{FEFF}status:: draft\n";
        let edits = [Edit::new(pos(0, 10), pos(0, 15), "active".to_owned())];
        let out = apply_edits(base, &edits, "note.md").expect("apply");
        assert_eq!(out, "\u{FEFF}status:: active\n");
    }

    #[test]
    fn overlapping_edits_are_refused() {
        let base = "abcdef";
        let edits = [
            Edit::new(pos(0, 0), pos(0, 3), "X".to_owned()),
            Edit::new(pos(0, 2), pos(0, 4), "Y".to_owned()),
        ];
        assert!(matches!(
            apply_edits(base, &edits, "note.md"),
            Err(VaultError::MalformedPatch { .. })
        ));
    }

    #[test]
    fn out_of_order_edits_are_refused() {
        let base = "abcdef";
        let edits = [
            Edit::new(pos(0, 4), pos(0, 5), "X".to_owned()),
            Edit::new(pos(0, 0), pos(0, 1), "Y".to_owned()),
        ];
        assert!(matches!(
            apply_edits(base, &edits, "note.md"),
            Err(VaultError::MalformedPatch { .. })
        ));
    }

    #[test]
    fn a_position_past_the_end_of_a_line_is_refused() {
        let base = "ab\ncd";
        let edits = [Edit::new(pos(0, 9), pos(0, 9), "x".to_owned())];
        assert!(matches!(
            apply_edits(base, &edits, "note.md"),
            Err(VaultError::MalformedPatch { .. })
        ));
    }

    #[test]
    fn no_edits_returns_the_base_unchanged() {
        let base = "unchanged\r\ncontent\r\n";
        let out = apply_edits(base, &[], "note.md").expect("apply");
        assert_eq!(out, base);
    }

    #[test]
    fn a_mixed_ending_note_keeps_every_line_ending_it_did_not_touch() {
        // The case ADR-0016 exists for: only the first line's own edit changes,
        // and the CRLF and lone-CR endings on the lines around it are copied
        // verbatim rather than normalised to anything.
        let base = "first\r\nsecond\rthird\n";
        let edits = [Edit::new(pos(0, 0), pos(0, 5), "1st".to_owned())];
        let out = apply_edits(base, &edits, "note.md").expect("apply");
        assert_eq!(out, "1st\r\nsecond\rthird\n");
    }
}
