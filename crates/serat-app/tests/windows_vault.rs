//! Windows-only vault checks the Linux runs cannot make (NFR-SEC-06, NFR-CMP-01,
//! FR-VLT-01; ADR-0017; tasks #72, #34, #35, #77).
//!
//! The whole file is `cfg(windows)`: on Linux and macOS it compiles to nothing, which is
//! the documented skip. It runs in the `workspace (windows-latest)` job and in the named
//! "Windows-only checks" step of `.github/workflows/ci.yml`.
//!
//! **Status: ready, awaiting runner (F-384).** Written by reading the Windows APIs, the
//! Linux twins in `serat-vault` and `serat-app`, and #72's `strip_verbatim`; compile-checked
//! against `x86_64-pc-windows-gnu`; never run. A failure on the first runner is a finding
//! about this file as much as about the product, and is reported as such.
//!
//! 1. Vault roots (MISSING-WIN-1, #72): a verbatim root, a UNC admin-share root, a `subst`
//!    drive root and a drive-relative root. `Vault::root()` and
//!    `serat_index::canonical_root` must agree in every spelling (F-458), neither may keep
//!    a `\\?\` prefix or the mangled `UNC\srv\share` #72 fixed, and the directory identity
//!    must be one value across spellings.
//! 2. NTFS names (#34, #35): the Linux-only `names_only_windows_would_refuse_are_indexed_on_
//!    this_platform` has this twin. It creates what NTFS allows, and (through `\\?\` paths,
//!    which bypass Win32 name rules) what Win32 tools refuse, and asserts the walk lists the
//!    first, drops none silently, and reports the second.
//! 3. OneDrive Files-On-Demand placeholders: designed here, `#[ignore]` until a runner with
//!    a OneDrive-synced folder exists (see the notes on each test).

#![cfg(windows)]

use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serat_core::VaultPath;
use serat_vault::Vault;

fn put(root: &Path, relative: &str, bytes: &[u8]) {
    let target = root.join(relative);
    fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
    fs::write(target, bytes).expect("write");
}

fn listed(vault: &Vault) -> Vec<String> {
    let mut out: Vec<String> = vault
        .walk()
        .expect("walk")
        .into_iter()
        .map(|entry| entry.path.to_string())
        .collect();
    out.sort();
    out
}

/// `\\?\` + the canonical form of `path`.
fn verbatim(path: &Path) -> PathBuf {
    let canonical = fs::canonicalize(path).expect("canonicalize");
    let text = canonical.to_string_lossy();
    if text.starts_with(r"\\?\") {
        canonical
    } else {
        PathBuf::from(format!(r"\\?\{text}"))
    }
}

/// What a vault opened at `spelling` must look like, checked the same way for each.
fn assert_one_vault(spelling: &Path, reference: &Vault, what: &str) {
    let vault = Vault::open(spelling).unwrap_or_else(|e| panic!("{what}: open: {e:?}"));
    let root = vault.root().to_string_lossy().into_owned();
    assert!(
        !root.starts_with(r"\\?\"),
        "{what}: verbatim prefix kept: {root}"
    );
    assert!(
        !root.starts_with("UNC\\"),
        "{what}: mangled UNC root (#72): {root}"
    );
    assert_eq!(
        serat_index::canonical_root(spelling),
        serat_index::canonical_root(reference.root()),
        "{what}: the index would see a different root"
    );
    assert_eq!(
        serat_index::canonical_root(spelling),
        root.trim_end_matches(['\\', '/']),
        "{what}: Vault::root and canonical_root disagree (F-458)"
    );
    assert_eq!(
        vault.identity(),
        reference.identity(),
        "{what}: identity differs"
    );
    assert_eq!(
        listed(&vault),
        listed(reference),
        "{what}: a different tree"
    );
    let note = VaultPath::new("Projects/a.md").expect("path");
    assert_eq!(
        vault.read(&note).expect("read").bytes,
        b"# a\n",
        "{what}: read through this root"
    );
    let absolute = vault.resolve(&note).expect("resolve");
    assert_eq!(
        vault.relativise(&absolute).expect("relativise"),
        note,
        "{what}"
    );
}

fn root_fixture() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().expect("tempdir");
    put(dir.path(), "Projects/a.md", b"# a\n");
    put(dir.path(), "b.md", b"# b\n");
    let vault = Vault::open(dir.path()).expect("open");
    (dir, vault)
}

// -- 1. vault roots ---------------------------------------------------------------

#[test]
fn a_verbatim_root_opens_as_the_plain_form_with_one_identity() {
    let (dir, reference) = root_fixture();
    assert_one_vault(&verbatim(dir.path()), &reference, r"\\?\C:\ root");
}

