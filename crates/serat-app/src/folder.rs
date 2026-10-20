//! `create_folder` (FR-VLT-11, NFR-SEC-06, T-07): one directory, inside the vault, or
//! nothing.
//!
//! The containment rule is the contract's (`contracts/ipc.rs`, `CreateFolderArgs`):
//! after `create_dir` the new directory is canonicalised and must `starts_with` the
//! canonical root. A parent can be swapped for a symlink between `Vault::resolve` and
//! `create_dir`, and the directory would then land outside the vault.
//!
//! **`remove_dir` runs only on a directory this call created.** The `AlreadyExists`
//! branch checks containment and refuses, but never removes: if a parent was swapped for
//! a symlink to a place that already holds an empty directory of that name, that
//! directory is not Serat's to delete (design goal 1).
//!
//! Residue, stated rather than hidden: in the first case an empty directory exists
//! outside the vault for the moment before `remove_dir`.

use std::io::ErrorKind;
use std::path::Path;

use serat_core::VaultPath;
use serat_vault::Vault;

use crate::error::from_vault_error;
use crate::ipc::{FolderCreated, IpcError};

/// Create the directory at `path`. See the module note for the containment rule.
///
/// # Errors
///
/// [`IpcError::PathEscapesVault`] for a bad path, a symlinked component, or a directory
/// that landed outside the root; [`IpcError::NotWritable`] for a path in or under an
/// ignored directory (compared case-insensitively); [`IpcError::NotFound`] when the
/// parent does not exist; [`IpcError::Io`] for a file already at `path` or an OS refusal.
pub fn create_folder(vault: &Vault, raw: &str) -> Result<FolderCreated, IpcError> {
    create_folder_with(vault, raw, || {})
}

/// [`create_folder`] with a hook that runs after the resolve and before `create_dir`,
/// which is the window the hostile tests swap a parent for a symlink in.
///
/// # Errors
///
/// As [`create_folder`].
pub fn create_folder_with(
    vault: &Vault,
    raw: &str,
    between_resolve_and_create: impl FnOnce(),
) -> Result<FolderCreated, IpcError> {
    let path = VaultPath::new(raw).map_err(from_vault_error)?;

    // `is_ignored_dir` folds case, which is the point: on macOS and Windows
    // `.Obsidian/x` lands in `.obsidian/` (NFR-CMP-01), and `.serat/` is the index's.
    if path
        .as_str()
        .split('/')
        .any(|segment| vault.ignore_rules().is_ignored_dir(segment))
    {
        return Err(IpcError::NotWritable {
            path: path.to_string(),
        });
    }

    let target = vault.resolve(&path).map_err(from_vault_error)?;

    // The parent must exist: a typo must not create a chain of folders.
    let parent_is_dir = target.parent().is_some_and(is_dir);
    if !parent_is_dir {
        return Err(IpcError::NotFound {
            path: path.to_string(),
        });
    }

    between_resolve_and_create();

    let root = std::fs::canonicalize(vault.root()).map_err(|err| io(&path, &err))?;
    let escaped = || IpcError::PathEscapesVault {
        path: path.to_string(),
    };

    match std::fs::create_dir(&target) {
        Ok(()) => {
            // We made this directory, so we may unmake it.
            match std::fs::canonicalize(&target) {
                Ok(real) if real.starts_with(&root) => Ok(FolderCreated { created: true }),
                Ok(_) => {
                    let _ = std::fs::remove_dir(&target);
                    Err(escaped())
                }
                Err(err) => {
                    let _ = std::fs::remove_dir(&target);
                    Err(io(&path, &err))
                }
            }
        }
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            // Containment, and never a removal: this call did not create it.
            match std::fs::canonicalize(&target) {
                Ok(real) if real.starts_with(&root) => {
                    if is_dir(&real) {
                        Ok(FolderCreated { created: false })
                    } else {
                        Err(IpcError::Io {
                            path: path.to_string(),
                            message: "a file already exists at this path".to_owned(),
                        })
                    }
                }
                Ok(_) => Err(escaped()),
                Err(err) => Err(io(&path, &err)),
            }
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Err(IpcError::NotFound {
            path: path.to_string(),
        }),
        Err(err) => Err(io(&path, &err)),
    }
}

