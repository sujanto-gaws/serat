//! Checks that the PRD's Must list and architecture.md's design accounting agree.
//!
//! **P0 finding F-303.** §Traceability's preamble makes a claim about a file it
//! does not live in: *"Every requirement of priority Must in the baseline is
//! accounted for."* The baseline is `docs/02-requirements/prd.md`; the claim is
//! in `docs/03-design/architecture.md`; the two have different owners who
//! cannot see each other's edits. NFR-SEC-06 was allocated in the PRD at v1.1
//! and the claim went stale the moment product-analyst committed — the table
//! was correct when it was written and the file it depends on changed under
//! it. [`extract_citations`](crate::extract_citations) already checks that a
//! cited *path* resolves; nothing checked that two *documents* agree, which is
//! a different failure than either F-239 (a path that does not exist) or
//! F-238 (an ID missing from one table) — this is one document's claim about
//! another document's contents, uncheckable from inside either file alone.
//!
//! **What this does not cover, stated because a green join reads as more than
//! it is.** This checks that every PRD Must is *accounted for* — cited by name
//! somewhere in architecture.md's five accounting places. It does not check
//! that the design element cited is real, adequate, or still means what it
//! said: F-239 already showed a table citing five files that did not exist,
//! and this check would have called that row "accounted for" too. Citing and
//! designing are different claims, and this crate checks the weaker one.
//!
//! **The five places**, per architecture.md's own §Traceability preamble and
//! architect's accounting of it: the main Traceability table's Satisfies
//! column; §10.1's distribution table; §10.2's security table; §10.4's
//! "conventional design" table; and the "Musts with no design element yet"
//! subsection, which is coverage of a kind precisely because it says plainly
//! that none exists — a Must named there is accounted for the way F-239's
//! `*(planned)*` marker accounts for a path that does not resolve yet.
//! Treating §10.4 as the only supplement is the mistake that made an earlier,
//! by-hand count of this same property overstate itself by nearly double
//! (15 reported, 8 real) — FR-PLT-01..05, NFR-SEC-01..05 and FR-BIN-19 live in
//! §10.1/§10.2 and nowhere else.

use std::collections::BTreeSet;

/// One requirement from the PRD's definition tables.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Requirement {
    /// e.g. `"FR-VLT-01"`.
    pub id: String,
    /// Whether its Pri column reads exactly `Must`.
    pub is_must: bool,
}

/// Priorities the PRD's Pri column uses. Matched on the full word, not the
/// single-letter code the legend defines (`M`/`S`/`C`/`W`) — the tables
/// themselves spell it out, and matching the code would also match the `M` in
/// "Must" as a substring of things it should not.
const PRIORITIES: &[&str] = &["Must", "Should", "Could", "Won't"];

/// Extract every requirement the PRD's own definition tables declare.
///
/// Scoped to the text between `## Functional requirements` and
/// `## Traceability` deliberately: the PRD has its *own* traceability
/// appendix, shaped exactly like the definition tables (`| ID | **Title** |
/// Pri | Phase | ... |`), and scanning it too does not change the resulting
/// set on a coherent document — every ID there is also in a definition table
/// — but it did, once, on an incoherent one: an earlier by-hand pass that
/// pattern-matched too loosely picked up appendix rows and "silently changed
/// the denominator" (architect's own words). Scoping to the definition tables
/// is the version of the check that cannot repeat that mistake.
///
/// # Errors
/// When the PRD does not contain both boundary headings, since a document
/// that has moved this far cannot be read for its Must list at all.
pub fn extract_prd_requirements(prd_text: &str) -> Result<Vec<Requirement>, String> {
    let zone = slice_between(prd_text, "## Functional requirements", "\n## Traceability")
        .ok_or_else(|| {
            "the PRD no longer has a \"## Functional requirements\" ... \"## Traceability\" \
             span; extract_prd_requirements cannot find the definition tables"
                .to_string()
        })?;

    let mut out = Vec::new();
    for line in zone.lines() {
        let Some(row) = table_row_cells(line) else {
            continue;
        };
        // ID, Requirement, Pri, Phase, Criteria, Verify -- six columns, every
        // definition table in the PRD (FR and NFR alike).
        if row.len() < 3 {
            continue;
        }
        let Some(id) = as_requirement_id(row[0]) else {
            continue;
        };
        let Some(&pri) = PRIORITIES.iter().find(|p| **p == row[2]) else {
            continue;
        };
        out.push(Requirement {
            id,
            is_must: pri == "Must",
        });
    }
    Ok(out)
}

