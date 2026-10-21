//! Task #45 (NFR-MNT-01, C-06): doclint must check the tree it is *run* in, not
//! the tree it was *built* in. A binary reused from a shared `CARGO_TARGET_DIR`
//! across worktrees used to read the builder's tree (code-reviewer-2's "88 vs 89
//! Musts"). Every test here runs the real binary against a temp copy.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_check-design-paths");
/// A path no checkout contains, so only the temp copy can mention it.
const GHOST: &str = "crates/ghost-crate-for-task-45/src/lib.rs";

/// A minimal repository: workspace manifest, CLAUDE.md, and one design doc that
/// cites a path which does not exist.
fn temp_repo() -> tempfile::TempDir {
    let t = tempfile::tempdir().expect("tempdir");
    let r = t.path();
    fs::write(r.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
    fs::write(r.join("CLAUDE.md"), "# stub\n").unwrap();
    fs::create_dir_all(r.join("docs/03-design")).unwrap();
    fs::write(
        r.join("docs/03-design/copy.md"),
        format!("The contract is `{GHOST}`.\n"),
    )
    .unwrap();
    t
}

fn run(cwd: &Path, args: &[&str], env_root: Option<&Path>) -> (Output, String) {
    let mut c = Command::new(BIN);
    c.current_dir(cwd).args(args).env_remove("SERAT_REPO_ROOT");
    if let Some(e) = env_root {
        c.env("SERAT_REPO_ROOT", e);
    }
    let out = c.output().expect("run check-design-paths");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    (out, text)
}

fn assert_read_the_copy(out: &Output, text: &str, copy: &Path) {
    assert!(
        !out.status.success(),
        "the copy cites a missing path, so the check must fail:\n{text}"
    );
    assert!(
        text.contains(GHOST),
        "report must name the copy's path:\n{text}"
    );
    assert!(
        // The copy holds one citation in a file only the copy has; "1 checked"
        // plus that file name rules out the builder's own tree.
        text.contains("docs/03-design/copy.md") && text.contains("of 1 checked"),
        "report must cite the copy's own file (root {}):\n{text}",
        copy.display()
    );
}

#[test]
fn runs_in_a_temp_copy_and_reads_that_copy_from_the_working_directory() {
    let t = temp_repo();
    // From a subdirectory: the walk-up must find the copy's workspace manifest.
    let sub = t.path().join("docs/03-design");
    let (out, text) = run(&sub, &[], None);
    assert_read_the_copy(&out, &text, t.path());
}

#[test]
fn explicit_root_flag_wins_over_the_working_directory() {
    let t = temp_repo();
    let elsewhere = tempfile::tempdir().unwrap();
    let root = t.path().display().to_string();
    let (out, text) = run(elsewhere.path(), &["--root", &root], None);
    assert_read_the_copy(&out, &text, t.path());
    let eq = format!("--root={root}");
    let (out, text) = run(elsewhere.path(), &[&eq], None);
    assert_read_the_copy(&out, &text, t.path());
}

#[test]
fn environment_variable_names_the_root() {
    let t = temp_repo();
    let elsewhere = tempfile::tempdir().unwrap();
    let (out, text) = run(elsewhere.path(), &[], Some(t.path()));
    assert_read_the_copy(&out, &text, t.path());
}

#[test]
fn resolution_precedence_and_fallback() {
    use doclint::{Rule, resolve_root};
    let s = |x: &str| x.to_string();
    let env = PathBuf::from("/env");
    let r = resolve_root(
        &[s("--root"), s("/flag")],
        Some(&env),
        Some(Path::new("/cwd")),
    )
    .unwrap();
    assert_eq!((r.path, r.rule), (PathBuf::from("/flag"), Rule::Flag));
    let r = resolve_root(&[], Some(&env), Some(Path::new("/cwd"))).unwrap();
    assert_eq!((r.path, r.rule), (env, Rule::Env));
    // No flag, no env, and a cwd with no workspace above it: the compiled root.
    let t = tempfile::tempdir().unwrap();
    let r = resolve_root(&[], None, Some(t.path())).unwrap();
    assert_eq!(r.rule, Rule::CompiledIn);
    assert!(
        r.path.join("bench/doclint/Cargo.toml").exists(),
        "fallback should be the build-time checkout, got {}",
        r.path.display()
    );
}

/// The first line of every bin's report names the root and the rule (task #54).
#[test]
fn the_first_line_names_the_root_and_the_rule_that_chose_it() {
    let t = temp_repo();
    let (_, text) = run(t.path(), &[], None);
    let first = text.lines().next().unwrap_or("");
    assert!(
        first.starts_with("root: ") && first.contains("[workspace] Cargo.toml above"),
        "first line: {first:?}"
    );
    let root = t.path().display().to_string();
    let (_, text) = run(t.path(), &["--root", &root], None);
    assert!(text.lines().next().unwrap().ends_with("(from --root)"));
    let (_, text) = run(t.path(), &[], Some(t.path()));
    assert!(
        text.lines()
            .next()
            .unwrap()
            .ends_with("(from SERAT_REPO_ROOT)")
    );
}

/// With no `[workspace]` above the working directory the compiled-in checkout is
/// used; that must be said on the first line, not discovered by reading the output.
#[test]
fn the_compiled_in_fallback_is_announced() {
    let elsewhere = tempfile::tempdir().unwrap();
    let (_, text) = run(elsewhere.path(), &[], None);
    let first = text.lines().next().unwrap_or("");
    assert!(
        first.starts_with("root: ") && first.contains("compiled-in fallback"),
        "first line: {first:?}"
    );
}

#[test]
fn a_bare_root_flag_is_an_error_not_a_silent_fallback() {
    let t = temp_repo();
    for args in [&["--root"][..], &["--root="][..]] {
        let (out, text) = run(t.path(), args, None);
        assert!(!out.status.success(), "{args:?} must fail:\n{text}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("--root needs a directory"), "{args:?}: {err}");
    }
}
