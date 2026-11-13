//! No committed document may carry an unresolved merge-conflict marker
//! (NFR-MNT-01; F-444 the defect, F-255 the can-it-fail rule, F-27 the
//! scanned-count floor).
//!
//! **Twice in one day**, and the second time with the cause written down
//! after the first: a resolution script reported success for the edit it was
//! asked to make, the merge had conflicted in a *different* file, and the
//! "ok" was read as the merge being clean. The habit that would catch it —
//! *check the file, do not trust a tool's success message about a different
//! operation* — was identified after the first instance and not adopted
//! before the second.
//!
//! **That is the argument for a check rather than a habit.** A habit has to
//! be adopted by the person who is, at that moment, busy doing the thing that
//! produced the mistake. This costs nothing and cannot be forgotten, and it is
//! the same shape as `check-table-structure`: a document that no longer reads
//! the way its source suggests, found by looking rather than by remembering.
//!
//! **The class needs no judgement at all** — less than F-496's, which needed
//! three rules and a measurement to reach usable precision. A line beginning
//! `<<<<<<< `, `||||||| ` (the diff3 base), `=======` or `>>>>>>> ` is either a conflict or a document
//! *about* conflicts, and the second is excluded structurally: fenced code
//! blocks and indented blocks are skipped, which is where an example would
//! live.
//!
//! **What it does not cover.** Only Markdown that agents and people read as
//! instructions: `docs/**`, `CLAUDE.md`, `.claude/agents/*.md` and
//! `bench/**/README.md`. A conflict marker committed into Rust would
//! not compile and into a workflow would not parse, so those fail loudly on
//! their own; Markdown is the format where a marker is *invisible until
//! someone reads that paragraph*, which is exactly what happened both times.

use std::path::{Path, PathBuf};

/// One committed conflict marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkerProblem {
    /// Repo-relative path.
    pub file: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// The marker as found, trimmed.
    pub marker: String,
}

impl MarkerProblem {
    /// One line, in the shape the other checks use.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "{}:{}: unresolved conflict marker `{}`. The document does not read the way its \
             source suggests, and nothing else would have said so.",
            self.file.display(),
            self.line,
            self.marker
        )
    }
}

/// Whether a line opens or closes a fenced code block.
fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// Find every conflict marker in one document.
///
/// Markers inside fenced or indented code are **not** reported: a document
/// explaining a merge conflict shows one, and a check that cannot tell an
/// example from an accident is a check that gets disabled.
#[must_use]
pub fn check_text(file: &Path, text: &str) -> Vec<MarkerProblem> {
    let mut problems = Vec::new();
    let mut fenced = false;
    for (i, line) in text.lines().enumerate() {
        if is_fence(line) {
            fenced = !fenced;
            continue;
        }
        if fenced || line.starts_with("    ") || line.starts_with('\t') {
            continue;
        }
        let marker = if line.starts_with("<<<<<<< ") || line.trim_end() == "<<<<<<<" {
            "<<<<<<<"
        } else if line.starts_with("||||||| ") || line.trim_end() == "|||||||" {
            // The diff3 / zdiff3 base marker (`merge.conflictStyle`).
            "|||||||"
        } else if line.trim_end() == "=======" {
            "======="
        } else if line.starts_with(">>>>>>> ") || line.trim_end() == ">>>>>>>" {
            ">>>>>>>"
        } else {
            continue;
        };
        problems.push(MarkerProblem {
            file: file.to_path_buf(),
            line: i + 1,
            marker: marker.to_string(),
        });
    }
    problems
}

