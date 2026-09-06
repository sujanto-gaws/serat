# ADR-0012: Declare the Tauri app ACL manifest, and grant only the commands the webview calls
**Status:** accepted · **Date:** 2026-09-06 (accepted after the run named below) · **Serves:** NFR-SEC-06, T-08, design goal 5 (corrected by task #50; see the correction below)

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

**Positive.** `open_vault` stops being reachable from the webview — **which is worth more than the "one command with no caller" framing above admits, and the framing is the fault of this ADR rather than of the change.** `open_vault` takes an arbitrary absolute root and nothing checks it against the root already open, so it is the one registered command able to **relocate the containment boundary** that NFR-SEC-06 and T-07 are written about; the other five operate within whatever boundary is in force. Withholding it is therefore a containment control, not tidiness (task #50, T-08). `windows: ["main"]`
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

## Correction, task #50: what this decision serves

**This ADR was accepted with a `Serves:` line naming three IDs it does not serve, and
omitting the one it does.** The header now reads NFR-SEC-06 and T-08; it read NFR-SEC-02,
T-02 and FR-BIN-19. The previous wording is kept here rather than quietly replaced,
because a traceability link is a claim like any other and the way it failed is reusable.

The test that settles it is F-40's, asked of a link rather than of a guard: **if reverting
the decision cannot change the requirement's acceptance evidence, the decision does not
serve the requirement.**

- **NFR-SEC-02** is *no unconfigured network*, and its acceptance is a network capture
  confirming silence. Not one of the six commands opens a socket. Reverting the manifest
  changes no capture.
- **T-02** *is* NFR-SEC-02 in the threat table — the row reads "No unconfigured network
  — violation of NFR-SEC-02" — and its own ADR column reads "—". The link existed in
  one direction only, and that direction was the wrong one.
- **FR-BIN-19** is *binaries are never rewritten*, and its acceptance is asset content
  hashes unchanged. `write_file` is granted; the non-Markdown refusal lives in
  `write_atomic` and in the IPC check, neither of which the ACL touches.

**NFR-SEC-06 passes the same test, through the grant that was withheld.**
`OpenVaultArgs { root: String }` is an arbitrary absolute host path, `open_vault_with`
hands it to `Vault::open` and refuses only a root that does not exist or cannot be read,
and nothing compares it with the root already open. A webview able to invoke `open_vault`
therefore **moves the vault root**, and every subsequent read and write is contained
inside a root the user did not choose. T-07's mitigation column already states the rule as
*"nothing resolves to a path outside the root **the user opened**"*, which is what makes
this NFR-SEC-06 in substance and not by analogy.

**Why the wrong ID looked right, which is the part worth keeping.** The threat model had
no row for the webview's IPC reach at all — it lived only in §Trust boundaries prose —
so the nearest row on the same boundary was T-02, and citing the requirement behind it
read as locating the decision rather than mis-stating it. **An absent ID is filled by the
nearest present one**, and the nearest is chosen by adjacency rather than by
consequence. T-08 now exists, so the next decision on this boundary has a right answer to
find.

## Objections recorded
(from teammates consulted)

**On the task #50 correction, recorded in the tense it happened in.** tauri-app-dev was consulted before the correction was written, because the NFR-SEC-06 claim is a claim about their crate. **At the time of writing they had not replied, and the question that could have changed the correction was settled by reading the crate rather than by waiting for them (F-343).** It was whether anything in `serat-app` already refuses a webview-initiated relocation of the root: `AppState::open_vault_with` takes `args.root`, hands it to `Vault::open`, maps only a missing or unreadable directory to `VaultNotFound`, and compares it with nothing. There is no such refusal. Their answer will be recorded here when it arrives, and if it contradicts the reading above then this section changes rather than the reading being defended.

tauri-app-dev was consulted before this was written. **The one question that could have
changed the decision rather than its wording was answered before they replied, and
answered by measurement rather than by waiting** — their reply, when it came, was the
implementation itself (below).

The question was whether the `generate_handler!` command list is reachable from a test at
all — because if it is not, the condition above is unsatisfiable and option 1 becomes the
honest ruling. `generate_handler!` is a proc macro over function *paths*
(`tauri-macros-2.6.3`), so it cannot read a `const` array and the list is not available
as data. **But the file can read itself.** A test in the same crate may `include_str!`
its own source and extract the block textually, which is `security_config.rs`'s existing
principle — the bytes under test are the bytes that ship — applied one file over. Built
as a standalone crate and run before this paragraph was written: the test extracts the
command names and passes; there is no circularity, because `include_str!` is a byte read
at compile time rather than a macro expansion.

**Its limits, stated because selling the technique as airtight is how the next F-305
happens.** It is a textual match, not a semantic one. It would be fooled by the literal
`generate_handler![` appearing earlier in a comment or a string, and by any future
handler list that is itself macro-generated. Asserting that the block occurs exactly once
covers the first; the second would need this ADR revisited. **The mechanism is
`serat-app`'s to choose and this is an existence proof, not a specification** — what
matters to the ruling is only that a satisfying test can exist, and it can.

**tauri-app-dev raised no objection and implemented the ruling as written** (3960bdf,
task #48), including the five-not-six grant. They chose their own mechanism for the
condition — `crates/serat-app/src/acl_manifest.rs`, four textual guards comparing
`generate_handler!`, `build.rs`'s manifest and `capabilities/default.json` against each
other — which is what "an existence proof, not a specification" was for.

**One thing they found that this ADR did not anticipate, and it is the reason the
verification below is a manual run rather than an automated test.** `tauri::test::mock_app()`,
the crate's own documented minimal example, crashes with `STATUS_ENTRYPOINT_NOT_FOUND` on
this toolchain with no Serat code involved. So the settling evidence could not be left
behind as a test, and that is stated here rather than quietly enjoyed.

## Tested against

**Run, by tauri-app-dev, against the running app — which is why this is `accepted`.**
The premise was checked against framework source and the tree (the invoke path's three
disjuncts, `resolve_access`'s window and webview filtering, the absence of `__app-acl__`
from the generated manifests, and `boot.js`'s five `invoke(...)` sites against the six in
`generate_handler!`). **That tested the premise. The decision was tested by the run.**

A temporary `boot.js` probe invoking `open_vault` after the real boot sequence recorded
the framework's refusal verbatim in the startup trace — *"open_vault not allowed.
Permissions associated with this command: allow-open-vault"* — while `read_file`,
`index_status`, `mark_first_paint` and `mark_vault_usable` continued to work **through
the same run**. Both halves in one execution is what makes it evidence rather than two
anecdotes: a build where nothing worked would also have refused `open_vault`.

**What the run did not cover, stated because four of five is not five.** `write_file` is
granted and was **not** exercised by the probe. Its grant rests on the same
`acl_manifest.rs` guards as the others and on the framework behaviour the other four
demonstrated, which is good but is not the same as having been observed. If a cheap
write-side probe becomes available, it is worth one run.
