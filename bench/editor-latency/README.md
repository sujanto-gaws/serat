# bench/editor-latency

The NFR-PRF-02 budget harness, per **architecture.md §9.3** ("NFR-PRF-02: what is measured — frames, not milliseconds"), landed by the architect on `task/p1-05-editor` @ `bdd4fd5` (task #5). Task #11 (P1/test). Owned by test-engineer.

## The definition (§9.3), not this harness's to redefine

**"Keystroke to paint <= 16ms at p95", read literally, cannot be met at 60Hz by any editor** — 16ms is one frame, and a keystroke lands at a random phase of the refresh cycle, so the wait for the next vsync alone averages ~8ms and reaches ~16ms at p95. An earlier version of this harness measured a bare `<div contenteditable>` at p95 17.1ms under a plain wall-clock definition; that finding is what §9.3 is built on, and it is why the requirement is read as a **frame budget** instead:

**Binding: at least 95% of keystrokes are painted in the first display frame after the keystroke, in every scenario, in a 5,000-line note.** A bare-contenteditable **control arm**, run on every execution through the same harness, must reach **>= 99%** or the run is rejected. Diagnostic (b) — the editor's own handler + render work, excluding the vsync wait — is reported beside the frame metric and never gates.

Full mechanism (frame-start recording, the refresh interval `R`, `frames_to_paint`'s counting rule, the long-frame adjustment, the handler/render diagnostic) is in `tests/measure.ts`'s file header, mirroring §9.3 clause for clause; the acceptance wording offered to product-analyst as task #32 is quoted in `tests/nfr-prf-02.spec.ts`.

**Fix 1 (code review, second pass, 2026-09-24): `f_k` is when a frame's callback *runs*, not its scheduled timestamp.** The first implementation picked `f_k` as the first frame whose `rAF` timestamp was `>= p`; review's probe found real cases where `p` landed after a frame's timestamp but before that frame's callback had actually executed, which the old rule wrongly counted as a missed frame. `selectFk` in `tests/measure.ts` now finds the first frame whose callback *ran* after `p`, tracked as `{ ts, run }` pairs. This is not a cosmetic fix: the control arm went from failing at 92.4% (misattributed to the environment) to passing at 99.4% once corrected — **the instrument was wrong, not the machine.**

**Fix 2 (same pass): one implementation measures, not two.** The in-page frame walk used to reimplement the counting/long-frame logic by hand inside the browser context, as a second, hand-typed copy of the same logic the unit tests exercised. Review mutated the in-page copy alone and every unit test stayed green — the guard was tested, its caller was not. The pure functions (`selectFk`, `countFrameStartsInRange`, `isLongFrame`, `medianRefreshInterval`) are now injected into the page verbatim via `Function.prototype.toString()` (`installLatencyCapture`'s `pureFunctionSource`), so the page runs the exact function the unit tests call, not a lookalike.

## Status

**Green today:** `tests/measure.unit.spec.ts` — 25 unit tests over §9.3's pure functions, including the fix-1 boundary case (`ts < p < run` giving `frames_to_paint = 1`) and mutant-killing cases for `paintedFirstFrame`'s exact-1 check and `medianRefreshInterval`'s even-count averaging. No browser page needed, nothing blocked on #21.

**Runs today against a real browser and passes, both controls:**

- The **positive control arm** (`tests/control-arm.ts`) — a bare contenteditable — measured **99.40% on time over 500 keystrokes** against the 99% bar in this development environment (a headless, resource-constrained container, not a reference machine). The 92.4% reading from before fix 1 is now understood as the instrument miscounting, not the environment being unrepresentative — that earlier README claim is withdrawn.
- The **negative control arm** (`tests/negative-control-arm.ts`, new) — a contenteditable whose `input` handler deliberately busy-waits — measured **100% late over 100 keystrokes** against the required >=95%. Proves the harness can report a *bad* result for a genuinely bad subject, not just a good one for a good subject; a harness that always passes would clear the positive control too. **The busy-wait duration is 3x the measured refresh interval, not the ~1.5x first suggested** — measured empirically (1.5x: 0% late, 2x: 7% late, 3x: 100% late, repeatably), and the file header explains the structural reason 1.5x isn't enough: a block that happens *before* `p` is invisible to the base frame count for the same reason the harness's own recorder is blocked by it too.
- The real **subject** test fails at `openNoteForLatencyTest`, not at a timeout — still blocked on #21 (vue-frontend-dev's implementation of the editor #5 designed).

**Do not treat a red subject test as "the editor is over budget"** until both control arms pass on the machine the reading is taken on.

## Run it

```
npm install
npx playwright install-deps chromium   # if system deps are missing; the browser itself is already at /opt/pw-browsers/chromium
npm test
```

Chromium is not downloaded by Playwright's own installer here — `playwright.config.ts` points `executablePath` at the pre-provisioned `/opt/pw-browsers/chromium`, overridable with `PW_CHROMIUM_PATH`. The control-arm run alone takes on the order of a minute (500 keystrokes at the §9.3-mandated 100ms cadence).

## What lands this for real

1. vue-frontend-dev task #21 lands the hybrid live-preview editor `tests/subject.ts` needs a locator for, per architecture.md §9.3's decorations/reveal-rule design.
2. A 5,000-line fixture note per §9.3's shape (dense wikilinks, 50 GFM tables, task lists, footnotes, fenced code, headings every ~40 lines, a CRLF and a BOM variant), generated deterministically (NFR-MNT-02) rather than hand-authored.
3. The remaining §9.3 scenarios beyond the single representative one this harness currently drives: typing at the start/middle/end, inside a table cell, in a link-dense paragraph, Enter mid-note, Backspace across a bold delimiter, and continuous typing in split-preview mode. Each needs its own locator/setup from the real editor, so each is added as #21 exposes the surface for it.
4. Once 1–3 land, `subject.ts`'s `openNoteForLatencyTest` gets a real body, the spec goes from throwing to asserting, and this harness is wired into `.github/workflows/budgets.yml` beside the Rust jobs — following NFR-PRF-08 and RISK-013's rule that a CI job is only claimed once it can actually run. **That CI job must run `npx playwright test tests/nfr-prf-02.spec.ts` specifically, not a bare `npm test`** — the unit spec passing alone is necessary but not sufficient, and **both control arms** must also run and pass (per §9.3 and code review's second pass) before the subject result means anything.

Task #31 (NFR-PRF-01 benches requiring `tree_first_screen` in place of `buffer_rendered`, per ADR-0015 §5) landed separately, in `bench/budget/` (Rust) — not part of this (TypeScript) harness.

## Serves

NFR-PRF-02 · NFR-PRF-08 (once wired) · US-35 · C-06.

## Current state (task #11 final, 2026-10-21)

The harness implements rule E0 (see `tests/measure.ts` header and the `RULE_STATEMENT` printed at the start of every run: R, R >= 16 validity rule, absent-key handling, verdict rule). The README sections above describing the older rule (frame-start recording, `selectFk`) are historical. Run: `SERAT_BENCH_DIST=<app>/dist-bench SERAT_SUBJECT_LABEL=<name> [SERAT_DUMP_DIR=<dir>] npx playwright test tests/nfr-prf-02.spec.ts` (build the subject with `npm run build:bench` in `app/`). The controls block (positive strict + swept, negative, render2, mutation) must be green in the same run for the subject figures to count; the three subject positions (start, middle, end) are independent tests. `record/subject-report.mjs` summarises SERAT_DUMP_DIR files per key.

Numbers on task/p1-21-editor build, headless Chromium, R=16.70 ms (not gate-eligible): positive controls 100% on time (strict and swept); negative and render2 100% late; mutation red; subject start 99.3% / 99.0% on time (PASS), middle 3.3% / 4.7% and end 3.3% / 7.0% (FAIL; mean Event Timing 7.8 s, i.e. the editor stalls on keystrokes away from document start; task #42).