/// Extract every requirement ID architecture.md's five accounting places
/// cite, by name, anywhere in their text.
///
/// Deliberately a blanket token scan within each zone rather than a
/// column-aware table parser: the five places have five different cell
/// shapes (a bare ID in §10.1, `ID · label` in §10.2, a comma-separated list
/// in §10.4 and in the main table's Satisfies column, a bolded ID inside
/// prose in "Musts with no design element yet"), and a parser that assumes
/// one shape silently stops covering the others the day a row is reformatted.
/// A requirement ID does not otherwise appear in these sections' prose, so
/// scanning for the token is the same claim as scanning the specific column,
/// without needing to trust that the column stays in one place.
///
/// # Errors
/// When architecture.md is missing any of the five zones' boundary headings.
pub fn extract_accounted_ids(architecture_text: &str) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();

    // §10.1 (distribution) and §10.2 (security posture) together -- adjacent,
    // both requirement-ID tables, §10.3 is prose about the benchmark vault
    // and cites none.
    scan_zone(
        architecture_text,
        "### 10.1 Distribution",
        "### 10.3 The verification harness",
        &mut out,
    )?;

    // §10.4, conventional-design table.
    scan_zone(
        architecture_text,
        "### 10.4 Requirements whose design is conventional",
        "\n## Related ADRs",
        &mut out,
    )?;

    // The main Traceability table plus "Musts with no design element yet",
    // which is its subsection -- both close before Open design questions.
    scan_zone(
        architecture_text,
        "\n## Traceability",
        "\n## Open design questions",
        &mut out,
    )?;

    Ok(out)
}

fn scan_zone(text: &str, start: &str, end: &str, out: &mut BTreeSet<String>) -> Result<(), String> {
    let zone = slice_between(text, start, end).ok_or_else(|| {
        format!(
            "architecture.md is missing the {start:?} .. {end:?} span extract_accounted_ids needs"
        )
    })?;
    for id in extract_id_tokens(zone) {
        out.insert(id);
    }
    Ok(())
}

/// Every requirement ID (or expanded range) mentioned in `text`.
fn extract_id_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in id_tokens(text) {
        if let Some(range) = expand_range(token) {
            out.extend(range);
        } else if let Some(id) = as_requirement_id(token) {
            out.push(id);
        }
    }
    out
}

fn is_id_char(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-' || c == '.'
}

/// Maximal runs of `[A-Z0-9.-]`, which is exactly wide enough to also catch a
/// `FR-BIN-05..10` range as one token.
fn id_tokens(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut last_end = 0;
    for (i, c) in text.char_indices() {
        if is_id_char(c) {
            if start.is_none() {
                start = Some(i);
            }
            last_end = i + c.len_utf8();
        } else if let Some(s) = start.take() {
            out.push(&text[s..last_end]);
        }
    }
    if let Some(s) = start {
        out.push(&text[s..last_end]);
    }
    out
}

/// Whether `token` is exactly a requirement ID: `FR-` or `NFR-`, an area code
/// of uppercase letters, then digits, and nothing else riding along.
///
/// `FR-VLT-01` matches. `FR-BIN-05..10` does not (`expand_range` handles it
/// first); `P2` and `10` do not (no `FR`/`NFR` prefix); `US-01` does not (not
/// a functional or non-functional requirement, and out of `extract_id_tokens`'
/// scope by design -- this check is about FR-/NFR- Musts, the ones
/// §Traceability's preamble makes the claim about).
fn as_requirement_id(token: &str) -> Option<String> {
    if token.contains("..") {
        return None;
    }
    let rest = token
        .strip_prefix("NFR-")
        .or_else(|| token.strip_prefix("FR-"))?;
    let (area, num) = rest.split_once('-')?;
    if area.is_empty() || !area.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(token.to_string())
}

