//! Property and hostile tests for the editor save path (ADR-0016, tasks #33 and #55;
//! FR-VLT-05 byte-range patching and its hash guard, NFR-CMP-02 line-ending
//! preservation, US-07 editing a note without disturbing the rest of it).
//!
//! The reference model here is deliberately a different implementation from the
//! one under test: it splits lines by walking `char`s and counts columns by
//! building a UTF-16 vector, so an error in `LineIndex` is not repeated by the
//! oracle.

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};

use serat_core::VaultError;

use crate::ignore::TEMP_PREFIX;
use crate::patch::{Edit, apply_edits};
use crate::position::Position;
use crate::{Vault, VaultPath, hash_bytes};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Oracle: `(content_start_byte, content_end_byte, terminator_len)` per line.
fn model_lines(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        if c == '\r' {
            out.push((start, at));
            if chars.get(i + 1).map(|p| p.1) == Some('\n') {
                i += 1;
            }
            start = chars.get(i + 1).map_or(text.len(), |p| p.0);
        } else if c == '\n' {
            out.push((start, at));
            start = chars.get(i + 1).map_or(text.len(), |p| p.0);
        }
        i += 1;
    }
    out.push((start, text.len()));
    out
}

/// Oracle: byte offset of a `(line, char index within line)` choice, and the
/// UTF-16 column it corresponds to.
fn model_pos(
    text: &str,
    lines: &[(usize, usize)],
    line: usize,
    nth_char: usize,
) -> (Position, usize) {
    let (start, end) = lines[line];
    let content = &text[start..end];
    let mut byte = start;
    let mut units: Vec<u16> = Vec::new();
    for (n, c) in content.chars().enumerate() {
        if n == nth_char {
            break;
        }
        byte += c.len_utf8();
        let mut buf = [0u16; 2];
        units.extend_from_slice(c.encode_utf16(&mut buf));
    }
    (Position::new(line as u32, units.len() as u32), byte)
}

const WORDS: [&str; 8] = [
    "alpha",
    "",
    "\u{e9}t\u{e9}",
    "x\u{1F600}y",
    "# h",
    "- item",
    "key:: v",
    "\u{FEFF}",
];

fn random_doc(rng: &mut Rng) -> String {
    let sep = ["\n", "\r\n", "\r"][rng.below(3)];
    let mixed = rng.below(2) == 0;
    let lines = 1 + rng.below(10);
    let mut doc = if rng.below(4) == 0 {
        "\u{FEFF}".to_owned()
    } else {
        String::new()
    };
    for i in 0..lines {
        if i > 0 {
            doc.push_str(if mixed {
                ["\n", "\r\n", "\r"][rng.below(3)]
            } else {
                sep
            });
        }
        doc.push_str(WORDS[rng.below(WORDS.len())]);
        doc.push_str(WORDS[rng.below(WORDS.len())]);
    }
    doc
}

/// Random ascending, disjoint edits plus the oracle's byte ranges.
fn random_edits(rng: &mut Rng, text: &str) -> (Vec<Edit>, Vec<(usize, usize)>) {
    let lines = model_lines(text);
    let mut points: Vec<(Position, usize)> = (0..2 * (1 + rng.below(3)))
        .map(|_| {
            let line = rng.below(lines.len());
            let len = text[lines[line].0..lines[line].1].chars().count();
            model_pos(text, &lines, line, rng.below(len + 1))
        })
        .collect();
    points.sort_by_key(|p| p.1);
    let mut edits = Vec::new();
    let mut ranges = Vec::new();
    for pair in points.chunks(2) {
        // All three separators CodeMirror splits on: the front end sends `\n`, but the
        // patcher must treat `\r\n` and a lone `\r` in an insert as one separator each.
        let insert = [
            "",
            "NEW",
            "a\nb",
            "\n",
            "\u{1F600}",
            "a\r\nb",
            "a\rb",
            "\r",
            "\r\n",
            "x\r\n\ny\r",
        ][rng.below(10)];
        edits.push(Edit::new(pair[0].0, pair[1].0, insert.to_owned()));
        ranges.push((pair[0].1, pair[1].1));
    }
    (edits, ranges)
}

/// The oracle's reading of ADR-0016 §2: most common of `\r\n` and `\n`, ties to the
/// first seen, a lone `\r` only when it is the only kind. Written as a walk over
/// chars rather than by calling `line_ending`.
fn model_ending(text: &str) -> &'static str {
    let seps: Vec<&str> = {
        let mut v = Vec::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' if chars.peek() == Some(&'\n') => {
                    chars.next();
                    v.push("\r\n");
                }
                '\r' => v.push("\r"),
                '\n' => v.push("\n"),
                _ => {}
            }
        }
        v
    };
    let crlf = seps.iter().filter(|s| **s == "\r\n").count();
    let lf = seps.iter().filter(|s| **s == "\n").count();
    if crlf == 0 && lf == 0 {
        return if seps.is_empty() { "\n" } else { "\r" };
    }
    if crlf != lf {
        return if crlf > lf { "\r\n" } else { "\n" };
    }
    seps.iter().find(|s| **s != "\r").unwrap()
}

