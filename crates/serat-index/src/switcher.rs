//! The quick switcher's corpus and matcher (architecture §9.2; FR-NAV-01, NFR-PRF-06,
//! US-04).
//!
//! A [`Corpus`] is an immutable snapshot: per note its title, its vault path and its
//! aliases, plus the headings of every note. It is built from the index by fixed-shape
//! reads on its own read-only connection ([`Corpus::from_index`], no schema change) or,
//! before the index is `Ready`, from the tree's paths alone ([`Corpus::from_paths`]).
//! The caller owns swapping snapshots by generation; a query never waits on a build.
//!
//! [`Corpus::query`] is a pure function of the snapshot, the query text, a
//! [`History`] and a clock, so every ordering rule below is a unit test and none depends
//! on a crate (goal 3).
//!
//! # Matching and ranking
//!
//! Subsequence match with smart case (a query with no capitals is case-insensitive) and
//! no diacritic folding. Each note appears once, under its best candidate. Results order
//! by, in turn:
//!
//! 1. match class: exact title, alias or file name, then prefix of one, then a
//!    contiguous substring or all characters at word starts, then scattered;
//! 2. frecency, descending;
//! 3. the scorer's score, descending (a two-state dynamic programme in the shape of
//!    fzf v2: match, word-boundary and consecutive bonuses, affine gap penalties);
//! 4. shorter candidate (in bytes), then path in byte order.
//!
//! A `#` switches to heading mode: the text before it filters notes exactly as above
//! (empty matches every note), the text after it matches those notes' headings. An
//! empty query returns the most frecent notes, then fills to 20 with the most recently
//! modified (`notes.mtime`, then path); a tree corpus has no mtime and is frecent-only.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serat_core::IndexError;

use crate::history::History;

/// The most results a query returns.
pub const MAX_RESULTS: usize = 50;
/// How many notes an empty query returns.
pub const EMPTY_QUERY_RESULTS: usize = 20;

/// Candidates longer than this many characters are matched on their first this many.
const MAX_CANDIDATE_CHARS: usize = 256;
/// Query characters beyond this are ignored.
const MAX_QUERY_CHARS: usize = 64;

const SCORE_MATCH: i32 = 16;
const BONUS_START: i32 = 12;
const BONUS_BOUNDARY: i32 = 8;
const BONUS_CAMEL: i32 = 7;
const BONUS_CONSECUTIVE: i32 = 5;
const GAP_OPEN: i32 = 3;
const GAP_EXTEND: i32 = 1;
/// Largest query-length by window-width the DP will fill; beyond it the leftmost
/// alignment is scored instead.
const MAX_DP_CELLS: usize = 2048;
/// DP cells one query may fill in total, consumed in candidate order (so results are
/// reproducible); once spent, every remaining candidate takes the leftmost alignment.
/// Ordinary queries use a fraction of it (about 13M at 50k notes for a 3-character
/// query); it exists to bound an adversarial corpus to tens of milliseconds.
const QUERY_CELL_BUDGET: usize = 20_000_000;
const NEG: i32 = i32::MIN / 2;

/// The corpus reads (§9.2): fixed shapes, no schema change. Notes in path order.
pub const SQL_NOTES: &str =
    "SELECT id, path, title, mtime FROM notes WHERE kind = 'note' ORDER BY path";
/// Frontmatter aliases, one row per list item as serat-parse emits them.
pub const SQL_ALIASES: &str = "SELECT note_id, val_text FROM fields \
     WHERE source = 'frontmatter' AND key IN ('aliases', 'alias') \
     AND val_text IS NOT NULL ORDER BY note_id, id";
/// Headings, in document order.
pub const SQL_HEADINGS: &str = "SELECT note_id, text, byte_start FROM blocks \
     WHERE kind = 'heading' AND text IS NOT NULL ORDER BY note_id, ord";

/// What a [`Corpus`] was built from (`SwitcherCorpus` in the IPC contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorpusSource {
    /// Titles, paths, aliases and headings from the `Ready` index.
    Index,
    /// Paths and file names only, from the tree.
    Tree,
}

/// Which candidate of a note matched (`SwitcherMatch` in the IPC contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matched {
    /// The title (frontmatter title, H1 or file name).
    Title,
    /// A frontmatter alias.
    Alias,
    /// The vault-relative path.
    Path,
    /// A heading.
    Heading,
}

/// One result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Vault-relative path of the note.
    pub path: String,
    /// The note's title.
    pub title: Option<String>,
    /// Which candidate matched.
    pub matched: Matched,
    /// The matched candidate's text.
    pub candidate: String,
    /// Matched runs in `candidate`, as half-open ranges of UTF-16 code units.
    pub highlight: Vec<(u32, u32)>,
    /// The heading's text and indexed byte offset, when `matched` is `Heading`.
    pub heading: Option<(String, u64)>,
    /// The shortest link text that resolves back to the note, without brackets: the file
    /// stem when it is unique among notes, otherwise the path without `.md`.
    pub link_text: String,
}

#[derive(Debug)]
struct NoteEntry {
    path: String,
    title: String,
    /// Byte range of the file stem in `path`.
    stem: (u32, u32),
    aliases: (u32, u32),
    headings: (u32, u32),
    stem_unique: bool,
    /// `notes.mtime`; 0 for a tree corpus, which has none.
    mtime: i64,
}

#[derive(Debug)]
struct Heading {
    text: String,
    byte_start: u64,
}

/// An immutable snapshot of what the switcher matches against.
#[derive(Debug)]
pub struct Corpus {
    source: CorpusSource,
    notes: Vec<NoteEntry>,
    aliases: Vec<String>,
    headings: Vec<Heading>,
    by_path: HashMap<String, u32>,
}

fn sqlite(e: &rusqlite::Error) -> IndexError {
    IndexError::Sqlite {
        message: e.to_string(),
    }
}

