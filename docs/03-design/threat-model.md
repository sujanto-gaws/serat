---
project: Serat
artifact: Threat model
phase: 03-design
derived_from: Serat Design Description 1.0 §10.2 "Security posture" and §7.1 "Isolation"
---

# Threat Model — Serat

The security requirements in the baseline are mostly negative — statements about what must not happen — and a negative is only credible if something enforces it. Each therefore has a mechanism rather than an intention.

**Every mechanism below was written in the present tense, and until F-270 one of the six existed.** That is the defect F-239 found in §Traceability arriving here: a control described as operating reads as covered, and a reader checking whether a negative is credible finds a sentence saying it is. So the table now separates **the mitigation**, which is the design and does not change, from **what enforces it today**, which mostly does not exist yet. A mechanism marked *(planned)* is not built.

**Two different absences, and conflating them is how the honest version becomes alarmist.** T-02, T-03 and T-04 are unenforced because the crates they govern — `serat-ai`, `serat-extract`, `serat-mcp` — have not been written; there is nothing yet to enforce anything against, and the design is not in doubt. **T-01 and T-05 are different: both could be enforced today against the code that exists, and neither is.**

## Assets
- The user's vault: plain Markdown notes and binary assets on local disk — the only source of truth (C-01, C-02).
- The rebuildable index in `.serat/` and the extraction cache `.serat/extract/`.
- AI provider credentials.
- The user's privacy: no telemetry, no network traffic the user did not configure.

## Trust boundaries
- **Main process ↔ extraction worker:** untrusted binaries are parsed only in a short-lived child process with time and memory caps and no network; the worker binary links no HTTP client.
- **App ↔ network:** all outbound calls go through one client in `serat-ai`, constructed only when a provider is configured.
- **App ↔ external agents:** the MCP server binds stdio or a unix socket only; every agent write enters the diff-review queue (C-05).
- **App ↔ OS keychain:** credentials stored via the `keyring` crate, never on the filesystem.

- **Webview ↔ app:** the six IPC commands are reachable from any webview. Tauri v2 capabilities gate *plugin* commands, not the application commands in `generate_handler!`, and none of the six takes a `Window` parameter, so **no command knows its caller**. Containment today is *there is exactly one window*, not *the commands are gated* — a property of the current configuration rather than of the command surface, and it stops holding the day a second window exists.

Three of the first four boundaries do not exist yet, because the crates on the far side of them do not. They are stated as the design; the table below states what holds today.

