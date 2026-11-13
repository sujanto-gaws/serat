//! Tauri's build step: it reads `tauri.conf.json` and generates the context the
//! `tauri::generate_context!` macro expands to.
//!
//! The `ts-rs` export that ADR-0007 depends on is *not* a build step: it runs with the
//! crate's tests, which is what makes a stale committed `.ts` file a test failure rather
//! than something a build silently fixes.
//!
//! # The app ACL manifest (ADR-0012)
//!
//! `Attributes::app_manifest` below declares the six commands in `generate_handler!`
//! (`src/lib.rs`) so `AppManifest::commands()` autogenerates an `allow-$command` and a
//! `deny-$command` permission for each, with underscores slugified to hyphens
//! (`tauri-utils-2.9.3/src/acl/build.rs`, `autogenerate_command_permissions`). No
//! permission files are authored by hand.
//!
//! Listing a command here only makes a permission for it *exist*; whether it is
//! *granted* is `capabilities/default.json`'s decision, made separately and on purpose
//! (ADR-0012 grants five of the six — `open_vault` is deliberately left out because
//! nothing in the webview calls it, F-342). `crates/serat-app/src/acl_manifest.rs`'s
//! test module is the other half of the ADR's condition: it fails if a command is added
//! to `generate_handler!` without being added here, which is the failure this manifest
//! would otherwise let through silently at invoke time.
//!
//! **Do not confuse this with `tauri_build::WindowsAttributes::app_manifest`**, a
//! same-named method on a different builder that sets the Windows side-by-side manifest
//! XML — an unrelated, Windows-only concept that a name search finds first (F-336).

fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "open_vault",
            "choose_vault",
            "read_file",
            "write_file",
            "index_status",
            "mark_first_paint",
            "mark_vault_usable",
            "refused_names",
            "create_folder",
            "get_ui_settings",
            "set_live_preview",
            "set_keybinding",
            "gitignore_offer_status",
            "append_gitignore_entry",
            "decline_gitignore_offer",
            "search",
            "switcher_query",
            "backlinks",
            "open_tree",
            "mark_tree",
            "note_opened",
            "note_closed",
            "save_patch",
            "request_external_diff",
            "rebase_buffer",
            "flush_ack",
            "asset_info",
            "read_asset_range",
        ]),
    ))
    .expect(
        "tauri-build attributes are well-formed and capabilities/default.json only names \
         permissions the app manifest declares",
    );
}
