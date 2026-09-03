---
project: Serat
artifact: Threat model
phase: 03-design
derived_from: Serat Design Description 1.0 §10.2 "Security posture" and §7.1 "Isolation"
---

# Threat Model — Serat

The security requirements in the baseline are mostly negative — statements about what must not happen — and a negative is only credible if something enforces it. Each therefore has a mechanism rather than an intention.

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

## Threats (STRIDE)
| ID | Threat | Category | Mitigation | ADR |
|---|---|---|---|---|
| T-01 | No telemetry — violation of NFR-SEC-01 | Information disclosure | There is no analytics dependency in the tree, and CI fails on a deny-list of known telemetry crates. A network capture over a full session is part of the release checklist. | — |
| T-02 | No unconfigured network — violation of NFR-SEC-02 | Information disclosure | All outbound calls go through one client in serat-ai. With no provider configured it is never constructed, and a test asserts the process opens no socket during a scripted session. | — |
| T-03 | Sandboxed extraction — violation of NFR-SEC-03 | Elevation of privilege / DoS | Child process with time and memory caps; the worker binary links no HTTP client at all, so a network call is a link error rather than a policy violation. | ADR-0004 |
| T-04 | Credentials — violation of NFR-SEC-04 | Information disclosure | OS keychain via the keyring crate. A filesystem scan after configuring a key is part of the release checklist and must find no key material. | — |
| T-05 | Local-only services — violation of NFR-SEC-05 | Spoofing / elevation of privilege | The MCP server binds stdio or a unix socket. There is no TCP listener in the codebase. | — |
| T-06 | Binaries never rewritten — violation of FR-BIN-19 | Tampering | No write path accepts a non-Markdown target. Verified by comparing content hashes of every asset before and after a scripted session. | — |

## Verification
The mitigations are release-checklist items and CI tests as stated in the design description: a network capture over a full session, a scripted-session socket assertion, a filesystem scan for key material after configuring a key, a content-hash comparison of every asset before and after a scripted session, and fuzzing of the MIME dispatcher (RISK-004).
