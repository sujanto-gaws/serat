//! A count written in prose immediately above a list must match the list.
//!
//! **P0 finding F-496.** *"Four properties have to hold"* over a five-row
//! table; *"28 quality obligations"* over 30 rows; *"Three checks, one crate"*
//! over four binaries. Five instances in one day, and **two of them were
//! created by the commit that fixed the previous one** — because the number is
//! maintained by whoever changes the list, and that person is not reading the
//! paragraph above it. The cheap rule F-496 states is *do not write a count
//! over a list you can point at*: "every property below" costs nothing and
//! cannot decay.
//!
//! This is the one of the three decay classes that needs **no judgement**.
//! F-471's promises and F-495's task-numbered column headers both require
//! knowing what was intended; a count over an adjacent list is arithmetic, and
//! the same shape `check-table-structure` already handles.
//!
//! # The three edges, which are the whole design
//!
//! **1. Adjacency is the condition.** A count is checkable only while it and
//! its items sit in one block — prose, then the list, with nothing between.
//! *"Serat is built from three crates"* in a paragraph with no list under it
//! is a claim about the world, not a caption over an enumeration, and this
//! check must not touch it. Adjacency is what makes the arithmetic decidable
//! without reading for meaning.
//!
//! **2. Counts over fixed quantities are excluded**, and edge 1 does the
//! excluding for free: *"two marks on one timeline"* is a property of
//! NFR-PRF-01, and no list follows it, so nothing fires. That is why the rule
//! is written as adjacency rather than as a vocabulary of exceptions — a list
//! of nouns that mean "fixed" would be the second list that drifts from the
//! first.
//!
//! **3. A count inside an assertion is a guard, not decay.** `assert_eq!(
//! P0_BUDGETS.len(), 3)` fails the moment the list moves, which is the
//! opposite of silent decay. This check reads Markdown only, so assertions are
//! excluded by construction rather than by a rule that could be
//! misapplied — *"editing those would be following the letter of the rule past
//! its reason."*
//!
//! # What it does not catch, stated because the gap is the point
//!
//! **Rust doc comments over a `const` array.** F-496's own `P0_BUDGETS`
//! instance is a count in a `///` comment above a code list, and this check
//! does not read Rust. Detecting it needs the count, the list and the
//! adjacency to survive a language whose "list" is an expression — that is a
//! different, non-trivial parse, and claiming it here would be exactly the
//! over-promise this crate keeps recording. **A count in a Rust doc comment
//! is still F-496's class; it is simply still on a human to notice.**

use std::path::{Path, PathBuf};

/// A count in prose that does not match the list immediately beneath it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CountProblem {
    /// Repo-relative path of the document.
    pub file: PathBuf,
    /// 1-based line number of the prose carrying the count.
    pub line: usize,
    /// The number as written.
    pub claimed: usize,
    /// The number of items actually in the list beneath it.
    pub actual: usize,
    /// The counted noun, for the message.
    pub noun: String,
}

impl CountProblem {
    /// One line, in the shape the other checks use.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "{}:{}: prose says {} {} and the list beneath it has {}. F-496: do not write a \
             count over a list you can point at -- \"every {} below\" cannot decay, a number \
             has to be maintained by whoever changes the list, and that is exactly the person \
             not reading the line above it.",
            self.file.display(),
            self.line,
            self.claimed,
            self.noun,
            self.actual,
            self.noun.trim_end_matches('s')
        )
    }
}

/// Number words this check understands, plus digits.
///
/// Deliberately small: one to twelve, which is the range a human writes as a
/// word and therefore the range where the decay happens. A list of forty
/// items is not summarised as "forty" in prose above it.
const WORDS: [(&str, usize); 12] = [
    ("one", 1),
    ("two", 2),
    ("three", 3),
    ("four", 4),
    ("five", 5),
    ("six", 6),
    ("seven", 7),
    ("eight", 8),
    ("nine", 9),
    ("ten", 10),
    ("eleven", 11),
    ("twelve", 12),
];

fn as_count(token: &str) -> Option<usize> {
    let clean: String = token
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    if clean.is_empty() {
        return None;
    }
    if let Ok(n) = clean.parse::<usize>() {
        return (1..=99).contains(&n).then_some(n);
    }
    WORDS.iter().find(|(w, _)| *w == clean).map(|(_, n)| *n)
}

