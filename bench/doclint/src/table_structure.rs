//! Checks that a Markdown table actually renders as a table.
//!
//! **P0 finding F-346/F-348 (task #49, #51).** `check-design-paths` and
//! `check-prd-join` both check that a document points at something real —
//! a path that resolves, an ID the other document also has. Neither checks
//! that the document can be *read*. Task #49 found the threat model's STRIDE
//! table broken below T-05 for seven commits across six passes, two of them
//! written specifically about that section: a stray unescaped `|` inside a
//! cell — `[[Target|Alias]]`, `file_name = target || '.md'`, a shell `||` —
//! splits a GFM table row at the pipe, silently truncating everything after
//! it and pushing every following cell one column left. `doclint` passed
//! throughout, correctly, because every cited path and every cited ID was
//! still there in the raw text; the table simply no longer rendered them
//! where a reader would look.
//!
//! Two rules, both about whether a table is *well-formed*, not about its
//! content:
//!
//! 1. Every row's unescaped-pipe count matches its header's. `\|` does not
//!    count — GFM splits on a raw `|` even inside inline code, so an escaped
//!    one is the only way to put a literal pipe in a cell.
//! 2. No line starting with `|` sits outside a table: a header row with no
//!    delimiter row beneath it, or a row-shaped line with prose immediately
//!    above and no blank line to separate them, is not a table to a GFM
//!    renderer even though it is one to a naive glance at the source.
//!
//! **What this does not cover.** It checks that a table renders as *a*
//! table — the right number of cells, in the right place — not that its
//! *content* is correct. A column count that has always matched proves
//! nothing about whether the header names the right thing.

use std::path::{Path, PathBuf};

/// One place a table does not render the way its source suggests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableProblem {
    pub file: PathBuf,
    /// 1-based line number.
    pub line: usize,
    pub reason: String,
}

/// Unescaped `|` count: GFM's column separator, `\|` is a literal pipe and
/// does not count.
fn pipes(line: &str) -> usize {
    line.replace("\\|", "").matches('|').count()
}

/// Whether `line` (already trimmed) is a delimiter row: starts with `|`, and
/// once every `|` and space is stripped, what remains is non-empty and made
/// only of `-` and `:`.
fn is_delimiter_row(line: &str) -> bool {
    if !line.starts_with('|') {
        return false;
    }
    let stripped: String = line.chars().filter(|&c| c != '|' && c != ' ').collect();
    !stripped.is_empty() && stripped.chars().all(|c| c == '-' || c == ':')
}

/// Check one document's text for table-structure problems.
#[must_use]
pub fn check_table_structure(text: &str, source_file: &Path) -> Vec<TableProblem> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut problems = Vec::new();
    let mut in_table = false;
    let mut width = 0usize;
    let mut prev_blank = true;

    for (idx, raw) in lines.iter().enumerate() {
        let s = raw.trim();
        let is_row = s.starts_with('|');
        let is_delim = is_delimiter_row(s);
        let line_no = idx + 1;

        if in_table {
            if is_row {
                if pipes(s) != width {
                    problems.push(TableProblem {
                        file: source_file.to_path_buf(),
                        line: line_no,
                        reason: format!("column count {}, header has {width}", pipes(s)),
                    });
                }
            } else if s.is_empty() {
                in_table = false;
            } else {
                // Prose immediately after a row, with no blank line between,
                // ends the table right there -- GFM does not look further.
                in_table = false;
                problems.push(TableProblem {
                    file: source_file.to_path_buf(),
                    line: line_no,
                    reason: "prose ends the table with no blank line".to_string(),
                });
            }
        } else if is_delim && idx >= 1 && lines[idx - 1].trim().starts_with('|') {
            in_table = true;
            width = pipes(lines[idx - 1].trim());
            if pipes(s) != width {
                problems.push(TableProblem {
                    file: source_file.to_path_buf(),
                    line: line_no,
                    reason: format!("delimiter has {} pipes, header {width}", pipes(s)),
                });
            }
        } else if is_row && !is_delim {
            // A row-shaped line that is not itself a delimiter and was not
            // reached from inside a table: it is only a real table header if
            // the very next line is a delimiter row.
            let next_is_delim = lines
                .get(idx + 1)
                .is_some_and(|next| is_delimiter_row(next.trim()));
            if !next_is_delim {
                let rendering = if prev_blank {
                    "literal text, not a table"
                } else {
                    "literal text continuing the paragraph above"
                };
                problems.push(TableProblem {
                    file: source_file.to_path_buf(),
                    line: line_no,
                    reason: format!("orphan row: renders as {rendering}"),
                });
            }
        }
        prev_blank = s.is_empty();
    }
    problems
}

/// Check every file in `files`.
#[must_use]
pub fn check_files(files: &[(PathBuf, String)]) -> Vec<TableProblem> {
    let mut out = Vec::new();
    for (path, text) in files {
        out.extend(check_table_structure(text, path));
    }
    out
}