## Threats (STRIDE)
| ID | Threat | Category | Mitigation (the design) | Enforced today by | ADR |
|---|---|---|---|---|---|
| T-01 | No telemetry — violation of NFR-SEC-01 | Information disclosure | No analytics dependency in the tree; CI fails on a deny-list of known telemetry crates; a network capture over a full session on the release checklist. | **Nothing** *(planned)*. `.github/workflows/` holds `budgets.yml` and `design-docs.yml` only. The absence of an analytics dependency is true today and is asserted by nobody, so it holds until someone adds one. **Buildable now.** | — |
| T-02 | No unconfigured network — violation of NFR-SEC-02 | Information disclosure | All outbound calls through one client in `serat-ai`, never constructed without a configured provider; a test asserts no socket opens during a scripted session. | **Partly, and not by the stated mechanism.** `crates/serat-app/tauri.conf.json` sets a CSP of `default-src`/`script-src`/`style-src 'self'` with **no `connect-src`**, which is real enforcement at the webview — the one network-capable component at P0. **Nothing tests it.** No test references `tauri.conf.json` or `capabilities/`; the CSP, the `core:default` grant, the window label and `withGlobalTauri` are each removable without anything going red, so **the one genuine control found here is also the one with nothing guarding it** (F-40). The socket assertion does not exist; `serat-ai` does not exist. *(planned)* | — |
| T-03 | Sandboxed extraction — violation of NFR-SEC-03 | Elevation of privilege / DoS | Child process with time and memory caps; the worker links no HTTP client, so a network call is a link error rather than a policy violation. | **Nothing on main** *(planned)*. `FORBIDDEN_EXTERNAL` covers neither `serat-extract`, `serat-ai` nor `serat-mcp`, and the staleness check runs only forbid-to-`POLICY`. The link-error mechanism exists on `task/f61-capability-forbid-list`, unmerged. The crates do not exist. | ADR-0004 |
| T-04 | Credentials — violation of NFR-SEC-04 | Information disclosure | OS keychain via the `keyring` crate; a filesystem scan after configuring a key on the release checklist. | **Nothing** *(planned)*. There is no `keyring` dependency in the tree. | — |
| T-05 | Local-only services — violation of NFR-SEC-05 | Spoofing / elevation of privilege | The MCP server binds stdio or a unix socket. There is no TCP listener in the codebase. | **Nothing, and a forbid list cannot supply it** *(planned)*. The claim is true today, but a listener comes from `std::net`, which is not a dependency and so cannot be denied by a crate deny-list. **This control needs a source lint** — a grep or clippy `disallowed-methods` — and saying "forbid list" here would have been wrong rather than merely absent. **Buildable now.** | — |
| T-07 | Reading or writing outside the vault root — violation of NFR-SEC-06 | Information disclosure / Tampering | Nothing resolves to a path outside the root the user opened. The vault is the trust boundary; a path that escapes it escapes the product's central promise (design goal 1, C-01, C-02). | **Partly, and the halves disagree.** `VaultPath::new` refuses `..` segments, absolute paths and empty paths, and is tested — the lexical escapes are closed. `walk.rs` skips **every** symlink, so a link is never indexed. `Vault::resolve` was `root.join(path)` with **no canonicalisation and no containment re-check**, and `Vault::read` used it directly, so a symlink inside the vault was readable through the `read_file` IPC command even though the walk refused it — **the guard was on the path that enumerates and absent from the path that reads.** Task #20 closes it **at `resolve`**, which is the narrowest point every filesystem access in the crate passes through, so `read` and `write_atomic` both inherit it; checked component by component with `symlink_metadata` rather than by `canonicalize`, and a component that does not exist yet is not a symlink, so a new file in a new subdirectory is still allowed.

That fix is **on main**, so this row now describes a closed gap rather than an open one. It was written as a live gap while the fix sat on a branch, and the sentence saying so survived about ten minutes — which is the argument for the rule rather than against it. A threat model describing a branch as though it were running is the defect this table was corrected for once already, and **the safe direction for a security document is to under-claim**: claiming early asserts a protection that is not there, claiming late describes a gap that has closed, and only one of the two gets somebody hurt.

**The guard has no test that fails when it is deleted.** This paragraph first said it "has its own tests in serat-vault", which was written by reading the tests next to it rather than by asking F-40's question, and is false: delete the four lines that return `PathEscapesVault` and all 62 serat-vault tests pass, `cargo test --workspace` passes, and nothing anywhere goes red. Verified by mutation twice, independently.

Both near-misses read as coverage. `resolve_does_not_mistake_a_new_subdirectory_for_a_symlink` pins the false-positive edge and passes with or without the check, because it tests that the guard does **not** fire. The `PathEscapesVault` assertions at the IPC boundary use `../outside.md`, `/etc/passwd` and the like — every one refused by `VaultPath::new` before `resolve` is reached, so they cover the *lexical* boundary while reading as coverage of the containment one.

So T-07's control today is **a mechanism, a careful doc comment, and no evidence**. The absence has a real cause rather than being an oversight — Windows symlink creation needs privilege or Developer Mode — and the answer is to gate the test to Unix or skip loudly on denial, now that CI runs somewhere it can. Tracked as tasks #30 and #29: #30 for the guard itself, #29 for NFR-SEC-06's acceptance criterion, which is written at the IPC boundary and is a separate claim from whether the mechanism fires. The rule both halves implement is stated once in architecture.md, *§A symlink is not a vault file* — which also records that FR-VLT-01's lexical walk strip is sound **because** of this property, so a fix that unified the two paths by making the walk *follow* links would close T-07 and silently break a performance budget. Write side is bounded rather than guarded: `write_atomic` renames a temp over the target, replacing a link rather than writing through it, and requires `.md`. Read by inspection, not executed. | — |
| T-06 | Binaries never rewritten — violation of FR-BIN-19 | Tampering | No write path accepts a non-Markdown target; content hashes of every asset compared before and after a scripted session. | **`serat-vault`'s `write_atomic`**, which refuses a non-Markdown target with `NotAMarkdownTarget` and cites T-06 at the refusal, with a second check at the IPC boundary (`serat-app/src/state.rs`, refusing a non-`.md`/`.markdown` target as `IpcError::NotWritable`). `a_binary_target_is_refused` covers `.pdf`/`.png`/`.xlsx` and asserts no file was created; **removing the vault guard fails it**, so that one can fire (F-40). The IPC check has not been mutation-tested, so it is a second barrier rather than a second *verified* barrier. The only non-vault writes are the startup trace and the settings file. | — |

