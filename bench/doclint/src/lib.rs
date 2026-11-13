//! Checks that paths the design documents cite as fact actually resolve.
//!
//! **P0 finding F-239.** `docs/03-design/architecture.md` names seven
//! "repo-ready" companion files by path, in the present tense — "the full
//! contract is `contracts/extractor.rs`" — and states that where the document
//! and those files disagree, the files are correct. Two of the seven exist.
//! Five do not, and there is no `grammar/` directory at all.
//!
//! **This is a different failure shape from a missing requirement ID
//! (F-238).** A missing ID reads as *incomplete* and invites someone to
//! finish the table. A citation to a file that is not there reads as
//! *complete* and stops anyone from looking, because nothing about reading
//! the sentence tells you the evidence it points at does not exist — the
//! traceability table's own preamble said coverage may live "here or in the
//! repo-ready files", so for roughly twenty requirement IDs that sentence was
//! doing the work of a design element nobody wrote.
//!
//! **The marker.** Task #7 closed the finding on the document side by
//! marking every cited-but-unwritten path `*(planned)*` in the traceability
//! table (architecture.md §Traceability preamble: "A path in the Where column
//! marked *(planned)* names a repo-ready file that is not written yet...
//! Every unmarked path resolves today, and that is a property a check can
//! assert"). That sentence is this crate's specification, not a suggestion:
//! [`extract_citations`] reads the same marker task #7 writes, so a citation
//! is either real (it must resolve) or explicitly marked deferred (it must
//! not, and the marker says so in the one place a reader is already looking).
//! There is deliberately no second, separately-maintained list of planned
//! paths — a list that can drift from the prose next to it is worse than
//! having none, and the whole point of F-239 was a document disagreeing with
//! itself undetected.

pub mod conflict_markers;
pub mod count_over_list;
pub mod prd_join;
pub mod table_structure;

use std::path::{Path, PathBuf};

/// Top-level directories the design documents cite as fact. A run of
/// path-shaped characters is only a citation candidate if its first `/`
/// segment is one of these.
///
/// This is what keeps `` `.serat/index.db` `` — a runtime path inside a
/// user's vault, describing the open-check's own absence test, never a file
/// in this repository — out of the check entirely, rather than needing a
/// special case for every such reference architecture.md makes.
pub const KNOWN_PREFIXES: &[&str] = &[
    "docs",
    "crates",
    "bench",
    "app",
    "schema",
    "grammar",
    "contracts",
];

/// Extensions the design documents use when citing a real repo-ready file.
///
/// `.pdf` is deliberately absent. ADR-0005 writes `` `contracts/lease.pdf` ``
/// as an example of a *user's vault content* — a binary and the sidecar note
/// that would pair with it — not a citation of a design artifact, and it
/// happens to share the `contracts/` prefix with the real `contracts/`
/// directory this crate checks. Restricting to source-like extensions keeps
/// that example out without a rule that names the string.
pub const KNOWN_EXTENSIONS: &[&str] = &["md", "rs", "sql", "ebnf", "json", "toml", "html"];

/// The one example [`KNOWN_EXTENSIONS`] cannot rule out on its own.
///
/// `` `contracts/lease.pdf.md` `` is the *sidecar name* ADR-0005 uses to
/// illustrate its naming rule (the binary's full filename with `.md`
/// appended) — it ends in `.md` like a real citation, and starts with
/// `contracts/` like one. Named explicitly, with its source, rather than
/// folded into a broader heuristic that would also swallow a real miss: the
/// next false positive should have to earn its own line here, the way this
/// one did (found while building this check, not guessed at in advance).
pub const KNOWN_NON_CITATIONS: &[&str] = &["contracts/lease.pdf", "contracts/lease.pdf.md"];

/// The marker task #7 writes next to a citation that is deliberately not
/// written yet: `*(planned)*`, or the row-scoped variant
/// `*(planned — the whole of this row)*`. Matched as a prefix, on the text
/// immediately following the citation, so either form counts.
pub const PLANNED_MARKER: &str = "*(planned";

