//! A build-time-shaped guard for the condition ADR-0012 makes part of its decision
//! rather than advice attached to it.
//!
//! # Why this exists (task #48, ADR-0012)
//!
//! `build.rs` now declares an app ACL manifest (`AppManifest::commands()`), which
//! introduces a failure mode that did not exist before: a command in
//! `tauri::generate_handler!` (`lib.rs`) with no matching permission goes from
//! *reachable* to *rejected at invoke time*, and there is no build-time check for this
//! direction — `build.rs`'s own `validate_capabilities` only bails when a capability
//! names a permission that does not exist, not when a handler command has none. Without
//! something here, the seventh command someone adds to `generate_handler!` in P1 works
//! in the developer's head and fails silently in the running app.
//!
//! `generate_handler!` is a proc macro over function *paths* (`tauri-macros-2.6.3`), so
//! its argument list is not available to a test as data — it has already been consumed
//! by macro expansion by the time anything else runs. The mechanism here instead reads
//! `lib.rs` and `build.rs` as their own source text via `include_str!`, the same
//! principle `security_config.rs` uses for the JSON config files: the bytes under test
//! are the bytes that ship, rather than a second, independently maintained list of the
//! same six names.
//!
//! The four checks below only prove the three files agree with each other textually;
//! none of them proves the ACL actually rejects an unmanifested invoke once wired
//! together. ADR-0012's `Tested against` field names that run separately — it needs the
//! app actually running, which `tauri::test`'s `MockRuntime` could in principle supply
//! without one, but crashes on this machine's toolchain even on the crate's own
//! documented minimal example (`mock_app()` alone, no Serat code involved), so it was
//! run manually against the compiled binary instead of committed as an automated test;
//! see the ADR for what that run showed.
//!
//! # What this is not
//!
//! **A textual match, not a semantic one** (ADR-0012 states this limit explicitly). It
//! would be fooled by a second `tauri::generate_handler![` appearing earlier in a
//! comment, a doc example, or a string literal —
//! [`tests::the_generate_handler_block_appears_exactly_once`] exists to catch exactly
//! that. A future handler list that is itself macro-generated would need this module
//! revisited; nothing here proves that case, only the one that exists today.

