# bench/editor-latency

The NFR-PRF-02 budget harness, per **architecture.md §9.3** ("NFR-PRF-02: what is measured"). Task #11 (P1/test), owned by test-engineer. Serves NFR-PRF-02 · NFR-PRF-08 (once wired in CI) · US-35 · C-06.

## The definition (rule E0), not this harness's to redefine

"Keystroke to paint <= 16 ms at p95" cannot be met at 60 Hz by any editor, so the requirement is read as a frame budget: **at least 95% of keystrokes are painted in the first display frame after the keystroke, in every scenario, in a 5,000-line note.**

Rule E0 (header of `tests/measure.ts`; selected empirically from `record/`): for each trusted keydown at time `t`, `ts_j` is the last rAF timestamp at or before `t`, `D = ts_j + 2R` (`R` = median rAF delta over 120 frames, measured per run), and `ETend` is the maximum `startTime + duration` of the key's keydown/keypress/beforeinput/input Event Timing entries (time window `[t, t+4 ms]`, keyup excluded). The key is **late iff `ETend > D`**.

- **Validity:** a run with `R < 16 ms` is rejected (throws): an absent entry (duration < 16 ms) is only provably on time when `D - t > R >= 16`.
- **Absent key:** no entry means presented in < 16 ms; scored on time, counted as "on time by absence", worst margin `D - t - 16` printed.
- **Per-key invariants (rejects the run, naming the key index):** a key with any entry must have its keydown entry; its entry ends lie within 8 ms of each other. The widest spread of the run is printed (limit 8).
- **Verdict:** shares of on-time keys at threshold shifts -4, 0, +4 ms. PASS iff the -4 share meets the bar, FAIL iff even the +4 share misses it, else INDETERMINATE (not a pass).
- **Warm-up: none** (`warm-up 0` is printed); every key counts.
- Event Timing is Chromium/WebView2 only and `duration` is rounded to 8 ms. Headless Chromium output is labelled "not gate-eligible".

## Structure (five Playwright projects, workers 1)

| Project | File | Contents |
|---|---|---|
| `unit` | `tests/measure.unit.spec.ts` | pure E0 functions, each run on the direct import and on the `new Function` round trip the page uses; invariant tests |
| `controls` (depends on `unit`) | `tests/nfr-prf-02-controls.spec.ts` | positive bare contenteditable (strict and phase-swept, 300 keys, >= 99% on time); negative (busy-wait in the input handler) and render2 (busy-wait in a queued rAF), each over both cadences, 100 keys, >= 95% late; mutation (`isLate -> false` must turn both negative arms red); graded 1.5R (reported, not gated) |
| `subject` (depends on `controls`) | `tests/nfr-prf-02-subject.spec.ts` | the real editor build, 5,000-line note, caret at start / middle / end (asserted, not assumed), 500 keys per cadence (strict and swept), >= 95% on time and PASS on both cadences |
| `scenarios` (depends on `controls`) | `tests/nfr-prf-02-scenarios.spec.ts`, `tests/caret.ts` | task #61: the other five §9.3 scenarios (table cell, link-dense paragraph, Enter mid-note, Backspace across a bold delimiter, split-preview typing) and the fixture loaded as CRLF and as BOM+CRLF. Each: 500 keys per cadence, a fresh document per cadence, the caret placed by a real click and asserted. Report only |
| `real-ipc` (depends on `controls`) | `tests/nfr-prf-02-real-ipc.spec.ts` | task #61: mid-note typing in bursts (25 keys, then a pause that puts the 1 s autosave at the next burst), in two arms: the instant stub, and **real serat-app** behind a loopback bridge (`crates/serat-app/examples/editor_ipc_bridge.rs`). Asserts the real saves landed (the note on disk is the fixture plus exactly the typed characters) and prints the bridge's own p50/p95/max per command. Needs the bridge binary. Report only |