/// One citation: a path, where it was cited, and whether that citation
/// carries the [`PLANNED_MARKER`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Citation {
    /// The repo-relative path as written in the document.
    pub path: String,
    /// The document that cited it.
    pub file: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// Whether this specific occurrence is marked `*(planned)*`.
    pub planned: bool,
}

/// Extract every candidate path citation from `text`.
///
/// A candidate is a maximal run of `[A-Za-z0-9/_.-]`, trimmed of trailing
/// dots (a sentence ending right after a filename reads as one token
/// otherwise — `` schema/001_init.sql. `` is the path plus a full stop),
/// whose first `/`-segment is in [`KNOWN_PREFIXES`], which contains no `..`
/// (`` docs/03-design/adr/0001..0010 `` is a range naming ten ADRs, not a
/// path), which ends in one of [`KNOWN_EXTENSIONS`], and which is not one of
/// [`KNOWN_NON_CITATIONS`].
///
/// Deliberately not a regex: the rule is a handful of character-class checks
/// over plain text, and every one of them is named above with the citation
/// that forced it, which a `Regex::new` string cannot be.
#[must_use]
pub fn extract_citations(text: &str, source_file: &Path) -> Vec<Citation> {
    let mut out = Vec::new();
    for (line_idx, line) in text.lines().enumerate() {
        for token in path_tokens(line) {
            if let Some(path) = as_candidate_path(token) {
                let offset = token.as_ptr() as usize - line.as_ptr() as usize;
                let rest = &line[offset + token.len()..];
                let planned = rest.trim_start().starts_with(PLANNED_MARKER);
                out.push(Citation {
                    path,
                    file: source_file.to_path_buf(),
                    line: line_idx + 1,
                    planned,
                });
            }
        }
    }
    out
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-')
}

/// Maximal runs of path-shaped characters on one line, in order, as slices of
/// `line` (so their byte offset within it can be recovered by pointer
/// arithmetic, which is how [`extract_citations`] finds what follows one).
fn path_tokens(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut last_end = 0;
    for (i, c) in line.char_indices() {
        if is_path_char(c) {
            if start.is_none() {
                start = Some(i);
            }
            last_end = i + c.len_utf8();
        } else if let Some(s) = start.take() {
            out.push(&line[s..last_end]);
        }
    }
    if let Some(s) = start {
        out.push(&line[s..last_end]);
    }
    out
}

/// Whether `token` is a citation candidate, normalised (trailing dots
/// trimmed) if so.
fn as_candidate_path(token: &str) -> Option<String> {
    let trimmed = token.trim_end_matches('.');
    if trimmed.is_empty() || !trimmed.contains('/') || trimmed.contains("..") {
        return None;
    }
    if KNOWN_NON_CITATIONS.contains(&trimmed) {
        return None;
    }
    let first_segment = trimmed.split('/').next().unwrap_or("");
    if !KNOWN_PREFIXES.contains(&first_segment) {
        return None;
    }
    let extension = trimmed.rsplit('.').next().unwrap_or("");
    if !KNOWN_EXTENSIONS.contains(&extension) {
        return None;
    }
    Some(trimmed.to_string())
}

/// One path that was cited but does not resolve, and everywhere it was cited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingCitation {
    /// The path that does not exist under the repo root.
    pub path: String,
    /// Every place it was cited, `file` then `line`.
    pub cited_at: Vec<(PathBuf, usize)>,
}

/// A path marked `*(planned)*` that now resolves on disk.
///
/// Not a failure — a planned path is *supposed* to eventually exist — but
/// reported so the marker does not sit in the document forever once the
/// artefact it excused has landed. A marker nobody ever has to remove is a
/// check that only ever weakens; this is what keeps someone able to notice
/// (the same argument as F-152, one level up: a guard nobody revisits is
/// worth exactly as much as one that was deleted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StalePlannedMarker {
    /// The path.
    pub path: String,
    /// Every place the now-stale marker was written, `file` then `line`.
    pub cited_at: Vec<(PathBuf, usize)>,
}

