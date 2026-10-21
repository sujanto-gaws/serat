//! The two named exceptions to T-06 (only Markdown is writable), and nothing else:
//! [`Vault::append_gitignore_entry`] and [`Vault::update_prefs`] (NFR-CMP-04,
//! NFR-SEC-06, FR-VLT-03; T-06, T-07; ADR-0008).
//!
//! Neither takes a path. The first writes the single line `/.serat/` to the vault
//! root's `.gitignore`; the second writes `.serat/prefs.json`. Both reuse the note write
//! path's temp-file, fsync, rename and permission handling from `atomic.rs`, hold the
//! same per-path in-process lock, and add the lost-update guard a file that other
//! programs edit needs: the file is read, a new version is built, and the file is read
//! **again immediately before the rename**. If it changed, the work is discarded and
//! redone once; a second collision is reported rather than overwriting someone's edit.
//! That narrows the window to the gap between the last read and the rename. It cannot
//! close it: a rename is not a compare-and-swap, and that residue is stated, not hidden.
//!
//! A symlink at either target (or a symlinked `.serat`) is refused rather than replaced:
//! a rename over a symlink would swap the user's link for a regular file.

use std::fs;
use std::path::Path;

use serat_core::VaultError;

use crate::{Vault, atomic};

/// The line the offer adds.
const ENTRY: &str = "/.serat/";
const IGNORE_FILE: &str = ".gitignore";
const SERAT_DIR: &str = ".serat";
const PREFS_LABEL: &str = ".serat/prefs.json";

/// What [`Vault::append_gitignore_entry`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitignoreAppended {
    /// The file did not exist and was created.
    pub created: bool,
    /// An existing line already ignored `.serat`; nothing was written.
    pub already_present: bool,
}

/// Whether any line of `bytes` ignores `.serat` (`.serat`, `.serat/`, `/.serat`,
/// `/.serat/`). Trailing whitespace and `\r` are not part of a gitignore pattern.
#[must_use]
pub fn gitignore_ignores_serat(bytes: &[u8]) -> bool {
    String::from_utf8_lossy(bytes).lines().any(|line| {
        matches!(
            line.trim_end(),
            ".serat" | ".serat/" | "/.serat" | "/.serat/"
        )
    })
}

/// `\r\n` if the file's line breaks are mostly `\r\n`, else `\n` (also the choice for a
/// file with no line break, and for a new file).
fn line_ending(bytes: &[u8]) -> &'static str {
    let crlf = bytes.windows(2).filter(|w| w == b"\r\n").count();
    let lf = bytes.iter().filter(|&&b| b == b'\n').count();
    if crlf > lf - crlf { "\r\n" } else { "\n" }
}

/// What a guarded replacement ended up doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// `build` returned `None`: nothing to write.
    Nothing,
    Created,
    Replaced,
}

impl Vault {
    /// Append `/.serat/` to the vault root's `.gitignore` (the one T-06 exception).
    ///
    /// Takes no path. Idempotent: any of the four spellings of the entry already there
    /// means no write. The file's existing bytes are untouched; the added line uses the
    /// file's own line ending, and an unterminated last line is terminated first.
    ///
    /// # Errors
    ///
    /// [`VaultError::AtomicWriteFailed`] if the file is a symlink, cannot be read or
    /// written, or was changed by another program twice while it was being updated.
    pub fn append_gitignore_entry(&self) -> Result<GitignoreAppended, VaultError> {
        self.append_gitignore_hooked(|_attempt| {})
    }

    /// [`Vault::append_gitignore_entry`] with a hook that runs after the temp file is
    /// written and before the lost-update check, which is the window a test mutates the
    /// file in.
    pub(crate) fn append_gitignore_hooked(
        &self,
        before_check: impl FnMut(u32),
    ) -> Result<GitignoreAppended, VaultError> {
        let target = self.root().join(IGNORE_FILE);
        let outcome = self.replace_guarded(
            &target,
            IGNORE_FILE,
            |existing| {
                if existing.is_some_and(gitignore_ignores_serat) {
                    return None;
                }
                let mut bytes = existing.map(<[u8]>::to_vec).unwrap_or_default();
                let ending = line_ending(&bytes);
                if !bytes.is_empty() && !bytes.ends_with(b"\n") {
                    bytes.extend_from_slice(ending.as_bytes());
                }
                bytes.extend_from_slice(ENTRY.as_bytes());
                bytes.extend_from_slice(ending.as_bytes());
                Some(bytes)
            },
            before_check,
        )?;
        Ok(GitignoreAppended {
            created: outcome == Outcome::Created,
            already_present: outcome == Outcome::Nothing,
        })
    }

