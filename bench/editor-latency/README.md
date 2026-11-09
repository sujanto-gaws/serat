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

## Structure (three Playwright projects, workers 1)

| Project | File | Contents |
|---|---|---|
| `unit` | `tests/measure.unit.spec.ts` | pure E0 functions, each run on the direct import and on the `new Function` round trip the page uses; invariant tests |
| `controls` (depends on `unit`) | `tests/nfr-prf-02-controls.spec.ts` | positive bare contenteditable (strict and phase-swept, 300 keys, >= 99% on time); negative (busy-wait in the input handler) and render2 (busy-wait in a queued rAF), each over both cadences, 100 keys, >= 95% late; mutation (`isLate -> false` must turn both negative arms red); graded 1.5R (reported, not gated) |
| `subject` (depends on `controls`) | `tests/nfr-prf-02-subject.spec.ts` | the real editor build, 5,000-line note, caret at start / middle / end (asserted, not assumed), 500 keys per cadence (strict and swept), >= 95% on time and PASS on both cadences |

**Subject coverage is partial: 3 of 8 §9.3 scenarios.** Missing: inside a table cell, link-dense paragraph, Enter mid-note, Backspace across a bold delimiter, split-preview continuous typing, CRLF/BOM fixture variants, and real-binary IPC latencies (the IPC client is an instant null stub). A PASS here is not a PASS of NFR-PRF-02. This is printed on every subject line.

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
```

`npm run build:bench` (in `app/`) is on the branch carrying it, **`task/p1-42-prf02-stalls`** (`app/package.json`); not yet on integration; without a build the subject fails loudly with the build command. `SERAT_DUMP_DIR` receives per-key JSON; `node record/subject-report.mjs <files>` summarises it. A subject run takes about 20 minutes (3 positions x 2 cadences x 500 keys at 100 ms).

Do not read a red subject test as "the editor is over budget" unless the controls passed in the same run on the same machine.

## Numbers

See `docs/05-testing/` reports and the task #11 hand-off; headless Chromium in the development container, R = 16.70 ms. Not gate-eligible until reproduced on the reference machine.

## History

`record/` holds the rule-selection recordings and analyzer that led to E0 (rules A, B, C, D were rejected on recorded frames).