/// Whether a token reads as a plural noun this check will count.
///
/// Any word ending in `s` that is not a stop word. Deliberately permissive on
/// the noun and strict on the adjacency: guessing at a vocabulary of
/// "countable" nouns is the judgement this class is supposed to avoid.
fn is_plural_noun(token: &str) -> bool {
    let clean: String = token.chars().filter(|c| c.is_alphabetic()).collect();
    if clean.len() < 4 || !clean.ends_with('s') {
        return false;
    }
    // `is`, `has`, `was`, `does`, `its` and friends end in `s` and are not
    // nouns; so do possessives, which arrive here stripped of their apostrophe.
    const STOP: [&str; 12] = [
        "this", "thus", "less", "was", "has", "is", "its", "does", "goes", "yes", "us", "as",
    ];
    !STOP.contains(&clean.to_lowercase().as_str())
}

/// How many items the list starting at `lines[from]` has, or `None` if what
/// is there is not a list.
///
/// A Markdown bullet or ordered list counts its items; a GFM table counts its
/// body rows, header and delimiter excluded. Nested items (indented) are not
/// counted as separate items, because prose says "four properties" about the
/// four top-level ones.
fn list_length(lines: &[&str], from: usize) -> Option<usize> {
    let first = lines.get(from)?.trim_start();
    if first.starts_with('|') {
        // A table: skip header and delimiter, count body rows.
        let mut n = 0;
        let mut row = from + 2;
        while lines
            .get(row)
            .is_some_and(|l| l.trim_start().starts_with('|'))
        {
            n += 1;
            row += 1;
        }
        return (n > 0).then_some(n);
    }
    let is_item = |l: &str| {
        let t = l.trim_start();
        t.starts_with("- ")
            || t.starts_with("* ")
            || t.split_once(". ").is_some_and(|(head, _)| {
                !head.is_empty() && head.chars().all(|c| c.is_ascii_digit())
            })
    };
    if !is_item(first) {
        return None;
    }
    let mut n = 0;
    for line in &lines[from..] {
        if line.trim().is_empty() {
            // A blank line inside a list is legal in GFM (loose lists), so
            // only a blank followed by a non-item ends it.
            continue;
        }
        // Indented continuations and nested items belong to the item above.
        let indented = line.starts_with("  ") || line.starts_with('\t');
        if is_item(line) && !indented {
            n += 1;
        } else if !indented && !is_item(line) {
            break;
        }
    }
    (n > 0).then_some(n)
}

/// The sentence a list is captioned by: the contiguous non-empty block
/// immediately above the list, joined, reduced to its final sentence.
///
/// **Captions wrap, and F-496's own primary instance is one that does** --
/// *"Four properties have to hold and they do not have one / enforcer between
/// them:"* spans two lines. Reading a line at a time misses it, which would
/// have left this check catching none of the cases that motivated it. Taking
/// the *last* sentence rather than the whole block is what keeps the
/// preceding paragraph's numbers out of the comparison.
fn caption_sentence(lines: &[&str], end: usize) -> String {
    let mut start = end;
    while start > 0 {
        let prev = lines[start - 1].trim();
        if prev.is_empty() || prev.starts_with('|') || prev.starts_with("- ") {
            break;
        }
        start -= 1;
    }
    let block = lines[start..=end]
        .iter()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join(" ");
    block
        .rsplit_once(". ")
        .map_or_else(|| block.clone(), |(_, last)| last.to_string())
}