fn stem_range(path: &str) -> (u32, u32) {
    let name_start = path.rfind('/').map_or(0, |i| i + 1);
    let name = &path[name_start..];
    let end = match name.rfind('.') {
        Some(dot) if dot > 0 => name_start + dot,
        _ => path.len(),
    };
    (to_u32(name_start), to_u32(end))
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl Corpus {
    /// An empty snapshot.
    #[must_use]
    pub fn empty(source: CorpusSource) -> Self {
        Self {
            source,
            notes: Vec::new(),
            aliases: Vec::new(),
            headings: Vec::new(),
            by_path: HashMap::new(),
        }
    }

    /// What this snapshot was built from.
    #[must_use]
    pub fn source(&self) -> CorpusSource {
        self.source
    }

    /// Number of notes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.notes.len()
    }

    /// Whether there are no notes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// Number of alias candidates.
    #[must_use]
    pub fn alias_count(&self) -> usize {
        self.aliases.len()
    }

    /// Number of heading candidates.
    #[must_use]
    pub fn heading_count(&self) -> usize {
        self.headings.len()
    }

    /// A snapshot of paths and file names only, for use before the index is `Ready`.
    /// Paths that are not Markdown notes are skipped.
    pub fn from_paths<S: AsRef<str>>(paths: impl IntoIterator<Item = S>) -> Self {
        let mut sorted: Vec<String> = paths
            .into_iter()
            .map(|p| p.as_ref().to_owned())
            .filter(|p| p.len() > 3 && p[p.len() - 3..].eq_ignore_ascii_case(".md"))
            .collect();
        sorted.sort_unstable();
        sorted.dedup();
        let mut b = Builder::new(CorpusSource::Tree);
        for path in sorted {
            let stem = stem_range(&path);
            let title = path[stem.0 as usize..stem.1 as usize].to_owned();
            b.note(path, title, &[], &[], 0);
        }
        b.finish()
    }

    /// Build a snapshot from the index at `db_path` on a connection of its own. Three
    /// fixed-shape reads, none on a critical path; no schema change (§9.2).
    ///
    /// # Errors
    ///
    /// [`IndexError::Sqlite`] if the database cannot be opened or read.
    pub fn from_index(db_path: &Path) -> Result<Self, IndexError> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| sqlite(&e))?;
        conn.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 5000;")
            .map_err(|e| sqlite(&e))?;
        Self::from_connection(&conn)
    }

    /// As [`Corpus::from_index`], over an open connection.
    ///
    /// # Errors
    ///
    /// [`IndexError::Sqlite`] if a read fails.
    pub fn from_connection(conn: &Connection) -> Result<Self, IndexError> {
        let e = |e: rusqlite::Error| sqlite(&e);
        // Notes in path order, so the corpus is deterministic and ids can be binary-searched.
        let mut rows: Vec<(i64, String, Option<String>, i64)> = Vec::new();
        {
            let mut stmt = conn.prepare(SQL_NOTES).map_err(e)?;
            let mut q = stmt.query([]).map_err(e)?;
            while let Some(r) = q.next().map_err(e)? {
                rows.push((
                    r.get(0).map_err(e)?,
                    r.get(1).map_err(e)?,
                    r.get(2).map_err(e)?,
                    r.get(3).map_err(e)?,
                ));
            }
        }
        let id_to_idx: HashMap<i64, usize> = rows
            .iter()
            .enumerate()
            .map(|(i, (id, ..))| (*id, i))
            .collect();

        let mut aliases: Vec<Vec<String>> = vec![Vec::new(); rows.len()];
        {
            let mut stmt = conn.prepare(SQL_ALIASES).map_err(e)?;
            let mut q = stmt.query([]).map_err(e)?;
            while let Some(r) = q.next().map_err(e)? {
                let id: i64 = r.get(0).map_err(e)?;
                let text: String = r.get(1).map_err(e)?;
                if let (Some(&i), false) = (id_to_idx.get(&id), text.is_empty()) {
                    aliases[i].push(text);
                }
            }
        }

        let mut headings: Vec<Vec<(String, u64)>> = vec![Vec::new(); rows.len()];
        {
            let mut stmt = conn.prepare(SQL_HEADINGS).map_err(e)?;
            let mut q = stmt.query([]).map_err(e)?;
            while let Some(r) = q.next().map_err(e)? {
                let id: i64 = r.get(0).map_err(e)?;
                let text: String = r.get(1).map_err(e)?;
                let start: Option<i64> = r.get(2).map_err(e)?;
                if let Some(&i) = id_to_idx.get(&id) {
                    headings[i].push((text, u64::try_from(start.unwrap_or(0)).unwrap_or(0)));
                }
            }
        }

        let mut b = Builder::new(CorpusSource::Index);
        for (i, (_, path, title, mtime)) in rows.into_iter().enumerate() {
            let stem = stem_range(&path);
            let title = title
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| path[stem.0 as usize..stem.1 as usize].to_owned());
            b.note(path, title, &aliases[i], &headings[i], mtime);
        }
        Ok(b.finish())
    }
}

struct Builder {
    corpus: Corpus,
    stem_counts: HashMap<String, u32>,
}

impl Builder {
    fn new(source: CorpusSource) -> Self {
        Self {
            corpus: Corpus::empty(source),
            stem_counts: HashMap::new(),
        }
    }

    fn note(
        &mut self,
        path: String,
        title: String,
        aliases: &[String],
        headings: &[(String, u64)],
        mtime: i64,
    ) {
        let c = &mut self.corpus;
        let stem = stem_range(&path);
        *self
            .stem_counts
            .entry(serat_core::fold_case(
                &path[stem.0 as usize..stem.1 as usize],
            ))
            .or_insert(0) += 1;
        let a0 = to_u32(c.aliases.len());
        c.aliases.extend(aliases.iter().cloned());
        let h0 = to_u32(c.headings.len());
        c.headings.extend(headings.iter().map(|(t, b)| Heading {
            text: t.clone(),
            byte_start: *b,
        }));
        c.by_path.insert(path.clone(), to_u32(c.notes.len()));
        c.notes.push(NoteEntry {
            path,
            title,
            stem,
            aliases: (a0, to_u32(c.aliases.len())),
            headings: (h0, to_u32(c.headings.len())),
            stem_unique: false,
            mtime,
        });
    }