    /// Read-modify-write `.serat/prefs.json` (the other T-06 exception; losing the file
    /// is safe under C-02).
    ///
    /// `update` receives the current bytes (`None` if the file is absent) and returns
    /// the complete new bytes, or `None` to leave the file alone. It may run twice if
    /// another program changed the file in between, so it must be a pure function of
    /// its input. `.serat/` is created if absent.
    ///
    /// # Errors
    ///
    /// [`VaultError::AtomicWriteFailed`] if `.serat` or `prefs.json` is a symlink or the
    /// file cannot be read or written.
    pub fn update_prefs(
        &self,
        update: impl FnMut(Option<&[u8]>) -> Option<Vec<u8>>,
    ) -> Result<(), VaultError> {
        let dir = self.root().join(SERAT_DIR);
        let failed = |message: String| VaultError::AtomicWriteFailed {
            path: PREFS_LABEL.to_owned(),
            message,
        };
        match fs::symlink_metadata(&dir) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(failed(".serat is a symbolic link".to_owned()));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&dir).map_err(|e| failed(e.to_string()))?;
            }
            Err(e) => return Err(failed(e.to_string())),
        }
        self.replace_guarded(&dir.join("prefs.json"), PREFS_LABEL, update, |_| {})
            .map(|_| ())
    }

    /// The shared body of both exceptions: lock, read, build, temp, re-read, rename.
    fn replace_guarded(
        &self,
        target: &Path,
        label: &str,
        mut build: impl FnMut(Option<&[u8]>) -> Option<Vec<u8>>,
        mut before_check: impl FnMut(u32),
    ) -> Result<Outcome, VaultError> {
        let failed = |message: String| VaultError::AtomicWriteFailed {
            path: label.to_owned(),
            message,
        };
        let parent = target
            .parent()
            .ok_or_else(|| failed("has no parent directory".to_owned()))?;
        let file_name = target
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());

        let lock = self.path_locks().lock_for(label);
        let _held = lock.hold();

        for attempt in 0..2 {
            let existing = read_current(target, label)?;
            let Some(bytes) = build(existing.as_deref()) else {
                return Ok(Outcome::Nothing);
            };
            let permissions = existing
                .as_ref()
                .and_then(|_| fs::metadata(target).ok())
                .map(|m| m.permissions());
            let temp = atomic::write_temp(parent, &file_name, &bytes, permissions)
                .map_err(|e| failed(e.to_string()))?;
            before_check(attempt);

            if matches!(read_current(target, label), Ok(ref now) if *now == existing) {
                atomic::commit(&temp, target).map_err(|e| failed(e.to_string()))?;
                return Ok(if existing.is_none() {
                    Outcome::Created
                } else {
                    Outcome::Replaced
                });
            }
            let _ = fs::remove_file(&temp);
        }
        Err(failed(format!(
            "{label} was changed by another program while it was being updated, so it was \
             not modified; try again"
        )))
    }
}

