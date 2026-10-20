//! The `.serat/` ignore offer (NFR-CMP-04, T-06).
//!
//! `append_gitignore_entry` is **the single named exception to T-06** (only Markdown is
//! writable): it takes no path, and writes exactly one line, `/.serat/`, to the
//! `.gitignore` at the vault root. Everything else here reads.
//!
//! # What counts as a repository, and the gap
//!
//! A repository is a `.git` entry **at the vault root, file or directory**: a worktree or
//! a submodule has a `.git` *file*, and an offer that ignored those would be wrong for
//! exactly the people who use them. **Stated gap:** only the vault root is looked at. A
//! vault that is a subfolder of a larger repository (the repository root is above it), or
//! one that contains nested repositories, reports [`GitignoreOffer::NotAGitRepo`], and the
//! `.gitignore` consulted is the vault root's own, not one higher up. Walking upward would
//! mean reading and writing outside the vault, which T-07 forbids.
//!
//! # The write
//!
//! Atomic (temp file, fsync, rename), the file's existing bytes untouched, and the added
//! line uses **the file's own line ending**: `\r\n` if the file's line breaks are mostly
//! `\r\n`, else `\n` (also the choice for a file with no line break, and for a new file).
//! A final line with no terminator gets one first. A `.gitignore` that is a symlink is
//! refused rather than replaced: a rename over a symlink would swap the user's link for a
//! regular file.
//!
//! "Don't ask again" lives in `.serat/prefs.json`, safe under C-02: losing it only shows
//! the offer again.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::ipc::{GitignoreAppended, GitignoreOffer, IpcError};

/// The line the offer adds.
pub const ENTRY: &str = "/.serat/";

const IGNORE_FILE: &str = ".gitignore";
const PREFS_FILE: &str = "prefs.json";
const DECLINED_KEY: &str = "gitignore_offer_declined";

/// Whether to show the offer for the vault rooted at `root`.
#[must_use]
pub fn status(root: &Path) -> GitignoreOffer {
    if std::fs::symlink_metadata(root.join(".git")).is_err() {
        return GitignoreOffer::NotAGitRepo;
    }
    if let Ok(bytes) = std::fs::read(root.join(IGNORE_FILE))
        && has_entry(&bytes)
    {
        return GitignoreOffer::AlreadyIgnored;
    }
    if declined(root) {
        return GitignoreOffer::Declined;
    }
    GitignoreOffer::Offer
}

/// Whether any line of `bytes` ignores `.serat` (`.serat`, `.serat/`, `/.serat`,
/// `/.serat/`). Trailing whitespace and `\r` are not part of a gitignore pattern.
fn has_entry(bytes: &[u8]) -> bool {
    String::from_utf8_lossy(bytes).lines().any(|line| {
        matches!(
            line.trim_end(),
            ".serat" | ".serat/" | "/.serat" | "/.serat/"
        )
    })
}

/// `\r\n` if the file's line breaks are mostly `\r\n`, else `\n`.
fn line_ending(bytes: &[u8]) -> &'static str {
    let crlf = bytes.windows(2).filter(|w| w == b"\r\n").count();
    let lf = bytes.iter().filter(|&&b| b == b'\n').count();
    if crlf > lf - crlf { "\r\n" } else { "\n" }
}

/// Append [`ENTRY`] to the vault root's `.gitignore`.
///
/// # Errors
///
/// [`IpcError::Io`] if the file is a symlink, cannot be read, or cannot be written.
pub fn append(root: &Path) -> Result<GitignoreAppended, IpcError> {
    let path = root.join(IGNORE_FILE);
    let io = |message: String| IpcError::Io {
        path: IGNORE_FILE.to_owned(),
        message,
    };

    let existing = match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(io(
                ".gitignore is a symbolic link, so it was not modified".to_owned()
            ));
        }
        Ok(_) => Some(std::fs::read(&path).map_err(|e| io(e.to_string()))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(io(e.to_string())),
    };

    if existing.as_deref().is_some_and(has_entry) {
        return Ok(GitignoreAppended {
            created: false,
            already_present: true,
        });
    }

    let mut bytes = existing.clone().unwrap_or_default();
    let ending = line_ending(&bytes);
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bytes.extend_from_slice(ending.as_bytes());
    }
    bytes.extend_from_slice(ENTRY.as_bytes());
    bytes.extend_from_slice(ending.as_bytes());

    write_atomic(&path, &bytes, existing.is_some()).map_err(|e| io(e.to_string()))?;
    Ok(GitignoreAppended {
        created: existing.is_none(),
        already_present: false,
    })
}

/// Record "Don't ask again" in `.serat/prefs.json`.
///
/// # Errors
///
/// [`IpcError::Io`] if `.serat` is a symlink or the file cannot be written.
pub fn decline(root: &Path) -> Result<(), IpcError> {
    let dir = root.join(".serat");
    let io = |message: String| IpcError::Io {
        path: ".serat/prefs.json".to_owned(),
        message,
    };
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(io(".serat is a symbolic link".to_owned()));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&dir).map_err(|e| io(e.to_string()))?;
        }
        Err(e) => return Err(io(e.to_string())),
    }
    // Preserve any other keys a later build wrote.
    let mut prefs = read_prefs(&dir.join(PREFS_FILE));
    prefs.insert(DECLINED_KEY.to_owned(), serde_json::Value::Bool(true));
    let text = serde_json::to_vec_pretty(&prefs).map_err(|e| io(e.to_string()))?;
    write_atomic(&dir.join(PREFS_FILE), &text, false).map_err(|e| io(e.to_string()))
}