/// What one run of the check found.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Cited paths that do not resolve and are not marked planned.
    pub missing: Vec<MissingCitation>,
    /// Paths marked planned that now exist and can drop the marker.
    pub stale_planned: Vec<StalePlannedMarker>,
    /// How many citation *occurrences* were checked (before deduplication),
    /// so a report of "0 missing" over 0 checked reads as vacuous rather than
    /// clean (P0 finding F-27's lesson: a check that ran over nothing is not
    /// a passing check).
    pub checked: usize,
}

impl Report {
    /// Whether the run found anything that must be fixed.
    ///
    /// A stale planned marker does not fail the run — see
    /// [`StalePlannedMarker`] — so this is exactly `!missing.is_empty()`,
    /// named separately so a caller does not have to know that.
    #[must_use]
    pub fn has_failures(&self) -> bool {
        !self.missing.is_empty()
    }

    /// Human-readable report, one line per missing citation, naming every
    /// place it was cited so the message says what to fix rather than only
    /// that something is wrong.
    #[must_use]
    pub fn to_report(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        if self.missing.is_empty() {
            let _ = writeln!(
                s,
                "PASS  every cited path resolves or is marked *(planned)* ({} citation(s) checked)",
                self.checked
            );
        } else {
            let _ = writeln!(
                s,
                "FAIL  {} cited path(s) do not resolve and are not marked *(planned)*, of {} \
                 checked (F-239)",
                self.missing.len(),
                self.checked
            );
            for m in &self.missing {
                let _ = writeln!(s, "  {}", m.path);
                for (file, line) in &m.cited_at {
                    let _ = writeln!(s, "    cited at {}:{line}", file.display());
                }
            }
        }
        for stale in &self.stale_planned {
            let _ = writeln!(
                s,
                "  note  {} is marked *(planned)* and now exists on disk -- drop the marker",
                stale.path
            );
            for (file, line) in &stale.cited_at {
                let _ = writeln!(s, "    marked at {}:{line}", file.display());
            }
        }
        s
    }
}

/// Judge a set of citations. Split from the filesystem walk in
/// [`check_paths`] so the judgement can be tested without touching disk
/// beyond the fixed set a test controls.
///
/// A path counts as planned if *any* of its citations carries the marker —
/// architecture.md's own top-of-file summary cites all seven repo-ready
/// files in flowing prose with no per-citation marker at all, relying
/// instead on the traceability table's row to carry it, so requiring every
/// occurrence to be marked would fail on a document task #7 already fixed.
///
/// # Panics
/// Never; `exists` is total over its domain.
#[must_use]
pub fn judge(citations: &[Citation], exists: impl Fn(&str) -> bool) -> Report {
    /// One cited path, every citation of it, and whether any of them is planned.
    struct Grouped {
        path: String,
        planned: bool,
        cited_at: Vec<(PathBuf, usize)>,
    }

    let mut by_path: Vec<Grouped> = Vec::new();
    for c in citations {
        match by_path.iter_mut().find(|g| g.path == c.path) {
            Some(g) => {
                g.planned |= c.planned;
                g.cited_at.push((c.file.clone(), c.line));
            }
            None => by_path.push(Grouped {
                path: c.path.clone(),
                planned: c.planned,
                cited_at: vec![(c.file.clone(), c.line)],
            }),
        }
    }

    let mut missing = Vec::new();
    let mut stale_planned = Vec::new();
    for g in &by_path {
        let resolves = exists(&g.path);
        if g.planned {
            if resolves {
                stale_planned.push(StalePlannedMarker {
                    path: g.path.clone(),
                    cited_at: g.cited_at.clone(),
                });
            }
            continue;
        }
        if !resolves {
            missing.push(MissingCitation {
                path: g.path.clone(),
                cited_at: g.cited_at.clone(),
            });
        }
    }
    // Deterministic order: citation-collection order is the file-scan order,
    // which is stable but not obviously so to a reader diffing two reports.
    missing.sort_by(|a, b| a.path.cmp(&b.path));
    stale_planned.sort_by(|a, b| a.path.cmp(&b.path));

    Report {
        missing,
        stale_planned,
        checked: citations.len(),
    }
}

pub use reporoot::{ROOT_ENV, Resolved, Rule};

/// Where this crate was built from. A fallback only: it is the wrong answer
/// whenever the binary is reused from another checkout.
fn compiled_root() -> PathBuf {
    reporoot::compiled_root(env!("CARGO_MANIFEST_DIR"))
}

