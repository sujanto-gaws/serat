//! The `.serat/` ignore offer (NFR-CMP-04; T-06's single named exception; task #19).
//!
//! Three operations, all on the **vault root's** ignore file and nothing else:
//! [`status`], [`append_entry`] and [`decline`]. `append_entry` takes no path:
//! the file, the line and the root are fixed here, so there is no argument an
//! attacker-influenced caller could aim at another file. It is the one write in
//! P1 that is not a Markdown note, and `Vault::write_atomic` correctly refuses
//! it (T-06), which is why this is its own small, auditable function.
//!
//! # Known gap, stated
//!
//! Only the vault root is examined. A vault that is a **subfolder of a larger
//! repository** has no `.git` at its root, so [`status`] says
//! [`GitignoreOffer::NotAGitRepo`] and no offer is made, even though the parent
//! repository will happily track `.serat/index.db`. Walking upwards would mean
//! reading files outside the vault, which no other part of the application does;
//! that trade is deliberate for P1 and recorded here and in the task report.
//! Likewise only the root `.gitignore` is read: an ignore rule in a parent
//! directory, a global excludes file or `.git/info/exclude` is not consulted, so
//! the offer may be shown for a directory already ignored by one of those.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ipc::{GitignoreAppended, GitignoreOffer, IpcError};

/// The entry appended: exactly this, with the file's own line ending after it.
pub(crate) const ENTRY: &str = "/.serat/";
const IGNORE_FILE: &str = ".gitignore";
const PREFS_FILE: &str = "prefs.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct Prefs {
    #[serde(default)]
    gitignore_offer_declined: bool,
}

fn io_err(path: &Path, err: &std::io::Error) -> IpcError {
    IpcError::Io {
        path: path.display().to_string(),
        message: err.to_string(),
    }
}

/// `.git` as a directory (an ordinary repository) **or a file** (a worktree or
/// submodule `gitdir:` pointer). A symlink does not count: it is not the
/// vault's own repository metadata.
fn is_git_repo(root: &Path) -> bool {
    fs::symlink_metadata(root.join(".git")).is_ok_and(|meta| meta.is_dir() || meta.is_file())
}

/// Whether any line of `text` ignores `.serat`: `.serat`, `.serat/`, `/.serat`
/// or `/.serat/`, once trailing whitespace and a CR are dropped. Comments and
/// negations (`!`) never match.
fn ignores_serat(text: &str) -> bool {
    text.lines().any(|line| {
        matches!(
            line.trim_end(),
            ".serat" | ".serat/" | "/.serat" | "/.serat/"
        )
    })
}