/// Human-readable report.
#[must_use]
pub fn to_report(problems: &[TableProblem]) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    if problems.is_empty() {
        let _ = writeln!(s, "PASS  every table in the scanned documents renders as written");
    } else {
        let _ = writeln!(
            s,
            "FAIL  {} table-structure problem(s) found",
            problems.len()
        );
        for p in problems {
            let _ = writeln!(s, "  {}:{}: {}", p.file.display(), p.line, p.reason);
        }
    }
    s
}

/// Run the check against every `.md` file under `docs/` in the real
/// repository.
///
/// # Errors
/// A filesystem failure while walking or reading.
pub fn check_repo_docs(repo_root: &Path) -> Result<Vec<TableProblem>, String> {
    let mut files = Vec::new();
    collect_markdown(&repo_root.join("docs"), &mut files)
        .map_err(|e| format!("walking docs: {e}"))?;
    let mut with_text = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let rel = path.strip_prefix(repo_root).unwrap_or(&path);
        let rel = PathBuf::from(rel.to_string_lossy().replace('\\', "/"));
        with_text.push((rel, text));
    }
    Ok(check_files(&with_text))
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

    fn check(text: &str) -> Vec<(usize, String)> {
        check_table_structure(text, Path::new("x.md"))
            .into_iter()
            .map(|p| (p.line, p.reason))
            .collect()
    }

    #[test]
    fn a_well_formed_table_has_no_problems() {
        let text = "\
| A | B |
|---|---|
| 1 | 2 |
| 3 | 4 |
";
        assert_eq!(check(text), vec![]);
    }

    #[test]
    fn a_stray_unescaped_pipe_in_a_cell_is_caught() {
        // The exact shape task #49 found: `[[Target|Alias]]` inside a cell
        // splits the row, so the row has more pipes than the header.
        let text = "\
| A | B |
|---|---|
| [[Target|Alias]] | 2 |
";
        let problems = check(text);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].0, 3);
        assert!(problems[0].1.contains("column count"), "{:?}", problems[0]);
    }

    #[test]
    fn an_escaped_pipe_does_not_count() {
        let text = "\
| A | B |
|---|---|
| a \\| b | 2 |
";
        assert_eq!(check(text), vec![]);
    }

    #[test]
    fn a_shell_double_pipe_in_a_cell_is_caught() {
        // F-336/F-349's exact shape: `|| echo "no conflicts"` inside a cell.
        let text = "\
| Command | Effect |
|---|---|
| cmd || echo \"no conflicts\" | runs a fallback |
";
        let problems = check(text);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].1.contains("column count"));
    }

    #[test]
    fn a_header_with_no_delimiter_beneath_it_is_an_orphan_row() {
        let text = "\
Some prose.

| A | B |
Not a delimiter, just text.
";
        let problems = check(text);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].1.contains("orphan row"), "{:?}", problems[0]);
    }

    #[test]
    fn prose_immediately_after_a_table_with_no_blank_line_ends_it_and_is_flagged() {
        // T-06's exact shape: a heading or sentence directly below the last
        // row, no blank line, renders as loose text rather than a new
        // section or a continuation of the table.
        let text = "\
| A | B |
|---|---|
| 1 | 2 |
This sentence has nowhere to go.
";
        let problems = check(text);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].0, 4);
        assert!(problems[0].1.contains("no blank line"));
    }

    #[test]
    fn a_blank_line_after_a_table_ends_it_cleanly() {
        let text = "\
| A | B |
|---|---|
| 1 | 2 |

Ordinary prose, well separated.
";
        assert_eq!(check(text), vec![]);
    }

    #[test]
    fn a_delimiter_row_with_the_wrong_pipe_count_is_caught_at_the_delimiter() {
        let text = "\
| A | B |
|---|---|---|
| 1 | 2 |
";
        let problems = check(text);
        // The delimiter itself is flagged (3 pipes against a 2-column
        // header), and every row after inherits the header's width, not the
        // delimiter's, so the mismatched delimiter is the single finding.
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].0, 2);
        assert!(problems[0].1.contains("delimiter"));
    }

    #[test]
    fn a_table_row_that_is_not_orphaned_because_the_next_line_is_the_delimiter_is_fine() {
        let text = "| A | B |\n|---|---|\n";
        assert_eq!(check(text), vec![]);
    }

    // ---------------------------------------------------------- F-58/F-27

    #[test]
    fn the_check_can_fail_and_the_real_docs_tree_is_clean() {
        // Prove it can fail (F-58, F-27) on a synthetic fixture, then assert
        // the property against the real tree -- the same split this crate's
        // other two checks already use, forced by F-255's lesson: a
        // real-tree proof-of-failure breaks the day someone fixes the thing
        // it was pinned to.
        let broken = "| A | B |\n| x|y |\n";
        assert!(!check_table_structure(broken, Path::new("x.md")).is_empty());

        let problems = check_repo_docs(&crate::repo_root()).expect("docs tree reads");
        assert!(
            problems.is_empty(),
            "table-structure problems in the real docs tree:\n{}",
            to_report(&problems)
        );
    }
}