    fn finish(mut self) -> Corpus {
        for n in &mut self.corpus.notes {
            let key = serat_core::fold_case(&n.path[n.stem.0 as usize..n.stem.1 as usize]);
            n.stem_unique = self.stem_counts.get(&key).copied() == Some(1);
        }
        self.corpus
    }
}

// ---------------------------------------------------------------------------------------
// The scorer
// ---------------------------------------------------------------------------------------

struct Pattern {
    chars: Vec<char>,
    case_sensitive: bool,
    ascii: bool,
}

/// All query characters, in order, in an ASCII candidate: the cheap reject that most
/// candidates fail, before any buffer is filled.
fn ascii_subsequence(cand: &str, pat: &Pattern) -> bool {
    let mut want = pat.chars.iter().map(|&c| c as u8).peekable();
    for &b in cand.as_bytes() {
        let b = if pat.case_sensitive {
            b
        } else {
            b.to_ascii_lowercase()
        };
        if want.peek() == Some(&b) {
            want.next();
            if want.peek().is_none() {
                return true;
            }
        }
    }
    want.peek().is_none()
}

impl Pattern {
    fn new(text: &str) -> Self {
        let case_sensitive = text.chars().any(char::is_uppercase);
        let chars: Vec<char> = text
            .chars()
            .take(MAX_QUERY_CHARS)
            .map(|c| if case_sensitive { c } else { lower(c) })
            .collect();
        let ascii = chars.iter().all(char::is_ascii);
        Self {
            chars,
            case_sensitive,
            ascii,
        }
    }
}

fn lower(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

/// Match class; lower is better.
type Class = u8;
const EXACT: Class = 0;
const PREFIX: Class = 1;
const WORD_OR_SUBSTRING: Class = 2;
const SCATTERED: Class = 3;

#[derive(Debug, Clone, Copy)]
struct Scored {
    class: Class,
    score: i32,
}

/// Reusable buffers; one per query.
#[derive(Default)]
struct Matcher {
    orig: Vec<char>,
    cmp: Vec<char>,
    bonus: Vec<i32>,
    m: Vec<i32>,
    g: Vec<i32>,
    /// Matched character indexes of the last call that asked for them.
    positions: Vec<usize>,
    /// The leftmost alignment of the query, found by the early reject.
    greedy: Vec<usize>,
    /// DP cells filled so far in this query.
    spent: usize,
}

impl Matcher {
    fn load(&mut self, cand: &str, case_sensitive: bool) {
        self.orig.clear();
        self.orig.extend(cand.chars().take(MAX_CANDIDATE_CHARS));
        self.cmp.clear();
        if case_sensitive {
            self.cmp.extend_from_slice(&self.orig);
        } else {
            self.cmp.extend(self.orig.iter().map(|&c| lower(c)));
        }
    }

    /// Score `cand` against `pat`. With `want_positions`, `self.positions` holds the
    /// matched character indexes afterwards.
    fn score(&mut self, cand: &str, pat: &Pattern, want_positions: bool) -> Option<Scored> {
        if pat.ascii && cand.is_ascii() && !ascii_subsequence(cand, pat) {
            return None;
        }
        self.load(cand, pat.case_sensitive);
        self.score_loaded(pat, want_positions)
    }

    /// Only the two top classes: the candidate equals the query, or starts with it.
    fn edge(&mut self, cand: &str, pat: &Pattern, want_positions: bool) -> Option<Scored> {
        let m = pat.chars.len();
        if m == 0 || cand.len() < m {
            return None;
        }
        self.load(cand, pat.case_sensitive);
        if self.cmp.len() < m || self.cmp[..m] != pat.chars[..] {
            return None;
        }
        self.positions.clear();
        if want_positions {
            self.positions.extend(0..m);
        }
        Some(Scored {
            class: if self.cmp.len() == m { EXACT } else { PREFIX },
            score: 0,
        })
    }

    fn score_loaded(&mut self, pat: &Pattern, want_positions: bool) -> Option<Scored> {
        let (m, n) = (pat.chars.len(), self.cmp.len());
        if m == 0 || m > n {
            return None;
        }
        // Reject early: all query characters, in order. The leftmost alignment found
        // here is also the fallback when the DP would be too large.
        self.greedy.clear();
        let mut at = 0;
        for &q in &pat.chars {
            at += self.cmp[at..].iter().position(|&c| c == q)?;
            self.greedy.push(at);
            at += 1;
        }

        if want_positions {
            self.positions.clear();
        }
        if n == m && self.cmp == pat.chars {
            if want_positions {
                self.positions.extend(0..m);
            }
            return Some(Scored {
                class: EXACT,
                score: 0,
            });
        }
        if self.cmp[..m] == pat.chars[..] {
            if want_positions {
                self.positions.extend(0..m);
            }
            return Some(Scored {
                class: PREFIX,
                score: 0,
            });
        }

        // Bonuses by position.
        self.bonus.clear();
        for j in 0..n {
            let b = if j == 0 {
                BONUS_START
            } else {
                let (p, c) = (self.orig[j - 1], self.orig[j]);
                if !p.is_alphanumeric() {
                    BONUS_BOUNDARY
                } else if p.is_lowercase() && c.is_uppercase() {
                    BONUS_CAMEL
                } else {
                    0
                }
            };
            self.bonus.push(b);
        }

        // The DP runs only over the window from the first occurrence of the first query
        // character to the last occurrence of the last one (fzf v2), and only if that
        // window is small enough; otherwise the leftmost alignment stands in, so the
        // work per candidate is bounded whatever the text.
        let lo = self.greedy[0];
        let hi = self.cmp.iter().rposition(|&c| c == pat.chars[m - 1])?;
        let w = hi + 1 - lo;
        let mut positions = std::mem::take(&mut self.positions);
        positions.clear();
        let best;
        if m * w > MAX_DP_CELLS || self.spent + m * w > QUERY_CELL_BUDGET {
            positions.extend_from_slice(&self.greedy);
            let mut score = 0;
            for (i, &p) in positions.iter().enumerate() {
                score += SCORE_MATCH + self.bonus[p];
                if i > 0 && p != positions[i - 1] + 1 {
                    score -= GAP_OPEN;
                }
            }
            best = score;
        } else {
            self.spent += m * w;
            self.m.clear();
            self.m.resize(m * w, NEG);
            self.g.clear();
            self.g.resize(m * w, NEG);
            for i in 0..m {
                for j in (lo + i)..=hi {
                    let k = i * w + (j - lo);
                    if self.cmp[j] == pat.chars[i] {
                        self.m[k] = if i == 0 {
                            SCORE_MATCH + self.bonus[j]
                        } else if j == lo {
                            NEG
                        } else {
                            let consecutive = self.m[k - w - 1];
                            let gapped = self.g[k - w - 1];
                            let a = if consecutive > NEG {
                                consecutive + SCORE_MATCH + BONUS_CONSECUTIVE
                            } else {
                                NEG
                            };
                            let b = if gapped > NEG {
                                gapped + SCORE_MATCH + self.bonus[j]
                            } else {
                                NEG
                            };
                            a.max(b)
                        };
                    }
                    if j > lo {
                        let open = if self.m[k - 1] > NEG {
                            self.m[k - 1] - GAP_OPEN
                        } else {
                            NEG
                        };
                        let extend = if self.g[k - 1] > NEG {
                            self.g[k - 1] - GAP_EXTEND
                        } else {
                            NEG
                        };
                        self.g[k] = open.max(extend);
                    }
                }
            }
            let last = (m - 1) * w;
            let (mut best_j, mut top) = (usize::MAX, NEG);
            for j in 0..w {
                if self.m[last + j] > top {
                    top = self.m[last + j];
                    best_j = j;
                }
            }
            if best_j == usize::MAX {
                return None;
            }
            best = top;

            // Trace back; needed for the class as well as for highlighting.
            let (mut i, mut j) = (m - 1, best_j);
            loop {
                positions.push(lo + j);
                if i == 0 {
                    break;
                }
                let k = i * w + j;
                let here = self.m[k];
                let consecutive = self.m[k - w - 1];
                if consecutive > NEG && here == consecutive + SCORE_MATCH + BONUS_CONSECUTIVE {
                    i -= 1;
                    j -= 1;
                    continue;
                }
                // From a gap: walk G back to the M cell it started from.
                i -= 1;
                let mut gj = j - 1;
                loop {
                    let gk = i * w + gj;
                    let from_open = self.m[gk - 1];
                    if from_open > NEG && self.g[gk] == from_open - GAP_OPEN {
                        gj -= 1;
                        break;
                    }
                    gj -= 1;
                }
                j = gj;
            }
            positions.reverse();
        }

        let contiguous = positions.windows(2).all(|w| w[1] == w[0] + 1);
        let word_starts = positions.iter().all(|&p| self.bonus[p] >= BONUS_BOUNDARY);
        let mut class = if contiguous || word_starts {
            WORD_OR_SUBSTRING
        } else {
            SCATTERED
        };
        if class == SCATTERED
            && let Some(w) = self.cmp.windows(m).position(|w| w == &pat.chars[..])
        {
            positions.clear();
            positions.extend(w..w + m);
            class = WORD_OR_SUBSTRING;
        }
        if want_positions {
            self.positions = positions;
        } else {
            positions.clear();
            self.positions = positions;
        }
        Some(Scored { class, score: best })
    }
}

/// A candidate's identity within a note, for materialising the winner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Title,
    Alias(u32),
    Path,
    Heading(u32),
}