/// Repository root and the rule that chose it, resolved **at run time** (tasks
/// #45, #54). The rules and their precedence live in the `reporoot` crate, shared
/// with `budget` and `vaultgen`: `--root`, `SERAT_REPO_ROOT`, a `[workspace]`
/// manifest above the working directory, the compiled-in location.
///
/// # Errors
/// A `--root` with no value.
pub fn resolve_repo_root() -> Result<Resolved, String> {
    reporoot::from_process(&compiled_root())
}

/// The pure form, so each rule is testable without touching process state.
///
/// # Errors
/// A `--root` with no value.
pub fn resolve_root(
    args: &[String],
    env: Option<&Path>,
    cwd: Option<&Path>,
) -> Result<Resolved, String> {
    reporoot::resolve_root(args, env, cwd, &compiled_root())
}

/// What every bin starts with: resolve the root and print `root: <path> (from
/// <rule>)` as the first line of the report, so a run that fell back to the
/// compiled-in checkout says so instead of silently reading the builder's tree.
///
/// # Errors
/// A `--root` with no value.
pub fn announce_root() -> Result<PathBuf, String> {
    let r = resolve_repo_root()?;
    println!("{}", r.report_line());
    Ok(r.path)
}

/// Root for the in-crate tests, which read the real tree. A bad `--root` cannot
/// reach them: the test harness does not take one.
#[must_use]
pub fn repo_root() -> PathBuf {
    resolve_repo_root().map_or_else(|_| compiled_root(), |r| r.path)
}