fn read_prefs(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| match v {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

fn declined(root: &Path) -> bool {
    read_prefs(&root.join(".serat").join(PREFS_FILE))
        .get(DECLINED_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Temp file in the same directory, fsync, rename; permissions copied when replacing.
fn write_atomic(path: &Path, bytes: &[u8], replacing: bool) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let temp: PathBuf = dir.join(format!(".serat-tmp-{name}.{}", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if replacing {
            std::fs::set_permissions(&temp, std::fs::metadata(path)?.permissions())?;
        }
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("serat-gitignore-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture");
        dir
    }

    #[test]
    fn no_git_entry_means_no_offer() {
        assert_eq!(status(&root("none")), GitignoreOffer::NotAGitRepo);
    }

    #[test]
    fn a_git_directory_or_a_git_file_both_count() {
        let a = root("gitdir");
        std::fs::create_dir(a.join(".git")).unwrap();
        assert_eq!(status(&a), GitignoreOffer::Offer);
        let b = root("gitfile");
        std::fs::write(b.join(".git"), "gitdir: ../elsewhere\n").unwrap();
        assert_eq!(status(&b), GitignoreOffer::Offer);
    }

    #[test]
    fn the_four_spellings_are_already_ignored_and_near_misses_are_not() {
        for line in [".serat", ".serat/", "/.serat", "/.serat/"] {
            let r = root("spell");
            std::fs::create_dir(r.join(".git")).unwrap();
            std::fs::write(r.join(".gitignore"), format!("target\r\n{line}  \r\n")).unwrap();
            assert_eq!(status(&r), GitignoreOffer::AlreadyIgnored, "{line}");
        }
        let r = root("near");
        std::fs::create_dir(r.join(".git")).unwrap();
        std::fs::write(r.join(".gitignore"), "# /.serat/\n.serat-old\n!/.serat/\n").unwrap();
        assert_eq!(status(&r), GitignoreOffer::Offer);
    }

    #[test]
    fn append_creates_the_file_and_is_idempotent() {
        let r = root("create");
        std::fs::create_dir(r.join(".git")).unwrap();
        let first = append(&r).unwrap();
        assert!(first.created && !first.already_present);
        assert_eq!(
            std::fs::read_to_string(r.join(".gitignore")).unwrap(),
            "/.serat/\n"
        );
        let second = append(&r).unwrap();
        assert!(!second.created && second.already_present);
        assert_eq!(
            std::fs::read_to_string(r.join(".gitignore")).unwrap(),
            "/.serat/\n"
        );
        assert_eq!(status(&r), GitignoreOffer::AlreadyIgnored);
    }

    #[test]
    fn append_keeps_existing_bytes_and_uses_the_files_line_ending() {
        let r = root("crlf");
        std::fs::write(r.join(".gitignore"), b"target\r\n*.log").unwrap();
        append(&r).unwrap();
        assert_eq!(
            std::fs::read(r.join(".gitignore")).unwrap(),
            b"target\r\n*.log\r\n/.serat/\r\n"
        );

        let r = root("lf");
        std::fs::write(r.join(".gitignore"), b"target\n").unwrap();
        append(&r).unwrap();
        assert_eq!(
            std::fs::read(r.join(".gitignore")).unwrap(),
            b"target\n/.serat/\n"
        );

        // Mixed, mostly LF: LF wins.
        let r = root("mixed");
        std::fs::write(r.join(".gitignore"), b"a\r\nb\nc\nd\n").unwrap();
        append(&r).unwrap();
        assert!(
            std::fs::read(r.join(".gitignore"))
                .unwrap()
                .ends_with(b"d\n/.serat/\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_gitignore_is_refused_and_left_a_symlink() {
        let r = root("symlink");
        let target = r.join("real-ignore");
        std::fs::write(&target, "x\n").unwrap();
        std::os::unix::fs::symlink(&target, r.join(".gitignore")).unwrap();
        assert!(matches!(append(&r), Err(IpcError::Io { .. })));
        assert!(
            std::fs::symlink_metadata(r.join(".gitignore"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "x\n");
    }

    #[test]
    fn declining_is_remembered_in_prefs_json_and_writes_nothing_else() {
        let r = root("decline");
        std::fs::create_dir(r.join(".git")).unwrap();
        decline(&r).unwrap();
        assert_eq!(status(&r), GitignoreOffer::Declined);
        assert!(!r.join(".gitignore").exists());
        // Idempotent, and other keys survive.
        std::fs::write(
            r.join(".serat/prefs.json"),
            r#"{"future":1,"gitignore_offer_declined":true}"#,
        )
        .unwrap();
        decline(&r).unwrap();
        let text = std::fs::read_to_string(r.join(".serat/prefs.json")).unwrap();
        assert!(text.contains("future"), "{text}");
        // Losing the file only shows the offer again (C-02).
        std::fs::remove_file(r.join(".serat/prefs.json")).unwrap();
        assert_eq!(status(&r), GitignoreOffer::Offer);
    }

    #[test]
    fn already_ignored_wins_over_declined() {
        let r = root("both");
        std::fs::create_dir(r.join(".git")).unwrap();
        decline(&r).unwrap();
        append(&r).unwrap();
        assert_eq!(status(&r), GitignoreOffer::AlreadyIgnored);
    }
}