/// The file's own line ending: CRLF if the first line break is one, else LF.
fn line_ending(text: &str) -> &'static str {
    match text.find('\n') {
        Some(at) if at > 0 && text.as_bytes()[at - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

fn read_ignore_file(root: &Path) -> Result<Option<String>, IpcError> {
    let path = root.join(IGNORE_FILE);
    match fs::read(&path) {
        // Lossy only for the line scan; the write path below appends to the
        // original bytes and never re-encodes them.
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io_err(&path, &err)),
    }
}

fn prefs_path(root: &Path) -> PathBuf {
    root.join(".serat").join(PREFS_FILE)
}

fn read_prefs(root: &Path) -> Prefs {
    // A missing or unreadable prefs file only means the offer is shown again
    // (C-02: nothing depends on state that lives only in `.serat/`).
    fs::read(prefs_path(root))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// `gitignore_offer_status`.
#[must_use]
pub(crate) fn status(root: &Path) -> GitignoreOffer {
    if !is_git_repo(root) {
        return GitignoreOffer::NotAGitRepo;
    }
    if read_ignore_file(root)
        .ok()
        .flatten()
        .is_some_and(|text| ignores_serat(&text))
    {
        return GitignoreOffer::AlreadyIgnored;
    }
    if read_prefs(root).gitignore_offer_declined {
        return GitignoreOffer::Declined;
    }
    GitignoreOffer::Offer
}

/// Temp file in the same directory, flushed, then renamed over the target. The
/// original's permissions carry over, as `Vault::write_atomic`'s do (F-33).
fn replace_atomically(target: &Path, bytes: &[u8]) -> Result<(), IpcError> {
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    let name = target.file_name().map_or_else(
        || "file".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let temp = dir.join(format!(".serat-tmp-{}-{name}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Ok(meta) = fs::metadata(target) {
            let _ = fs::set_permissions(&temp, meta.permissions());
        }
        fs::rename(&temp, target)
    })();
    if let Err(err) = result {
        let _ = fs::remove_file(&temp);
        return Err(io_err(target, &err));
    }
    Ok(())
}

/// `append_gitignore_entry`. Idempotent; atomic; creates the file if absent.
///
/// # Errors
///
/// [`IpcError::Io`] if the root is not a repository, the ignore file is a
/// symlink (a rename would replace the link, not its target), or any step fails;
/// the original file is untouched on every failure.
pub(crate) fn append_entry(root: &Path) -> Result<GitignoreAppended, IpcError> {
    let target = root.join(IGNORE_FILE);
    if !is_git_repo(root) {
        return Err(IpcError::Io {
            path: target.display().to_string(),
            message: "the vault root is not a git repository".to_owned(),
        });
    }
    if fs::symlink_metadata(&target).is_ok_and(|meta| meta.is_symlink()) {
        return Err(IpcError::Io {
            path: target.display().to_string(),
            message: ".gitignore is a symlink; refusing to replace it".to_owned(),
        });
    }

    let original = match fs::read(&target) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(io_err(&target, &err)),
    };
    let text = original
        .as_deref()
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
    if text.as_deref().is_some_and(ignores_serat) {
        return Ok(GitignoreAppended {
            created: false,
            already_present: true,
        });
    }

    let eol = text.as_deref().map_or("\n", line_ending);
    let mut bytes = original.clone().unwrap_or_default();
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bytes.extend_from_slice(eol.as_bytes());
    }
    bytes.extend_from_slice(ENTRY.as_bytes());
    bytes.extend_from_slice(eol.as_bytes());

    replace_atomically(&target, &bytes)?;
    Ok(GitignoreAppended {
        created: original.is_none(),
        already_present: false,
    })
}

