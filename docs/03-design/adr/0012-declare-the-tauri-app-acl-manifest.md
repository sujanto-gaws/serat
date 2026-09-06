# ADR-0012: Declare the Tauri app ACL manifest, and grant only the commands the webview calls
**Status:** proposed · **Date:** 2026-09-06 · **Serves:** NFR-SEC-02, T-02, FR-BIN-19, design goal 5

## Context

Tauri v2 gates an IPC command when **any** of three things is true — it is a plugin
command, the app declares an ACL manifest, or the origin is not local:

```rust
// tauri-2.11.5/src/webview/mod.rs, Webview::on_message
if (plugin_command.is_some() || has_app_acl_manifest || !is_local)
    && request.cmd != FETCH_CHANNEL_DATA_COMMAND
    && invoke.acl.is_none()
{ reject }
```

Serat declares no app manifest, so for a **local** webview the first two are false and
the ACL is skipped entirely for the six commands in `generate_handler!`. A remote origin
is still refused by the third disjunct. This is F-280 as corrected by F-336: the
framework *can* allowlist application commands per window and per webview —
`RuntimeAuthority::resolve_access` filters resolved commands by the capability's
`windows` and `webviews` selectors, and application commands live in the same
`allowed_commands` map as plugin commands under `APP_ACL_KEY` (`__app-acl__`). Serat has
not opted in. **That converts a limitation into a decision, which is why this ADR
exists at all.**

Two facts about the tree make the decision concrete rather than theoretical.

**`capabilities/default.json` already says `"windows": ["main"]`, and for these six
commands that field does nothing.** It is already written down as the intent. Declaring
the manifest is closer to connecting a wire than to adding a mechanism.

**One of the six is reachable from the webview and called by nothing there.**
`crates/serat-app/webview/boot.js` is the whole of the P0 front end and it invokes
`read_file`, `write_file`, `index_status`, `mark_first_paint` and `mark_vault_usable` —
**five.** `open_vault` is invoked only from Rust, by `state.open_vault(...)` inside
`setup` (`crates/serat-app/src/lib.rs`), which does not cross the IPC boundary at all.
So the sixth command is exposed to the webview purely as a side effect of being
registered. That is present-tense surface, not a hypothetical.

## Options considered

1. **Do nothing, and revisit when a second window appears.** Cheapest today and honest
   about YAGNI: with one window loading only bundled local content, per-window
   allowlisting protects against nothing that can currently happen.
2. **Declare the manifest and grant all six commands.** Engages the ACL and makes
   `windows: ["main"]` live, but grants the exact set that is reachable today, so it
   buys only the future per-window filtering.
3. **Declare the manifest and grant the five the webview actually calls.** Same as 2,
   and additionally removes `open_vault` from the webview's reach until something asks
   for it.
4. **Give the commands `Window` parameters so they can identify their caller.** A
   different and larger ask — per-window *behaviour* rather than per-window
   *allowlisting* — and not required by anything today.

## Decision

**Option 3, on one condition that is part of the decision rather than advice attached to
it: the change that declares the manifest must also land a test that fails when a
command in `generate_handler!` has no matching permission.**

The condition is not optional because declaring the manifest **introduces a failure mode
that does not exist today**. Today an unlisted command is reachable; afterwards it is
rejected at invoke time. There is no build-time check for this — `build.rs` validates
the reverse direction only, bailing when a capability names a permission that does not
exist. Without the test, the seventh command someone adds in P1 works in their head and
fails in the app, and the change would have made things worse.

The test is buildable with the pattern already in `crates/serat-app/src/security_config.rs`,
which compiles `tauri.conf.json` and `capabilities/default.json` in with `include_str!`
so the bytes under test are the bytes that ship. The mechanism is `serat-app`'s to
choose; the requirement is that the handler list and the granted set are compared by
something that goes red.

Cost, checked rather than assumed: `AppManifest::commands(&[...])` autogenerates
`allow-$command` and `deny-$command` permissions with underscores slugified to hyphens
(`tauri-utils-2.9.3/src/acl/build.rs`, `autogenerate_command_permissions`). No permission
files are authored by hand. `build.rs` gains a command array and the capability gains
five `allow-*` entries.

**Explicitly not decided here:** option 4. No command takes a `Window` or
`WebviewWindow` parameter, so even with the ACL engaged **no command can tell who called
it**. This ADR buys per-window *allowlisting* and leaves per-window *behaviour* to
whoever needs it.

## Consequences

**Positive.** `open_vault` stops being reachable from the webview. `windows: ["main"]`
becomes enforcement instead of documentation. A second window in P1 arrives with the
allowlisting mechanism already engaged and tested, so the question it raises is "which
commands does this window get", answered in a capability file, rather than "is anything
gating this at all", answered by an investigation.

**Negative.** Every command added from now on needs a permission entry, and the cost is
permanent. Granting five rather than six means a P1 vault-picker UI must add
`allow-open-vault` before `open_vault` works from the front end — a deliberate speed
bump, and the reason the failure has to be legible.

**Risks.** The rejection a developer sees in release builds is
`Command {cmd} not allowed by ACL`; debug builds get the richer message from
`resolve_access_message`. If that turns out to be too thin in practice, the remedy is a
better message, not reverting the ACL. And `app/src/` currently holds only generated
`ts-rs` types — the Vue front end is not wired yet, so **the five-command set describes
the P0 boot shell and will need revisiting when the real front end lands.** That
revisiting is the mechanism working, not a defect in it.

**Why now rather than later**, which is the whole argument: roughly three lines of
`build.rs` plus a permission list before a second window exists, against a security
investigation after one does. Design goal 5 — capability degrades, nothing breaks —
argues for engaging the mechanism while there is nothing to break.

## Objections recorded
(from teammates consulted)

*Pending tauri-app-dev, consulted before this was written. Their answer to one question
can change the decision rather than its wording: whether the `generate_handler!` command
list is reachable from a test at all. If it is not, the condition above is not
satisfiable as stated and option 1 becomes the honest ruling, because option 3 without
the test is worse than doing nothing.*

## Tested against

**Not yet, and the case is named.** The reasoning here was checked against the framework
source at the versions `Cargo.lock` resolves and against the tree — the invoke path's
three disjuncts, `resolve_access`'s window and webview filtering, the absence of
`__app-acl__` from `crates/serat-app/gen/schemas/acl-manifests.json`, and the five
`invoke(...)` call sites in `boot.js` against the six in `generate_handler!`. **All of
that tests the premise, not the decision.**

What would test the decision is one run of the implementing change: declare the manifest,
grant the five, and confirm that `read_file` still works from the webview and
`open_vault` is refused. **tauri-app-dev should supply it**, because it needs the app
running and this ADR's author has only read the code. Until that exists this decision is
`proposed`, not `accepted` — the distinction being exactly what this field is for.