/// Run the check against every scanned document.
///
/// Returns the number of files scanned alongside the problems, so a caller can
/// tell "0 problems" from "checked nothing" (F-27).
///
/// # Errors
/// A filesystem failure while walking or reading.
pub fn check_repo_docs(repo_root: &Path) -> Result<(usize, Vec<MarkerProblem>), String> {
    let mut files = vec![repo_root.join("CLAUDE.md")];
    collect_markdown(&repo_root.join("docs"), &mut files)
        .map_err(|e| format!("walking docs: {e}"))?;
    // `.claude/agents/*.md` and `bench/**/README.md`: read as instructions too. Absent
    // directories are not an error (a temp repo in a test has neither).
    let agents = repo_root.join(".claude/agents");
    if agents.is_dir() {
        let mut found = Vec::new();
        collect_markdown(&agents, &mut found)
            .map_err(|e| format!("walking .claude/agents: {e}"))?;
        files.extend(found);
    }
    let bench = repo_root.join("bench");
    if bench.is_dir() {
        collect_readmes(&bench, &mut files).map_err(|e| format!("walking bench: {e}"))?;
    }
    files.sort();
    let scanned = files.len();
    let mut problems = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let rel = path.strip_prefix(repo_root).unwrap_or(&path);
        let rel = PathBuf::from(rel.to_string_lossy().replace('\\', "/"));
        problems.extend(check_text(&rel, &text));
    }
    Ok((scanned, problems))
}

