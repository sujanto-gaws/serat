//! Repository root, resolved **at run time** (tasks #45, #54; NFR-MNT-01, C-06).
//!
//! The old rule was `env!("CARGO_MANIFEST_DIR")`, fixed when the binary is built.
//! Several worktrees sharing one `CARGO_TARGET_DIR` reuse one binary, so a tool
//! silently read (or, for vaultgen, wrote into) whichever tree built it.
//! Precedence:
//!
//! 1. `--root <dir>` / `--root=<dir>` on the command line;
//! 2. the `SERAT_REPO_ROOT` environment variable;
//! 3. the nearest ancestor of the current directory whose `Cargo.toml` declares
//!    a `[workspace]`;
//! 4. the compile-time manifest location, only as a last resort.
//!
//! The result carries the rule that picked it, so a tool can print it: rule 4
//! is the one that is wrong whenever the binary was reused from another
//! checkout, and it must not be silent.

use std::path::{Path, PathBuf};

/// Environment variable that names the repository root explicitly.
pub const ROOT_ENV: &str = "SERAT_REPO_ROOT";

/// Which rule picked the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    /// `--root` on the command line.
    Flag,
    /// `SERAT_REPO_ROOT`.
    Env,
    /// A `[workspace]` `Cargo.toml` found above the working directory.
    WorkspaceAbove,
    /// Nothing else applied; the checkout the binary was built from.
    CompiledIn,
}

impl Rule {
    /// A short phrase naming the rule, for the report line.
    #[must_use]
    pub fn describe(&self) -> &'static str {
        match self {
            Rule::Flag => "--root",
            Rule::Env => "SERAT_REPO_ROOT",
            Rule::WorkspaceAbove => "[workspace] Cargo.toml above the working directory",
            Rule::CompiledIn => {
                "compiled-in fallback: no [workspace] Cargo.toml above the working \
                 directory, so this is the checkout the binary was BUILT in"
            }
        }
    }
}

/// A resolved root and the rule that chose it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The directory chosen.
    pub path: PathBuf,
    /// Why it was chosen.
    pub rule: Rule,
}

impl Resolved {
    /// `root: <path> (from <rule>)` -- the first line of every tool's report.
    #[must_use]
    pub fn report_line(&self) -> String {
        format!(
            "root: {} (from {})",
            self.path.display(),
            self.rule.describe()
        )
    }
}

/// The compile-time root for a bench crate: its manifest dir is `<root>/bench/<name>`.
/// Callers pass `env!("CARGO_MANIFEST_DIR")`.
#[must_use]
pub fn compiled_root(manifest_dir: &str) -> PathBuf {
    Path::new(manifest_dir)
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// The pure form of [`from_process`]: each rule is testable without process state.
///
/// # Errors
/// `--root` with no value, or with an empty one. Silently ignoring it would let
/// a mistyped CI command check the wrong tree and pass.
pub fn resolve_root(
    args: &[String],
    env: Option<&Path>,
    cwd: Option<&Path>,
    compiled: &Path,
) -> Result<Resolved, String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let value = if a == "--root" {
            Some(it.next().map_or("", String::as_str))
        } else {
            a.strip_prefix("--root=")
        };
        if let Some(v) = value {
            if v.is_empty() {
                return Err("--root needs a directory".to_string());
            }
            return Ok(Resolved {
                path: PathBuf::from(v),
                rule: Rule::Flag,
            });
        }
    }
    if let Some(e) = env.filter(|p| !p.as_os_str().is_empty()) {
        return Ok(Resolved {
            path: e.to_path_buf(),
            rule: Rule::Env,
        });
    }
    if let Some(found) = cwd.and_then(workspace_root_above) {
        return Ok(Resolved {
            path: found,
            rule: Rule::WorkspaceAbove,
        });
    }
    Ok(Resolved {
        path: compiled.to_path_buf(),
        rule: Rule::CompiledIn,
    })
}

/// Resolve from the real command line, environment and working directory.
///
/// # Errors
/// As [`resolve_root`].
pub fn from_process(compiled: &Path) -> Result<Resolved, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = std::env::var_os(ROOT_ENV).map(PathBuf::from);
    let cwd = std::env::current_dir().ok();
    resolve_root(&args, env.as_deref(), cwd.as_deref(), compiled)
}

/// Nearest ancestor of `start` (inclusive) whose `Cargo.toml` has a `[workspace]` table.
fn workspace_root_above(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|dir| {
        let text = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
        text.lines()
            .any(|l| l.trim() == "[workspace]")
            .then(|| dir.to_path_buf())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> String {
        x.to_string()
    }

    #[test]
    fn precedence_flag_then_env_then_workspace_then_compiled() {
        let env = PathBuf::from("/env");
        let compiled = Path::new("/built");
        let r = resolve_root(
            &[s("--root"), s("/flag")],
            Some(&env),
            Some(Path::new("/c")),
            compiled,
        )
        .unwrap();
        assert_eq!((r.path, r.rule), (PathBuf::from("/flag"), Rule::Flag));
        let r = resolve_root(&[s("--root=/eq")], None, None, compiled).unwrap();
        assert_eq!(r.path, PathBuf::from("/eq"));
        let r = resolve_root(&[], Some(&env), Some(Path::new("/c")), compiled).unwrap();
        assert_eq!((r.path, r.rule), (env, Rule::Env));

        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        let sub = t.path().join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        let r = resolve_root(&[], None, Some(&sub), compiled).unwrap();
        assert_eq!(
            (r.path, r.rule),
            (t.path().to_path_buf(), Rule::WorkspaceAbove)
        );
    }

    #[test]
    fn no_workspace_above_names_the_compiled_in_fallback() {
        let t = tempfile::tempdir().unwrap();
        let r = resolve_root(&[], None, Some(t.path()), Path::new("/built")).unwrap();
        assert_eq!(r.path, Path::new("/built"));
        assert_eq!(r.rule, Rule::CompiledIn);
        let line = r.report_line();
        assert!(
            line.starts_with("root: /built (from compiled-in fallback"),
            "{line}"
        );
        assert!(line.contains("BUILT"), "{line}");
    }

    #[test]
    fn a_bare_or_empty_root_flag_is_an_error() {
        let c = Path::new("/built");
        assert!(resolve_root(&[s("--root")], None, None, c).is_err());
        assert!(resolve_root(&[s("--root=")], None, None, c).is_err());
        assert!(resolve_root(&[s("--root"), s("")], None, None, c).is_err());
    }

    #[test]
    fn compiled_root_is_two_levels_above_the_manifest_dir() {
        assert_eq!(compiled_root("/r/bench/budget"), PathBuf::from("/r"));
    }
}