/// Check one document.
#[must_use]
pub fn check_text(file: &Path, text: &str) -> Vec<CountProblem> {
    let lines: Vec<&str> = text.lines().collect();
    let mut problems = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('|') || trimmed.starts_with("- ") {
            continue;
        }
        // **The caption rule, and it is the second half of adjacency.** Only a
        // line that *introduces* a list governs it: one ending in a colon,
        // allowing trailing emphasis markers. Measured against the real tree
        // before it was written down -- without it the check fired six times
        // and every one was wrong: durations ("~30 weeks to v1.", "6 months")
        // that happened to precede a table, a millisecond figure above a
        // one-item list, and numbered-list items whose "list beneath" was the
        // rest of their own list. **A check whose reports are mostly wrong is
        // one that gets disabled**, and this register has the receipts on
        // exactly that.
        if !trimmed
            .trim_end_matches(['*', '_', '`', ' '])
            .ends_with(':')
        {
            continue;
        }
        // **Adjacency**: the list must start on the next line, or after
        // exactly one blank line. Anything further away is prose about the
        // world rather than a caption over an enumeration.
        let next = if lines.get(i + 1).is_some_and(|l| l.trim().is_empty()) {
            i + 2
        } else {
            i + 1
        };
        let Some(actual) = list_length(&lines, next) else {
            continue;
        };

        // The count must be the last number-plus-noun pair on the line, so
        // "the four properties in the three tables below" is read as three.
        // **The count must LEAD the caption**, after any markup. Measured, and
        // this is the rule that made the difference: with the count merely
        // *somewhere* on the line, all four hits against the real tree were
        // wrong, and every one was wrong the same way -- the number was
        // attached to the data's provenance or unit rather than to the
        // enumeration. "taken across two sessions (F-327):" over a five-row
        // table, "From 48 traces ...:" over three, "carries two rows per
        // filesystem-bound budget:" over one, "In scope for v1 (six
        // pillars):" over seven bullets of which six are lettered pillars.
        // A count that leads its caption is the one the caption is *about*;
        // a count anywhere else needs a reader to decide what it refers to,
        // which is the judgement this class was chosen for not needing.
        // A caption may wrap, and F-496's own primary instance does: "Four
        // properties have to hold and they do not have one / enforcer between
        // them:" is one sentence across two lines, and a line-at-a-time rule
        // misses it -- which would have left this check catching none of the
        // instances that motivated it. So the caption is the sentence, not
        // the line.
        let sentence = caption_sentence(&lines, i);
        let lead = sentence.trim_start_matches(['*', '_', '#', '>', '`', '-', ' ']);
        let tokens: Vec<&str> = lead.split_whitespace().take(2).collect();
        let Some((claimed, noun)) = (match tokens.as_slice() {
            [a, b] if is_plural_noun(b) => as_count(a).map(|n| {
                (
                    n,
                    b.chars().filter(|c| c.is_alphabetic()).collect::<String>(),
                )
            }),
            _ => None,
        }) else {
            continue;
        };
        if claimed != actual {
            problems.push(CountProblem {
                file: file.to_path_buf(),
                line: i + 1,
                claimed,
                actual,
                noun,
            });
        }
    }
    problems
}

/// Run the check against every `.md` file under `docs/`.
///
/// Returns the number of files scanned alongside the problems, so a caller can
/// tell "0 problems" from "checked nothing" (F-27).
///
/// # Errors
/// A filesystem failure while walking or reading.
pub fn check_repo_docs(repo_root: &Path) -> Result<(usize, Vec<CountProblem>), String> {
    let mut files = Vec::new();
    collect_markdown(&repo_root.join("docs"), &mut files)
        .map_err(|e| format!("walking docs: {e}"))?;
    files.sort();
    let mut problems = Vec::new();
    let scanned = files.len();
    for path in files {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let rel = path.strip_prefix(repo_root).unwrap_or(&path);
        let rel = PathBuf::from(rel.to_string_lossy().replace('\\', "/"));
        problems.extend(check_text(&rel, &text));
    }
    Ok((scanned, problems))
}

fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_markdown(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(text: &str) -> Vec<CountProblem> {
        check_text(Path::new("t.md"), text)
    }

    #[test]
    fn a_count_that_does_not_match_the_list_beneath_it_is_caught() {
        // F-496's own shape: the number was right when written and a later
        // edit added a row.
        let problems = check("Four properties have to hold:\n\n- a\n- b\n- c\n- d\n- e\n");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].claimed, 4);
        assert_eq!(problems[0].actual, 5);
        assert_eq!(problems[0].noun, "properties");
        assert!(problems[0].describe().contains("F-496"));
    }

    #[test]
    fn f496s_own_primary_instance_is_caught_verbatim() {
        // **The recall test, and the check earns its place or fails here.** A
        // check that catches none of the cases that motivated it is the
        // guard-that-cannot-fail class this register has thirteen of, and it
        // would be worse than no check because it would look like F-496 was
        // closed. This is ADR-0014's text as it stood at `1811f19^`, verbatim
        // including the line wrap, against the five-row table it captioned.
        let historical = "is correct placement, not a split. Four properties have to hold and                           they do not have one
enforcer between them:

                          | Property | Enforced | By what |
|---|---|---|
                          | a | x | y |
| b | x | y |
| c | x | y |
                          | d | x | y |
| e | x | y |
";
        let found = check(historical);
        assert_eq!(
            found.len(),
            1,
            "F-496's own instance must be caught: {found:?}"
        );
        assert_eq!((found[0].claimed, found[0].actual), (4, 5));
        assert_eq!(found[0].noun, "properties");
    }

    #[test]
    fn a_count_that_matches_is_not_reported() {
        assert!(check("Four properties have to hold:\n\n- a\n- b\n- c\n- d\n").is_empty());
    }

    #[test]
    fn a_table_is_counted_by_its_body_rows_not_its_header() {
        let table = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n";
        assert!(
            check(&format!("Two rows follow:\n\n{table}")).is_empty(),
            "header and delimiter must not count as rows"
        );
        let wrong = check(&format!("Three rows follow:\n\n{table}"));
        assert_eq!(wrong.len(), 1);
        assert_eq!((wrong[0].claimed, wrong[0].actual), (3, 2));
    }

    #[test]
    fn a_count_with_no_list_under_it_is_left_alone() {
        // **Edge 1, and it is what makes edge 2 free.** "Two marks on one
        // timeline" is a property of NFR-PRF-01, not a caption. Adjacency is
        // the condition; without it this check would be guessing at meaning,
        // which is the judgement this class exists to avoid.
        assert!(check("NFR-PRF-01 has two marks on one timeline.\n\nAnd then prose.\n").is_empty());
        assert!(check("Serat is built from eight crates in one workspace.\n").is_empty());
    }

    #[test]
    fn a_list_that_is_too_far_below_the_count_is_left_alone() {
        // Adjacency means one blank line at most. A paragraph between them
        // and the count is no longer a caption over the list.
        let far = "Three things matter:\n\nSome intervening prose.\n\n- a\n- b\n- c\n- d\n";
        assert!(check(far).is_empty(), "{:?}", check(far));
    }

    #[test]
    fn a_count_that_does_not_lead_its_caption_is_left_alone() {
        // The rule that made this check usable, as a test. In every real
        // false positive the number was attached to the data's provenance or
        // its unit rather than to the enumeration -- "taken across two
        // sessions:", "From 48 traces ...:", "carries two rows per budget:".
        // A count that leads its caption is what the caption is about; a
        // count anywhere else needs a reader to decide its referent, which is
        // the judgement this class was chosen for not needing.
        assert!(
            check(
                "Taken across two sessions (F-327):

- a
- b
- c
- d
- e
"
            )
            .is_empty()
        );
        assert!(
            check(
                "From 48 traces taken on this machine:

- a
- b
- c
"
            )
            .is_empty()
        );
        assert!(
            check(
                "A gate report carries two rows per budget:

- a
"
            )
            .is_empty()
        );
        // And it still catches the shape F-496 is about.
        let caught = check(
            "**Three checks**, one crate:

- a
- b
- c
- d
",
        );
        assert_eq!(caught.len(), 1, "{caught:?}");
        assert_eq!((caught[0].claimed, caught[0].actual), (3, 4));
    }

    #[test]
    fn words_that_end_in_s_but_are_not_nouns_do_not_start_a_count() {
        // "one is" / "two has" would otherwise read as a count.
        assert!(check("Only one is listed:\n\n- a\n- b\n").is_empty());
    }

    #[test]
    fn the_real_docs_tree_is_clean_and_the_check_can_fail() {
        // F-255: the can-it-fail proof lives on synthetic fixtures, and the
        // real tree gets a floor so "0 problems" cannot mean "scanned
        // nothing" (F-27).
        let (scanned, problems) = check_repo_docs(&crate::repo_root()).expect("docs/ is readable");
        assert!(scanned > 20, "only {scanned} files scanned");
        assert!(
            problems.is_empty(),
            "{}",
            problems
                .iter()
                .map(CountProblem::describe)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
