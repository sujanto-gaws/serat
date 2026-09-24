// NFR-PRF-02 budget: keystroke to paint p95 < 16ms in a 5,000-line note.
// Task #11 (P1/test), depends on P1 task #5 (architect design) and #21
// (vue-frontend-dev implementation). See README.md for status.
//
// Fails loudly rather than silently, per the same rule `bench/budget`
// follows (test-plan.md rule 1: a budget with no job is not a budget) — a
// harness that reported "skipped" here could be mistaken for a green run
// once this file is wired into CI.
import { test, expect } from "@playwright/test";
import { openNoteForLatencyTest } from "./subject";
import { measureKeystrokeToPaint, p95 } from "./measure";

const BUDGET_MS = 16;
const SAMPLE_COUNT = 100;

test("NFR-PRF-02: keystroke-to-paint p95 under 16ms in a 5,000-line note", async ({
  page,
}) => {
  // This call throws today (see subject.ts) — the test is expected to fail
  // until #5/#21 land, and it fails with the blocking task numbers in the
  // message rather than a generic timeout, so whoever reads a red CI run
  // does not have to rediscover why.
  const editor = await openNoteForLatencyTest(page);

  const samples = [];
  for (let i = 0; i < SAMPLE_COUNT; i++) {
    samples.push(await measureKeystrokeToPaint(editor, "x"));
  }

  const result = p95(samples);
  console.log(
    `NFR-PRF-02 p95 over ${SAMPLE_COUNT} keystrokes: ${result.toFixed(2)}ms ` +
      `(budget ${BUDGET_MS}ms)`,
  );
  expect(result).toBeLessThan(BUDGET_MS);
});