**T-07 cited no requirement when it was written, and now cites NFR-SEC-06.** Every other row named an NFR-SEC or an FR; T-07 named two constraints and a design goal, because the baseline covered telemetry, network, extraction, credentials and listeners — every one about reaching outward over a *network*, none about reaching outward over the *filesystem*. **The vault boundary was the product's first design goal and the one boundary with no requirement to violate.** Product-analyst allocated NFR-SEC-06 for it at v1.1, phrased to cover a direct path, a relative traversal and a symlink alike, and satisfiable by a check at the read rather than a promise about callers.

The row was written and left citing nothing, deliberately, rather than inventing a requirement to fill the column: **a threat model may name a threat the requirements missed, and must not quietly mint the requirement to match.** Kept here because the empty column is what made the gap visible, and a table that had been tidied would have hidden it.

## Verification

Five items were named here as though they ran: a network capture over a full session, a scripted-session socket assertion, a filesystem scan for key material after configuring a key, a content-hash comparison of every asset before and after a scripted session, and fuzzing of the MIME dispatcher (RISK-004). **None of the five exists, in CI or in a release checklist.** They are the design for verification and they are listed as such, not as a description of what runs.

The one control that is verified is verified by a unit test rather than by anything on this list — T-06's `a_binary_target_is_refused` — and it was checked by mutation rather than by reading, which is the standard the other five have to meet before they count.

## What this model does not cover

Stated so the model is not read as broader than it is, which is the failure the rest of this document was just corrected for.

- The pass behind these entries was **entirely static**. No dynamic verification was performed, and only T-06's guard was mutation-checked.
- **The wider Tauri surface was not audited** — per-window command allowlisting, the asset protocol, IPC origin checks. Given that the webview is the one network-capable component at P0, this is the largest remaining gap, and the CSP credited under T-02 is one control on a boundary nobody has examined whole.
- **Nothing ties a CSP edit to NFR-SEC-02.** The `tauri.conf.json` policy is enforcement this model did not know it had, which means it is also enforcement no test would notice being loosened.
- `contracts/ipc.rs` was not reviewed against **C-05**, because no AI or MCP path exists yet that could bypass the diff-review queue. That review is owed the moment one does.
- The Tauri pass behind T-02's and T-07's entries was **also entirely static, and here there was no test to mutate** — which is itself its first finding. T-07's read path was traced by inspection and never executed; a test that creates a symlink inside a vault and calls `read_file` is what would settle it.
- The claim that Tauri v2 capabilities do not gate application commands rests on **documented framework semantics, not on the framework source**. The project's own `capabilities/default.json` states the same independently, which is agreement rather than confirmation. Worth one check from tauri-app-dev before anything is built on it.
- **`app.security.assetProtocol` is absent, which is correct today and is the thing to watch.** FR-BIN's P2 viewing is what will make someone enable it, and `assetProtocol.scope` is then what decides whether T-07 reopens on a wider path than the IPC commands.
- The `withGlobalTauri` judgement in this model's favour holds, but **not for the reason first given**: the CSP does not gate `initialization_script` at all, since that is host-injected rather than fetched. What makes it safe is that `boot_info` is serialised with `serde_json`, and JSON is a subset of JS expression syntax, so the assignment cannot break out of its literal. Recorded because a control credited to the wrong mechanism survives the removal of the mechanism it was credited to.
- `webview/` itself, the bundler and the updater were not reviewed.