#[derive(Debug, Clone, Copy)]
struct Ranked {
    note: u32,
    which: Which,
    class: Class,
    frecency: f64,
    score: i32,
    len: u32,
}

impl Which {
    fn rank(self) -> u8 {
        match self {
            Which::Title | Which::Heading(_) => 0,
            Which::Alias(_) => 1,
            Which::Path => 2,
        }
    }
}

/// `a` is better than `b` among candidates of one note.
fn better_candidate(a: &Ranked, b: &Ranked) -> bool {
    (a.class, -a.score, a.len, a.which.rank()) < (b.class, -b.score, b.len, b.which.rank())
}

impl Corpus {
    fn candidate_text(&self, note: u32, which: Which) -> &str {
        let n = &self.notes[note as usize];
        match which {
            Which::Title => &n.title,
            Which::Alias(i) => &self.aliases[i as usize],
            Which::Path => &n.path,
            Which::Heading(i) => &self.headings[i as usize].text,
        }
    }

    /// Score the path candidate: the file stem first (exact and prefix of the file name
    /// are the top classes), then the whole path.
    fn score_path(
        &self,
        mt: &mut Matcher,
        n: &NoteEntry,
        pat: &Pattern,
        positions: bool,
    ) -> Option<Scored> {
        let stem = &n.path[n.stem.0 as usize..n.stem.1 as usize];
        if let Some(s) = mt.edge(stem, pat, positions) {
            if positions {
                let shift = n.path[..n.stem.0 as usize].chars().count();
                for p in &mut mt.positions {
                    *p += shift;
                }
            }
            return Some(s);
        }
        // A prefix of the whole path is a substring, not a file-name match: the top two
        // classes belong to titles, aliases and file names (§9.2).
        let mut s = mt.score(&n.path, pat, positions)?;
        s.class = s.class.max(WORD_OR_SUBSTRING);
        Some(s)
    }

    /// The best of a note's title, aliases and path against `pat`.
    fn best_for_note(&self, mt: &mut Matcher, note: u32, pat: &Pattern) -> Option<Ranked> {
        let n = &self.notes[note as usize];
        let mut best: Option<Ranked> = None;
        let mut offer = |which: Which, s: Option<Scored>, len: usize| {
            if let Some(s) = s {
                let r = Ranked {
                    note,
                    which,
                    class: s.class,
                    frecency: 0.0,
                    score: s.score,
                    len: to_u32(len),
                };
                if best.as_ref().is_none_or(|b| better_candidate(&r, b)) {
                    best = Some(r);
                }
            }
        };
        offer(Which::Title, mt.score(&n.title, pat, false), n.title.len());
        for i in n.aliases.0..n.aliases.1 {
            let a = &self.aliases[i as usize];
            offer(Which::Alias(i), mt.score(a, pat, false), a.len());
        }
        offer(
            Which::Path,
            self.score_path(mt, n, pat, false),
            n.path.len(),
        );
        best
    }