/// `\\localhost\C$\...` is the administrative share of the runner's own drive: a real UNC
/// path with no second machine. Skips, loudly, where administrative shares are off.
#[test]
fn a_unc_root_opens_as_a_unc_form_not_the_mangled_one() {
    let (dir, reference) = root_fixture();
    let canonical = fs::canonicalize(dir.path()).expect("canonical");
    let plain = canonical.to_string_lossy().replace(r"\\?\", "");
    let (drive, rest) = plain.split_at(2); // "C:" and "\Users\..."
    let unc = PathBuf::from(format!(r"\\localhost\{}${rest}", &drive[..1]));
    if fs::metadata(&unc).is_err() {
        eprintln!(
            "SKIPPED: {} is not reachable; administrative shares are disabled",
            unc.display()
        );
        return;
    }
    assert_one_vault(&unc, &reference, "UNC admin share");
    let verbatim_unc = PathBuf::from(format!(r"\\?\UNC\localhost\{}${rest}", &drive[..1]));
    assert_one_vault(&verbatim_unc, &reference, r"\\?\UNC\ admin share");
    let root = Vault::open(&unc)
        .expect("open")
        .root()
        .to_string_lossy()
        .into_owned();
    assert!(
        root.starts_with(r"\\"),
        "a UNC root keeps its two backslashes: {root}"
    );
}

/// A `subst` drive is a mapped drive that needs no network and no elevation. The ruling
/// in `identity.rs` is that `canonicalize` collapses it onto the real directory.
#[test]
fn a_mapped_drive_root_is_the_same_vault_as_the_directory_behind_it() {
    struct Subst(String);
    impl Drop for Subst {
        fn drop(&mut self) {
            let _ = std::process::Command::new("subst")
                .args([&self.0, "/d"])
                .status();
        }
    }
    let (dir, reference) = root_fixture();
    let Some(letter) = ('E'..='Z')
        .rev()
        .find(|l| !Path::new(&format!("{l}:\\")).exists())
    else {
        eprintln!("SKIPPED: every drive letter is in use");
        return;
    };
    let drive = format!("{letter}:");
    let made = std::process::Command::new("subst")
        .arg(&drive)
        .arg(dir.path())
        .status();
    if !made.is_ok_and(|s| s.success()) {
        eprintln!("SKIPPED: subst {drive} failed on this runner");
        return;
    }
    let _guard = Subst(drive.clone());
    assert_one_vault(
        Path::new(&format!("{drive}\\")),
        &reference,
        "subst drive root",
    );
}

/// `C:name` means "`name` in the current directory of drive C", a different place from
/// `C:\name`. A root spelled that way must open exactly what the OS resolves it to or fail;
/// it must never open some other directory. A name that exists nowhere is an error.
#[test]
fn a_drive_relative_root_that_names_nothing_is_an_error() {
    let result = Vault::open(r"C:serat-no-such-directory-0f3a9");
    assert!(
        result.is_err(),
        "opened a vault at a drive-relative root that does not exist"
    );
}

// -- 2. NTFS names ----------------------------------------------------------------

/// Win32 refuses these names; NTFS stores them, and a vault synced from another system
/// (or written by a `\\?\` tool) holds them. #34's rule is that the walk neither lists nor
/// silently drops them: each is reported, with a count, and the rest of the vault is intact.
const NTFS_ONLY: [&str; 3] = ["aux.md", "Misc./n.md", "trailing.md."];

/// Names NTFS allows and Obsidian vaults use, including ones Unix would also take.
const ALLOWED: [&str; 5] = [
    "Meeting 10-30.md",
    "with spaces/inner note.md",
    "Ünïcode — dash.md",
    "dots.in.name.md",
    "UPPER.MD",
];

fn create_refused(root: &Path) {
    let base = verbatim(root);
    for name in NTFS_ONLY {
        let target = base.join(name.replace('/', "\\"));
        fs::create_dir_all(target.parent().expect("parent")).expect("mkdir through \\\\?\\");
        fs::write(&target, b"# refused by Win32, stored by NTFS\n").expect("write through \\\\?\\");
    }
}

fn remove_refused(root: &Path) {
    // `tempfile`'s cleanup uses Win32 paths and cannot delete these; use the verbatim form.
    let _ = fs::remove_dir_all(verbatim(root));
}

#[test]
fn the_walk_lists_what_ntfs_allows_and_reports_what_win32_would_refuse() {
    let dir = tempfile::tempdir().expect("tempdir");
    for name in ALLOWED {
        put(dir.path(), name, b"# ok\n");
    }
    // An alternate data stream is a stream of `plain.md`, not a file: one entry, never two.
    put(dir.path(), "plain.md", b"# plain\n");
    fs::write(dir.path().join("plain.md:note"), b"stream body").expect("write stream");
    create_refused(dir.path());

    let vault = Vault::open(dir.path()).expect("open");
    let found = listed(&vault);

    let mut expect: Vec<String> = ALLOWED.iter().map(|s| (*s).to_owned()).collect();
    expect.push("plain.md".to_owned());
    expect.sort();
    assert_eq!(
        found, expect,
        "the walk listed a refused name, or dropped an allowed one"
    );

    let refused: Vec<String> = vault
        .refusals()
        .snapshot()
        .into_iter()
        .map(|r| r.path)
        .collect();
    for name in ["aux.md", "Misc.", "trailing.md."] {
        assert!(
            refused.iter().any(|path| path.contains(name)),
            "{name} was dropped without a report: {refused:?}"
        );
    }
    assert!(vault.refusals().len() >= 3, "{refused:?}");
    remove_refused(dir.path());
}

#[test]
fn relativise_refuses_what_ntfs_stores_and_win32_would_not_create() {
    let dir = tempfile::tempdir().expect("tempdir");
    create_refused(dir.path());
    let vault = Vault::open(dir.path()).expect("open");
    for name in ["aux.md", "trailing.md."] {
        let result = vault.relativise(&dir.path().join(name));
        assert!(
            matches!(result, Err(serat_core::VaultError::InvalidPath { .. })),
            "{name}: {result:?}"
        );
    }
    // The control: an ordinary neighbour still relativises.
    put(dir.path(), "ok.md", b"# ok\n");
    assert_eq!(
        vault
            .relativise(&dir.path().join("ok.md"))
            .expect("relativise")
            .to_string(),
        "ok.md"
    );
    remove_refused(dir.path());
}

// -- 3. OneDrive Files-On-Demand placeholders -------------------------------------
//
// A placeholder is a reparse point whose data is not on the disk. Win32 marks it with
// FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS (0x0040_0000), and some with RECALL_ON_OPEN
// (0x0004_0000) and OFFLINE (0x0000_1000). Reading its bytes makes OneDrive download the
// file, which on a metered or absent connection blocks, fails or costs money, and which a
// walk, a rebuild or a catch-up would do to *every* file in an online-only vault.
//
// Two properties, one of which is believed to hold today and one of which is known not to:
//
// * stat-only paths (`walk`, `stat`, the tree) never hydrate. Believed true: they call
//   `symlink_metadata` and `read_dir` only. Tested by `stat_paths_do_not_hydrate_a_placeholder`.
// * `Vault::read` of a placeholder must not hydrate *silently*. It does today
//   (`fs::read`); `asset::open_verified` documents that it deliberately follows reparse
//   points so that placeholders still open. No ADR rules on what the correct behaviour
//   is, so `reading_a_placeholder_is_reported_not_silent` states the weakest requirement
//   that satisfies "not silent" (an error, or a refusal entry naming the file) and is
//   EXPECTED TO FAIL until an architect ruling and an implementation exist.
//
// Both need a folder that OneDrive has synced and whose `placeholder.md` is "online only".
// Point SERAT_ONEDRIVE_PLACEHOLDER_VAULT at that folder and run with `--ignored`.

const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
const RECALL_ON_OPEN: u32 = 0x0004_0000;
const OFFLINE: u32 = 0x0000_1000;
const RECALL_BITS: u32 = RECALL_ON_DATA_ACCESS | RECALL_ON_OPEN | OFFLINE;

fn placeholder() -> Option<(PathBuf, VaultPath)> {
    let root = PathBuf::from(std::env::var_os("SERAT_ONEDRIVE_PLACEHOLDER_VAULT")?);
    let note = VaultPath::new("placeholder.md").expect("path");
    let attributes = fs::symlink_metadata(root.join("placeholder.md"))
        .ok()?
        .file_attributes();
    assert_ne!(
        attributes & RECALL_BITS,
        0,
        "placeholder.md is already hydrated (attributes {attributes:#x}); set it to \"free up space\" and rerun"
    );
    Some((root, note))
}

#[test]
#[ignore = "needs a OneDrive-synced folder with an online-only placeholder.md: SERAT_ONEDRIVE_PLACEHOLDER_VAULT"]
fn stat_paths_do_not_hydrate_a_placeholder() {
    let Some((root, note)) = placeholder() else {
        panic!("SERAT_ONEDRIVE_PLACEHOLDER_VAULT is not set");
    };
    let vault = Vault::open(&root).expect("open");
    assert!(
        listed(&vault).contains(&"placeholder.md".to_owned()),
        "the walk must list it"
    );
    vault.stat(&note).expect("stat");
    let after = fs::symlink_metadata(root.join("placeholder.md"))
        .expect("stat")
        .file_attributes();
    assert_ne!(
        after & RECALL_BITS,
        0,
        "walk or stat hydrated the placeholder (attributes now {after:#x})"
    );
}

#[test]
#[ignore = "needs a OneDrive placeholder (SERAT_ONEDRIVE_PLACEHOLDER_VAULT) and an architect ruling on placeholder policy; EXPECTED TO FAIL today"]
fn reading_a_placeholder_is_reported_not_silent() {
    let Some((root, note)) = placeholder() else {
        panic!("SERAT_ONEDRIVE_PLACEHOLDER_VAULT is not set");
    };
    let vault = Vault::open(&root).expect("open");
    let outcome = vault.read(&note);
    let after = fs::symlink_metadata(root.join("placeholder.md"))
        .expect("stat")
        .file_attributes();
    let hydrated = after & RECALL_BITS == 0;
    let reported = outcome.is_err()
        || vault
            .refusals()
            .snapshot()
            .iter()
            .any(|r| r.path.contains("placeholder.md"));
    assert!(
        !hydrated || reported,
        "Vault::read downloaded the file from OneDrive and said nothing"
    );
}