/// Expand `FR-BIN-05..10` into every ID from 05 to 10 inclusive, preserving
/// the zero-padded width the document uses. `None` if `token` is not a range.
fn expand_range(token: &str) -> Option<Vec<String>> {
    let (id_part, end_num) = token.split_once("..")?;
    if end_num.is_empty() || !end_num.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let prefix = as_requirement_id(id_part)?;
    let (stem, start_num) = prefix.rsplit_once('-')?;
    let start: u32 = start_num.parse().ok()?;
    let end: u32 = end_num.parse().ok()?;
    let width = start_num.len();
    if end < start {
        return None;
    }
    Some(
        (start..=end)
            .map(|n| format!("{stem}-{n:0width$}"))
            .collect(),
    )
}

/// Trimmed, non-empty cells of one Markdown table row, or `None` if `line` is
/// not a table row (a heading, a separator, prose, a blank line).
fn table_row_cells(line: &str) -> Option<Vec<&str>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') {
        return None;
    }
    // A separator row is every cell made of only `-` and `:`.
    let cells: Vec<&str> = trimmed
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect();
    if cells
        .iter()
        .all(|c| !c.is_empty() && c.bytes().all(|b| matches!(b, b'-' | b':')))
    {
        return None;
    }
    Some(cells)
}

fn slice_between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let after_start = text.find(start)? + start.len();
    let rest = &text[after_start..];
    let end_at = rest.find(end).unwrap_or(rest.len());
    Some(&rest[..end_at])
}

/// One run of the PRD/architecture.md join.
#[derive(Clone, Debug, Default)]
pub struct JoinReport {
    /// How many Musts the PRD declares.
    pub prd_musts: usize,
    /// PRD Musts that architecture.md's five places do not cite anywhere.
    pub unaccounted: Vec<String>,
    /// IDs architecture.md cites that the PRD does not declare at all (any
    /// priority) -- a design element pointing at a requirement that has been
    /// renamed, re-split or dropped since the row was written.
    pub stale_citations: Vec<String>,
}

impl JoinReport {
    /// Whether the join holds: no PRD Must goes unaccounted for, and no
    /// citation points at an ID the PRD no longer has.
    #[must_use]
    pub fn holds(&self) -> bool {
        self.unaccounted.is_empty() && self.stale_citations.is_empty()
    }

    /// Human-readable report.
    #[must_use]
    pub fn to_report(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        if self.holds() {
            let _ = writeln!(
                s,
                "PASS  every one of the PRD's {} Must(s) is accounted for in architecture.md, \
                 and every ID architecture.md cites still exists in the PRD (F-303)",
                self.prd_musts
            );
        } else {
            let _ = writeln!(
                s,
                "FAIL  the PRD and architecture.md's design accounting disagree ({} Must(s) \
                 in the PRD) (F-303)",
                self.prd_musts
            );
            if !self.unaccounted.is_empty() {
                let _ = writeln!(
                    s,
                    "  {} Must(s) the PRD declares and architecture.md's five places do not \
                     account for anywhere:",
                    self.unaccounted.len()
                );
                for id in &self.unaccounted {
                    let _ = writeln!(s, "    {id}");
                }
            }
            if !self.stale_citations.is_empty() {
                let _ = writeln!(
                    s,
                    "  {} citation(s) in architecture.md naming an ID the PRD no longer has \
                     at any priority:",
                    self.stale_citations.len()
                );
                for id in &self.stale_citations {
                    let _ = writeln!(s, "    {id}");
                }
            }
        }
        s
    }
}

/// Judge the join between a set of PRD requirements and a set of IDs
/// architecture.md accounts for.
///
/// Split from [`check_prd_join`] so it can be tested against synthetic input
/// without touching the real documents (F-58: a test that can only reach the
/// real files proves nothing about the logic on its own).
#[must_use]
pub fn judge(prd: &[Requirement], accounted: &BTreeSet<String>) -> JoinReport {
    let prd_ids: BTreeSet<&str> = prd.iter().map(|r| r.id.as_str()).collect();
    let musts: Vec<&str> = prd
        .iter()
        .filter(|r| r.is_must)
        .map(|r| r.id.as_str())
        .collect();

    let mut unaccounted: Vec<String> = musts
        .iter()
        .filter(|id| !accounted.contains(**id))
        .map(|id| (*id).to_string())
        .collect();
    unaccounted.sort();
    unaccounted.dedup();

    let mut stale_citations: Vec<String> = accounted
        .iter()
        .filter(|id| !prd_ids.contains(id.as_str()))
        .cloned()
        .collect();
    stale_citations.sort();

    JoinReport {
        prd_musts: musts.len(),
        unaccounted,
        stale_citations,
    }
}

