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

use crate::position::{LineIndex, Position, byte_offset_to_position};

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

/// The note's line ending: the one [`apply_edits`] writes for every separator in an
/// insert (ADR-0016 §2 step 3).
///
/// The most common terminator in `text`, `\r\n` against `\n`, with a tie going to
/// whichever appears first. A lone `\r` is the ending only when it is the note's
/// only kind of terminator, so one stray `\r` in an LF note does not turn every new
/// line into a lone `\r`. A note with no terminator uses `\n`. Stateless: read from
/// the base at every save, remembered nowhere.
#[must_use]
pub fn line_ending(text: &str) -> &'static str {
    let bytes = text.as_bytes();
    let (mut crlf, mut lf, mut cr) = (0usize, 0usize, 0usize);
    let mut first_is_crlf = None;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                crlf += 1;
                first_is_crlf.get_or_insert(true);
                i += 2;
            }
            b'\r' => {
                cr += 1;
                i += 1;
            }
            b'\n' => {
                lf += 1;
                first_is_crlf.get_or_insert(false);
                i += 1;
            }
            _ => i += 1,
        }
    }
    if crlf == 0 && lf == 0 {
        return if cr > 0 { "\r" } else { "\n" };
    }
    match crlf.cmp(&lf) {
        std::cmp::Ordering::Greater => "\r\n",
        std::cmp::Ordering::Less => "\n",
        std::cmp::Ordering::Equal if first_is_crlf == Some(true) => "\r\n",
        std::cmp::Ordering::Equal => "\n",
    }
}

/// Apply `edits` to `base`, returning the new document text.
///
/// # Errors
///
/// [`VaultError::MalformedPatch`] if any position does not map to a byte offset in
/// `base` ([`LineIndex::offset`]), or if the edits are not given in
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
    let index = LineIndex::new(base);

    let mut offsets = Vec::with_capacity(edits.len());
    for edit in edits {
        let from = index.offset(base, edit.from).ok_or_else(|| {
            malformed(format!(
                "edit `from` position {:?} does not map to a byte offset in the base",
                edit.from
            ))
        })?;
        let to = index.offset(base, edit.to).ok_or_else(|| {
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
    // Base offset of the lone `\r` the output currently ends with, if it came from
    // a copied gap. A following `\n` would fuse with it (the deletion form).
    let mut lone_cr: Option<usize> = None;
    let fuse = |at: usize| VaultError::LineEndingsWouldFuse {
        path: path.to_owned(),
        line: byte_offset_to_position(base, at).line,
    };
    for (edit, (from, to)) in edits.iter().zip(&offsets) {
        // The gap since the last edit, copied verbatim -- this is the invariant:
        // nothing between patched ranges is ever re-derived.
        let gap = &base[cursor..*from];
        if let (Some(cr_at), true) = (lone_cr, gap.starts_with('\n')) {
            return Err(fuse(cr_at));
        }
        out.push_str(gap);
        if !gap.is_empty() {
            lone_cr = gap.ends_with('\r').then(|| *from - 1);
        }
        // Every separator in the insert (`\n`, `\r\n` or a lone `\r`, the three CM6
        // splits on) becomes the note's ending. The front end only ever sends
        // `\n`; accepting the others keeps a stray `\r` from landing in the file
        // as a separator the position model never counted.
        let mut chars = edit.insert.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\r' | '\n' => {
                    if ch == '\r' && chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    // The insert form of a fuse is avoided by choosing what Serat
                    // writes: `\n` straight after a lone `\r` would read as one
                    // `\r\n`, so write `\r\n`, which fuses with nothing.
                    if ending == "\n" && out.ends_with('\r') {
                        out.push_str("\r\n");
                    } else {
                        out.push_str(ending);
                    }
                }
                other => out.push(other),
            }
        }
        if !edit.insert.is_empty() {
            lone_cr = None;
        }
        cursor = *to;
    }
    let tail = &base[cursor..];
    if let (Some(cr_at), true) = (lone_cr, tail.starts_with('\n')) {
        return Err(fuse(cr_at));
    }
    out.push_str(tail);

    // Backstop: the patched text must have the lines the edits promise. No position
    // falls inside a `\r\n`, so the only way the count can differ is two separators
    // becoming one. Reachable only if the junction checks above missed a case.
    let mut expected = index.line_count() as isize;
    for (edit, &(from, to)) in edits.iter().zip(&offsets) {
        expected += LineIndex::new(&edit.insert).line_count() as isize - 1;
        expected -= LineIndex::new(&base[from..to]).line_count() as isize - 1;
    }
    if LineIndex::new(&out).line_count() as isize != expected {
        let line = edits.first().map_or(0, |e| e.from.line);
        return Err(VaultError::LineEndingsWouldFuse {
            path: path.to_owned(),
            line,
        });
    }

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
        let edits = [Edit::new(pos(0, 9), pos(0, 14), "active".to_owned())];
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
