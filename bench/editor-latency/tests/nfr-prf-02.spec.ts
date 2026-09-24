// NFR-PRF-02 budget, per architecture.md §9.3 (task/p1-05-editor @
// bdd4fd5, task #5): at least 95% of keystrokes in a 5,000-line note are
// painted in the first display frame after the keystroke, in every
// scenario, with a bare-contenteditable control arm at >=99% required on
// every run. Task #11 (P1/test), depends on task #5 (landed, this file)
// and #21 (vue-frontend-dev implementation, not yet landed — the real
// subject still throws, see subject.ts).
//
// Acceptance wording (§9.3, offered to product-analyst as task #32):
// "Given a 5,000-line note in the editor, when each benchmark scenario is
// typed through the browser's input pipeline, then at least 95% of
// keystrokes are painted in the first display frame after the keystroke,
// in every scenario, and a bare-contenteditable control arm on the same
// run is at 99% or better; the editor's own per-keystroke work is
// reported beside it."
//
// Fails loudly rather than silently, per the same rule `bench/budget`
// follows (test-plan.md rule 1: a budget with no job is not a budget) — a
// harness that reported "skipped" here could be mistaken for a green run
// once this file is wired into CI.
import { test, expect } from "@playwright/test";
import { openNoteForLatencyTest } from "./subject";
import { runScenario, percentPaintedFirstFrame, p95FramesToPaint } from "./measure";
import { measureControlArm, CONTROL_ARM_MIN_ON_TIME } from "./control-arm";
import { measureNegativeControlArm, NEGATIVE_CONTROL_MIN_LATE } from "./negative-control-arm";

const SUBJECT_MIN_ON_TIME = 0.95;
const SAMPLE_COUNT = 500; // §9.3: "each at least 500 keystrokes"
const CONTROL_SAMPLE_COUNT = 500;
const NEGATIVE_CONTROL_SAMPLE_COUNT = 100; // cheaper: this arm exists to prove the harness can fail, not to be precise

test("control arm: a bare contenteditable is on time at least 99% of the time", async ({
  page,
}) => {
  // Runs unconditionally, whatever the state of #5/#21 — see
  // control-arm.ts. Required on every run per §9.3: if this fails, the
  // subject result below (whenever it is unblocked) cannot be trusted —
  // the harness or machine itself cannot keep up with a trivial editor.
  const { percentOnTime, samples } = await measureControlArm(page, CONTROL_SAMPLE_COUNT);
  console.log(
    `control arm: ${(percentOnTime * 100).toFixed(2)}% on time over ${CONTROL_SAMPLE_COUNT} keystrokes ` +
      `(must be >= ${(CONTROL_ARM_MIN_ON_TIME * 100).toFixed(0)}%); p95 frames_to_paint = ` +
      `${p95FramesToPaint(samples)}`,
  );
  expect(
    percentOnTime,
    `control arm on-time rate is ${(percentOnTime * 100).toFixed(2)}%, not >= ` +
      `${(CONTROL_ARM_MIN_ON_TIME * 100).toFixed(0)}% — the NFR-PRF-02 subject result cannot be ` +
      `trusted until this passes (architecture.md §9.3)`,
  ).toBeGreaterThanOrEqual(CONTROL_ARM_MIN_ON_TIME);
});

test("negative control arm: a deliberately-blocking contenteditable is late at least 95% of the time", async ({
  page,
}) => {
  // The complement of the positive control above (code review,
  // 2026-09-24): proves the harness can report a bad result for a
  // genuinely bad subject, not just a good one for a good subject — a
  // harness that always reports "on time" regardless of the subject would
  // pass the positive control too.
  const { percentLate, refreshIntervalMs } = await measureNegativeControlArm(
    page,
    NEGATIVE_CONTROL_SAMPLE_COUNT,
  );
  console.log(
    `negative control arm: ${(percentLate * 100).toFixed(2)}% late over ` +
      `${NEGATIVE_CONTROL_SAMPLE_COUNT} keystrokes (must be >= ` +
      `${(NEGATIVE_CONTROL_MIN_LATE * 100).toFixed(0)}%; busy-wait 3R = ` +
      `${(refreshIntervalMs * 3).toFixed(1)}ms)`,
  );
  expect(
    percentLate,
    `negative control arm's late rate is ${(percentLate * 100).toFixed(2)}%, not >= ` +
      `${(NEGATIVE_CONTROL_MIN_LATE * 100).toFixed(0)}% — the harness failed to detect a ` +
      `deliberately blocking handler, which means it cannot be trusted to detect a real one`,
  ).toBeGreaterThanOrEqual(NEGATIVE_CONTROL_MIN_LATE);
});

test("NFR-PRF-02: at least 95% of keystrokes painted in the first frame, 5,000-line note", async ({
  page,
}) => {
  // This call throws today (see subject.ts) — the test is expected to fail
  // until #5's implementation (#21) lands, and it fails with the blocking
  // task numbers in the message rather than a generic timeout, so whoever
  // reads a red CI run does not have to rediscover why.
  const editor = await openNoteForLatencyTest(page);

  const { samples, actualCadenceMs } = await runScenario(editor, SAMPLE_COUNT);

  const percentOnTime = percentPaintedFirstFrame(samples);
  const missedFrameSamples = samples.filter((s) => s.framesToPaint > 1);
  const meanHandlerMs =
    samples.reduce((sum, s) => sum + s.handlerMs, 0) / samples.length;
  const meanRenderMs = samples.reduce((sum, s) => sum + s.renderMs, 0) / samples.length;

  console.log(
    `NFR-PRF-02: ${(percentOnTime * 100).toFixed(2)}% on time over ${SAMPLE_COUNT} keystrokes ` +
      `(budget >= ${(SUBJECT_MIN_ON_TIME * 100).toFixed(0)}%); p95 frames_to_paint = ` +
      `${p95FramesToPaint(samples)}; ${missedFrameSamples.length}/${SAMPLE_COUNT} missed a frame; ` +
      `diagnostic (b) mean handler ${meanHandlerMs.toFixed(2)}ms, mean render ${meanRenderMs.toFixed(2)}ms; ` +
      `cadence recorded, ${actualCadenceMs.length} intervals`,
  );
  expect(percentOnTime).toBeGreaterThanOrEqual(SUBJECT_MIN_ON_TIME);
});