#[test]
fn nothing_outside_the_patched_ranges_changes() {
    let mut rng = Rng(0xDEAD_BEEF_1234_5678);
    let (mut applied, mut refused) = (0, 0);
    for _ in 0..4000 {
        let base = random_doc(&mut rng);
        let (edits, ranges) = random_edits(&mut rng, &base);
        let ending = model_ending(&base);
        // Rebuild from the oracle's byte ranges.
        let mut expect = String::new();
        let mut cursor = 0;
        let mut promised = model_lines(&base).len() as isize;
        for (edit, &(from, to)) in edits.iter().zip(&ranges) {
            expect.push_str(&base[cursor..from]);
            let mut chars = edit.insert.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\r' || c == '\n' {
                    if c == '\r' && chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    // The insert form of a fuse is written as CRLF.
                    expect.push_str(if ending == "\n" && expect.ends_with('\r') {
                        "\r\n"
                    } else {
                        ending
                    });
                } else {
                    expect.push(c);
                }
            }
            promised += model_lines(&edit.insert).len() as isize - 1;
            promised -= model_lines(&base[from..to]).len() as isize - 1;
            cursor = to;
        }
        expect.push_str(&base[cursor..]);
        let fused = model_lines(&expect).len() as isize != promised;
        match apply_edits(&base, &edits, "n.md") {
            Ok(out) => {
                applied += 1;
                assert!(!fused, "the oracle saw a fuse: {base:?} {edits:?}");
                assert_eq!(out, expect, "base {base:?} edits {edits:?}");
                // Independent restatement of the property: the bytes before the
                // first range and after the last are untouched.
                assert!(out.as_bytes().starts_with(&base.as_bytes()[..ranges[0].0]));
                assert!(
                    out.as_bytes()
                        .ends_with(&base.as_bytes()[ranges[ranges.len() - 1].1..])
                );
            }
            // Only a mixed-ending note can fuse endings; a uniform one never refuses.
            Err(VaultError::LineEndingsWouldFuse { line, .. }) => {
                refused += 1;
                assert!(fused, "refused without a fuse: {base:?} {edits:?}");
                assert!(base.contains('\r') && base.contains('\n'), "{base:?}");
                assert!((line as usize) < model_lines(&base).len());
            }
            Err(other) => panic!("{other}"),
        }
    }
    assert!(applied > 2500, "applied {applied}, refused {refused}");
    assert!(refused > 0, "the generator never produced a deletion fuse");
}

#[test]
fn a_noop_or_empty_patch_set_returns_the_base_byte_for_byte() {
    let mut rng = Rng(7);
    for _ in 0..200 {
        let base = random_doc(&mut rng);
        assert_eq!(apply_edits(&base, &[], "n.md").unwrap(), base);
    }
}

#[test]
fn a_lone_cr_is_one_separator_and_an_inserted_newline_becomes_cr() {
    let base = "a\rb\rc";
    // Line 1 is "b": replace it, and add a line after it.
    let edits = [Edit::new(
        Position::new(1, 0),
        Position::new(1, 1),
        "B\nB2".to_owned(),
    )];
    assert_eq!(apply_edits(base, &edits, "n.md").unwrap(), "a\rB\rB2\rc");
}

#[test]
fn columns_past_the_line_end_or_inside_a_surrogate_pair_are_rejected() {
    let base = "a\u{1F600}b\nsecond";
    for bad in [
        Position::new(0, 5), // past the end of line 0 (4 units)
        Position::new(0, 2), // between the two halves of the emoji
        Position::new(1, 7), // past "second"
        Position::new(2, 0), // no such line
    ] {
        let edits = [Edit::new(bad, bad, "x".to_owned())];
        assert!(
            matches!(
                apply_edits(base, &edits, "n.md"),
                Err(VaultError::MalformedPatch { .. })
            ),
            "{bad:?}"
        );
    }
    // The boundaries on either side of the pair are fine.
    for ok in [
        Position::new(0, 1),
        Position::new(0, 3),
        Position::new(0, 4),
    ] {
        let edits = [Edit::new(ok, ok, "x".to_owned())];
        assert!(apply_edits(base, &edits, "n.md").is_ok(), "{ok:?}");
    }
}

#[test]
fn an_inserted_separator_after_a_lone_cr_is_written_as_crlf_not_refused() {
    // ADR-0016 §2, the insert form. Majority ending is `\n`; Enter at the start of
    // `b`'s line sits straight after the lone `\r` and would fuse with it.
    let base = "a\rb\nc\n";
    let edits = [Edit::new(
        Position::new(1, 0),
        Position::new(1, 0),
        "\n".to_owned(),
    )];
    assert_eq!(apply_edits(base, &edits, "n.md").unwrap(), "a\r\r\nb\nc\n");
}