fn is_dir(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_dir())
}

fn io(path: &VaultPath, err: &std::io::Error) -> IpcError {
    IpcError::Io {
        path: path.to_string(),
        message: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dirs(name: &str) -> (PathBuf, PathBuf, Vault) {
        let base = std::env::temp_dir().join(format!("serat-folder-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("vault");
        let outside = base.join("outside");
        std::fs::create_dir_all(root.join("parent")).expect("fixture");
        std::fs::create_dir_all(&outside).expect("fixture");
        let vault = Vault::open(&root).expect("open");
        (root, outside, vault)
    }

    #[test]
    fn creates_one_directory_and_is_idempotent() {
        let (root, _outside, vault) = dirs("basic");
        assert_eq!(
            create_folder(&vault, "parent/new").unwrap(),
            FolderCreated { created: true }
        );
        assert!(root.join("parent/new").is_dir());
        assert_eq!(
            create_folder(&vault, "parent/new").unwrap(),
            FolderCreated { created: false }
        );
    }

    #[test]
    fn a_missing_parent_is_not_found_and_creates_no_chain() {
        let (root, _o, vault) = dirs("noparent");
        let err = create_folder(&vault, "nope/deeper").unwrap_err();
        assert!(matches!(err, IpcError::NotFound { .. }), "{err:?}");
        assert!(!root.join("nope").exists());
    }

    #[test]
    fn a_file_at_the_path_is_io() {
        let (root, _o, vault) = dirs("file");
        std::fs::write(root.join("parent/f"), b"x").unwrap();
        let err = create_folder(&vault, "parent/f").unwrap_err();
        assert!(matches!(err, IpcError::Io { .. }), "{err:?}");
    }

    #[test]
    fn ignored_directories_are_refused_case_insensitively() {
        let (root, _o, vault) = dirs("ignored");
        for p in [
            ".serat/x",
            ".Obsidian",
            ".GIT/x",
            "parent/.hidden",
            "Node_Modules",
        ] {
            let err = create_folder(&vault, p).unwrap_err();
            assert!(matches!(err, IpcError::NotWritable { .. }), "{p}: {err:?}");
        }
        assert!(!root.join(".Obsidian").exists());
    }

    #[test]
    fn escaping_paths_are_refused() {
        let (_r, _o, vault) = dirs("escape");
        for p in ["../x", "/abs", "a/../../x"] {
            let err = create_folder(&vault, p).unwrap_err();
            assert!(
                matches!(err, IpcError::PathEscapesVault { .. }),
                "{p}: {err:?}"
            );
        }
    }

    /// Swap `parent` (empty, inside the vault) for a symlink to `outside`, exactly in
    /// the window between the resolve and the create.
    #[cfg(unix)]
    fn swap_parent(root: &Path, outside: &Path) -> impl FnOnce() {
        let link = root.join("parent");
        let outside = outside.to_path_buf();
        move || {
            std::fs::remove_dir(&link).expect("parent was empty");
            std::os::unix::fs::symlink(&outside, &link).expect("symlink");
        }
    }

    #[cfg(unix)]
    #[test]
    fn hostile_a_swapped_parent_with_nothing_outside_is_refused_and_removed() {
        let (root, outside, vault) = dirs("hostile-a");
        let err = create_folder_with(&vault, "parent/new", swap_parent(&root, &outside))
            .expect_err("the directory landed outside the vault");
        assert!(matches!(err, IpcError::PathEscapesVault { .. }), "{err:?}");
        assert!(
            !outside.join("new").exists(),
            "the directory this call created outside the vault was not removed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hostile_a_swapped_parent_with_an_existing_outside_dir_is_refused_and_left_alone() {
        let (root, outside, vault) = dirs("hostile-b");
        std::fs::create_dir(outside.join("new")).unwrap();
        let err = create_folder_with(&vault, "parent/new", swap_parent(&root, &outside))
            .expect_err("the existing directory is outside the vault");
        assert!(matches!(err, IpcError::PathEscapesVault { .. }), "{err:?}");
        assert!(
            outside.join("new").is_dir(),
            "a directory this call did not create was removed (design goal 1)"
        );
    }
}
