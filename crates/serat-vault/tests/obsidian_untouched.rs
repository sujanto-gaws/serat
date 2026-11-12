//! An Obsidian vault opened and used for a session is left as it was, and every file
//! Serat writes is plain text (NFR-CMP-01, NFR-CMP-02, US-01, US-29; ADR-0008; C-01).
//!
//! The session: open the vault, walk it, read every note, create a note, save it whole,
//! patch it, patch an existing note, add the `.gitignore` entry and write the prefs
//! file. Afterwards:
//!
//! 1. every byte under `.obsidian/` and every file the session did not write is
//!    unchanged, and so is the modification time of the `.obsidian/` files;
//! 2. the only paths that differ from the starting tree are the ones the session was
//!    asked to write, plus `.serat/` (the disposable index directory, C-02);
//! 3. every file the session wrote is valid UTF-8 with no byte the caller did not give
//!    it: no wrapper, header or sidecar format (C-01).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serat_vault::{Edit, Position, Vault, hash_bytes};

fn put(root: &Path, relative: &str, bytes: &[u8]) {
    let target = root.join(relative);
    fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
    fs::write(target, bytes).expect("write");
}

/// Every file under `root` -> (bytes, mtime), keyed by relative path with `/`.
fn snapshot(root: &Path) -> BTreeMap<String, (Vec<u8>, std::time::SystemTime)> {
    fn go(root: &Path, dir: &Path, out: &mut BTreeMap<String, (Vec<u8>, std::time::SystemTime)>) {
        for entry in fs::read_dir(dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                go(root, &path, out);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .expect("inside")
                    .to_string_lossy()
                    .replace('\\', "/");
                let mtime = fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .expect("mtime");
                out.insert(rel, (fs::read(&path).expect("read"), mtime));
            }
        }
    }
    let mut out = BTreeMap::new();
    go(root, root, &mut out);
    out
}

fn vp(path: &str) -> serat_core::VaultPath {
    serat_core::VaultPath::new(path).expect("vault path")
}

#[test]
fn a_session_leaves_obsidian_config_and_untouched_files_byte_identical_and_writes_plain_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    put(root, ".obsidian/app.json", b"{\"legacyEditor\":false}\n");
    put(root, ".obsidian/workspace.json", b"{\"main\":{}}");
    put(root, ".obsidian/plugins/x/main.js", b"module.exports={}");
    put(root, ".obsidian/themes/t/theme.css", b"body{}");
    put(root, "Inbox.md", b"# Inbox\r\nline two\r\n");
    put(
        root,
        "Projects/Alpha.md",
        b"---\ntags: [a]\n---\nbody [[Inbox]]\n",
    );
    put(root, "Projects/spec.pdf", b"%PDF-1.4 not really");
    put(root, "Untouched.md", b"leave me\n");

    let before = snapshot(root);

    let vault = Vault::open(root).expect("open");
    for entry in vault.walk().expect("walk") {
        if entry.path.as_str().ends_with(".md") {
            vault.read(&entry.path).expect("read");
        }
    }
    // Create, save whole, patch.
    let created = vp("New note.md");
    let made = vault
        .write_atomic(&created, b"# New\nfirst\n")
        .expect("create");
    vault
        .write_patch(
            &created,
            made,
            &[Edit::new(
                Position::new(1, 5),
                Position::new(1, 5),
                " draft".to_owned(),
            )],
        )
        .expect("patch new");
    // Patch an existing CRLF note: one field, nothing else moves.
    let inbox_bytes = fs::read(root.join("Inbox.md")).expect("inbox");
    vault
        .write_patch(
            &vp("Inbox.md"),
            hash_bytes(&inbox_bytes),
            &[Edit::new(
                Position::new(1, 0),
                Position::new(1, 4),
                "LINE".to_owned(),
            )],
        )
        .expect("patch inbox");
    vault.append_gitignore_entry().expect("gitignore");
    vault
        .update_prefs(|_| Some(b"{\"x\":1}".to_vec()))
        .expect("prefs");

    let after = snapshot(root);

    // 1. Nothing under .obsidian/ and no file the session did not write changed.
    for (path, (bytes, mtime)) in &before {
        if path == "Inbox.md" {
            continue;
        }
        let (now, now_mtime) = after.get(path).unwrap_or_else(|| panic!("{path} vanished"));
        assert_eq!(now, bytes, "{path} changed");
        if path.starts_with(".obsidian/") {
            assert_eq!(now_mtime, mtime, "{path} was touched");
        }
    }

    // 2. The only new paths are the note, the gitignore and the disposable .serat/.
    let new_paths: Vec<&String> = after.keys().filter(|p| !before.contains_key(*p)).collect();
    for path in &new_paths {
        assert!(
            path.as_str() == "New note.md"
                || path.as_str() == ".gitignore"
                || path.starts_with(".serat/"),
            "an unexpected file appeared: {path}"
        );
    }

    // 3. Plain text, and exactly the bytes the caller gave (the patch is one range).
    assert_eq!(after["New note.md"].0, b"# New\nfirst draft\n");
    assert_eq!(after["Inbox.md"].0, b"# Inbox\r\nLINE two\r\n");
    assert_eq!(after[".gitignore"].0, b"/.serat/\n");
    for path in new_paths {
        std::str::from_utf8(&after[path].0).unwrap_or_else(|_| panic!("{path} is not UTF-8 text"));
    }
}
