//! Behavioural verification of ADR-0012's ACL grants (task #53, closing F-345 and
//! F-347).
//!
//! **`#[cfg(not(windows))]`'d out, not `#[ignore]`'d -- tried the gentler one first and
//! it does not work.** `#[ignore]` only stops the test *harness* from running a test;
//! it does not stop the *process* from starting, and the crash this file reproduces
//! (`STATUS_ENTRYPOINT_NOT_FOUND`, confirmed locally on `mock_app()` alone, no Serat
//! code touched at all) happens at the OS loader's DLL resolution, before `main` --
//! and therefore before the test harness, and therefore before any `#[ignore]` --
//! ever runs. An `#[ignore]`d version of this file still crashed the whole binary on
//! this machine; a `#[cfg]`'d-out one is the only thing that actually keeps
//! `cargo test --workspace` green on Windows. The cost is that this file is invisible
//! rather than skipped-with-a-reason on Windows -- recorded here, and in F-345's
//! correction, so that absence is legible rather than assumed.
//!
//! **What this answers, if it runs at all.** `acl_manifest.rs` proves the handler
//! list, the app manifest and the granted permissions agree with each other
//! textually; none of those three checks proves the ACL actually *rejects* an
//! unmanifested invoke once wired together, which is what ADR-0012's hand-run probe
//! settled instead, unautomated (F-345). This test is the automated form, built the
//! same way tauri's own `tauri::test` module doc example builds one: `mock_builder()`
//! (the `MockRuntime`) with the *real* `tauri::generate_context!()` -- the actual
//! `capabilities/default.json`, not a fixture -- and the same `generate_handler!` list
//! `lib.rs` registers. If CI's ubuntu-latest leg can build and run this file, that
//! answers F-345's open question ("CI's ubuntu leg may or may not close it") by
//! actually closing it there; if it also cannot, that is a much larger finding than
//! ADR-0012 -- no Tauri integration test could ever run automatically on this
//! project's CI -- and deserves recording on its own. Either way it exercises
//! `write_file`, the one granted command ADR-0012's hand-run probe never touched
//! (F-347).
#![cfg(not(windows))]

use std::sync::Arc;

use serde_json::{Value, json};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder};
use tauri::webview::InvokeRequest;
use tauri::{WebviewUrl, WebviewWindowBuilder};

use serat_app::cli::Boot;
use serat_app::commands;
use serat_app::ipc::OpenVaultArgs;
use serat_app::settings::Settings;
use serat_app::startup::Startup;
use serat_app::state::AppState;

/// A vault with one pre-existing note, for `read_file` to exercise. `write_file`
/// deliberately targets a *different*, not-yet-existing path -- see the test body for
/// why.
fn fixture_vault() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("serat-app-acl-behaviour-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("fixture dir");
    std::fs::write(dir.join("existing.md"), "# Existing\n").expect("fixture note");
    dir
}