    /// The best heading of a note against `pat`; the first heading when `pat` is empty.
    fn best_heading(&self, mt: &mut Matcher, note: u32, pat: &Pattern) -> Option<Ranked> {
        let n = &self.notes[note as usize];
        let mut best: Option<Ranked> = None;
        for i in n.headings.0..n.headings.1 {
            let h = &self.headings[i as usize];
            let s = if pat.chars.is_empty() {
                Some(Scored {
                    class: SCATTERED,
                    score: 0,
                })
            } else {
                mt.score(&h.text, pat, false)
            };
            if let Some(s) = s {
                let r = Ranked {
                    note,
                    which: Which::Heading(i),
                    class: s.class,
                    frecency: 0.0,
                    len: if pat.chars.is_empty() {
                        0
                    } else {
                        to_u32(h.text.len())
                    },
                    score: s.score,
                };
                if best.as_ref().is_none_or(|b| better_candidate(&r, b)) {
                    best = Some(r);
                }
            }
        }
        best
    }

    fn order(&self, a: &Ranked, b: &Ranked) -> Ordering {
        a.class
            .cmp(&b.class)
            .then_with(|| b.frecency.total_cmp(&a.frecency))
            .then_with(|| b.score.cmp(&a.score))
            .then_with(|| a.len.cmp(&b.len))
            .then_with(|| {
                self.notes[a.note as usize]
                    .path
                    .as_bytes()
                    .cmp(self.notes[b.note as usize].path.as_bytes())
            })
    }

    /// Answer one switcher query.
    ///
    /// `limit` is clamped to [`MAX_RESULTS`]. `now_ms` (unix milliseconds) is the clock
    /// frecency is read against.
    #[must_use]
    pub fn query(&self, query: &str, limit: usize, history: &History, now_ms: i64) -> Vec<Hit> {
        let limit = limit.min(MAX_RESULTS);
        if limit == 0 || self.notes.is_empty() {
            return Vec::new();
        }
        let mut frecency = vec![0.0_f64; self.notes.len()];
        for (path, f) in history.scores(now_ms) {
            if let Some(&i) = self.by_path.get(path) {
                frecency[i as usize] = f;
            }
        }
        let mut mt = Matcher::default();
        let mut ranked: Vec<Ranked> = Vec::new();

        if query.is_empty() {
            for (i, &f) in frecency.iter().enumerate() {
                if f > 0.0 {
                    ranked.push(Ranked {
                        note: to_u32(i),
                        which: Which::Title,
                        class: EXACT,
                        frecency: f,
                        score: 0,
                        len: 0,
                    });
                }
            }
            let limit = limit.min(EMPTY_QUERY_RESULTS);
            let mut hits = self.finish(&mut mt, ranked, limit, None, None);
            // Fill with the most recently modified notes, once there is an index to say
            // which those are (§9.2); a tree corpus has no mtime and stays frecent-only.
            if self.source == CorpusSource::Index && hits.len() < limit {
                let mut rest: Vec<u32> = (0..to_u32(self.notes.len()))
                    .filter(|&i| frecency[i as usize] <= 0.0)
                    .collect();
                let by_mtime = |a: &u32, b: &u32| {
                    let (a, b) = (&self.notes[*a as usize], &self.notes[*b as usize]);
                    b.mtime
                        .cmp(&a.mtime)
                        .then_with(|| a.path.as_bytes().cmp(b.path.as_bytes()))
                };
                let want = limit - hits.len();
                if rest.len() > want {
                    rest.select_nth_unstable_by(want - 1, by_mtime);
                    rest.truncate(want);
                }
                rest.sort_by(by_mtime);
                for note in rest {
                    let r = Ranked {
                        note,
                        which: Which::Title,
                        class: SCATTERED,
                        frecency: 0.0,
                        score: 0,
                        len: 0,
                    };
                    hits.push(self.hit(&mut mt, &r, None));
                }
            }
            return hits;
        }

        if let Some((prefix, text)) = query.split_once('#') {
            let (prefix, text) = (Pattern::new(prefix), Pattern::new(text));
            for (i, (n, &f)) in self.notes.iter().zip(&frecency).enumerate() {
                let note = to_u32(i);
                if n.headings.0 == n.headings.1 {
                    continue;
                }
                if !prefix.chars.is_empty() && self.best_for_note(&mut mt, note, &prefix).is_none()
                {
                    continue;
                }
                if let Some(mut r) = self.best_heading(&mut mt, note, &text) {
                    r.frecency = f;
                    ranked.push(r);
                }
            }
            return self.finish(&mut mt, ranked, limit, None, Some(&text));
        }

        let pat = Pattern::new(query);
        for (i, &f) in frecency.iter().enumerate() {
            if let Some(mut r) = self.best_for_note(&mut mt, to_u32(i), &pat) {
                r.frecency = f;
                ranked.push(r);
            }
        }
        self.finish(&mut mt, ranked, limit, Some(&pat), None)
    }

    fn finish(
        &self,
        mt: &mut Matcher,
        mut ranked: Vec<Ranked>,
        limit: usize,
        pat: Option<&Pattern>,
        heading_pat: Option<&Pattern>,
    ) -> Vec<Hit> {
        let cmp = |a: &Ranked, b: &Ranked| self.order(a, b);
        if ranked.len() > limit {
            ranked.select_nth_unstable_by(limit - 1, cmp);
            ranked.truncate(limit);
        }
        ranked.sort_by(cmp);
        // The budget bounded the ranking pass; highlighting 50 hits needs none.
        mt.spent = 0;
        ranked
            .iter()
            .map(|r| self.hit(mt, r, pat.or(heading_pat)))
            .collect()
    }

