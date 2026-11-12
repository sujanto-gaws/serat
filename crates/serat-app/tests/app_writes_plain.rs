//! Everything `serat-app` writes on its own is plain, Obsidian-acceptable text
//! (NFR-CMP-02, NFR-CMP-01, C-01, C-02; ADR-0001).
//!
//! `serat-vault/tests/obsidian_untouched.rs` proves it for the vault layer. This is the
//! same question one layer up, through `AppState`'s own commands: create a note, create
//! a folder, save a patch, take and decline the `.gitignore` offer, let the index build,
//! write the settings file. Then, on a vault that has an `.obsidian/` directory, a
//! `.git/` directory, a CRLF `.gitignore` and a CRLF note:
//!
//! 1. outside `.serat/`, the only paths that changed are the ones the test asked for;
//!    `.obsidian/`, `.git/` and every other file are byte-identical;
//! 2. every written file outside `.serat/` is valid UTF-8, has no byte order mark the
//!    vault did not have, and keeps the vault's own line endings (no bare `\n` in a CRLF
//!    file);
//! 3. a created note is exactly the bytes the caller sent: no Serat-only frontmatter key,
//!    no marker comment, nothing the caller did not type;
//! 4. what is new outside `.serat/` is a note, a folder or the `.gitignore` entry: nothing
//!    Obsidian would not open;
//! 5. `.serat/` holds a SQLite database (and its WAL files) and UTF-8 JSON, and the vault
//!    with `.serat/` deleted opens and indexes again with every note byte-identical
//!    (the index is disposable, C-02);
//! 6. the settings file, which lives outside the vault, is UTF-8 TOML that parses back.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serat_app::cli::Boot;
use serat_app::ipc::{
    CreateFolderArgs, GitignoreOffer, IndexState, OpenVaultArgs, Patch, Pos, SavePatchArgs,
    WriteFileArgs,
};
use serat_app::settings::Settings;
use serat_app::startup::Startup;
use serat_app::state::AppState;

type Tree = BTreeMap<String, Vec<u8>>;

fn put(root: &Path, relative: &str, bytes: &[u8]) {
    let target = root.join(relative);
    fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
    fs::write(target, bytes).expect("write");
}

/// Every file under `root` (directories as empty-valued `name/` keys), `/`-separated.
fn snapshot(root: &Path) -> Tree {
    fn go(root: &Path, dir: &Path, out: &mut Tree) {
        for entry in fs::read_dir(dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            let rel = path
                .strip_prefix(root)
                .expect("inside")
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                out.insert(format!("{rel}/"), Vec::new());
                go(root, &path, out);
            } else {
                out.insert(rel, fs::read(&path).expect("read"));
            }
        }
    }
    let mut out = Tree::new();
    go(root, root, &mut out);
    out
}

fn new_state() -> AppState {
    AppState::with_settings(
        Arc::new(Startup::begin()),
        Boot::default(),
        Settings::default(),
    )
}

fn wait_ready(state: &AppState) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if matches!(state.index_status().state, IndexState::Ready { .. }) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the index never became ready: {:?}", state.index_status());
}

fn create_note(state: &AppState, path: &str, text: &str) {
    state
        .write_file(WriteFileArgs {
            path: path.to_owned(),
            text: text.to_owned(),
            base_hash: None,
        })
        .unwrap_or_else(|e| panic!("create {path}: {e:?}"));
}

/// Words and shapes only Serat would put in a note. Obsidian would show them as content.
fn serat_only_markup(text: &str) -> Option<String> {
    for line in text.lines() {
        let lower = line.trim_start().to_ascii_lowercase();
        let hit = lower.starts_with("serat")
            || lower.starts_with("<!-- serat")
            || lower.starts_with("<!--serat")
            || lower.starts_with("%% serat")
            || lower.starts_with("%%serat")
            || lower.contains("serat_id")
            || lower.contains("serat-id");
        if hit {
            return Some(line.to_owned());
        }
    }
    None
}

