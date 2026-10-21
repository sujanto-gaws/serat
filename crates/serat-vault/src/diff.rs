//! The edit script: the minimal edit that turns one text into another, in
//! `{line, UTF-16 col}` coordinates of the first (ADR-0016 §3, §4).
//!
//! Two callers, one function (so they cannot disagree):
//!
//! * `ExternalChange.edits` -- base text to the bytes now on disk;
//! * `rebase_buffer` (*Keep mine*) -- disk text to the editor's buffer.
//!
//! Both texts are compared **in CodeMirror's line model**: lines split on `\r\n`,
//! `\n` and a lone `\r`, terminators excluded. A difference that is only a line
//! ending produces no edit, because the editor cannot represent one; and every
//! `insert` is `\n`-joined, the shape [`crate::patch::apply_edits`] expects.
//!
//! Algorithm: Myers' O(ND) diff over lines, under a deadline; on timeout the
//! changed middle (after trimming common leading and trailing lines) becomes one
//! hunk. Each replacement hunk is then refined to the characters that differ by
//! trimming its common prefix and suffix. If the edits cover more than half the
//! base, one whole-document edit is returned instead and `whole` is set (§3.3:
//! position is meaningless at that point).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::patch::Edit;
use crate::position::{LineIndex, Position};

/// The deadline ADR-0016 §3 gives the line diff before it falls back.
pub const DIFF_DEADLINE: Duration = Duration::from_millis(50);

/// The result of [`edit_script`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditScript {
    /// Ordered, disjoint edits in the coordinates of the first text.
    pub edits: Vec<Edit>,
    /// True when `edits` is the single whole-document edit (the half-file fallback).
    pub whole: bool,
}

/// One changed run: base lines `a.0..a.1` become other lines `b.0..b.1`.
#[derive(Debug, Clone, Copy)]
struct Hunk {
    a: (usize, usize),
    b: (usize, usize),
}

fn content_lines<'t>(text: &'t str, index: &LineIndex) -> Vec<&'t str> {
    (0..index.line_count())
        .map(|i| {
            let (start, end) = index.content_range(i as u32).unwrap_or((0, 0));
            &text[start..end]
        })
        .collect()
}

fn utf16_len(s: &str) -> u32 {
    s.chars().map(|c| c.len_utf16() as u32).sum()
}