fn invoke<W: AsRef<tauri::Webview<tauri::test::MockRuntime>>>(
    webview: &W,
    cmd: &str,
    body: Value,
) -> Result<tauri::ipc::InvokeResponseBody, Value> {
    get_ipc_response(
        webview,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: if cfg!(any(windows, target_os = "android")) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .unwrap(),
            body: InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
}

/// The ACL's own refusal message names the permission, e.g. "open_vault not allowed.
/// Permissions associated with this command: allow-open-vault" -- ADR-0012 quotes this
/// verbatim from the hand-run probe, so matching on it here is matching on the same
/// string that settled the ADR, not a paraphrase of it.
fn is_acl_rejection(err: &Value) -> bool {
    err.as_str().is_some_and(|s| s.contains("not allowed"))
}

#[test]
fn open_vault_is_refused_and_the_five_granted_commands_are_not() {
    let vault = fixture_vault();
    let state = Arc::new(AppState::with_settings(
        Arc::new(Startup::begin()),
        Boot::default(),
        Settings::default(),
    ));

    let app = mock_builder()
        .manage(Arc::clone(&state))
        .invoke_handler(tauri::generate_handler![
            commands::open_vault,
            commands::read_file,
            commands::write_file,
            commands::index_status,
            commands::mark_first_paint,
            commands::mark_vault_usable,
            commands::refused_names,
            commands::create_folder,
            commands::get_ui_settings,
            commands::set_live_preview,
            commands::set_keybinding,
            commands::search,
            commands::backlinks,
            commands::resolve_links,
            commands::note_anchors,
            commands::open_tree,
            commands::mark_tree,
            commands::note_opened,
            commands::note_closed,
            commands::save_patch,
            commands::request_external_diff,
            commands::rebase_buffer,
            commands::flush_ack,
            commands::gitignore_offer_status,
            commands::append_gitignore_entry,
            commands::decline_gitignore_offer,
        ])
        .build(tauri::generate_context!())
        .expect(
            "mock app builds with the real generated context (capabilities/default.json, \
             the real app manifest)",
        );

    // Label must be "main" -- capabilities/default.json's own `windows` selector, not
    // an arbitrary choice this test is free to make.
    let webview = WebviewWindowBuilder::new(&app, "main", WebviewUrl::default())
        .build()
        .expect("main window builds under the mock runtime");

    // open_vault: granted no permission at all (ADR-0012, F-342) -- refused by the ACL
    // layer before AppState::open_vault ever runs, regardless of arguments. The vault
    // path given is real and would succeed if the call reached the handler, so a
    // refusal here is the ACL's doing and not a coincidental argument failure.
    let open_vault_result = invoke(
        &webview,
        "open_vault",
        json!({ "args": { "root": vault.display().to_string() } }),
    );
    let err = open_vault_result.expect_err("open_vault must be refused, not merely fail");
    assert!(
        is_acl_rejection(&err),
        "open_vault should be refused by the ACL layer (ADR-0012, F-342); got: {err}"
    );

    // The five granted commands must not be refused by the ACL layer. The vault is
    // opened directly through AppState here, not through the (refused) open_vault
    // command, so read_file and write_file have something real to act on and a
    // domain-level failure is distinguishable from an ACL rejection.
    state
        .open_vault(OpenVaultArgs {
            root: vault.display().to_string(),
        })
        .expect("fixture vault opens");

    let read = invoke(
        &webview,
        "read_file",
        json!({ "args": { "path": "existing.md" } }),
    );
    assert!(
        !matches!(&read, Err(e) if is_acl_rejection(e)),
        "read_file was refused by the ACL layer: {read:?}"
    );
    let contents = read.expect("read_file must succeed once past the ACL");
    assert_eq!(
        contents
            .deserialize::<serde_json::Value>()
            .expect("FileContents deserialises")["text"],
        "# Existing\n"
    );

    // write_file (F-347): the one granted command ADR-0012's hand-run probe never
    // exercised, and the one that mutates the vault. `base_hash: null` both keeps the
    // test independent of BLAKE3's exact digest and exercises the "create" branch,
    // which is still a real disk write and still a real ACL-gated invoke.
    let write = invoke(
        &webview,
        "write_file",
        json!({
            "args": {
                "path": "created.md",
                "text": "# Created\nby the acl behaviour test\n",
                "base_hash": Value::Null,
            }
        }),
    );
    assert!(
        !matches!(&write, Err(e) if is_acl_rejection(e)),
        "write_file was refused by the ACL layer: {write:?}"
    );
    write.expect(
        "write_file must succeed once past the ACL, or its grant is reasoned about nothing",
    );
    assert_eq!(
        std::fs::read_to_string(vault.join("created.md")).expect("read back what write_file wrote"),
        "# Created\nby the acl behaviour test\n",
        "write_file's grant let the call through the ACL, but the bytes on disk do not \
         match what was sent -- a grant that lets the call through and a command that \
         works are two different claims (F-347)"
    );

    let index = invoke(&webview, "index_status", json!({}));
    assert!(
        !matches!(&index, Err(e) if is_acl_rejection(e)),
        "index_status was refused by the ACL layer: {index:?}"
    );

    let first_paint = invoke(
        &webview,
        "mark_first_paint",
        json!({ "firstContentfulPaintMs": 123.4 }),
    );
    assert!(
        !matches!(&first_paint, Err(e) if is_acl_rejection(e)),
        "mark_first_paint was refused by the ACL layer: {first_paint:?}"
    );

    let usable = invoke(
        &webview,
        "mark_vault_usable",
        json!({ "note": Value::Null }),
    );
    assert!(
        !matches!(&usable, Err(e) if is_acl_rejection(e)),
        "mark_vault_usable was refused by the ACL layer: {usable:?}"
    );

    // The P1 grants: each must pass the ACL layer (a domain failure is fine, an ACL
    // rejection is not), and `create_folder` must actually create.
    let folder = invoke(
        &webview,
        "create_folder",
        json!({ "args": { "path": "made" } }),
    );
    assert!(
        !matches!(&folder, Err(e) if is_acl_rejection(e)),
        "create_folder was refused by the ACL layer: {folder:?}"
    );
    folder.expect("create_folder must succeed once past the ACL");
    assert!(vault.join("made").is_dir());

    for (cmd, body) in [
        ("refused_names", json!({})),
        ("get_ui_settings", json!({})),
        ("gitignore_offer_status", json!({})),
        (
            "search",
            json!({ "args": { "query": "x", "offset": 0, "limit": 5 } }),
        ),
        ("backlinks", json!({ "args": { "path": "existing.md" } })),
        ("note_anchors", json!({ "args": { "path": "existing.md" } })),
        (
            "resolve_links",
            json!({ "args": { "source_path": "existing.md", "targets": ["existing"] } }),
        ),
        ("decline_gitignore_offer", json!({})),
        ("append_gitignore_entry", json!({})),
        ("set_live_preview", json!({ "args": { "enabled": true } })),
        (
            "set_keybinding",
            json!({ "args": { "command_id": "x.y", "change": { "kind": "reset" } } }),
        ),
    ] {
        let answer = invoke(&webview, cmd, body);
        assert!(
            !matches!(&answer, Err(e) if is_acl_rejection(e)),
            "{cmd} was refused by the ACL layer: {answer:?}"
        );
    }

    // Task #18c: the tree and editor-buffer grants. `open_tree` needs a webview-made
    // channel the mock cannot supply, so it is only required to get past the ACL (its
    // failure is the channel's); the rest are driven end to end.
    let tree = invoke(
        &webview,
        "open_tree",
        json!({ "onMessage": "__CHANNEL__:1" }),
    );
    assert!(
        !matches!(&tree, Err(e) if is_acl_rejection(e)),
        "open_tree was refused by the ACL layer: {tree:?}"
    );
    let marked = invoke(
        &webview,
        "mark_tree",
        json!({ "args": { "mark": "first_screen", "entries_in_model": 0 } }),
    );
    assert!(
        !matches!(&marked, Err(e) if is_acl_rejection(e)),
        "mark_tree was refused by the ACL layer: {marked:?}"
    );

    let read = invoke(
        &webview,
        "read_file",
        json!({ "args": { "path": "existing.md" } }),
    )
    .expect("read_file");
    let hash = read.deserialize::<Value>().expect("FileContents")["hash"]
        .as_str()
        .expect("hash")
        .to_owned();
    let buffer = json!({ "args": { "path": "existing.md", "base_hash": hash } });
    for (cmd, body) in [
        ("note_opened", buffer.clone()),
        ("request_external_diff", buffer.clone()),
    ] {
        let answer = invoke(&webview, cmd, body);
        assert!(
            !matches!(&answer, Err(e) if is_acl_rejection(e)),
            "{cmd} was refused by the ACL layer: {answer:?}"
        );
        answer.unwrap_or_else(|e| panic!("{cmd} must succeed once past the ACL: {e}"));
    }
    let saved = invoke(
        &webview,
        "save_patch",
        json!({ "args": {
            "path": "existing.md",
            "base_hash": hash,
            "patches": [{
                "from": { "line": 0, "col": 2 },
                "to": { "line": 0, "col": 10 },
                "insert": "Patched",
            }],
        } }),
    );
    assert!(
        !matches!(&saved, Err(e) if is_acl_rejection(e)),
        "save_patch was refused by the ACL layer: {saved:?}"
    );
    saved.expect("save_patch must succeed once past the ACL");
    assert_eq!(
        std::fs::read_to_string(vault.join("existing.md")).expect("read back"),
        "# Patched\n",
        "save_patch's grant let the call through, but the patch did not land"
    );
    for (cmd, body) in [
        (
            "rebase_buffer",
            json!({ "args": {
            "path": "existing.md",
            "disk_hash": hash,
            "buffer": "# Patched\n",
        } }),
        ),
        ("note_closed", buffer),
        ("flush_ack", json!({ "args": { "vault_id": "v1" } })),
    ] {
        let answer = invoke(&webview, cmd, body);
        assert!(
            !matches!(&answer, Err(e) if is_acl_rejection(e)),
            "{cmd} was refused by the ACL layer: {answer:?}"
        );
    }
}

/// Task #151: the dialog plugin is registered (its `choose_vault` caller needs it) but no
/// `dialog:*` permission is granted, so the webview cannot reach its commands. The grant
/// list is the only guard, so this is a runtime test of the refusal, not just of the file.
#[test]
fn every_dialog_plugin_command_is_refused_to_the_webview() {
    let state = Arc::new(AppState::with_settings(
        Arc::new(Startup::begin()),
        Boot::default(),
        Settings::default(),
    ));
    let app = mock_builder()
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::clone(&state))
        // `choose_vault` takes the real runtime's window type, which the mock runtime cannot
        // supply; the plugin's own commands are what is under test.
        .invoke_handler(tauri::generate_handler![commands::index_status])
        .build(tauri::generate_context!())
        .expect("mock app builds with the real generated context");
    let webview = WebviewWindowBuilder::new(&app, "main", WebviewUrl::default())
        .build()
        .expect("main window builds under the mock runtime");

    // A control: a granted command is not refused, so a refusal below is the dialog's.
    let status = invoke(&webview, "index_status", json!({}));
    assert!(
        !matches!(&status, Err(e) if is_acl_rejection(e)),
        "index_status was refused: {status:?}"
    );

    for cmd in [
        "plugin:dialog|open",
        "plugin:dialog|save",
        "plugin:dialog|message",
        "plugin:dialog|ask",
        "plugin:dialog|confirm",
    ] {
        // On a thread with a bounded wait: if the grant ever lets the call through, the
        // plugin command really runs under the mock runtime and waits for a dialog nobody
        // answers, which would hang this test instead of failing it with a reason.
        let (done, answered) = std::sync::mpsc::channel();
        let caller = webview.clone();
        std::thread::spawn(move || {
            let answer = invoke(
                &caller,
                cmd,
                json!({ "options": { "directory": true, "title": "x" } }),
            );
            let _ = done.send(answer);
        });
        let answer = answered
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|_| {
                panic!("{cmd} was not answered in 5 s: the ACL let it through and it is waiting")
            });
        let err = answer.expect_err(&format!("{cmd} must be refused to the webview"));
        assert!(
            is_acl_rejection(&err),
            "{cmd} was not refused by the ACL layer: {err}"
        );
    }
}