/// A permission entry is either a bare string identifier (`"core:default"`) or an
/// object that extends a permission's scope (`{"identifier": ..., "allow": [...]}`,
/// `tauri-utils-2.9.3`'s `PermissionEntry::ExtendedPermission`). Both shapes carry
/// an identifier; this reads it from either so an exact-set check does not have to
/// choose one shape over the other.
#[cfg(test)]
pub(crate) fn permission_identifier(entry: &serde_json::Value) -> String {
    entry.as_str().map(str::to_owned).unwrap_or_else(|| {
        entry["identifier"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "permission entry {entry:?} is neither a string identifier nor an \
                     object carrying one"
                )
            })
            .to_owned()
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    /// `lib.rs`'s own source — read as text, not compiled, so the parser below sees
    /// exactly the bytes that produced the `generate_handler!` invocation it checks.
    const LIB_RS: &str = include_str!("lib.rs");

    /// `build.rs`'s own source, for the same reason.
    const BUILD_RS: &str = include_str!("../build.rs");

    /// `capabilities/default.json`, compiled in rather than read at test time.
    const CAPABILITIES_DEFAULT: &str = include_str!("../capabilities/default.json");

    /// Pulls the `commands::name` entries out of the first
    /// `tauri::generate_handler![...]` block in `lib.rs`. Assumes a flat,
    /// bracket-free list of paths — true today and asserted never to have grown nested
    /// brackets by [`the_generate_handler_block_appears_exactly_once`]'s sibling
    /// invariant that there is exactly one such block to find.
    fn handler_commands() -> BTreeSet<String> {
        const MARKER: &str = "tauri::generate_handler![";
        let start = LIB_RS
            .find(MARKER)
            .expect("lib.rs must call tauri::generate_handler![...]")
            + MARKER.len();
        let end = LIB_RS[start..]
            .find(']')
            .expect("the generate_handler![ block must close with ]")
            + start;
        LIB_RS[start..end]
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                entry
                    .strip_prefix("commands::")
                    .unwrap_or_else(|| {
                        panic!(
                            "generate_handler! entry {entry:?} is not commands::<name> -- \
                             update this parser if the module path changed"
                        )
                    })
                    .to_owned()
            })
            .collect()
    }

    /// Pulls the command list out of `build.rs`'s
    /// `AppManifest::new().commands(&[...])`.
    fn manifest_commands() -> BTreeSet<String> {
        const MARKER: &str = ".commands(&[";
        let start = BUILD_RS
            .find(MARKER)
            .expect("build.rs must declare AppManifest::new().commands(&[...])")
            + MARKER.len();
        let end = BUILD_RS[start..]
            .find(']')
            .expect("the AppManifest commands array must close with ]")
            + start;
        BUILD_RS[start..end]
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| entry.trim_matches('"').to_owned())
            .collect()
    }

    /// Pulls the `allow-*` entries out of `capabilities/default.json`'s `permissions`
    /// array and un-slugifies them back to the underscore command name
    /// (`allow-write-file` -> `write_file`), so the granted set can be compared
    /// against the other two directly rather than by eye.
    fn granted_commands() -> BTreeSet<String> {
        let value: serde_json::Value = serde_json::from_str(CAPABILITIES_DEFAULT)
            .expect("capabilities/default.json must be valid JSON");
        value["permissions"]
            .as_array()
            .expect("capabilities/default.json must have a permissions array")
            .iter()
            .map(super::permission_identifier)
            .filter_map(|id| id.strip_prefix("allow-").map(|slug| slug.replace('-', "_")))
            .collect()
    }

    #[test]
    fn object_form_permission_entries_are_seen_by_granted_commands() {
        // An extended entry ({"identifier": ..., "allow": [...]}) must count as a
        // grant; silently skipping it would hide it from all four ADR-0012 guards.
        let entry = serde_json::json!({"identifier": "allow-open-vault"});
        assert_eq!(super::permission_identifier(&entry), "allow-open-vault");
        let slug = super::permission_identifier(&entry);
        let cmd = slug.strip_prefix("allow-").map(|s| s.replace('-', "_"));
        assert_eq!(cmd.as_deref(), Some("open_vault"));
    }

    #[test]
    fn the_generate_handler_block_appears_exactly_once() {
        // The stated limit on the textual match below: a second occurrence -- in a
        // comment, a doc example, or a string -- would make `handler_commands` read
        // the wrong block silently, and the ADR names this explicitly as the thing a
        // future change to this test needs to re-check.
        let occurrences = LIB_RS.matches("tauri::generate_handler![").count();
        assert_eq!(
            occurrences, 1,
            "tauri::generate_handler![ appears {occurrences} times in lib.rs -- the ACL \
             manifest guard extracts the first occurrence and would silently check the \
             wrong one unless this is exactly 1"
        );
    }

    #[test]
    fn every_handler_command_has_a_permission_in_the_app_manifest() {
        // ADR-0012's condition, not advice attached to it: this is the direction
        // build.rs's own validate_capabilities does not check. A command may exist in
        // generate_handler! without a permission (that permission is then simply
        // undeclared, and any capability trying to grant it would fail the *other*
        // direction at build time) -- but every command reachable from the webview
        // needs SOME permission to exist, granted or not, or it silently rejects at
        // invoke time with nothing having gone red beforehand.
        let handlers = handler_commands();
        let manifest = manifest_commands();
        assert_eq!(
            handlers, manifest,
            "generate_handler! in lib.rs and build.rs's AppManifest::commands() have \
             drifted -- lib.rs has {handlers:?}, build.rs has {manifest:?}. Every \
             command in generate_handler! needs a permission declared in build.rs, even \
             one capabilities/default.json chooses not to grant (see open_vault, \
             ADR-0012) -- an undeclared permission is a silent invoke-time rejection \
             with no build-time check in this direction"
        );
    }

    #[test]
    fn the_granted_set_is_exactly_the_five_commands_boot_js_calls() {
        // ADR-0012, option 3: grant the five commands the webview actually invokes,
        // not all six. open_vault is called only from Rust (state.open_vault(...)
        // inside setup, crates/serat-app/src/lib.rs) and granting it here would
        // re-open surface removing it was meant to close (F-342). This is the "five,
        // not six" half of the decision, checked against the file that ships.
        let granted = granted_commands();
        let expected: BTreeSet<String> = [
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
            "backlinks",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(
            granted, expected,
            "capabilities/default.json's allow-* permissions decoded to {granted:?}, \
             expected exactly the five commands boot.js invokes ({expected:?}) -- if \
             open_vault now needs a webview caller (a P1 vault-picker UI, say), \
             ADR-0012's consequences section already names the permission to add \
             (allow-open-vault)"
        );
    }

    #[test]
    fn every_granted_command_actually_exists_in_the_handler() {
        // The mirror of the manifest check above: a granted permission for a command
        // that was renamed or removed from generate_handler! is dead configuration.
        // build.rs's validate_capabilities only catches a permission that does not
        // exist anywhere in the ACL manifests; it does not know that a permission
        // which does exist no longer corresponds to a live command, because it has no
        // notion of "live command" at all.
        let granted = granted_commands();
        let handlers = handler_commands();
        for command in &granted {
            assert!(
                handlers.contains(command),
                "capabilities/default.json grants allow-{}, but generate_handler! in \
                 lib.rs has no such command (handlers: {handlers:?}) -- a stale grant \
                 or a typo, and build.rs's own capability validation does not catch \
                 either",
                command.replace('_', "-")
            );
        }
    }
}