/// `README.md` files under `bench/`, skipping generated and vendored trees (the
/// benchmark vaults hold tens of thousands of generated notes; `node_modules` and
/// `target` are not ours).
fn collect_readmes(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if matches!(name, "vaults" | "node_modules" | "target") || name.starts_with('.') {
                continue;
            }
            collect_readmes(&path, out)?;
        } else if path.file_name().is_some_and(|n| n == "README.md") {
            out.push(path);
        }
    }
    Ok(())
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

    fn check(text: &str) -> Vec<MarkerProblem> {
        check_text(Path::new("t.md"), text)
    }

    #[test]
    fn a_committed_conflict_is_caught_in_the_shape_it_actually_arrives() {
        // **Verbatim shape, and checked against the real event rather than
        // imagined.** Both of today's committed conflicts looked exactly like
        // this; I ran this check against ADR-0014 as it stood at `0660fa7^`
        // -- the second instance, before its fix -- and it reported the
        // markers. That verification is recorded here rather than left as a
        // test, because the only way to keep it as a test is a fixture that
        // can go missing, and a test that silently skips when its input is
        // absent is the guard-that-cannot-fail class this register has
        // thirteen of.
        let text = "Some prose.\n\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> task/x (subject)\n";
        let found = check(text);
        assert_eq!(found.len(), 3, "{found:?}");
        assert_eq!(found[0].line, 3);
        assert_eq!(found[0].marker, "<<<<<<<");
        assert_eq!(found[1].marker, "=======");
        assert_eq!(found[2].marker, ">>>>>>>");
        assert!(
            found[0]
                .describe()
                .contains("does not read the way its source suggests")
        );
    }

    #[test]
    fn the_diff3_base_marker_is_caught_too() {
        // `merge.conflictStyle = diff3`/`zdiff3` adds a `||||||| base` line between
        // ours and `=======`; a half-resolved diff3 conflict can leave only that.
        let text = "p\n<<<<<<< HEAD\nours\n||||||| merged common ancestors\nbase\n=======\ntheirs\n>>>>>>> x\n";
        let markers: Vec<String> = check(text).into_iter().map(|p| p.marker).collect();
        assert_eq!(markers, ["<<<<<<<", "|||||||", "=======", ">>>>>>>"]);
        assert_eq!(check("a\n|||||||\n").len(), 1, "bare base marker");
        // A table row that merely starts with a pipe is not one.
        assert!(check("| a | b |\n|---|---|\n").is_empty());
    }

    #[test]
    fn a_clean_document_reports_nothing() {
        assert!(check("Prose.\n\n- a\n- b\n\n| h |\n|---|\n| r |\n").is_empty());
    }

    #[test]
    fn a_setext_h1_underline_is_reported_as_a_marker_by_design() {
        // `=======` under a line is Markdown's setext H1, and it is the one
        // shape that would make this check fire on an honest document.
        // Reported anyway, deliberately: this project writes ATX headings
        // everywhere, a setext H1 in docs/ would be a style break worth
        // seeing, and the alternative -- looking at the line above to guess
        // intent -- is the judgement this class does not need.
        let found = check("Title\n=======\n");
        assert_eq!(
            found.len(),
            1,
            "the trade is stated in the test, not hidden"
        );
    }

    #[test]
    fn markers_inside_code_are_examples_and_are_left_alone() {
        // A document explaining a merge conflict shows one. A check that
        // cannot tell an example from an accident is one that gets disabled,
        // and then it is not there for the accident either.
        let fenced = "Prose.\n\n```\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> other\n```\n";
        assert!(check(fenced).is_empty(), "{:?}", check(fenced));
        let indented = "Prose.\n\n    <<<<<<< HEAD\n    =======\n";
        assert!(check(indented).is_empty());
    }

    #[test]
    fn a_marker_planted_in_a_scanned_tree_is_found_by_the_repo_walk() {
        // F-255: the fixtures above prove `check_text`; this proves the walk
        // reaches the files. A marker planted in docs/ (nested) and in
        // CLAUDE.md is reported with its repo-relative path and line, and the
        // untouched sibling is not.
        let t = tempfile::tempdir().expect("tempdir");
        let r = t.path();
        std::fs::create_dir_all(r.join("docs/03-design")).unwrap();
        std::fs::write(r.join("CLAUDE.md"), "# stub\n<<<<<<< HEAD\n").unwrap();
        std::fs::write(r.join("docs/clean.md"), "# clean\n").unwrap();
        std::fs::write(
            r.join("docs/03-design/planted.md"),
            "a\nb\n=======\n>>>>>>> x\n",
        )
        .unwrap();
        // The two other scanned places: agent definitions and bench READMEs. A non-README
        // file under bench/ and a README under bench/vaults/ are not scanned.
        std::fs::create_dir_all(r.join(".claude/agents")).unwrap();
        std::fs::write(r.join(".claude/agents/a.md"), "x\n<<<<<<< HEAD\n").unwrap();
        std::fs::create_dir_all(r.join("bench/sub")).unwrap();
        std::fs::write(r.join("bench/sub/README.md"), "x\n>>>>>>> t\n").unwrap();
        std::fs::write(r.join("bench/sub/NOTES.md"), "<<<<<<< HEAD\n").unwrap();
        std::fs::create_dir_all(r.join("bench/vaults/v")).unwrap();
        std::fs::write(r.join("bench/vaults/v/README.md"), "<<<<<<< HEAD\n").unwrap();
        let (scanned, problems) = check_repo_docs(r).expect("temp tree reads");
        assert_eq!(scanned, 5);
        let found: Vec<(String, usize)> = problems
            .iter()
            .map(|p| (p.file.display().to_string(), p.line))
            .collect();
        assert_eq!(
            found,
            vec![
                (".claude/agents/a.md".to_string(), 2),
                ("CLAUDE.md".to_string(), 2),
                ("bench/sub/README.md".to_string(), 2),
                ("docs/03-design/planted.md".to_string(), 3),
                ("docs/03-design/planted.md".to_string(), 4),
            ]
        );
    }

    #[test]
    fn the_real_docs_tree_is_clean_and_the_check_can_fail() {
        // F-255: the can-it-fail proof is on fixtures above; the real tree
        // gets a floor so "0 problems" cannot mean "scanned nothing" (F-27).
        let (scanned, problems) = check_repo_docs(&crate::repo_root()).expect("docs/ is readable");
        assert!(scanned > 20, "only {scanned} files scanned");
        assert!(
            problems.is_empty(),
            "{}",
            problems
                .iter()
                .map(MarkerProblem::describe)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