    fn hit(&self, mt: &mut Matcher, r: &Ranked, pat: Option<&Pattern>) -> Hit {
        let n = &self.notes[r.note as usize];
        let candidate = self.candidate_text(r.note, r.which).to_owned();
        let mut highlight = Vec::new();
        if let Some(pat) = pat {
            let found = if r.which == Which::Path {
                self.score_path(mt, n, pat, true).is_some()
            } else {
                mt.score(&candidate, pat, true).is_some()
            };
            if found {
                highlight = utf16_runs(&candidate, &mt.positions);
            }
        }
        let (matched, heading) = match r.which {
            Which::Title => (Matched::Title, None),
            Which::Alias(_) => (Matched::Alias, None),
            Which::Path => (Matched::Path, None),
            Which::Heading(i) => {
                let h = &self.headings[i as usize];
                (Matched::Heading, Some((h.text.clone(), h.byte_start)))
            }
        };
        let link_text = if n.stem_unique {
            n.path[n.stem.0 as usize..n.stem.1 as usize].to_owned()
        } else {
            n.path[..n.stem.1 as usize].to_owned()
        };
        Hit {
            path: n.path.clone(),
            title: Some(n.title.clone()),
            matched,
            candidate,
            highlight,
            heading,
            link_text,
        }
    }
}

