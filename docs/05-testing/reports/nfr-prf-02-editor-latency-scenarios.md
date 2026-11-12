---
project: Serat
artifact: NFR-PRF-02 editor latency, the other scenarios and the real-IPC arm (task #61)
phase: 05-testing
owner: test-engineer
serves: NFR-PRF-02, FR-EDT-01, US-35, C-06
cites: architecture.md 9.3, ADR-0016, task #11, #42, #60
---

# NFR-PRF-02: the remaining scenarios, the fixture variants and the real-IPC arm

**Status:** harness landed; **figures are INFORMATIONAL, not evidence for the budget.** **Run by:** test-engineer, 2026-11-12, on `te61` (branch `task/p1-61-prf02-scenarios`).

## Read this first: the controls failed in this session

Rule E0's own precondition is that the positive control passes in the same run. **It did not.** The bare-`contenteditable` positive control read **98.3% on time (strict, n=300), below its 99% bar** (INDETERMINATE at -4/0/+4 ms = 97.7/98.3/98.3%). The cause is the machine: this container has 4 CPUs and was shared with other agents' builds and test runs, with a load average of 14-30 for the whole session (13:34: 18.9/23.9/29.6; 13:55: 9.8/12.1/17.2). Every result line prints its load average. Under that load a trivial editor already drops frames, so **no figure below says the editor is over or under budget.** They show that the scenarios run, that the carets land where they claim, and where the editor is *relatively* worse. A re-run on a quiet machine is owed before any of this is compared with a gate (and the reference-machine run, not headless Chromium, is the gate: architecture 9.3).

The negative worst-absent-key margins on some lines (-15.9 ms, -29.1 ms) are a further symptom: an absent Event Timing entry is only provably on time when the main thread ran `requestAnimationFrame` on time, and under this load it did not (the stale-anchor counts are on the same lines).

## Basis (every figure, §9.3)

| | |
|---|---|
| Engine | headless Chromium **153.0.8010.12**, `executablePath /opt/pw-browsers/chromium` (full binary, new headless), not gate-eligible |
| Refresh interval | R = 16.70 ms on every run (valid, >= 16) |
| Harness | Playwright 1.63, rule E0 (`measure.ts`), workers 1, 500 keystrokes per cadence, strict (100 ms) and phase-swept (seeded) |
| Note | the 5,000-line fixture (`buildFixtureNote`), LF unless the line says CRLF or BOM+CRLF |
| Build | `app/dist-bench` from this branch (`npm run build:bench`) |
| Machine | 4 CPUs, shared, load average 10-30 (see above) |
| Mode | report only: no scenario gates; `--no-deps` was used because the controls failed |

## Scenarios (task #61)

On-time share at threshold 0; the verdict is E0's (PASS / INDETERMINATE / FAIL, from the -4/0/+4 ms shares). p95 frames is 1 unless stated.

| Scenario | Strict | Swept | What is measured |
|---|---|---|---|
| table cell | 93.6% INDETERMINATE (p95 frames 2) | 95.2% INDETERMINATE | typing in the first cell of the last row of a revealed pipe table near the middle; a solo run earlier in the session read 80.0% FAIL / 90.0% INDETERMINATE |
| link-dense paragraph | 97.0% INDETERMINATE | 98.0% INDETERMINATE | typing after the first of three wikilinks of a paragraph |
| Enter mid-note | 96.2% INDETERMINATE | 98.8% INDETERMINATE | 500 Enters at the end of a paragraph (the note grows by a line a key); keydown only, no native input event |
| Backspace across a bold delimiter | **99.4% PASS** | **99.2% PASS** | from after the closing `**` of a 526-character bold span: the first two keys delete the delimiter, the rest delete inside what is then plain text |
| split preview | 93.2% INDETERMINATE (p95 frames 2; max entry 280 ms) | 96.4% INDETERMINATE | live preview off; source pane plus the rendered preview, typing at a paragraph end |
| CRLF fixture | 98.2% INDETERMINATE | 96.8% INDETERMINATE | the same 5,000 lines loaded as CRLF, typing mid-note |
| BOM + CRLF fixture | 98.6% INDETERMINATE | 98.4% PASS | the same, with a U+FEFF in front |

What stands out even through the load: the table cell and the split preview are the two scenarios that read below the others on both cadences and are the only ones whose strict p95 is 2 frames; the table cell's solo run (less contention for that one) was the lowest figure of the session. The CRLF and BOM variants are within the noise of the LF mid-note figure, which is what the design predicts (CodeMirror holds the same lines; only the load path differs).

## The real-IPC arm

Mid-note typing in bursts (25 keys, then 950 ms more, so the 1 s autosave fires about 50 ms before the next burst's first key), in two arms on one run. In the **real** arm every IPC command goes to a real `serat-app` `AppState` with a real vault open, behind a loopback bridge (`crates/serat-app/examples/editor_ipc_bridge.rs`). After each run the harness asserts the note on disk is the fixture plus exactly the 500 typed characters, so the saves provably went through `save_patch` and the byte-range patcher.

| Arm | Strict | Swept |
|---|---|---|
| stub (instant IPC) | 98.2% INDETERMINATE | 97.8% INDETERMINATE |
| **real serat-app** | **98.8% PASS** | 95.4% INDETERMINATE |

Bridge-side time per command (cumulative over the two cadences of the real arm): `save_patch` n=40, **p50 455 ms, p95 723 ms, max 976 ms** (write, hash, patch, rename and submit to the index); `note_opened` p50 0.5 ms; `read_file` p50 0.6 ms; link resolution and the settings and status calls under 0.1 ms. The save is slow on this machine and it is off the renderer's main thread, so it did not cost the keystrokes frames beyond the stub arm's figure: the real arm is not worse than the stub arm beyond noise. The `save_patch` figure is the one to re-take on a quiet machine, because under a load of 10-30 on 4 CPUs the `fsync` and the index writer are starved.

**What the real arm does not include.** Tauri's own transport between the webview and the Rust process (WebView2's message channel, the custom-protocol serialisation). The bridge is a separate process on loopback. That cost is per call and small next to the 455 ms above, but it is not measured. It also cannot show what WebView2 does that headless Chromium does not.

## What would change these figures

1. A quiet machine: the positive control at or above 99% in the same run, then the whole table re-taken (README: "do not read a red subject test as over budget unless the controls passed").
2. The reference machine and WebView2.
3. Engine moves re-baseline the controls before any figure counts (§9.3); this is Chromium 153.0.8010.12.

## Findings

| # | Finding | Owner |
|---|---|---|
| 1 | Table cell and split preview are the weakest scenarios on both cadences (strict p95 frames 2). Worth a profile once the machine is quiet; no claim is made yet. | vue-frontend-dev, after a quiet re-run |
| 2 | `save_patch` is 455 ms p50 / 976 ms max for a 5,000-line note on this machine, through the real patcher and index submit. It is off the keystroke path (the debounced save runs in the bridge process). Re-measure quiet. | rust-core-dev, after a quiet re-run |
| 3 | The harness cannot reset a buffer by re-opening the same note path through `__seratBenchOpenNote`; scenarios that mutate the text reload the page. Recorded in the README. | none |
| 4 | Enter and Backspace raise no native `input` event (CodeMirror handles them in `keydown`), so those two scenarios do not check input counts; the keydown-entry invariant still applies. Recorded in the README. | none |

## Verdict

Harness: GO (all scenarios run, carets asserted, the real saves verified on disk, mutation of the scoring unchanged from #11). **Budget evidence: none from this session.** Figures: informational until a quiet re-run passes the controls.
