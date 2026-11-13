//! The write-back patcher against a corpus of real notes: nothing outside the patched
//! byte ranges changes (NFR-MNT-03, FR-VLT-05, FR-VLT-03, NFR-REL-01, NFR-CMP-02;
//! ADR-0008, ADR-0016 "Tested against" item 1; C-04).
//!
//! `patch_props.rs` proves the invariant over generated documents against an
//! independent model. This file proves it on real text: every Markdown file in the
//! repository's `docs/` tree (headings, tables, wikilink-like text, code fences, long
//! lines, non-ASCII punctuation such as the em dash and arrows), each in the three
//! shapes the benchmark vault carries -- as authored (LF), converted to CRLF, and
//! with a UTF-8 BOM in front of the CRLF form -- patched through the public
//! `Vault::write_patch` path into a real file and compared byte for byte.
//!
//! The oracle is deliberately not the patcher's: it works on raw bytes. Edits are
//! placed only inside the ASCII prefix of a line, where a UTF-16 column is a byte
//! offset, so the expected file is `base[..from] + insert + base[to..]` per edit with
//! nothing else computed. A multi-byte character elsewhere on the line, or on a later
//! line, must come through untouched.
//!
//! Deliberately not here, because they need the model in `patch_props.rs`: edit columns
//! past non-ASCII text (UTF-16 against bytes), deletions that span a CRLF, and the
//! conversion of an inserted newline to the note's own line ending.

use std::fs;
use std::path::{Path, PathBuf};

use serat_core::VaultPath;
use serat_vault::{Edit, Position, Vault, hash_bytes};

const BOM: &[u8] = b"\xEF\xBB\xBF";

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

fn corpus() -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .expect("read_dir")
            .map(|e| e.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "md") {
                out.push(path);
            }
        }
    }
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    let mut files = Vec::new();
    walk(&docs, &mut files);
    files
        .into_iter()
        .map(|p| {
            let name = p.file_name().expect("name").to_string_lossy().into_owned();
            (name, fs::read(&p).expect("read"))
        })
        .collect()
}

/// `(line start byte, length of the ASCII prefix of the line content)` per line, for a
/// file that uses `eol` as its only separator. `offset` is the BOM length.
fn lines(bytes: &[u8], offset: usize, eol: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = offset;
    while start <= bytes.len() {
        let rest = &bytes[start..];
        let end = rest
            .windows(eol.len())
            .position(|w| w == eol)
            .map_or(bytes.len(), |p| start + p);
        let ascii = bytes[start..end]
            .iter()
            .take_while(|b| b.is_ascii() && **b != b'\r' && **b != b'\n')
            .count();
        out.push((start, ascii));
        if end == bytes.len() {
            break;
        }
        start = end + eol.len();
    }
    out
}

fn shapes(original: &[u8]) -> Vec<(&'static str, Vec<u8>, usize, &'static [u8])> {
    let text = String::from_utf8(original.to_vec()).expect("docs are UTF-8");
    assert!(!text.contains('\r'), "the corpus is authored with LF");
    let crlf = text.replace('\n', "\r\n").into_bytes();
    let mut bom = BOM.to_vec();
    bom.extend_from_slice(&crlf);
    vec![
        ("lf", original.to_vec(), 0, b"\n"),
        ("crlf", crlf, 0, b"\r\n"),
        ("bom+crlf", bom, 3, b"\r\n"),
    ]
}

#[test]
fn nothing_outside_the_patched_ranges_changes_on_real_notes_in_every_shape() {
    let notes = corpus();
    assert!(notes.len() >= 20, "the corpus shrank to {}", notes.len());
    assert!(
        notes.iter().any(|(_, b)| !b.is_ascii()),
        "the corpus must contain multi-byte text"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(dir.path()).expect("open");
    let mut rng = Rng(0x5EED_0000_0000_0027);
    let mut patched = 0usize;

    for (name, original) in &notes {
        for (shape, base, bom_len, eol) in shapes(original) {
            let rel = format!("{shape}-{name}");
            let path = VaultPath::new(&rel).expect("path");
            fs::write(dir.path().join(&rel), &base).expect("seed");

            let table = lines(&base, bom_len, eol);
            // Up to three disjoint single-line edits, ascending by line.
            let mut picks: Vec<usize> = (0..3).map(|_| rng.below(table.len())).collect();
            picks.sort_unstable();
            picks.dedup();

            let mut edits = Vec::new();
            let mut ranges = Vec::new();
            for line in picks {
                let (start, ascii) = table[line];
                // ADR-0016 section 1: a BOM is one UTF-16 unit of line 0 (as in
                // CodeMirror), three bytes. Column 0 of line 0 is before it.
                let bom_here = bom_len > 0 && line == 0;
                let units = ascii + usize::from(bom_here);
                let a = rng.below(units + 1);
                let b = a + rng.below(units - a + 1);
                let byte = |col: usize| {
                    if bom_here {
                        start - bom_len + if col == 0 { 0 } else { bom_len + col - 1 }
                    } else {
                        start + col
                    }
                };
                edits.push(Edit::new(
                    Position::new(line as u32, a as u32),
                    Position::new(line as u32, b as u32),
                    "«Q»".to_owned(),
                ));
                ranges.push((byte(a), byte(b)));
            }

            let mut expected = Vec::new();
            let mut cursor = 0;
            for &(from, to) in &ranges {
                expected.extend_from_slice(&base[cursor..from]);
                expected.extend_from_slice("«Q»".as_bytes());
                cursor = to;
            }
            expected.extend_from_slice(&base[cursor..]);

            vault
                .write_patch(&path, hash_bytes(&base), &edits)
                .unwrap_or_else(|e| panic!("{rel}: {e:?}"));
            let got = fs::read(dir.path().join(&rel)).expect("read back");
            if got != expected {
                let first = got
                    .iter()
                    .zip(&expected)
                    .position(|(g, e)| g != e)
                    .unwrap_or(got.len().min(expected.len()));
                panic!(
                    "{rel}: bytes outside the patched ranges changed (first difference at byte {first}; \
                     ranges {ranges:?}; got {} bytes, expected {})",
                    got.len(),
                    expected.len()
                );
            }
            patched += 1;
        }
    }
    assert_eq!(patched, notes.len() * 3);
}