/// Myers' shortest edit script over interned line ids. `None` if the deadline
/// passes first. Hunks come back in ascending order.
fn myers(a: &[u32], b: &[u32], deadline: Instant) -> Option<Vec<Hunk>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = n + m;
    let offset = max;
    let width = (2 * max + 2) as usize;
    let mut v = vec![0isize; width];
    // trace[d] is the band [-d, d] of `v` as it stood before round d.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let band_lo = |d: isize| (offset - d).max(0);
    let mut last_d = None;
    'rounds: for d in 0..=max {
        if Instant::now() >= deadline {
            return None;
        }
        let lo = band_lo(d) as usize;
        let hi = ((offset + d + 1) as usize).min(width - 1);
        trace.push(v[lo..=hi].to_vec());
        let mut k = -d;
        while k <= d {
            let idx = (offset + k) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
                v[idx + 1]
            } else {
                v[idx - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x >= n && y >= m {
                last_d = Some(d);
                break 'rounds;
            }
            k += 2;
        }
    }
    let last_d = last_d?;

    // Backtrack, marking deleted base lines and inserted other lines.
    let mut deleted = vec![false; n as usize];
    let mut inserted = vec![false; m as usize];
    let (mut x, mut y) = (n, m);
    for d in (1..=last_d).rev() {
        let lo = band_lo(d);
        let get = |k: isize| trace[d as usize][(offset + k - lo) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && get(k - 1) < get(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = get(prev_k);
        let prev_y = prev_x - prev_k;
        if prev_k == k + 1 {
            inserted[prev_y as usize] = true;
        } else {
            deleted[prev_x as usize] = true;
        }
        x = prev_x;
        y = prev_y;
    }

    let mut hunks = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n as usize || j < m as usize {
        let is_del = i < n as usize && deleted[i];
        let is_ins = j < m as usize && inserted[j];
        if !is_del && !is_ins {
            i += 1;
            j += 1;
            continue;
        }
        let (a0, b0) = (i, j);
        while (i < n as usize && deleted[i]) || (j < m as usize && inserted[j]) {
            if i < n as usize && deleted[i] {
                i += 1;
            }
            if j < m as usize && inserted[j] {
                j += 1;
            }
        }
        hunks.push(Hunk {
            a: (a0, i),
            b: (b0, j),
        });
    }
    Some(hunks)
}

/// Position of byte offset `k` in `text`, where `text` begins at column 0 of
/// line `first_line`.
fn pos_in(text: &str, k: usize, first_line: usize) -> Position {
    let head = &text[..k];
    let newlines = head.matches('\n').count();
    let tail = head.rsplit('\n').next().unwrap_or("");
    Position::new((first_line + newlines) as u32, utf16_len(tail))
}

fn hunk_to_edit(la: &[&str], lb: &[&str], h: Hunk) -> Edit {
    let na = la.len();
    let a_text = la[h.a.0..h.a.1].join("\n");
    let b_text = lb[h.b.0..h.b.1].join("\n");
    let end_of = |line: usize| Position::new(line as u32, utf16_len(la[line]));
    if h.a.0 == h.a.1 {
        // Pure insertion of whole lines.
        return if h.a.0 < na {
            Edit::new(
                Position::new(h.a.0 as u32, 0),
                Position::new(h.a.0 as u32, 0),
                format!("{b_text}\n"),
            )
        } else {
            let at = end_of(na - 1);
            Edit::new(at, at, format!("\n{b_text}"))
        };
    }
    if h.b.0 == h.b.1 {
        // Pure deletion of whole lines, with one separator.
        return if h.a.1 < na {
            Edit::new(
                Position::new(h.a.0 as u32, 0),
                Position::new(h.a.1 as u32, 0),
                String::new(),
            )
        } else {
            // Through the end of the text: take the separator before the run.
            // (`a.0 > 0` here: the other text always has a line, so a hunk that
            // deletes every base line also inserts one.)
            Edit::new(end_of(h.a.0 - 1), end_of(na - 1), String::new())
        };
    }
    // Replacement: trim the common prefix and suffix down to the characters
    // that differ.
    let prefix: usize = a_text
        .chars()
        .zip(b_text.chars())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x.len_utf8())
        .sum();
    let (a_rest, b_rest) = (&a_text[prefix..], &b_text[prefix..]);
    let suffix: usize = a_rest
        .chars()
        .rev()
        .zip(b_rest.chars().rev())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x.len_utf8())
        .sum();
    let a_end = a_text.len() - suffix;
    let b_end = b_text.len() - suffix;
    Edit::new(
        pos_in(&a_text, prefix, h.a.0),
        pos_in(&a_text, a_end, h.a.0),
        b_text[prefix..b_end].to_owned(),
    )
}

/// The minimal edit script turning `base` into `other`, in `base`'s coordinates,
/// within [`DIFF_DEADLINE`].
#[must_use]
pub fn edit_script(base: &str, other: &str) -> EditScript {
    edit_script_within(base, other, DIFF_DEADLINE)
}

