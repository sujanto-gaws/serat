# bench/editor-latency

The NFR-PRF-02 budget harness, per **architecture.md §9.3** ("NFR-PRF-02: what is measured — frames, not milliseconds"), landed by the architect on `task/p1-05-editor` @ `bdd4fd5` (task #5). Task #11 (P1/test). Owned by test-engineer.

## The definition (§9.3), not this harness's to redefine

**"Keystroke to paint <= 16ms at p95", read literally, cannot be met at 60Hz by any editor** — 16ms is one frame, and a keystroke lands at a random phase of the refresh cycle, so the wait for the next vsync alone averages ~8ms and reaches ~16ms at p95. An earlier version of this harness measured a bare `<div contenteditable>` at p95 17.1ms under a plain wall-clock definition; that finding is what §9.3 is built on, and it is why the requirement is read as a **frame budget** instead:

**Binding: at least 95% of keystrokes are painted in the first display frame after the keystroke, in every scenario, in a 5,000-line note.** A bare-contenteditable **control arm**, run on every execution through the same harness, must reach **>= 99%** or the run is rejected. Diagnostic (b) — the editor's own handler + render work, excluding the vsync wait — is reported beside the frame metric and never gates.

Full mechanism (frame-start recording, the refresh interval `R`, `frames_to_paint`'s counting rule, the long-frame adjustment, the handler/render diagnostic) is in `tests/measure.ts`'s file header, mirroring §9.3 clause for clause; the acceptance wording offered to product-analyst as task #32 is quoted in `tests/nfr-prf-02.spec.ts`.

## Status

**Green today:** `tests/measure.unit.spec.ts` — 16 unit tests over §9.3's pure functions (`percentPaintedFirstFrame`, `p95FramesToPaint`, `countFrameStartsInRange`, `isLongFrame`, `medianRefreshInterval`), no browser page needed, nothing blocked on #21.

**Runs today against a real browser, and is informative even though it isn't gated on CI yet:**

- The **control arm** (`tests/control-arm.ts`) actually executes the full frame-walk instrumentation against a bare contenteditable and reports a real percentage. A reference run in this development environment (a headless, resource-constrained container, not a reference machine) measured **92.4% on time over 500 keystrokes** — below the 99% bar, and correctly rejected by the test rather than silently passed. This is recorded as an observation about *this* environment, not a claim about the technique: a headless container's compositor is not a real display's vsync, and the honest next step is to take this reading on an actual reference machine (`docs/05-testing/reference-machine.md`) before drawing any conclusion about the instrumentation itself.
- The real **subject** test fails at `openNoteForLatencyTest`, not at a timeout — still blocked on #21 (vue-frontend-dev's implementation of the editor #5 designed).

**Do not treat a red subject test as "the editor is over budget"** until the control arm passes on the machine the reading is taken on.

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
4. Once 1–3 land, `subject.ts`'s `openNoteForLatencyTest` gets a real body, the spec goes from throwing to asserting, and this harness is wired into `.github/workflows/budgets.yml` beside the Rust jobs — following NFR-PRF-08 and RISK-013's rule that a CI job is only claimed once it can actually run. **That CI job must run `npx playwright test tests/nfr-prf-02.spec.ts` specifically, not a bare `npm test`** — the unit spec passing alone is necessary but not sufficient, and the control arm must also be run and pass (per §9.3) before the subject result means anything.
5. task #31 (NFR-PRF-01 benches requiring `tree_first_screen` in place of `buffer_rendered`, per ADR-0015 §5) is a separate P1 task-engineer task, owned once architect task #2 merges — not part of this harness.

## Serves

NFR-PRF-02 · NFR-PRF-08 (once wired) · US-35 · C-06.
