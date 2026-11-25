---
project: Serat
artifact: NFR-PRF-02 quiet-machine re-run, real-IPC arm, triage of table-cell, split-preview and the bridge save_patch (task #147)
phase: 05-testing
owner: test-engineer
serves: NFR-PRF-02, NFR-PRF-03, FR-EDT-02, FR-EDT-10, C-06
cites: architecture.md 9.3, ADR-0006, ADR-0016, ADR-0019, task #11, #42, #60, #61, #62
---

# Test Report: NFR-PRF-02 quiet-machine re-run (task #147)

**Date:** 2026-11-24 · **Run by:** test-engineer · **Branch:** `task/p1-147-prf02-quiet-rerun`, from the integration head `6cf088e`.

**Read this first.** Every figure here is a **Linux-container figure** (4 CPUs, headless Chromium 153.0.8010.12 in new headless mode, `executablePath /opt/pw-browsers/chromium`, no GPU, software rendering). It is **supporting evidence, not a gate number** (ADR-0019; architecture 9.3): it is not WebView2, not the reference machine (`docs/05-testing/reference-machine.md`, RM-1), and no NFR-PRF-02 verdict follows from it. **The binding measurement is the reference-machine run on WebView2**, with and without the scroll margins; it has not been done.

## Summary

| | |
|---|---|
| What was asked | Re-take the #61 figures on a quiet machine; run the real-IPC arm; triage table-cell and split-preview typing and the bridge-side `save_patch` tail |
| Result | All 8 scenarios and all four control runs **PASS** on both cadences at 99.6 to 100.0% on time, p95 1 frame. The two weak scenarios of #61 (93.6% and 93.2% strict) read **100.0% and 100.0% strict** quiet: their weakness was machine load, not the editor. `save_patch` through the bridge is **272.7 ms p50 in the dev-profile bridge and 38.7 ms p50 with the bridge optimised** (against 455 ms under load in #61); it is a build-profile and CPU cost, not an index phase |
| Harness defect found and fixed | The harness at `6cf088e` did not run at all: every subject test timed out because the editor was hidden (see "A harness defect", below). Fixed in `bench/editor-latency/tests/subject.ts`; this is a comparability break with #61 and is stated per figure |
| Not a gate number | Said once more: Linux container, headless Chromium, software rendering |

Tests (each project run includes the unit and control specs it depends on): subject 65 passed (8.8 min), scenarios 69 passed (17.1 min), real-ipc 64 passed twice (8.4 and 8.3 min), 0 failed, 0 skipped. (The first attempt, before the harness fix, had 3 failed in the subject project and was discarded: see below.)

## Environment

| | |
|---|---|
| Machine | 4 CPUs, Linux 6.18 container, 16 GB. No other agent, gate or build ran from 02:44 to 03:34 UTC; `pgrep -af "rustc\|cc1\|ld\|cargo\|node"` showed only the harness's own node and browser processes |
| Load average (1 min), sampled every 10 s | subject window: mean 0.27, max 0.65; scenarios: mean 0.22, max 0.92; real-ipc (dev bridge): mean 0.28, max 0.49; real-ipc (optimised bridge): mean 0.35, max 0.74. The runs started at 0.17 (subject) and each followed the previous without a gap. #61 ran at load 10 to 30 |
| Per-scenario load | printed on every `SCENARIO` line: 0.1 to 0.3 (1 min) |
| Engine | headless Chromium 153.0.8010.12, refresh interval R = 16.70 ms on every run (valid, at least 16) |
| Harness | Playwright, rule E0 (`measure.ts`), workers 1, 500 keystrokes per cadence, strict (100 ms) and phase-swept (seeded), no warm-up; the 5,000-line fixture; `npx playwright test --project=subject`, `--project=scenarios`, `--project=real-ipc` (each runs unit then controls first; never `--no-deps`) |
| Build | `app/dist-bench` (`npm run build:bench`), this branch |
| Bridge | `crates/serat-app/examples/editor_ipc_bridge.rs`, two builds: **dev profile** (what CI builds: `cargo build --example editor_ipc_bridge -p serat-app`) and **dev profile with `opt-level=3`** (`CARGO_PROFILE_DEV_OPT_LEVEL=3`; debug assertions stay on). A `--release` bridge cannot be built: see finding F3 |
| Raw output | local scratch, not committed; the lines are quoted here |

### Controls (rule E0's precondition, passed in every run)

| Run | Positive strict (bar 99%) | Positive swept | Negative, render2 | Mutants |
|---|---|---|---|---|
| subject | 100.0% PASS | 100.0% PASS | FAIL (100% late), as required | red |
| scenarios | 100.0% | 100.0% | as required | red |
| real-ipc, dev bridge | 100.0% | 100.0% | as required | red |
| real-ipc, optimised bridge | 100.0% | 99.7% | as required | red |

In #61 the positive strict control read 98.3% (below its bar). Here it never read below 100.0%, and the worst absent-key margin is 0.6 to 0.9 ms, so the figures below are valid readings under E0.

## A harness defect found on the way (comparability with #61)

The first attempt failed at `subject.ts:168` in all three subject tests: `.cm-content` "resolved to hidden". Since the vault chooser (#151) the shell shows "No vault open" and hides the editor (`v-show`, `App.vue:297` and `:615`) unless `window.__SERAT_BOOT__.vault` names a vault, and the harness's Tauri stub set none. Two stub answers were also `null` where the app now needs a value (`mark_first_paint` and `mark_vault_usable` need a number, `resolve_links` an array), which threw `null.toFixed` and `null.map` on every open. The fix is in the stub in `bench/editor-latency/tests/subject.ts`: a boot object (`vault`, `vault_id`, `open: null`), `1` for the two mark commands and `[]` for `resolve_links`. No measuring code and no scoring changed.

Consequence for comparison: #61's figures were taken before #151 and #104 and the live-preview typography and callout work, with `resolve_links` answered `null`. The quiet figures below are therefore a different build of the editor as well as a different machine state. The comparison is "what does today's editor read on a quiet machine", not an A/B of load.

## Scenarios: quiet against the #61 figures under load

On-time share at threshold 0 and the E0 verdict; p95 frames in brackets when it is not 1. #61 rows are from `nfr-prf-02-editor-latency-scenarios.md`.

| Scenario | #61 strict (load 10 to 30) | #61 swept | **Quiet strict** | **Quiet swept** | Quiet ET entries, strict: share of keys, mean, max |
|---|---|---|---|---|---|
| typing at start | not in #61 | not in #61 | **100.0% PASS** | **99.8% PASS** | 217/500, 16.9 ms, 24.0 ms |
| typing mid-note | not in #61 | not in #61 | **100.0% PASS** | **100.0% PASS** | 233/500, 17.1 ms, 25.6 ms |
| typing at end of note | 97.8% INDETERMINATE (#95 dry run) | 97.6% INDETERMINATE (#95) | **100.0% PASS** | **99.6% PASS** | 217/500, 16.9 ms, 24.4 ms |
| table cell | 93.6% INDETERMINATE (p95 2) | 95.2% INDETERMINATE | **100.0% PASS** | **99.8% PASS** | 220/500, 17.2 ms, 25.0 ms |
| link-dense paragraph | 97.0% INDETERMINATE | 98.0% INDETERMINATE | **100.0% PASS** | **99.8% PASS** | 227/500, 17.4 ms, 26.3 ms |
| Enter mid-note | 96.2% INDETERMINATE | 98.8% INDETERMINATE | **100.0% PASS** | **100.0% PASS** | 238/500, 16.1 ms, 24.0 ms |
| Backspace across a bold delimiter | 99.4% PASS | 99.2% PASS | **100.0% PASS** | **100.0% PASS** | 231/500, 16.2 ms, 24.0 ms |
| split preview | 93.2% INDETERMINATE (p95 2; max entry 280 ms) | 96.4% INDETERMINATE | **100.0% PASS** | **100.0% PASS** | 284/500, 18.4 ms, 25.2 ms |
| CRLF fixture | 98.2% INDETERMINATE | 96.8% INDETERMINATE | **100.0% PASS** | **99.8% PASS** | 226/500, 17.1 ms, 25.0 ms |
| BOM + CRLF fixture | 98.6% INDETERMINATE | 98.4% PASS | **100.0% PASS** | **99.6% PASS** | 234/500, 17.3 ms, 25.8 ms |

Every quiet row is PASS on E0's own test (the -4 ms share, the strictest of the three, is 98.8% to 100.0%), p95 frames 1, 0 stale anchors, widest entry-end spread at most 7.7 ms (limit 8). The #61 INDETERMINATE and below-95% readings were produced by the load, as #61 itself said. The #95 "end-of-note INDETERMINATE" reading (97.8 / 97.6%) is also not reproduced quiet; that was a different run (CI dry run, unknown runner), so it is not a controlled comparison.

Two things the quiet table still shows. The longest Event Timing entry of any key is 80 ms (real arm, swept, dev bridge: the save's response arriving) and 56.3 ms (CRLF swept), against 280 ms in #61's split preview; both are single keys. And split preview has visibly more keys with an Event Timing entry than the others (284 and 315 of 500 against 217 to 243 elsewhere, mean 18.4 ms against about 17.2): it still passes, but it is the scenario with the least margin. That is the lead for the triage below.

## The real-IPC arm

Mid-note typing in bursts (25 keys, then 950 ms, so the 1 s autosave lands before the next burst); stub against real `serat-app` behind the loopback bridge. After every real run the harness asserts the note on disk is the fixture plus exactly the 500 typed characters at the caret, so every save provably went through `save_patch` and the byte-range patcher; all four real runs asserted green. The bridge's own timing is the bridge-side cost of the command, measured inside the bridge process.

| Arm | Strict | Swept | p95 frames | Longest ET entry |
|---|---|---|---|---|
| stub (instant IPC), dev-bridge run | 99.6% PASS | 99.6% PASS | 1 | 27.0 / 48.0 ms |
| real, dev-profile bridge | **100.0% PASS** | **99.8% PASS** | 1 | 25.3 / 80.0 ms |
| real, optimised bridge | **100.0% PASS** | **100.0% PASS** | 1 | 26.5 / 24.0 ms |

(#61, under load: stub 98.2% / 97.8% INDETERMINATE, real 98.8% PASS / 95.4% INDETERMINATE.) The save does not cost the keystrokes frames: the real arm is not worse than the stub arm. The 80 ms entry in the dev-bridge swept run is one key; the optimised bridge's longest is 26.5 ms. What the arm still does not include: Tauri's own webview-to-Rust transport and WebView2.

## `save_patch` through the bridge

Bridge-side milliseconds per `save_patch` (the command only: write, hash, patch, rename, and the hand-off to the index writer, which includes a synchronous read and parse of the note on the command thread; see the #172 correction below), cumulative over the two cadences, n = 40, quiet:

| | p50 | p95 | max |
|---|---|---|---|
| #61, under load 10 to 30 (dev bridge) | 455 | 723 | 976 |
| **dev-profile bridge, quiet** | **272.7** | **317.3** | **354.2** |
| **optimised bridge (`opt-level=3`, debug assertions on), quiet** | **38.7** | **59.7** | **62.0** |
| in-process hub-note save, NFR-PRF-03, attested run 3 (release; a different quantity, see below) | 11.54 | 17.01 | 148.86 |

A separate 60-save probe (the same bridge, a 5,000-line synthetic note, a one-character patch each second, under `strace -f -T`) agrees: dev 319.9 / 375.8 / 453.2 ms, optimised 49.1 / 72.0 / 96.8 ms (strace inflates both).

**Which phase carries the tail.** The `p1-save-tail` phase table (`docs/05-testing/p1-save-tail-2026-11.md`: read, parse, submit, wake, txn statements, COMMIT, checkpoint, post-commit) describes the **index writer's transaction**, and none of those phases is inside the `save_patch` call. The call ends at `submit_to_index` (`crates/serat-app/src/state.rs:1833`), whose cost is the `submit` row: **0.03 ms p95 attested** (0.02 ms p50). The wake, statements, COMMIT and post-commit phases (0.17 + 12.64 + 3.42 + 0.24 ms p95 attested, in that order: wake, statements, COMMIT, post-commit) run on the writer thread after the call has returned. So the bridge figure and NFR-PRF-03 are different quantities and neither bounds the other. What the call does contain is `Vault::write_patch` (`crates/serat-vault/src/lib.rs:710`: read the base, hash it, apply the edits, write, hash again before the rename, rename) and then `Editor::save_patch` applying the edits **a second time and hashing the result a second time** to advance the base (`crates/serat-app/src/editor.rs:266`, the `apply_edits` and `hash_bytes` after the write). The syscalls are small: under `strace -T` the durable-write fsync is p50 1.3 ms, p95 7.7 to 8.9 ms, max 17.3 to 22.4 ms, about three per save with the index writer's own included, and the rename p50 0.6 ms, p95 0.7 to 0.8 ms; that is 3 to 6 ms of the 39 ms and 273 ms medians. The rest is CPU: it scales 7x between `opt-level=0` and `opt-level=3`, which is the signature of the hash and the edit application at the debug opt-level, not of I/O. I did not time the hash against the apply separately: that needs an instrumented release build, and none can be made (F3).

**Correction (task #172).** The paragraph above is wrong on two points. `submit_to_index` is not just the `submit` row: `IndexHandle::apply` also runs the `p1-save-tail` read and parse phases synchronously on the command thread before submitting, so those phases are inside the call (on the 5,000-line note it is 87.5% of the release call, about 31 ms of it the parse), and the 0.03 ms p95 is the queue send alone. And the 7x opt-level scaling is that parse, not hash and edit application (the second `apply_edits` plus `hash_bytes` is 2.6% of the call). See `docs/05-testing/reports/save-patch-bridge-release-2026-11.md`.

**Is it over a budget?** No budget applies to this figure. NFR-PRF-03's 50 ms is the save-to-index path, attested at 17.01 ms p95; NFR-PRF-02 asks about the keystroke, and the save runs off the renderer's main thread. The 455 ms of #61 was a dev-profile (unoptimised) bridge under a load of 10 to 30 and says nothing about a shipped build. Neither is a gate number. The one thing the quiet figure does not settle is a release-profile `save_patch` p95 for a large note, because the instrument cannot be built in release (F3); that is task T3 below.

## Triage

### Table cell: the weakness does not persist

Quiet: 100.0% strict, 99.8% swept, p95 1 frame, ET entries on 220/500 keys, mean 17.2 ms, max 25.0 ms: indistinguishable from mid-note typing (233/500, 17.1 ms, 25.6 ms). #61's 93.6% was load. A one-keystroke profile (below) agrees: with the caret inside the revealed table source there is no table widget on the page, and the widget code costs 0.15 ms/key (`updateBlockWidgets`, `app/src/editor/blockWidgets.ts:803`, of which `replaceRegion` at `:695` is 0.075 ms) plus 0.18 ms/key in the reveal rule (`app/src/editor/reveal.ts:319`), about 0.3 ms of 5. No fix is proposed. Nothing is owed except the reference-machine run.

### Split preview: the weakness does not persist, but it is the scenario with the least margin

Quiet: 100.0% / 100.0%, p95 1 frame. Its margin is smaller than the rest, and the cause is not the keystroke's JavaScript. Method: Chrome DevTools Protocol sampling profiler at 100 microsecond intervals and a `devtools.timeline` trace over 300 keystrokes at 80 ms spacing (so the preview's 150 ms debounce never fires during the run, which is also true at the harness's 100 ms cadence), on a bundle built unminified with source maps (scratch build, not committed; the probe spec is not committed either), mid-note typing as the control. Per key:

| ms per key | mid-note (control) | table cell | **split preview** | split preview with `contain: strict` on the preview pane (experiment only) |
|---|---|---|---|---|
| CodeMirror `update` (inclusive) | 3.15 | 3.19 | 2.61 | not taken |
| of which incremental Lezer markdown reparse | 1.35 to 1.5 | 1.38 to 1.49 | 1.25 to 1.32 | not taken |
| keydown to next rAF callback, p50 / p95 | 4.9 / 7.8 (5.1 / 8.3, 5.3 / 8.1 in repeats) | 4.9 / 7.7 (5.3 / 8.1) | **6.5 / 15.4** (7.7 / 16.7; 6.5 / 15.7) | **5.0 / 14.6** |
| `Layout` | 0.63 | 0.55 | **2.04** | 2.02 |
| `LayoutView::HitTest` | 0.36 | 0.41 | **2.22** | **0.48** |
| `PrePaint` | 0.30 | 0.35 | **2.21** | below the top 22 |
| `RunTask` (all threads) | 12.6 | 13.4 | **18.7** | 16.5 |

The preview pane holds **10,214 DOM elements** for the 5,000-line fixture (the document has 10,250): it is rendered in full, appended in idle-time chunks (`app/src/editor/Preview.vue:43` to `:63`), is not virtualised, and has no containment. While the user types in the source pane, the browser's hit-testing, pre-paint and layout lifecycle each walk or revalidate that subtree on every frame: 1.9 ms of HitTest and 1.9 ms of PrePaint per key above the control, 1.4 ms of Layout, about 5 ms of main-thread time per key that no JavaScript profile shows. Adding `contain: strict` to the pane (injected as a style in the profiling run; nothing was changed in the repository) takes HitTest from 2.22 to 0.48 ms per key and the keydown-to-frame p50 from 6.5 to 5.0 ms. It does **not** remove the Layout excess (2.02 against 0.63): the remaining Layout cost is not in the preview subtree, and I did not isolate it (my guess, unverified, is the narrower source pane re-wrapping its lines). **Dominant cost, named:** the unvirtualised, uncontained preview subtree, `app/src/editor/CodeMirrorEditor.vue:1047` (the `.serat-editor-split-preview` rule) mounting `Preview.vue:43`; the JavaScript side is the same as the control.

**Proposed fix (task T1, vue-frontend-dev):** see "Proposed tasks".

### What every scenario pays: CodeMirror's update, and a whole-note string per keystroke

In all three profiles one keystroke is about 3.2 ms of main-thread JavaScript: `flush` (`@codemirror/view` `index.js:7447`) 3.1 to 3.7 ms, in which `EditorState.update` and the markdown reparse (`advance`, `@lezer/markdown` `index.js:749`, through `@codemirror/language` `work`, `index.js:343`) are 1.3 to 1.5 ms, view `update` and `measure` 0.5 to 0.6 ms each. Garbage collection is 0.5 to 0.6 ms/key in the live-preview scenarios. One part of that is the application's own: the update listener at `app/src/editor/CodeMirrorEditor.vue:718` does `previewSource.value = update.state.doc.toString()` **on every document change in every mode**, including live preview where no preview pane exists. `toString` is 0.17 to 0.20 ms/key inclusive (`@codemirror/state` `index.js:106` and `sliceString` `:271`) and allocates a string the size of the note on each key, which is the likeliest source of part of the garbage-collection figure (attribution not measured). This is not a budget breach and costs nothing visible quiet; it is margin on the reference machine, where the controls may be tighter. Task T2.

## Proposed tasks (assigned to the owning dev; not implemented here)

**T1. `[P1/impl] Contain the split-preview pane so typing does not pay for 10,000 preview elements`** (vue-frontend-dev). Serves NFR-PRF-02, FR-EDT-10; honours ADR-0006 and C-07. In `app/src/editor/CodeMirrorEditor.vue` (scoped style, `.serat-editor-split-preview`, line 1047) add layout and paint containment (start with `contain: strict`; compare `content-visibility: auto` with `contain-intrinsic-size` on the rendered chunks in `Preview.vue:43`) and check scrolling, find-in-page and the preview's own scroll position still behave. Reproduce: `app/` `npm run build:bench`, then in `bench/editor-latency` `SERAT_BENCH_DIST=../../app/dist-bench npx playwright test --project=scenarios -g split-preview`; take a trace over 300 keys and read `LayoutView::HitTest`, `PrePaint`, `Layout` per key. Acceptance: HitTest and PrePaint per key within 0.2 ms of the mid-note control, the scenario's share of keys with an Event Timing entry (284/500 quiet) within noise of mid-note's (233/500), E0 PASS on both cadences, and `layoutContract.test.ts` green. Expected effect (measured with the CSS injected): HitTest 2.22 to 0.48 ms/key, keydown-to-frame p50 6.5 to 5.0 ms. Open: where the remaining 1.4 ms/key of Layout comes from.

**T2. `[P1/impl] Do not flatten the whole note on every keystroke for a preview that is not showing`** (vue-frontend-dev). Serves NFR-PRF-02, FR-EDT-10. `CodeMirrorEditor.vue:718` sets `previewSource` from `update.state.doc.toString()` on every change in every mode. Hold the immutable `Text` (a `shallowRef`) and call `toString()` inside `Preview.vue`'s debounced `scheduleRender` (`:67`), or at least only when `!props.livePreview`. Acceptance: no `toString` in the keystroke profile in live-preview mode; the existing preview tests green (`preview.test.ts`, `CodeMirrorEditor.test.ts`); a before and after profile of `(garbage collector)` ms/key and the update listener's inclusive time. Expected: up to 0.2 ms/key directly plus an unmeasured share of 0.5 ms/key of garbage collection.

**T3. `[P1/test] Make the IPC bridge buildable in release so save_patch can be timed as shipped`** (tauri-app-dev with test-engineer). Serves NFR-PRF-02, NFR-PRF-03. `cargo build --release --example editor_ipc_bridge -p serat-app` fails the const assertions at `crates/serat-app/src/lib.rs:39` and `:45` because `[dev-dependencies]` enables `serat-index/test-hooks` for every example. Move the bridge out of `serat-app`'s examples (a bench crate under `bench/` that depends on `serat-app` without the test features), or gate the dev-dependency feature to tests. Then add per-phase timing inside `save_patch` (`write_patch`'s read, hash, apply, write, fsync, rename and `editor.rs:266`'s second apply and hash) to the bridge's stats, take a release figure on the 20k vault's hub-note, and compare to the 17.01 ms in-process figure. Acceptance: a release-profile `save_patch` p50 / p95 / max on a 5,000-line note with the four phase shares; if a second `apply_edits` plus `hash_bytes` in `editor.rs` is more than a quarter of it, a follow-up to advance the base from the first apply's result (rust-core-dev).

**T3 is done (task #172): `docs/05-testing/reports/save-patch-bridge-release-2026-11.md`.** The bridge builds in release behind a `bench-bridge` feature; shipped-profile `save_patch` is 48.8 / 68.8 / 82.0 ms p50 / p95 / max (n=60), 87.5% of it `submit_to_index` (read and parse on the command thread), 2.6% the second apply and hash, so the quarter-threshold follow-up is not raised. `submit_to_index` also runs the read and parse phases synchronously on the command thread, so the 'none of the `p1-save-tail` phases is inside the call' statement above is corrected there. This report's guess that hash and edit application carry the cost (the section above) is withdrawn.

## Follow-up #170/#171

Tasks T1 and T2 (branch `task/p1-170-split-preview-cost`). Changes: `.serat-editor-split-preview` gets `contain: strict` (its size comes from the flex row: `flex: 1 1 50%`, stretched height, `overflow: auto` unchanged, so size containment cannot collapse it); the update listener holds the immutable CodeMirror `Text` in a `shallowRef` instead of calling `doc.toString()` per keystroke, and `Preview.vue` stringifies inside its debounced render (preview output unchanged; the live-preview mode, which mounts no pane, no longer stringifies at all).

Same container, quiet (load average 0.5 to 0.6 on 4 CPUs, controls 99.7 to 100.0%), harness `--project=scenarios -g split-preview` and `--project=subject -g middle`, a bundle of origin/main (2900a37) against a bundle of the branch, back to back, n=500 per cadence (strict / swept). Linux container, headless Chromium: supporting evidence, never a verdict. The harness exposes on-time share, p95 frames and the Event Timing entry count and mean, not the per-key CDP figures of the triage; those were not re-taken.

| scenario | on time (strict / swept) | p95 frames | keys with an Event Timing entry (strict / swept) | mean entry ms |
|---|---|---|---|---|
| split preview, before | 99.6% / 99.8% | 1 | 286 / 304 | 18.7 / 18.0 |
| split preview, after | 100.0% / 100.0% | 1 | **221 / 254** | 17.4 / 17.2 |
| mid-note, before | 100.0% / 100.0% | 1 | 214 / 217 | 17.0 / 17.2 |
| mid-note, after | 100.0% / 100.0% | 1 | 239 / 223 | 17.0 / 17.0 |

Split preview's share of keys with an Event Timing entry falls to the mid-note level (T1's acceptance asked for "within noise of mid-note's"); mid-note itself did not move beyond run-to-run noise (214 to 239). On-time share is 100% in every arm after the change; before, split preview read 99.6% and 99.8%. Both runs PASS on E0.

Unresolved: (1) the remaining Layout excess (about 1.4 ms/key in the triage) was not looked into: `contain: strict` only addresses the preview subtree, and the cause (guess, unverified: the narrower source pane re-wrapping) is untouched. (2) The per-key HitTest, PrePaint and Layout figures were not re-profiled after the change (no CDP probe in the repository), so the 2.22 to 0.48 ms HitTest figure is still the injected-CSS measurement. (3) T1's `content-visibility: auto` alternative was not compared; strict was enough for the Event Timing share. (4) The per-keystroke GC share of `doc.toString()` was not measured separately; the test pins that it no longer happens. (5) Scroll, resize and find-in-page behaviour of the contained pane were reasoned from the CSS (`overflow: auto` retained, explicit flex basis) and unit-tested for the rule, not checked in a real WebView2.

## Findings

| # | Finding | Owner |
|---|---|---|
| F1 | NFR-PRF-02: quiet Linux container, controls 100%, every scenario PASS, 99.6 to 100.0%; #61's weak readings were load. Supporting evidence only; the WebView2 reference-machine run on RM-1 (with and without the scroll margins) is the binding measurement and is not done | product-analyst, architect (re-baseline on it); lead |
| F2 | The harness stub had rotted against the app (no boot vault, three commands answering `null`); every subject test failed on a hidden editor. Fixed in this branch. On this head the CI `editor-latency` jobs would fail the same way (not run: no runner); they are report-only, so nobody would have been blocked by it | test-engineer (done) |
| F3 | The bridge cannot be built with `--release` (the `TEST_HOOKS` const assertions); CI and #61 therefore measured a dev-profile `save_patch` (455 ms under load), which says nothing about a shipped build. A bridge-side figure needs T3 | tauri-app-dev |
| F4 | Split preview pays about 5 ms/key of non-JavaScript main-thread time for an unvirtualised 10,214-element preview; T1 | vue-frontend-dev |
| F5 | The update listener flattens the whole note to a string on every keystroke in every mode; T2 | vue-frontend-dev |

## Not run

The reference-machine run on WebView2; the scroll-margin A/B; a release-profile bridge (impossible, F3); per-phase timing inside `write_patch` (needs the instrumented release build); a re-measure of the experiment `contain: strict` through the full E0 harness (the CSS was injected in a profiling probe only, so no E0 figure exists for it); macOS and Windows.

## Verdict

**Harness: GO (after the stub fix).** **Budget evidence: none; supporting evidence: GO on the quiet container, 8 of 8 scenarios and the real-IPC arm PASS on E0.** The P1 gate still cannot cite an NFR-PRF-02 gate figure until the reference-machine run exists.

## Addendum 2026-11-25: Budgets run 442, harness exception in the middle placement (task #202, TC-181)

Run 442 (894cfb1, ubuntu-latest, job 139376436976): the headless NFR-PRF-02 job failed in the step "Unit, controls, then subject" with 1 failed / 64 passed. The failure was `[subject] nfr-prf-02-subject.spec.ts:107 [middle]`, an error thrown from `placeCaret` before a key was scored: a harness exception, not a latency verdict. The "Other scenarios" step was skipped (default `if: success()`); the Summary step and the artifact ran (`if: always()`). Runs 440, 441, 443 and 444 passed. Source: the lead's quotation of the job log; the error-context artifact was not reachable.

**Mechanism (most likely, not reproduced).** The middle placement was one shot: scroll to half of the estimated height, sleep a fixed 400 ms, click, step down to a Paragraph line, assert 45-55% (and that a Paragraph line was found). If CodeMirror re-measures after the click target was chosen, the caret lands outside the band and the assertion throws. Locally (Chromium 153, quiet) it passed at no throttling; with CDP CPU throttling 4x and 8x it still placed correctly (the subject then failed on latency, which is the expected verdict). So the race was not reproduced, and the throwing assertion of run 442 is not identified.

**Fix.** `placeCaretMiddle` (caret.ts) waits for the scroll geometry to be unchanged for 8 frames, clicks, asserts, steers by the observed miss and retries up to a 30 s deadline; failure is one error listing every attempt and stating it is not a latency verdict. `SERAT_CPU_THROTTLE=n` (never set in CI) slows the page n-fold for starvation repros.

**Gate wiring.** Neither `editor-latency` nor `editor-latency-real-ipc` is in `budgets-gate.needs` or its result loop; both are job-level `continue-on-error: true`. Nothing changed there. The "Other scenarios" step stays skipped after a subject failure: running it unconditionally would take the job (20 + 35 minutes) to the edge of its 60-minute timeout, so it is left to the owner.

**Not verified:** that the fix removes run 442's failure; a hosted run.