/// Run the whole join against the real documents.
///
/// # Errors
/// A filesystem failure, or either document missing a zone the extraction
/// needs.
pub fn check_prd_join(repo_root: &std::path::Path) -> Result<JoinReport, String> {
    let prd_path = repo_root.join("docs/02-requirements/prd.md");
    let arch_path = repo_root.join("docs/03-design/architecture.md");
    let prd_text = std::fs::read_to_string(&prd_path)
        .map_err(|e| format!("reading {}: {e}", prd_path.display()))?;
    let arch_text = std::fs::read_to_string(&arch_path)
        .map_err(|e| format!("reading {}: {e}", arch_path.display()))?;

    let prd = extract_prd_requirements(&prd_text)?;
    let accounted = extract_accounted_ids(&arch_text)?;
    Ok(judge(&prd, &accounted))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: &str, is_must: bool) -> Requirement {
        Requirement {
            id: id.to_string(),
            is_must,
        }
    }

    // --------------------------------------------------------- extraction

    #[test]
    fn a_definition_table_row_is_parsed_for_id_and_priority() {
        let prd = "\
## Functional requirements

### FR-VLT — Vault & files

| ID | Requirement | Pri | Phase | Criteria | Verify |
|---|---|---|---|---|---|
| FR-VLT-01 | **Open a vault folder** — text | Must | P1 | crit | Test |
| FR-VLT-07 | **Ignore rules** — text | Should | P1 | crit | Test |

## Traceability

not scanned
";
        let got = extract_prd_requirements(prd).expect("parses");
        assert_eq!(got, vec![req("FR-VLT-01", true), req("FR-VLT-07", false)]);
    }

    #[test]
    fn the_prds_own_traceability_appendix_is_not_scanned() {
        // architect's own warning: a looser pattern picked up this appendix
        // too and silently changed the denominator. It has the same row
        // shape as the definition tables and must be excluded by the zone
        // boundary, not by looking different.
        let prd = "\
## Functional requirements

| ID | Requirement | Pri | Phase | Criteria | Verify |
|---|---|---|---|---|---|
| FR-VLT-01 | **Open a vault folder** — text | Must | P1 | crit | Test |

## Traceability

| ID | Requirement | Pri | Phase | Stories | Verify | Gate |
|---|---|---|---|---|---|---|
| FR-GHOST-99 | **Not a real requirement** | Must | P1 | US-01 | Test | gate |
";
        let got = extract_prd_requirements(prd).expect("parses");
        assert_eq!(got, vec![req("FR-VLT-01", true)]);
    }

    #[test]
    fn a_plain_id_a_dotted_id_and_a_labelled_id_are_all_found() {
        let text = "FR-PLT-01 is here. NFR-SEC-01 · no telemetry is here too.";
        assert_eq!(extract_id_tokens(text), vec!["FR-PLT-01", "NFR-SEC-01"]);
    }

    #[test]
    fn a_comma_separated_list_is_found() {
        let text = "FR-EDT-01, FR-EDT-03, FR-EDT-05, FR-EDT-08";
        assert_eq!(
            extract_id_tokens(text),
            vec!["FR-EDT-01", "FR-EDT-03", "FR-EDT-05", "FR-EDT-08"]
        );
    }

    #[test]
    fn a_bolded_id_inside_prose_is_found() {
        let text = "**FR-NAV-01** quick switcher (Must, P1) | Nothing here designs it.";
        assert_eq!(extract_id_tokens(text), vec!["FR-NAV-01"]);
    }

    #[test]
    fn a_range_expands_to_every_id_zero_padded() {
        let text = "FR-BIN-05..10, FR-BIN-16";
        assert_eq!(
            extract_id_tokens(text),
            vec![
                "FR-BIN-05",
                "FR-BIN-06",
                "FR-BIN-07",
                "FR-BIN-08",
                "FR-BIN-09",
                "FR-BIN-10",
                "FR-BIN-16",
            ]
        );
    }

    #[test]
    fn trailing_non_id_text_in_a_cell_is_ignored() {
        // "P2 gate" must not be read as part of an ID or produce a spurious one.
        let text = "FR-STR-06, FR-STR-07, FR-STR-10, P2 gate";
        assert_eq!(
            extract_id_tokens(text),
            vec!["FR-STR-06", "FR-STR-07", "FR-STR-10"]
        );
    }

    #[test]
    fn a_story_id_is_not_a_requirement_id() {
        assert_eq!(
            extract_id_tokens("US-01, A-06, C-02, ADR-0002"),
            Vec::<String>::new()
        );
    }

    // -------------------------------------------------------------- judge

    #[test]
    fn a_must_with_no_citation_anywhere_is_reported() {
        let prd = vec![req("FR-VLT-01", true), req("FR-VLT-07", false)];
        let accounted = BTreeSet::new();
        let report = judge(&prd, &accounted);
        assert!(!report.holds());
        assert_eq!(report.unaccounted, vec!["FR-VLT-01".to_string()]);
        assert!(report.stale_citations.is_empty());
        // The Should must not appear -- the claim is about Musts only.
        assert!(!report.unaccounted.contains(&"FR-VLT-07".to_string()));
    }

    #[test]
    fn a_citation_the_prd_no_longer_declares_is_reported() {
        let prd = vec![req("FR-VLT-01", true)];
        let accounted: BTreeSet<String> =
            ["FR-VLT-01".to_string(), "FR-GHOST-99".to_string()].into();
        let report = judge(&prd, &accounted);
        assert!(!report.holds());
        assert!(report.unaccounted.is_empty());
        assert_eq!(report.stale_citations, vec!["FR-GHOST-99".to_string()]);
    }

    #[test]
    fn a_must_cited_anywhere_in_the_five_places_is_accounted_for() {
        let prd = vec![req("FR-VLT-01", true)];
        let accounted: BTreeSet<String> = ["FR-VLT-01".to_string()].into();
        let report = judge(&prd, &accounted);
        assert!(report.holds(), "{}", report.to_report());
    }

    #[test]
    fn zero_musts_and_zero_accounted_is_a_vacuous_pass_and_the_report_says_so() {
        let report = judge(&[], &BTreeSet::new());
        assert!(report.holds());
        assert!(report.to_report().contains("0 Must"));
    }

    // ---------------------------------------------------- F-304 (F-58/F-27)

    #[test]
    fn the_join_fails_on_a_tree_with_a_must_deliberately_removed_from_the_table() {
        // Prove it can fail (F-58, F-27, F-304): a synthetic PRD with a Must
        // architecture.md's synthetic accounting does not mention.
        let prd_text = "\
## Functional requirements

| ID | Requirement | Pri | Phase | Criteria | Verify |
|---|---|---|---|---|---|
| FR-VLT-01 | **Open a vault folder** — text | Must | P1 | crit | Test |
| NFR-SEC-06 | **Path containment** — text | Must | P0 | crit | Test |

## Traceability

not scanned
";
        let arch_text = "\
### 10.1 Distribution

| Requirement | Design |
|---|---|
| FR-VLT-01 | covered here |

### 10.3 The verification harness

prose

### 10.4 Requirements whose design is conventional

| Requirements | Design |
|---|---|

## Related ADRs

## Traceability

| Design element | Where | Satisfies |
|---|---|---|

### Musts with no design element yet

## Open design questions
";
        // NFR-SEC-06 is deliberately absent from every one of the five zones.
        let prd = extract_prd_requirements(prd_text).expect("parses");
        let accounted = extract_accounted_ids(arch_text).expect("parses");
        let report = judge(&prd, &accounted);
        assert!(!report.holds(), "the missing Must must be caught");
        assert_eq!(report.unaccounted, vec!["NFR-SEC-06".to_string()]);
    }

    #[test]
    fn the_real_documents_join_cleanly() {
        // The property that must hold from here on, not a snapshot of today's
        // documents (F-255's lesson, applied before repeating it): pass/fail
        // only, no pinned ID list, since the set on both sides is expected to
        // keep changing as the PRD and the design evolve together.
        let report = check_prd_join(&crate::repo_root()).expect("both documents read");
        assert!(report.holds(), "{}", report.to_report());
        assert!(
            report.prd_musts > 50,
            "only {} Musts found in the PRD, which suggests the extraction zone regressed \
             rather than that the PRD shrank that far",
            report.prd_musts
        );
    }
}