/// [`edit_script`] with an explicit deadline (tests use zero to force the
/// fallback).
#[must_use]
pub fn edit_script_within(base: &str, other: &str, deadline: Duration) -> EditScript {
    let deadline = Instant::now() + deadline;
    let ia = LineIndex::new(base);
    let ib = LineIndex::new(other);
    let la = content_lines(base, &ia);
    let lb = content_lines(other, &ib);
    if la == lb {
        return EditScript {
            edits: Vec::new(),
            whole: false,
        };
    }

    let mut table: HashMap<&str, u32> = HashMap::new();
    let mut id_of = |s| {
        let next = table.len() as u32;
        *table.entry(s).or_insert(next)
    };
    let ida: Vec<u32> = la.iter().map(|s| id_of(s)).collect();
    let idb: Vec<u32> = lb.iter().map(|s| id_of(s)).collect();

    let max_common = ida.len().min(idb.len());
    let mut p = 0;
    while p < max_common && ida[p] == idb[p] {
        p += 1;
    }
    let mut s = 0;
    while s < max_common - p && ida[ida.len() - 1 - s] == idb[idb.len() - 1 - s] {
        s += 1;
    }
    let (mid_a, mid_b) = (&ida[p..ida.len() - s], &idb[p..idb.len() - s]);
    let hunks: Vec<Hunk> = match myers(mid_a, mid_b, deadline) {
        Some(found) => found
            .into_iter()
            .map(|h| Hunk {
                a: (h.a.0 + p, h.a.1 + p),
                b: (h.b.0 + p, h.b.1 + p),
            })
            .collect(),
        None => vec![Hunk {
            a: (p, ida.len() - s),
            b: (p, idb.len() - s),
        }],
    };

    // "More than half the file": by the larger of the text removed and the text
    // put in, so a note rewritten from scratch is `whole` whichever side is short.
    let touched = |lines: &[&str], r: (usize, usize)| -> usize {
        lines[r.0..r.1].iter().map(|l| l.len() + 1).sum()
    };
    let removed: usize = hunks.iter().map(|h| touched(&la, h.a)).sum();
    let added: usize = hunks.iter().map(|h| touched(&lb, h.b)).sum();
    if removed.max(added) * 2 > base.len().max(other.len()) {
        let last = la.len() - 1;
        return EditScript {
            edits: vec![Edit::new(
                Position::new(0, 0),
                Position::new(last as u32, utf16_len(la[last])),
                lb.join("\n"),
            )],
            whole: true,
        };
    }

    EditScript {
        edits: hunks
            .into_iter()
            .map(|h| hunk_to_edit(&la, &lb, h))
            .collect(),
        whole: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::apply_edits;

    fn norm(s: &str) -> String {
        s.replace("\r\n", "\n").replace('\r', "\n")
    }

    /// Tiny deterministic generator; no dependency for a test helper.
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

    fn random_text(rng: &mut Rng, sep: &str) -> String {
        const WORDS: [&str; 7] = [
            "alpha",
            "beta",
            "",
            "\u{e9}t\u{e9}",
            "x\u{1F600}y",
            "# h",
            "- item",
        ];
        let lines = 1 + rng.below(12);
        (0..lines)
            .map(|_| WORDS[rng.below(WORDS.len())])
            .collect::<Vec<_>>()
            .join(sep)
    }

    fn mutate(rng: &mut Rng, text: &str, sep: &str) -> String {
        let mut lines: Vec<String> = norm(text).split('\n').map(str::to_owned).collect();
        for _ in 0..1 + rng.below(3) {
            let at = rng.below(lines.len());
            match rng.below(4) {
                0 => lines[at].push_str("\u{1F600}!"),
                1 if lines.len() > 1 => {
                    lines.remove(at);
                }
                2 => lines.insert(at, "new line".to_owned()),
                _ => lines[at] = format!("{}z", lines[at]),
            }
        }
        lines.join(sep)
    }

    #[test]
    fn applying_the_script_reproduces_the_other_text_for_random_pairs() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for round in 0..2000 {
            let sep = ["\n", "\r\n"][round % 2];
            let base = random_text(&mut rng, sep);
            let other = if round % 5 == 0 {
                random_text(&mut rng, "\n")
            } else {
                mutate(&mut rng, &base, "\n")
            };
            for deadline in [DIFF_DEADLINE, Duration::ZERO] {
                let script = edit_script_within(&base, &other, deadline);
                let got = apply_edits(&base, &script.edits, "t.md")
                    .unwrap_or_else(|e| panic!("{e}: {base:?} -> {other:?}: {:?}", script.edits));
                assert_eq!(norm(&got), norm(&other), "{base:?} -> {other:?}");
                // Endings of untouched lines survive: a CRLF base stays CRLF.
                if base.contains("\r\n") {
                    assert!(!got.replace("\r\n", "").contains('\n'), "{got:?}");
                }
            }
        }
    }

    #[test]
    fn a_one_word_change_is_one_small_edit() {
        let base = "line one\nline two\nline three\nline four\nline five\nline six\n";
        let other = base.replace("two", "2");
        let script = edit_script(base, &other);
        assert!(!script.whole);
        assert_eq!(
            script.edits,
            vec![Edit::new(
                Position::new(1, 5),
                Position::new(1, 8),
                "2".to_owned()
            )]
        );
    }

    #[test]
    fn identical_text_and_line_ending_only_changes_give_no_edits() {
        assert!(edit_script("a\nb", "a\nb").edits.is_empty());
        assert!(edit_script("a\r\nb\r\n", "a\nb\n").edits.is_empty());
    }

    #[test]
    fn separate_changes_stay_separate_edits() {
        let base = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let other = base
            .replace("line 2", "LINE 2")
            .replace("line 17", "LINE 17");
        let script = edit_script(&base, &other);
        assert_eq!(script.edits.len(), 2, "{:?}", script.edits);
        assert!(!script.whole);
    }

    #[test]
    fn past_the_deadline_the_changed_middle_is_one_refined_hunk() {
        // ADR-0016 §3: on timeout, trim common leading and trailing lines and send
        // what is between as one hunk (refined to the differing characters).
        let base = (0..200)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let other = base
            .replace("line 90\n", "LINE 90\n")
            .replace("line 110\n", "LINE 110\n");
        let fast = edit_script_within(&base, &other, DIFF_DEADLINE);
        assert_eq!(fast.edits.len(), 2);
        let slow = edit_script_within(&base, &other, Duration::ZERO);
        assert_eq!(slow.edits.len(), 1, "{:?}", slow.edits);
        assert!(!slow.whole);
        assert_eq!(slow.edits[0].from, Position::new(90, 0));
        assert_eq!(slow.edits[0].to.line, 110);
        assert_eq!(apply_edits(&base, &slow.edits, "n.md").unwrap(), other);
    }

    #[test]
    fn a_line_inserted_above_does_not_touch_positions_below() {
        let base = (0..30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let other = format!("{base}\nlast");
        let script = edit_script(&base, &other);
        assert_eq!(script.edits.len(), 1);
        assert_eq!(script.edits[0].from, Position::new(29, 7));
        assert_eq!(script.edits[0].insert, "\nlast");
    }

    #[test]
    fn rewriting_more_than_half_is_one_whole_edit() {
        let base = (0..10)
            .map(|i| format!("old {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let other = (0..10)
            .map(|i| format!("new {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let script = edit_script(&base, &other);
        assert!(script.whole);
        assert_eq!(script.edits.len(), 1);
        assert_eq!(script.edits[0].from, Position::new(0, 0));
        assert_eq!(script.edits[0].to, Position::new(9, 5));
        assert_eq!(apply_edits(&base, &script.edits, "t.md").unwrap(), other);
    }

    #[test]
    fn surrogate_pairs_are_never_split_by_the_refinement() {
        // Two different astral characters share a UTF-16 high surrogate in some
        // encodings; the trim works on chars, so the column never lands inside one.
        let base = "a\u{1F600}b\n0123456789\n0123456789\n0123456789";
        let other = "a\u{1F601}b\n0123456789\n0123456789\n0123456789";
        let script = edit_script(base, other);
        assert_eq!(
            script.edits,
            vec![Edit::new(
                Position::new(0, 1),
                Position::new(0, 3),
                "\u{1F601}".to_owned()
            )]
        );
    }
}