#[test]
fn what_serat_app_writes_is_plain_utf8_in_the_vaults_own_line_endings_and_nothing_obsidian_would_refuse()
 {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("vault");
    put(&root, ".obsidian/app.json", b"{\"legacyEditor\":false}\n");
    put(&root, ".obsidian/workspace.json", b"{\"main\":{}}");
    put(&root, ".git/HEAD", b"ref: refs/heads/main\n");
    put(&root, ".gitignore", b"node_modules\r\ntarget\r\n");
    put(&root, "Inbox.md", b"# Inbox\r\nline two\r\nline three\r\n");
    put(
        &root,
        "Projects/Alpha.md",
        b"---\ntags: [a]\n---\nbody [[Inbox]]\n",
    );
    put(&root, "Projects/spec.pdf", b"%PDF-1.4 not really");
    let before = snapshot(&root);

    let state = new_state();
    state
        .open_vault(OpenVaultArgs {
            root: root.display().to_string(),
        })
        .expect("open");
    wait_ready(&state);

    // Self-originated writes.
    state
        .create_folder(CreateFolderArgs {
            path: "Notes".to_owned(),
        })
        .expect("create folder");
    create_note(&state, "Untitled.md", "");
    create_note(&state, "Notes/Idea.md", "# Idea\r\nunicode: café — ok\r\n");
    let inbox = fs::read(root.join("Inbox.md")).expect("inbox");
    let hash = serat_vault::hash_bytes(&inbox).to_hex();
    state
        .save_patch(&SavePatchArgs {
            path: "Inbox.md".to_owned(),
            base_hash: hash,
            patches: vec![Patch {
                from: Pos { line: 1, col: 0 },
                to: Pos { line: 1, col: 4 },
                insert: "LINE".to_owned(),
            }],
        })
        .expect("save patch");
    assert!(matches!(
        state.gitignore_offer_status().expect("offer"),
        GitignoreOffer::Offer
    ));
    state.decline_gitignore_offer().expect("decline");
    state.append_gitignore_entry().expect("append");

    // Let the index settle, then close cleanly so `.serat/` is in its final shape.
    wait_ready(&state);
    state.close_vault();
    let after = snapshot(&root);

    // 1. Outside .serat/, only the asked-for paths changed.
    let outside = |p: &String| !p.starts_with(".serat");
    let allowed_new = ["Notes/", "Notes/Idea.md", "Untitled.md"];
    let allowed_changed = [".gitignore", "Inbox.md"];
    for (path, bytes) in before.iter().filter(|(p, _)| outside(p)) {
        let now = after.get(path).unwrap_or_else(|| panic!("{path} vanished"));
        if !allowed_changed.contains(&path.as_str()) {
            assert_eq!(now, bytes, "{path} changed");
        }
    }
    for path in after.keys().filter(|p| outside(p)) {
        assert!(
            before.contains_key(path) || allowed_new.contains(&path.as_str()),
            "an unexpected path appeared outside .serat/: {path}"
        );
    }

    // 2. Plain UTF-8, no BOM the vault lacked, the vault's own line endings.
    for path in ["Inbox.md", ".gitignore", "Untitled.md", "Notes/Idea.md"] {
        let bytes = &after[path];
        let text = std::str::from_utf8(bytes).unwrap_or_else(|_| panic!("{path} is not UTF-8"));
        assert!(!bytes.starts_with(b"\xEF\xBB\xBF"), "{path} gained a BOM");
        assert!(!text.contains('\0'), "{path} holds a NUL");
    }
    assert_eq!(after["Inbox.md"], b"# Inbox\r\nLINE two\r\nline three\r\n");
    let gitignore = std::str::from_utf8(&after[".gitignore"]).expect("utf-8");
    assert!(
        gitignore.starts_with("node_modules\r\ntarget\r\n"),
        "the user's lines moved: {gitignore:?}"
    );
    assert_eq!(
        gitignore.matches('\n').count(),
        gitignore.matches("\r\n").count(),
        "a bare LF in a CRLF .gitignore: {gitignore:?}"
    );
    assert!(gitignore.contains("/.serat/"), "{gitignore:?}");

    // 3. A created note is exactly what the caller sent.
    assert_eq!(after["Untitled.md"], b"");
    assert_eq!(
        after["Notes/Idea.md"],
        "# Idea\r\nunicode: café — ok\r\n".as_bytes()
    );
    for (path, bytes) in after
        .iter()
        .filter(|(p, _)| p.ends_with(".md") && outside(p))
    {
        if let Some(line) = serat_only_markup(std::str::from_utf8(bytes).expect("utf-8")) {
            panic!("{path} carries Serat-only markup: {line:?}");
        }
    }

    // 4. Nothing new outside .serat/ but notes, folders and the .gitignore.
    for path in after
        .keys()
        .filter(|p| outside(p) && !before.contains_key(*p))
    {
        assert!(path.ends_with('/') || path.ends_with(".md"), "{path}");
    }

    // 5. .serat/ is a database and JSON text; the vault opens again without it.
    let serat: Vec<&String> = after.keys().filter(|p| p.starts_with(".serat/")).collect();
    assert!(!serat.is_empty(), "no .serat/ was written at all");
    for path in &serat {
        if path.ends_with('/') {
            continue;
        }
        let bytes = &after[*path];
        if path.ends_with(".json") {
            let text = std::str::from_utf8(bytes).unwrap_or_else(|_| panic!("{path} not UTF-8"));
            serde_json::from_str::<serde_json::Value>(text)
                .unwrap_or_else(|e| panic!("{path} is not JSON: {e}"));
        } else {
            assert!(
                path.contains("index.db"),
                "an unexpected file in .serat/: {path}"
            );
        }
    }
    fs::remove_dir_all(root.join(".serat")).expect("delete the index");
    let notes_before = snapshot(&root);
    let again = new_state();
    again
        .open_vault(OpenVaultArgs {
            root: root.display().to_string(),
        })
        .expect("reopen without .serat/");
    wait_ready(&again);
    again.close_vault();
    let notes_after = snapshot(&root);
    for (path, bytes) in notes_before.iter().filter(|(p, _)| outside(p)) {
        assert_eq!(&notes_after[path], bytes, "{path} changed by the rebuild");
    }

    // 6. The settings file is UTF-8 TOML that parses back.
    let settings_path = dir
        .path()
        .join("config")
        .join("Serat")
        .join("settings.toml");
    let mut settings = Settings::default();
    settings.remember_open(&root, "Notes/Idea.md");
    settings.editor.live_preview = false;
    settings
        .keybindings
        .insert("switcher.open".to_owned(), "Mod-o".to_owned());
    settings.save_to(&settings_path).expect("save settings");
    let raw = fs::read(&settings_path).expect("settings");
    let text = std::str::from_utf8(&raw).expect("settings are UTF-8");
    assert!(!raw.starts_with(b"\xEF\xBB\xBF"));
    let parsed: toml::Table = text.parse().expect("settings are TOML");
    assert!(parsed.contains_key("editor"));
    assert!(
        !settings_path.starts_with(&root),
        "settings must not live in the vault (C-02)"
    );
}