/// `decline_gitignore_offer`: records "Don't ask again" in `.serat/prefs.json`.
///
/// # Errors
///
/// [`IpcError::Io`] if `.serat/` cannot be created or the file written.
pub(crate) fn decline(root: &Path) -> Result<(), IpcError> {
    let path = prefs_path(root);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|err| io_err(dir, &err))?;
    }
    let prefs = Prefs {
        gitignore_offer_declined: true,
    };
    let bytes = serde_json::to_vec_pretty(&prefs).map_err(|err| IpcError::Io {
        path: path.display().to_string(),
        message: err.to_string(),
    })?;
    replace_atomically(&path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn root(name: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "serat-gitignore-{name}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn repo(name: &str) -> PathBuf {
        let dir = root(name);
        fs::create_dir(dir.join(".git")).unwrap();
        dir
    }

    fn contents(dir: &Path) -> String {
        fs::read_to_string(dir.join(".gitignore")).unwrap()
    }

    #[test]
    fn no_git_means_no_offer_and_no_write() {
        let dir = root("nogit");
        assert_eq!(status(&dir), GitignoreOffer::NotAGitRepo);
        assert!(append_entry(&dir).is_err());
        assert!(!dir.join(".gitignore").exists());
    }

    #[test]
    fn git_as_a_directory_or_a_file_both_count() {
        let dir = repo("gitdir");
        assert_eq!(status(&dir), GitignoreOffer::Offer);
        let file = root("gitfile");
        fs::write(file.join(".git"), "gitdir: ../elsewhere\n").unwrap();
        assert_eq!(status(&file), GitignoreOffer::Offer);
    }

    #[test]
    fn appends_exactly_the_entry_creating_the_file() {
        let dir = repo("create");
        let done = append_entry(&dir).unwrap();
        assert!(done.created && !done.already_present);
        assert_eq!(contents(&dir), "/.serat/\n");
        assert_eq!(status(&dir), GitignoreOffer::AlreadyIgnored);
    }

    #[test]
    fn is_idempotent_and_writes_nothing_the_second_time() {
        let dir = repo("idem");
        append_entry(&dir).unwrap();
        let before = fs::metadata(dir.join(".gitignore"))
            .unwrap()
            .modified()
            .unwrap();
        let again = append_entry(&dir).unwrap();
        assert!(again.already_present && !again.created);
        assert_eq!(contents(&dir), "/.serat/\n");
        assert_eq!(
            fs::metadata(dir.join(".gitignore"))
                .unwrap()
                .modified()
                .unwrap(),
            before
        );
    }

    #[test]
    fn existing_lf_content_is_preserved_and_a_missing_final_newline_is_added() {
        let dir = repo("lf");
        fs::write(dir.join(".gitignore"), "target\n*.log").unwrap();
        append_entry(&dir).unwrap();
        assert_eq!(contents(&dir), "target\n*.log\n/.serat/\n");
    }

    #[test]
    fn crlf_files_get_crlf() {
        let dir = repo("crlf");
        fs::write(dir.join(".gitignore"), "target\r\n*.log").unwrap();
        append_entry(&dir).unwrap();
        assert_eq!(contents(&dir), "target\r\n*.log\r\n/.serat/\r\n");
    }

    #[test]
    fn every_spelling_of_the_entry_counts_as_already_ignored() {
        for line in [".serat", ".serat/", "/.serat", "/.serat/", "/.serat/  "] {
            let dir = repo("spellings");
            fs::write(dir.join(".gitignore"), format!("a\r\n{line}\r\nb\r\n")).unwrap();
            assert_eq!(status(&dir), GitignoreOffer::AlreadyIgnored, "{line:?}");
            let done = append_entry(&dir).unwrap();
            assert!(done.already_present, "{line:?}");
        }
    }

    #[test]
    fn comments_and_negations_do_not_count() {
        let dir = repo("comment");
        fs::write(dir.join(".gitignore"), "# .serat/\n!/.serat/\n").unwrap();
        assert_eq!(status(&dir), GitignoreOffer::Offer);
    }

    #[test]
    fn non_utf8_bytes_already_in_the_file_survive_the_append() {
        let dir = repo("bytes");
        fs::write(dir.join(".gitignore"), b"caf\xe9\n").unwrap();
        append_entry(&dir).unwrap();
        assert_eq!(
            fs::read(dir.join(".gitignore")).unwrap(),
            b"caf\xe9\n/.serat/\n"
        );
    }

    #[test]
    fn declining_persists_and_never_touches_the_ignore_file() {
        let dir = repo("decline");
        decline(&dir).unwrap();
        assert_eq!(status(&dir), GitignoreOffer::Declined);
        assert!(!dir.join(".gitignore").exists());
        assert!(dir.join(".serat").join("prefs.json").exists());
    }

    #[test]
    fn losing_the_prefs_file_only_shows_the_offer_again() {
        let dir = repo("c02");
        decline(&dir).unwrap();
        fs::remove_dir_all(dir.join(".serat")).unwrap();
        assert_eq!(status(&dir), GitignoreOffer::Offer);
    }

    #[test]
    fn leaves_no_temp_file_behind() {
        let dir = repo("temp");
        append_entry(&dir).unwrap();
        let litter: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".serat-tmp-"))
            .collect();
        assert!(litter.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_gitignore_is_refused_and_its_target_untouched() {
        let dir = repo("symlink");
        let outside = root("symlink-target").join("real");
        fs::write(&outside, "keep\n").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join(".gitignore")).unwrap();
        assert!(append_entry(&dir).is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "keep\n");
    }

    #[cfg(unix)]
    #[test]
    fn permissions_of_an_existing_file_are_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = repo("perm");
        fs::write(dir.join(".gitignore"), "a\n").unwrap();
        fs::set_permissions(dir.join(".gitignore"), fs::Permissions::from_mode(0o600)).unwrap();
        append_entry(&dir).unwrap();
        let mode = fs::metadata(dir.join(".gitignore"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