**Coverage (task #61).** Subject: 3 of the 8 §9.3 scenarios (start, middle, end typing). Scenarios: the other five and the two fixture variants. Real-IPC: the save/index round trip through a real `serat-app`. **Still not covered:** the real Tauri transport between the webview and Rust (WebView2's message channel and the custom-protocol serialisation are not in any figure), WebView2 itself (headless Chromium is the same engine, not the same process or compositor), and the reference machine. A PASS here is not a PASS of NFR-PRF-02 until that run exists; every line says so.

**Comparability.** The stub's `save_patch` now answers a plausible `FileWritten` (a fake success) instead of `null`, so subject-arm figures from before task #61 and after it are not strictly comparable: a save that fires while typing now takes the editor's success path.

**The IPC bridge is a write-capable server and is locked down** (task #61 review): a per-run random token printed on its READY line and required in `x-serat-bridge-token`; the `Host` header must be `127.0.0.1:<port>`; CORS only for a loopback `http://127.0.0.1:<port>` origin, never `*`; and the vault root must be inside the system temp directory.

**Known harness limits.** Keys that CodeMirror handles in its own `keydown` keymap (Enter, Backspace) raise no native `input` event, so those scenarios run with `expectInputs: false`; the per-key invariants still demand the keydown entry. The Backspace scenario grows a bold span to 520 characters first (unmeasured), steps over the closing `**`, and measures 500 Backspaces: the first two cross the delimiter, the rest delete inside what is then plain text. Re-opening the same note path through `__seratBenchOpenNote` does not reset the buffer, so a cadence that mutates the text reloads the page.

## Run it

```
npm ci                             # Chromium is pre-provisioned at /opt/pw-browsers/chromium (PW_CHROMIUM_PATH overrides); do not run playwright install
npx playwright test --project=subject    # THE GATE: unit, then controls, then subject; a failing control skips the subject
npx playwright test --project=controls   # unit + controls only
npm test                                  # bare: runs all three projects in the same order
```

`--no-deps` skips the controls; never use it for a gated run (a `-g` filter does not bypass them). The subject needs a bench build:

```
SERAT_BENCH_DIST=<checkout>/app/dist-bench SERAT_SUBJECT_LABEL=<name> [SERAT_DUMP_DIR=<dir>] npx playwright test --project=subject
npx playwright test --project=scenarios          # same environment; about 35 minutes
cargo build --example editor_ipc_bridge -p serat-app
SERAT_IPC_BRIDGE=<target>/debug/examples/editor_ipc_bridge npx playwright test --project=real-ipc
```

`npm run build:bench` (in `app/`) is on main (`app/package.json`). CI runs this exact sequence in the report-only `editor-latency` job (task #60; headless Chromium is not gate-eligible, so it never gates); without a build the subject fails loudly with the build command. `SERAT_DUMP_DIR` receives per-key JSON; `node record/subject-report.mjs <files>` summarises it. A subject run takes about 20 minutes (3 positions x 2 cadences x 500 keys at 100 ms).

Do not read a red subject test as "the editor is over budget" unless the controls passed in the same run on the same machine.

## Numbers

Quiet re-run 2026-11-24 (task #147): `docs/05-testing/reports/nfr-prf-02-quiet-rerun-2026-11.md`. The Tauri stub in `tests/subject.ts` now supplies `window.__SERAT_BOOT__` (a vault), because the shell hides the editor behind "No vault open" without one, and answers `mark_first_paint`, `mark_vault_usable` and `resolve_links` with a number, a number and an array; figures from before this change are not strictly comparable. The IPC bridge example cannot be built with `--release` (serat-app's `TEST_HOOKS` const assertions); `CARGO_PROFILE_DEV_OPT_LEVEL=3` gives an optimised build with debug assertions.

See `docs/05-testing/` reports and the task #11 hand-off; headless Chromium in the development container, R = 16.70 ms. Not gate-eligible until reproduced on the reference machine.

## History

`record/` holds the rule-selection recordings and analyzer that led to E0 (rules A, B, C, D were rejected on recorded frames).