/// Every markdown file the check scans: `docs/03-design/**.md` and
/// `CLAUDE.md`, which carries the same canonical-documents table exposure
/// (F-239's finding names both).
///
/// # Errors
/// A filesystem failure while walking `docs/03-design`.
pub fn scanned_files(repo_root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = vec![repo_root.join("CLAUDE.md")];
    collect_markdown(&repo_root.join("docs/03-design"), &mut out)
        .map_err(|e| format!("walking docs/03-design: {e}"))?;
    out.sort();
    Ok(out)
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

/// Run the whole check against the real repository at `repo_root`.
///
/// # Errors
/// A filesystem failure reading a scanned file or walking `docs/03-design`.
pub fn check_paths(repo_root: &Path) -> Result<Report, String> {
    let mut citations = Vec::new();
    for file in scanned_files(repo_root)? {
        let text = std::fs::read_to_string(&file)
            .map_err(|e| format!("reading {}: {e}", file.display()))?;
        let rel = file.strip_prefix(repo_root).unwrap_or(&file);
        // Forward slashes even on Windows: the documents themselves are always
        // written with them, and a report mixing the two per platform is
        // needlessly harder to diff between a Windows run and CI's own.
        let rel = PathBuf::from(rel.to_string_lossy().replace('\\', "/"));
        citations.extend(extract_citations(&text, &rel));
    }
    Ok(judge(&citations, |p| repo_root.join(p).exists()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cite(path: &str, file: &str, line: usize) -> Citation {
        Citation {
            path: path.to_string(),
            file: PathBuf::from(file),
            line,
            planned: false,
        }
    }

    fn planned(mut c: Citation) -> Citation {
        c.planned = true;
        c
    }

    // ------------------------------------------------------------- TC-093

    #[test]
    fn a_backticked_citation_is_found_with_its_line() {
        let text = "See `schema/001_init.sql` for the DDL.\nAnd also `contracts/ipc.rs`.";
        let got = extract_citations(text, Path::new("architecture.md"));
        assert_eq!(
            got,
            vec![
                cite("schema/001_init.sql", "architecture.md", 1),
                cite("contracts/ipc.rs", "architecture.md", 2),
            ]
        );
    }

    #[test]
    fn a_bare_citation_with_no_backticks_is_found_too() {
        // architecture.md's §5 prose -- "SQ is specified in grammar/sq.ebnf,
        // its lowering in grammar/lowering.md" -- no backticks at all.
        let text = "SQ is specified in grammar/sq.ebnf, its lowering in grammar/lowering.md.";
        let got = extract_citations(text, Path::new("architecture.md"));
        assert_eq!(
            got,
            vec![
                cite("grammar/sq.ebnf", "architecture.md", 1),
                cite("grammar/lowering.md", "architecture.md", 1),
            ]
        );
    }

    #[test]
    fn a_trailing_full_stop_is_not_part_of_the_path() {
        let text = "The full DDL is schema/001_init.sql.";
        let got = extract_citations(text, Path::new("x.md"));
        assert_eq!(got, vec![cite("schema/001_init.sql", "x.md", 1)]);
    }

    #[test]
    fn a_range_of_adrs_is_not_a_path() {
        // CLAUDE.md's table: "docs/03-design/adr/0001..0010" names ten ADRs,
        // not a file, and must not be reported as a missing citation.
        let text = "See docs/03-design/adr/0001..0010 for the decisions.";
        assert_eq!(extract_citations(text, Path::new("CLAUDE.md")), vec![]);
    }

    #[test]
    fn a_runtime_path_outside_the_repo_is_not_a_citation() {
        // architecture.md's stamp description: ".serat/index.db" is a path
        // inside a user's vault, not a file this repository ships.
        let text = "1. `.serat/index.db` absent -> rebuild, reason `NoIndex`.";
        assert_eq!(
            extract_citations(text, Path::new("architecture.md")),
            vec![]
        );
    }

    #[test]
    fn the_known_non_citations_are_excluded_by_name() {
        // ADR-0005's own naming example, sharing the contracts/ prefix with
        // the real design-file directory by coincidence.
        let text = "`contracts/lease.pdf` pairs with `contracts/lease.pdf.md`.";
        assert_eq!(extract_citations(text, Path::new("adr-5.md")), vec![]);
    }

    #[test]
    fn a_path_with_an_unrecognised_prefix_or_extension_is_ignored() {
        let text = "See _to_delete/openapi.yaml and windows_x86_64_msvc-0.53.1.lib.";
        assert_eq!(extract_citations(text, Path::new("x.md")), vec![]);
    }

    // ------------------------------------------------------- planned marker

    #[test]
    fn the_planned_marker_task_7_writes_is_recognised_in_both_its_forms() {
        // Exact syntax from task #7's traceability table (architecture.md).
        let table = "| SQ grammar and type rules | §5, grammar/sq.ebnf *(planned)* | FR-STR-06 |\n\
                     | Conformance suite | grammar/conformance.md *(planned — the whole of this row)* | FR-STR-10 |";
        let got = extract_citations(table, Path::new("architecture.md"));
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|c| c.planned), "{got:?}");
    }

    #[test]
    fn a_citation_with_no_marker_following_it_is_not_planned() {
        let text = "The full contract is contracts/extractor.rs.";
        let got = extract_citations(text, Path::new("architecture.md"));
        assert_eq!(got.len(), 1);
        assert!(!got[0].planned);
    }

    // ------------------------------------------------------------- TC-094

    #[test]
    fn a_missing_citation_is_reported_with_every_place_it_was_cited() {
        let citations = vec![
            cite("grammar/sq.ebnf", "architecture.md", 456),
            cite("grammar/sq.ebnf", "architecture.md", 674),
        ];
        let report = judge(&citations, |_| false);
        assert!(report.has_failures());
        assert_eq!(report.missing.len(), 1);
        assert_eq!(report.missing[0].path, "grammar/sq.ebnf");
        assert_eq!(report.missing[0].cited_at.len(), 2);
        let text = report.to_report();
        assert!(text.contains("grammar/sq.ebnf"), "{text}");
        assert!(text.contains("architecture.md:456"), "{text}");
        assert!(text.contains("architecture.md:674"), "{text}");
    }

    #[test]
    fn a_resolving_citation_is_not_reported() {
        let citations = vec![cite("contracts/ipc.rs", "architecture.md", 14)];
        let report = judge(&citations, |_| true);
        assert!(!report.has_failures());
        assert!(report.missing.is_empty());
    }

    #[test]
    fn a_planned_citation_that_does_not_exist_yet_is_not_a_failure() {
        let citations = vec![planned(cite("grammar/sq.ebnf", "architecture.md", 676))];
        let report = judge(&citations, |_| false);
        assert!(!report.has_failures(), "{}", report.to_report());
        assert!(report.stale_planned.is_empty());
    }

    #[test]
    fn a_path_planned_in_one_citation_is_excused_everywhere_it_is_cited() {
        // architecture.md's top-of-file summary cites all seven files in
        // flowing prose with no per-citation marker; only the traceability
        // table row carries `*(planned)*`. Requiring every occurrence to
        // carry it would fail on the document task #7 already fixed, so the
        // marker excuses the path, not the individual sentence.
        let citations = vec![
            cite("grammar/sq.ebnf", "architecture.md", 14),
            planned(cite("grammar/sq.ebnf", "architecture.md", 676)),
        ];
        let report = judge(&citations, |_| false);
        assert!(!report.has_failures(), "{}", report.to_report());
    }

    #[test]
    fn a_planned_marker_on_a_path_that_now_exists_is_reported_stale_but_not_failed() {
        let citations = vec![planned(cite("grammar/sq.ebnf", "architecture.md", 676))];
        let report = judge(&citations, |_| true);
        assert!(
            !report.has_failures(),
            "a landed planned artifact is not a failure: {}",
            report.to_report()
        );
        assert_eq!(report.stale_planned.len(), 1);
        assert_eq!(report.stale_planned[0].path, "grammar/sq.ebnf");
        assert!(report.to_report().contains("drop the marker"));
    }

    #[test]
    fn zero_citations_is_not_reported_as_a_clean_pass() {
        // F-27's lesson, one level up: a report over nothing must say so,
        // because "0 missing" and "checked nothing" look identical in a
        // one-line PASS unless the count is in it.
        let report = judge(&[], |_| true);
        assert!(!report.has_failures());
        assert_eq!(report.checked, 0);
        assert!(report.to_report().contains("0 citation"));
    }

    // --------------------------------------------------------- TC-095/096

    #[test]
    fn the_real_design_documents_have_no_unmarked_missing_paths() {
        // F-255. This test previously asserted that the real tree still had
        // F-239's five missing paths, to "prove it can fail before trusting
        // it" (F-58, F-27). That proof was real but it was pinned to a
        // transient property of main: task #7 marked those five *(planned)*
        // and the assertion went red -- the fix for the defect destroyed the
        // fixture for the detector, which is the whole of F-255. Its own
        // comment predicted it would "start passing with zero missing"; it
        // started failing instead, because the assertion demanded the five.
        //
        // The can-it-fail proof belongs on a synthetic fixture, where it is
        // deterministic and survives every future edit to the design docs --
        // see `a_missing_path_is_reported`, `a_planned_marker_exempts` and
        // the rest above, which cover all three arms including the one the
        // real tree never offered: that the exemption actually exempts.
        //
        // What is left for the real tree is the property that must hold from
        // now on, and it is worth asserting for its own sake: every path the
        // design documents cite either resolves or is marked planned.
        let report = check_paths(&repo_root()).expect("the real docs tree reads");
        assert!(
            !report.has_failures(),
            "the design documents cite a path that neither resolves nor is              marked *(planned)*:
{}",
            report.to_report()
        );
        // And the check must actually be looking at something -- F-27 one
        // level up, since "0 missing" and "checked nothing" are the same
        // one-line PASS unless the count is in it.
        assert!(
            report.checked > 10,
            "only {} citations found in the real design documents, which              suggests the extraction heuristic regressed rather than that              the documents stopped citing paths",
            report.checked
        );
    }

    #[test]
    fn the_check_finds_citations_in_more_than_one_document() {
        // Deleting the CLAUDE.md half of the scan, or the docs/03-design
        // walk, must fail this test rather than pass silently (F-27's
        // pattern: a mutation that removes half the coverage and nothing
        // notices).
        let files = scanned_files(&repo_root()).expect("scan");
        assert!(files.iter().any(|f| f.ends_with("CLAUDE.md")));
        assert!(files.iter().any(|f| f.ends_with("architecture.md")));
        assert!(
            files.len() > 5,
            "docs/03-design has more than five .md files"
        );
    }
}
