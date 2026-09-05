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

Three of those four boundaries do not exist yet, because the crates on the far side of them do not. They are stated as the design; the table below states what holds today.

## Threats (STRIDE)
| ID | Threat | Category | Mitigation (the design) | Enforced today by | ADR |
|---|---|---|---|---|---|
| T-01 | No telemetry — violation of NFR-SEC-01 | Information disclosure | No analytics dependency in the tree; CI fails on a deny-list of known telemetry crates; a network capture over a full session on the release checklist. | **Nothing** *(planned)*. `.github/workflows/` holds `budgets.yml` and `design-docs.yml` only. The absence of an analytics dependency is true today and is asserted by nobody, so it holds until someone adds one. **Buildable now.** | — |
| T-02 | No unconfigured network — violation of NFR-SEC-02 | Information disclosure | All outbound calls through one client in `serat-ai`, never constructed without a configured provider; a test asserts no socket opens during a scripted session. | **Partly, and not by the stated mechanism.** `crates/serat-app/tauri.conf.json` sets a CSP of `default-src`/`script-src`/`style-src 'self'` with **no `connect-src`**, which is real enforcement at the webview — the one network-capable component at P0. The socket assertion does not exist; `serat-ai` does not exist. *(planned)* | — |
| T-03 | Sandboxed extraction — violation of NFR-SEC-03 | Elevation of privilege / DoS | Child process with time and memory caps; the worker links no HTTP client, so a network call is a link error rather than a policy violation. | **Nothing on main** *(planned)*. `FORBIDDEN_EXTERNAL` covers neither `serat-extract`, `serat-ai` nor `serat-mcp`, and the staleness check runs only forbid-to-`POLICY`. The link-error mechanism exists on `task/f61-capability-forbid-list`, unmerged. The crates do not exist. | ADR-0004 |
| T-04 | Credentials — violation of NFR-SEC-04 | Information disclosure | OS keychain via the `keyring` crate; a filesystem scan after configuring a key on the release checklist. | **Nothing** *(planned)*. There is no `keyring` dependency in the tree. | — |
| T-05 | Local-only services — violation of NFR-SEC-05 | Spoofing / elevation of privilege | The MCP server binds stdio or a unix socket. There is no TCP listener in the codebase. | **Nothing, and a forbid list cannot supply it** *(planned)*. The claim is true today, but a listener comes from `std::net`, which is not a dependency and so cannot be denied by a crate deny-list. **This control needs a source lint** — a grep or clippy `disallowed-methods` — and saying "forbid list" here would have been wrong rather than merely absent. **Buildable now.** | — |
| T-06 | Binaries never rewritten — violation of FR-BIN-19 | Tampering | No write path accepts a non-Markdown target; content hashes of every asset compared before and after a scripted session. | **`serat-vault`'s `write_atomic`**, which refuses a non-Markdown target with `NotAMarkdownTarget` and cites T-06 at the refusal, with a second check at the IPC boundary (`serat-app/src/state.rs`, refusing a non-`.md`/`.markdown` target as `IpcError::NotWritable`). `a_binary_target_is_refused` covers `.pdf`/`.png`/`.xlsx` and asserts no file was created; **removing the vault guard fails it**, so that one can fire (F-40). The IPC check has not been mutation-tested, so it is a second barrier rather than a second *verified* barrier. The only non-vault writes are the startup trace and the settings file. | — |

## Verification

Five items were named here as though they ran: a network capture over a full session, a scripted-session socket assertion, a filesystem scan for key material after configuring a key, a content-hash comparison of every asset before and after a scripted session, and fuzzing of the MIME dispatcher (RISK-004). **None of the five exists, in CI or in a release checklist.** They are the design for verification and they are listed as such, not as a description of what runs.

The one control that is verified is verified by a unit test rather than by anything on this list — T-06's `a_binary_target_is_refused` — and it was checked by mutation rather than by reading, which is the standard the other five have to meet before they count.

## What this model does not cover

Stated so the model is not read as broader than it is, which is the failure the rest of this document was just corrected for.

- The pass behind these entries was **entirely static**. No dynamic verification was performed, and only T-06's guard was mutation-checked.
- **The wider Tauri surface was not audited** — per-window command allowlisting, the asset protocol, IPC origin checks. Given that the webview is the one network-capable component at P0, this is the largest remaining gap, and the CSP credited under T-02 is one control on a boundary nobody has examined whole.
- **Nothing ties a CSP edit to NFR-SEC-02.** The `tauri.conf.json` policy is enforcement this model did not know it had, which means it is also enforcement no test would notice being loosened.
- `contracts/ipc.rs` was not reviewed against **C-05**, because no AI or MCP path exists yet that could bypass the diff-review queue. That review is owed the moment one does.