/// Matched character indexes to merged half-open runs of UTF-16 code units.
fn utf16_runs(text: &str, positions: &[usize]) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    let mut at = 0_u32;
    let mut next = positions.iter().copied().peekable();
    for (i, c) in text.chars().enumerate() {
        let width = to_u32(c.len_utf16());
        if next.peek() == Some(&i) {
            next.next();
            match runs.last_mut() {
                Some(last) if last.1 == at => last.1 = at + width,
                _ => runs.push((at, at + width)),
            }
        }
        at += width;
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn corpus(notes: &[(&str, &str, &[&str], &[&str])]) -> Corpus {
        let mut b = Builder::new(CorpusSource::Index);
        for (path, title, aliases, headings) in notes {
            let aliases: Vec<String> = aliases.iter().map(|s| (*s).to_owned()).collect();
            let headings: Vec<(String, u64)> = headings
                .iter()
                .enumerate()
                .map(|(i, h)| ((*h).to_owned(), i as u64 * 10))
                .collect();
            b.note(
                (*path).to_owned(),
                (*title).to_owned(),
                &aliases,
                &headings,
                0,
            );
        }
        b.finish()
    }

    fn paths(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(|h| h.path.as_str()).collect()
    }

    fn q(c: &Corpus, query: &str) -> Vec<Hit> {
        c.query(query, 50, &History::default(), NOW)
    }

    #[test]
    fn classes_order_exact_prefix_substring_scattered() {
        let c = corpus(&[
            ("a/scattered.md", "s-c-a-t-t-e-r-e-d plan", &[], &[]),
            ("b/sub.md", "the plan b", &[], &[]),
            ("c/prefix.md", "plan for tomorrow", &[], &[]),
            ("d/exact.md", "plan", &[], &[]),
            ("e/pxlxaxn.md", "p x l x a x n", &[], &[]),
        ]);
        let hits = q(&c, "plan");
        let order = paths(&hits);
        assert_eq!(order[0], "d/exact.md");
        assert_eq!(order[1], "c/prefix.md");
        assert_eq!(order[2], "b/sub.md");
        assert!(order.contains(&"e/pxlxaxn.md"));
        assert!(order.contains(&"a/scattered.md"));
    }

    #[test]
    fn a_file_name_counts_as_a_title_for_the_top_classes() {
        let c = corpus(&[
            ("x/Inbox.md", "Something else entirely", &[], &[]),
            ("y/other-inbox-thing.md", "Other inbox thing", &[], &[]),
        ]);
        let hits = q(&c, "inbox");
        assert_eq!(hits[0].path, "x/Inbox.md");
        assert_eq!(hits[0].matched, Matched::Path);
        assert_eq!(hits[0].highlight, vec![(2, 7)], "inside the path, after x/");
    }

    #[test]
    fn aliases_match_and_each_note_appears_once_under_its_best_candidate() {
        let c = corpus(&[(
            "n/long-note-name.md",
            "Long note name",
            &["lnn", "Short"],
            &[],
        )]);
        let hits = q(&c, "short");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matched, Matched::Alias);
        assert_eq!(hits[0].candidate, "Short");
        let hits = q(&c, "lnn");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].candidate, "lnn");
    }

    #[test]
    fn smart_case_a_capital_makes_the_query_case_sensitive() {
        let c = corpus(&[("a.md", "alpha", &[], &[]), ("b.md", "Alpha", &[], &[])]);
        assert_eq!(q(&c, "alpha").len(), 2);
        assert_eq!(paths(&q(&c, "Alpha")), vec!["b.md"]);
    }

    #[test]
    fn frecency_breaks_ties_within_a_class_but_never_across_one() {
        let c = corpus(&[
            ("a.md", "plan one", &[], &[]),
            ("b.md", "plan two", &[], &[]),
            ("c.md", "plan", &[], &[]),
        ]);
        let mut h = History::default();
        h.record_open("b.md", NOW);
        let hits = c.query("plan", 50, &h, NOW);
        assert_eq!(
            paths(&hits),
            vec!["c.md", "b.md", "a.md"],
            "exact first, then frecent"
        );
        h.record_open("a.md", NOW - 1000);
        h.record_open("a.md", NOW - 2000);
        let hits = c.query("plan", 50, &h, NOW);
        assert_eq!(paths(&hits), vec!["c.md", "a.md", "b.md"], "frequency adds");
    }

    #[test]
    fn then_the_score_then_shorter_then_path() {
        let c = corpus(&[
            ("z.md", "xx plan", &[], &[]),
            ("y.md", "xx plan", &[], &[]),
            ("w.md", "xxx plan", &[], &[]),
        ]);
        assert_eq!(paths(&q(&c, "plan")), vec!["y.md", "z.md", "w.md"]);
    }

    #[test]
    fn word_starts_are_the_substring_class() {
        let c = corpus(&[
            ("a.md", "aqbpcx", &[], &[]),
            ("b.md", "quarterly project xray", &[], &[]),
        ]);
        let hits = q(&c, "qpx");
        assert_eq!(paths(&hits), vec!["b.md", "a.md"]);
        assert_eq!(hits[0].highlight, vec![(0, 1), (10, 11), (18, 19)]);
    }

    #[test]
    fn no_match_returns_nothing_and_so_does_a_longer_query() {
        let c = corpus(&[("a.md", "abc", &[], &[])]);
        assert!(q(&c, "zzz").is_empty());
        assert!(q(&c, "abcd").is_empty());
    }

    fn mtime_corpus(n: usize) -> Corpus {
        let mut b = Builder::new(CorpusSource::Index);
        for i in 0..n {
            // Newest is the highest index.
            b.note(
                format!("n{i:02}.md"),
                format!("note {i}"),
                &[],
                &[],
                i as i64,
            );
        }
        b.finish()
    }

    #[test]
    fn empty_query_with_no_history_is_the_twenty_most_recently_modified() {
        let c = mtime_corpus(30);
        let hits = q(&c, "");
        assert_eq!(hits.len(), 20);
        assert_eq!(hits[0].path, "n29.md");
        assert_eq!(hits[19].path, "n10.md");
    }

    #[test]
    fn empty_query_puts_frecent_notes_first_then_fills_by_mtime_without_duplicates() {
        let c = mtime_corpus(30);
        let mut h = History::default();
        h.record_open("n00.md", NOW - 3000);
        h.record_open("n05.md", NOW - 2000);
        h.record_open("n29.md", NOW - 1000);
        h.record_open("gone.md", NOW);
        let hits = c.query("", 50, &h, NOW);
        let order = paths(&hits);
        assert_eq!(order.len(), 20);
        assert_eq!(&order[..3], ["n29.md", "n05.md", "n00.md"]);
        assert_eq!(
            order[3], "n28.md",
            "then by mtime, the frecent ones not repeated"
        );
        assert_eq!(order[19], "n12.md");
        let mut unique = order.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 20);
    }

    #[test]
    fn a_tree_corpus_has_no_mtime_so_an_empty_query_is_frecent_only() {
        let c = Corpus::from_paths(["a.md", "b.md"]);
        assert!(q(&c, "").is_empty());
        let mut h = History::default();
        h.record_open("b.md", NOW);
        assert_eq!(paths(&c.query("", 20, &h, NOW)), vec!["b.md"]);
    }

    #[test]
    fn a_prefix_of_the_whole_path_is_a_substring_not_a_file_name_match() {
        // "pro" is a prefix of the first note's path and a word start of the second's
        // title; both are class 2, so the better score decides, never class 1 for a path.
        let c = corpus(&[
            ("projects/zeta.md", "Zeta", &[], &[]),
            ("misc/my pro tips.md", "my pro tips", &[], &[]),
        ]);
        let hits = q(&c, "pro");
        assert_eq!(hits.len(), 2);
        let mut mt = Matcher::default();
        let pat = Pattern::new("pro");
        let n = &c.notes[c.by_path["projects/zeta.md"] as usize];
        assert_eq!(
            c.score_path(&mut mt, n, &pat, false).unwrap().class,
            WORD_OR_SUBSTRING
        );
        // A file-name prefix is still class 1, and outranks both.
        let c = corpus(&[
            ("projects/zeta.md", "Zeta", &[], &[]),
            ("misc/my pro tips.md", "my pro tips", &[], &[]),
            ("x/prolog.md", "Prolog notes", &[], &[]),
        ]);
        assert_eq!(q(&c, "pro")[0].path, "x/prolog.md");
        // And the exact full-path query is capped too.
        let n = &c.notes[c.by_path["projects/zeta.md"] as usize];
        let pat = Pattern::new("projects/zeta.md");
        assert_eq!(
            c.score_path(&mut mt, n, &pat, false).unwrap().class,
            WORD_OR_SUBSTRING
        );
    }

    #[test]
    fn long_dense_candidates_and_queries_stay_bounded_and_correct() {
        let long = "e".repeat(200);
        let mut b = Builder::new(CorpusSource::Index);
        b.note("a.md".into(), long.clone(), &[], &[], 0);
        b.note("b.md".into(), format!("{long} tail"), &[], &[], 0);
        let c = b.finish();
        let started = std::time::Instant::now();
        for query in ["mee", &"e".repeat(16), &"e e ".repeat(6)] {
            let _ = q(&c, query);
        }
        assert_eq!(q(&c, &"e".repeat(16)).len(), 2);
        assert!(q(&c, "mee").is_empty());
        assert!(started.elapsed().as_millis() < 500);
    }

    #[test]
    fn heading_mode_matches_headings_of_the_notes_the_prefix_selects() {
        let c = corpus(&[
            ("a/alpha.md", "Alpha", &[], &["Intro", "Budget"]),
            ("b/beta.md", "Beta", &[], &["Budget", "Risks"]),
            ("c/none.md", "None", &[], &[]),
        ]);
        let hits = q(&c, "#bud");
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.matched == Matched::Heading));
        assert_eq!(hits[0].heading, Some(("Budget".to_owned(), 10)));
        assert_eq!(hits[0].highlight, vec![(0, 3)]);

        let hits = q(&c, "alp#bud");
        assert_eq!(paths(&hits), vec!["a/alpha.md"]);

        // Empty heading text matches every heading of the selected notes, one per note.
        let hits = q(&c, "be#");
        assert_eq!(paths(&hits), vec!["b/beta.md"]);
        assert_eq!(hits[0].heading, Some(("Budget".to_owned(), 0)));
        assert_eq!(q(&c, "#").len(), 2, "notes without headings are not listed");
        assert!(q(&c, "#zzz").is_empty());
    }

    #[test]
    fn link_text_is_the_stem_when_unique_else_the_path_without_extension() {
        let c = corpus(&[
            ("a/Dup.md", "Dup", &[], &[]),
            ("b/dup.md", "dup", &[], &[]),
            ("c/Solo.md", "Solo", &[], &[]),
        ]);
        let hits = q(&c, "d");
        let link = |p: &str| hits.iter().find(|h| h.path == p).unwrap().link_text.clone();
        assert_eq!(link("a/Dup.md"), "a/Dup");
        assert_eq!(link("b/dup.md"), "b/dup");
        assert_eq!(q(&c, "solo")[0].link_text, "Solo");
    }

    #[test]
    fn highlights_are_utf16_and_merge_adjacent_matches() {
        let c = corpus(&[("a.md", "𝔘nicode héllo", &[], &[])]);
        let hits = q(&c, "hé");
        assert_eq!(hits[0].highlight, vec![(9, 11)]);
        let c = corpus(&[("a.md", "abcdef", &[], &[])]);
        assert_eq!(q(&c, "bcd")[0].highlight, vec![(1, 4)]);
    }

    #[test]
    fn the_result_count_is_capped_and_partial_selection_agrees_with_a_full_sort() {
        let notes: Vec<(String, String)> = (0..300)
            .map(|i| (format!("d/n{i:03}.md"), format!("note {i:03} x")))
            .collect();
        let mut b = Builder::new(CorpusSource::Index);
        for (p, t) in &notes {
            b.note(p.clone(), t.clone(), &[], &[], 0);
        }
        let c = b.finish();
        let all = c.query("note", 50, &History::default(), NOW);
        assert_eq!(all.len(), 50);
        let five = c.query("note", 5, &History::default(), NOW);
        assert_eq!(paths(&five), paths(&all[..5]));
        assert_eq!(c.query("note", 500, &History::default(), NOW).len(), 50);
    }

    #[test]
    fn dp_positions_name_characters_that_spell_the_query() {
        let mut mt = Matcher::default();
        for (cand, query) in [
            ("The Quarterly Review of 2026", "qr26"),
            ("a-b-c-d-e-f", "bdf"),
            ("Projects/Roadmap/Q3 plan", "rmq3"),
            ("aaa bbb aaa", "aba"),
        ] {
            let pat = Pattern::new(query);
            assert!(mt.score(cand, &pat, true).is_some(), "{cand} / {query}");
            let chars: Vec<char> = cand.chars().collect();
            let spelled: String = mt.positions.iter().map(|&p| lower(chars[p])).collect();
            assert_eq!(spelled, query, "{cand}");
            assert!(mt.positions.windows(2).all(|w| w[0] < w[1]));
        }
    }

    #[test]
    fn a_better_alignment_beats_the_first_greedy_one() {
        let mut mt = Matcher::default();
        let pat = Pattern::new("fb");
        let greedy = mt.score("f_xxxxxxxx_foo_bar", &pat, true).unwrap();
        assert_eq!(mt.positions, vec![11, 15], "word starts of foo bar");
        assert_eq!(greedy.class, WORD_OR_SUBSTRING);
    }

    #[test]
    fn above_the_cell_cap_the_leftmost_alignment_is_scored_and_classed() {
        let q = "abcdefghij";
        let pat = Pattern::new(q);
        let mut mt = Matcher::default();

        // Leftmost is contiguous: class 2 although the window (~210 chars x 10) is over the cap.
        let tail = format!("{}{q}", "x".repeat(200));
        let s = mt.score(&tail, &pat, true).unwrap();
        assert_eq!(s.class, WORD_OR_SUBSTRING);
        assert_eq!(mt.positions, (200..210).collect::<Vec<_>>());

        // Leftmost is scattered and not at word starts, while a word-start alignment
        // exists further along: the fallback does not see it and says SCATTERED.
        let spread: String = q.chars().flat_map(|c| [c, 'x']).collect();
        let starts: String = q.chars().flat_map(|c| [' ', c]).collect();
        let cand = format!("{spread}{}{starts}", "x".repeat(200));
        assert!(cand.chars().count() <= MAX_CANDIDATE_CHARS);
        let s = mt.score(&cand, &pat, true).unwrap();
        assert_eq!(s.class, SCATTERED);
        assert_eq!(mt.positions, (0..10).map(|i| i * 2).collect::<Vec<_>>());
        // Same text under the cap (short query): the DP would find the word starts.
        let short = Pattern::new("ab");
        let s = mt.score("axb a b", &short, true).unwrap();
        assert_eq!(s.class, WORD_OR_SUBSTRING);
        assert_eq!(mt.positions, vec![4, 6]);
    }

    #[test]
    fn once_the_query_budget_is_spent_candidates_fall_back_deterministically() {
        let pat = Pattern::new("ab");
        let mut fresh = Matcher::default();
        assert_eq!(
            fresh.score("axb a b", &pat, true).unwrap().class,
            WORD_OR_SUBSTRING
        );
        let mut spent = Matcher {
            spent: QUERY_CELL_BUDGET,
            ..Matcher::default()
        };
        let s = spent.score("axb a b", &pat, true).unwrap();
        assert_eq!(s.class, SCATTERED);
        assert_eq!(spent.positions, vec![0, 2]);
        // The budget is consumed in candidate order: the first candidates get the DP.
        let mut mt = Matcher {
            spent: QUERY_CELL_BUDGET - 14 * 3,
            ..Matcher::default()
        };
        let classes: Vec<Class> = (0..6)
            .map(|_| mt.score("axb a b", &pat, false).unwrap().class)
            .collect();
        assert_eq!(
            classes,
            [
                WORD_OR_SUBSTRING,
                WORD_OR_SUBSTRING,
                WORD_OR_SUBSTRING,
                SCATTERED,
                SCATTERED,
                SCATTERED
            ]
        );
    }

    #[test]
    fn from_paths_serves_before_the_index_is_ready() {
        let c = Corpus::from_paths(["notes/Alpha.md", "img/x.png", "notes/beta.MD"]);
        assert_eq!(c.source(), CorpusSource::Tree);
        assert_eq!(c.len(), 2);
        let hits = q(&c, "alp");
        assert_eq!(hits[0].path, "notes/Alpha.md");
        assert_eq!(hits[0].title.as_deref(), Some("Alpha"));
        assert!(q(&c, "x").is_empty());
    }
}