#[test]
fn deleting_the_text_between_a_lone_cr_and_an_lf_is_refused_with_the_line() {
    // The deletion form: `a\rx\nc` minus `x` would read `a\r\nc`.
    let base = "a\rx\nc";
    let edits = [Edit::new(
        Position::new(1, 0),
        Position::new(1, 1),
        String::new(),
    )];
    match apply_edits(base, &edits, "n.md") {
        Err(VaultError::LineEndingsWouldFuse { line, path }) => {
            assert_eq!((line, path.as_str()), (0, "n.md"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_refused_fuse_leaves_the_file_untouched_through_the_write_path() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    let path = VaultPath::new("note.md").unwrap();
    let hash = vault.write_atomic(&path, b"a\rx\nc").unwrap();
    let edits = [Edit::new(
        Position::new(1, 0),
        Position::new(1, 1),
        String::new(),
    )];
    let err = vault.write_patch(&path, hash, &edits).unwrap_err();
    assert!(matches!(
        err,
        VaultError::LineEndingsWouldFuse { line: 0, .. }
    ));
    assert_eq!(vault.read(&path).unwrap().bytes, b"a\rx\nc");
    assert!(stray_temps(dir.path()).is_empty());
}

#[test]
fn the_ending_is_the_majority_with_ties_to_the_first_and_a_lone_cr_only_alone() {
    use crate::patch::line_ending;
    assert_eq!(line_ending(""), "\n");
    assert_eq!(line_ending("no separator"), "\n");
    assert_eq!(line_ending("a\rb\rc"), "\r");
    // One stray CR in an LF note does not make new lines CR.
    assert_eq!(line_ending("\ra\nb\nc\n"), "\n");
    assert_eq!(line_ending("a\r\nb\r\nc\n"), "\r\n");
    assert_eq!(line_ending("a\nb\n\r\nc\r\n"), "\n");
    assert_eq!(line_ending("a\r\nb\nc"), "\r\n");
    assert_eq!(line_ending("a\nb\r\nc"), "\n");
    // Lone CRs never win against even a single LF.
    assert_eq!(line_ending("a\rb\rc\rd\n"), "\n");
}

#[test]
fn removing_the_last_separator_of_a_crlf_note_makes_later_inserts_lf() {
    let base = "a\r\nb";
    let cut = [Edit::new(
        Position::new(0, 1),
        Position::new(1, 0),
        String::new(),
    )];
    let once = apply_edits(base, &cut, "n.md").unwrap();
    assert_eq!(once, "ab");
    let add = [Edit::new(
        Position::new(0, 1),
        Position::new(0, 1),
        "\n".to_owned(),
    )];
    assert_eq!(apply_edits(&once, &add, "n.md").unwrap(), "a\nb");
}

#[test]
fn a_crlf_and_bom_note_survives_many_sequential_saves_exactly_like_the_model() {
    // Multi-transaction composition: each save is against the base the previous one
    // produced (the front end's rule), through the real write path.
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    let path = VaultPath::new("note.md").unwrap();
    let mut model = "\u{FEFF}first line\r\nsecond \u{1F600} line\r\nthird\r\n".to_owned();
    let mut hash = vault.write_atomic(&path, model.as_bytes()).unwrap();
    let mut rng = Rng(99);
    for _ in 0..200 {
        let (edits, _) = random_edits(&mut rng, &model);
        let expected = apply_edits(&model, &edits, "note.md");
        match vault.write_patch(&path, hash, &edits) {
            Ok(new_hash) => {
                let had_crlf = model.contains("\r\n");
                model = expected.expect("same patch applies the same way");
                let disk = fs::read(dir.path().join("note.md")).unwrap();
                assert_eq!(disk, model.as_bytes());
                assert_eq!(new_hash, hash_bytes(&disk));
                hash = new_hash;
                // A note with no terminators left takes `\n`; until then, no LF leaks.
                if had_crlf && model.contains("\r\n") {
                    assert!(
                        !model.replace("\r\n", "").contains('\n'),
                        "LF leaked: {model:?}"
                    );
                }
            }
            Err(VaultError::MalformedPatch { .. }) => assert!(expected.is_err()),
            Err(other) => panic!("{other}"),
        }
        if model.len() > 4000 {
            model.truncate(0);
            model.push_str("\u{FEFF}reset\r\nline\r\n");
            hash = vault.write_atomic(&path, model.as_bytes()).unwrap();
        }
    }
}

#[test]
fn a_stale_base_is_rejected_and_the_other_writers_bytes_survive() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    let path = VaultPath::new("note.md").unwrap();
    let base_hash = vault.write_atomic(&path, b"one\ntwo\n").unwrap();
    vault.write_atomic(&path, b"one\nTWO\n").unwrap();
    let edits = [Edit::new(
        Position::new(0, 0),
        Position::new(0, 3),
        "1".to_owned(),
    )];
    let found = match vault.write_patch(&path, base_hash, &edits) {
        Err(VaultError::StaleHash { found, .. }) => found,
        other => panic!("{other:?}"),
    };
    assert_eq!(fs::read(dir.path().join("note.md")).unwrap(), b"one\nTWO\n");

    // The refusal comes with the diff: base to disk, in base coordinates, and the
    // disk hash the refusal reported.
    let (disk_hash, script) = vault
        .diff_against_disk(&path, base_hash, "one\ntwo\n")
        .unwrap()
        .expect("disk differs from the base");
    assert_eq!(disk_hash.to_hex(), found);
    assert_eq!(
        script.edits,
        vec![Edit::new(
            Position::new(1, 0),
            Position::new(1, 3),
            "TWO".to_owned()
        )]
    );
    // Disk equal to the base reports nothing.
    assert!(
        vault
            .diff_against_disk(&path, disk_hash, "one\nTWO\n")
            .unwrap()
            .is_none()
    );
}

fn stray_temps(dir: &std::path::Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(TEMP_PREFIX))
        .collect()
}

#[test]
fn an_external_write_between_the_temp_file_and_the_rename_is_refused() {
    // ADR-0016 §2 step 4: the case #23's hostile test also drives.
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    let path = VaultPath::new("note.md").unwrap();
    let base_hash = vault.write_atomic(&path, b"one\ntwo\n").unwrap();
    let edits = [Edit::new(
        Position::new(0, 0),
        Position::new(0, 3),
        "1".to_owned(),
    )];
    let target = dir.path().join("note.md");
    let result = vault.write_patch_hooked(
        &path,
        base_hash,
        &edits,
        || {
            fs::write(&target, b"external edit\n").unwrap();
        },
        || {},
    );
    assert!(
        matches!(result, Err(VaultError::StaleHash { .. })),
        "{result:?}"
    );
    assert_eq!(fs::read(&target).unwrap(), b"external edit\n");
    assert!(stray_temps(dir.path()).is_empty());
}

#[test]
fn a_crash_after_the_temp_file_is_written_leaves_the_old_note_and_the_next_save_works() {
    // FR-VLT-03: old-or-new. A "crash" (panic) before the rename leaves the old
    // bytes in place; the stray temp is `.serat-tmp-` and ignored by the walk.
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    let path = VaultPath::new("note.md").unwrap();
    let old = b"one\r\ntwo\r\n";
    let base_hash = vault.write_atomic(&path, old).unwrap();
    let edits = [Edit::new(
        Position::new(0, 0),
        Position::new(0, 3),
        "1".to_owned(),
    )];
    let crashed = catch_unwind(AssertUnwindSafe(|| {
        let _ = vault.write_patch_hooked(&path, base_hash, &edits, || panic!("power cut"), || {});
    }));
    assert!(crashed.is_err());
    assert_eq!(fs::read(dir.path().join("note.md")).unwrap(), old);
    assert_eq!(stray_temps(dir.path()).len(), 1);
    let new_hash = vault.write_patch(&path, base_hash, &edits).unwrap();
    assert_eq!(
        fs::read(dir.path().join("note.md")).unwrap(),
        b"1\r\ntwo\r\n"
    );
    assert_eq!(new_hash, hash_bytes(b"1\r\ntwo\r\n"));
}

#[test]
fn a_patch_set_applied_in_one_save_equals_the_same_edits_in_two_saves() {
    // Multi-range composition: two disjoint ranges, one save versus two.
    let base = "alpha\nbeta\ngamma\ndelta\n";
    let first = Edit::new(Position::new(0, 0), Position::new(0, 5), "ALPHA".to_owned());
    let second = Edit::new(Position::new(3, 0), Position::new(3, 5), "DELTA".to_owned());
    let together = apply_edits(base, &[first.clone(), second.clone()], "n.md").unwrap();
    let step = apply_edits(base, &[first], "n.md").unwrap();
    // Same-length first edit, so the second keeps its coordinates.
    assert_eq!(apply_edits(&step, &[second], "n.md").unwrap(), together);
    assert_eq!(together, "ALPHA\nbeta\ngamma\nDELTA\n");
}

#[test]
fn an_inverted_range_is_refused() {
    let edits = [Edit::new(
        Position::new(0, 4),
        Position::new(0, 1),
        "x".to_owned(),
    )];
    assert!(matches!(
        apply_edits("abcdef", &edits, "n.md"),
        Err(VaultError::MalformedPatch { .. })
    ));
}