/// The file's bytes, `None` if absent. A symlink is refused, never followed.
fn read_current(path: &Path, label: &str) -> Result<Option<Vec<u8>>, VaultError> {
    let failed = |message: String| VaultError::AtomicWriteFailed {
        path: label.to_owned(),
        message,
    };
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(failed(format!(
            "{label} is a symbolic link, so it was not modified"
        ))),
        Ok(_) => fs::read(path).map(Some).map_err(|e| failed(e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(failed(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().expect("fixture");
        let vault = Vault::open(dir.path()).expect("open");
        (dir, vault)
    }

    fn ignore_path(v: &Vault) -> std::path::PathBuf {
        v.root().join(".gitignore")
    }

    fn leftovers(v: &Vault) -> Vec<String> {
        fs::read_dir(v.root())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".serat-tmp-"))
            .collect()
    }

    #[test]
    fn append_creates_the_file_and_is_idempotent() {
        let (_d, v) = vault();
        let first = v.append_gitignore_entry().unwrap();
        assert!(first.created && !first.already_present);
        assert_eq!(fs::read_to_string(ignore_path(&v)).unwrap(), "/.serat/\n");
        let second = v.append_gitignore_entry().unwrap();
        assert!(!second.created && second.already_present);
        assert_eq!(fs::read_to_string(ignore_path(&v)).unwrap(), "/.serat/\n");
    }

    #[test]
    fn append_keeps_existing_bytes_and_uses_the_files_line_ending() {
        for (before, after) in [
            (
                &b"target\r\n*.log"[..],
                &b"target\r\n*.log\r\n/.serat/\r\n"[..],
            ),
            (b"target\n", b"target\n/.serat/\n"),
            (b"target", b"target\n/.serat/\n"),
            // Mixed, mostly LF: LF wins.
            (b"a\r\nb\nc\nd\n", b"a\r\nb\nc\nd\n/.serat/\n"),
        ] {
            let (_d, v) = vault();
            fs::write(ignore_path(&v), before).unwrap();
            v.append_gitignore_entry().unwrap();
            assert_eq!(fs::read(ignore_path(&v)).unwrap(), after);
        }
    }

    #[test]
    fn the_four_spellings_are_already_present_and_near_misses_are_not() {
        for line in [".serat", ".serat/", "/.serat", "/.serat/"] {
            assert!(gitignore_ignores_serat(
                format!("target\r\n{line}  \r\n").as_bytes()
            ));
        }
        assert!(!gitignore_ignores_serat(
            b"# /.serat/\n.serat-old\n!/.serat/\n"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_gitignore_is_refused_and_left_a_symlink() {
        let (_d, v) = vault();
        let target = v.root().join("real-ignore");
        fs::write(&target, "x\n").unwrap();
        std::os::unix::fs::symlink(&target, ignore_path(&v)).unwrap();
        assert!(matches!(
            v.append_gitignore_entry(),
            Err(VaultError::AtomicWriteFailed { .. })
        ));
        assert!(
            fs::symlink_metadata(ignore_path(&v))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "x\n");
    }

    #[cfg(unix)]
    #[test]
    fn hostile_a_symlink_planted_at_a_guessed_temp_name_is_not_followed() {
        let (_d, v) = vault();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim");
        fs::write(&victim, "precious\n").unwrap();

        // The exclusive create itself refuses a planted link.
        let planted = v.root().join(".serat-tmp-.gitignore.guess");
        std::os::unix::fs::symlink(&victim, &planted).unwrap();
        assert!(atomic::write_exclusive(&planted, b"/.serat/\n").is_err());
        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious\n");

        // And links at the old predictable (pid.counter) names change nothing.
        for n in 0..4 {
            let old = v
                .root()
                .join(format!(".serat-tmp-.gitignore.{}.{n}", std::process::id()));
            std::os::unix::fs::symlink(&victim, &old).unwrap();
        }
        v.append_gitignore_entry().unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious\n");
        assert_eq!(fs::read_to_string(ignore_path(&v)).unwrap(), "/.serat/\n");
    }

    #[test]
    fn a_concurrent_edit_before_the_rename_is_not_lost() {
        let (_d, v) = vault();
        let ignore = ignore_path(&v);
        fs::write(&ignore, "target\n").unwrap();
        v.append_gitignore_hooked(|attempt| {
            if attempt == 0 {
                fs::write(&ignore, "target\n*.log\n").unwrap();
            }
        })
        .unwrap();
        assert_eq!(
            fs::read_to_string(&ignore).unwrap(),
            "target\n*.log\n/.serat/\n"
        );
        assert!(leftovers(&v).is_empty(), "{:?}", leftovers(&v));
    }

    #[test]
    fn two_collisions_report_and_leave_the_others_edit() {
        let (_d, v) = vault();
        let ignore = ignore_path(&v);
        fs::write(&ignore, "a\n").unwrap();
        let mut n = 0;
        let err = v
            .append_gitignore_hooked(|_| {
                n += 1;
                fs::write(&ignore, format!("edit {n}\n")).unwrap();
            })
            .expect_err("a second collision is reported");
        assert!(
            matches!(err, VaultError::AtomicWriteFailed { .. }),
            "{err:?}"
        );
        assert_eq!(fs::read_to_string(&ignore).unwrap(), "edit 2\n");
        assert!(leftovers(&v).is_empty());
    }

    #[test]
    fn a_file_created_by_someone_else_before_the_rename_is_not_overwritten() {
        let (_d, v) = vault();
        let ignore = ignore_path(&v);
        v.append_gitignore_hooked(|attempt| {
            if attempt == 0 {
                fs::write(&ignore, "theirs\n").unwrap();
            }
        })
        .unwrap();
        assert_eq!(fs::read_to_string(&ignore).unwrap(), "theirs\n/.serat/\n");
    }

    #[test]
    fn prefs_are_written_under_serat_and_other_keys_are_the_callers_to_keep() {
        let (_d, v) = vault();
        v.update_prefs(|old| {
            assert!(old.is_none());
            Some(b"{\"a\":1}".to_vec())
        })
        .unwrap();
        let path = v.root().join(".serat/prefs.json");
        assert_eq!(fs::read(&path).unwrap(), b"{\"a\":1}");
        v.update_prefs(|old| {
            assert_eq!(old, Some(&b"{\"a\":1}"[..]));
            None
        })
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"a\":1}");
        // Nothing but prefs.json appeared in .serat, and nothing at the root.
        assert_eq!(fs::read_dir(v.root().join(".serat")).unwrap().count(), 1);
        assert!(!ignore_path(&v).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_serat_dir_or_prefs_file_is_refused() {
        let (_d, v) = vault();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), v.root().join(".serat")).unwrap();
        assert!(v.update_prefs(|_| Some(b"{}".to_vec())).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);

        let (_d2, v2) = vault();
        fs::create_dir(v2.root().join(".serat")).unwrap();
        let victim = outside.path().join("victim");
        fs::write(&victim, "precious").unwrap();
        std::os::unix::fs::symlink(&victim, v2.root().join(".serat/prefs.json")).unwrap();
        assert!(v2.update_prefs(|_| Some(b"{}".to_vec())).is_err());
        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
    }
}
