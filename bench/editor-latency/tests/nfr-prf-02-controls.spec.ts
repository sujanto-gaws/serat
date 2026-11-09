// NFR-PRF-02 budget job, controls (rule E0, see measure.ts). Serves NFR-PRF-02, NFR-PRF-08, US-35, C-06.
//
// Playwright project "controls". The "subject" project depends on it, so a failing control means the
// subject tests do not run (a subject result is meaningless on a harness or machine that fails its
// controls). Run the gate with `npx playwright test --project=subject`, which runs unit, controls, then
// subject; `--no-deps` skips the controls; never use it for a gated run (a `-g` filter does not bypass them).
//   1. positive control   bare contenteditable, 300 keys, strict 100 ms AND phase-swept: >= 99% on time
//   2. negative control   2.2R busy-wait in the input handler, 100 keys, BOTH cadences: >= 95% late
//   3. negative control 2 2.2R busy-wait in a rAF queued by the input handler, 100 keys, BOTH cadences: >= 95% late
//   4. mutation           verdict forced "on time" must turn BOTH negative arms red
// Reported, never gated: graded 1.5R (expected band 85-95% late).
//
// Engine label: headless Chromium. Presentation timing there is a proxy for a real compositor, so a
// result is NOT gate-eligible for NFR-PRF-02 until reproduced on a reference machine (WebView2).
import { test, expect } from "@playwright/test";
import { namespaceSource, percentLate, percentOnTime, p95FramesToPaint, type Cadence } from "./measure";
import { measureArm, POSITIVE_MIN_ON_TIME, NEGATIVE_MIN_LATE, GRADED_BAND, type ArmName } from "./arms";
import { ENGINE, RULE_STATEMENT, fmt, pct } from "./report";

test.beforeAll(async ({ browser }) => { console.log(RULE_STATEMENT(browser.version())); });
test.describe.configure({ mode: "serial" });

const CADENCES: [string, Cadence][] = [
  ["strict 100 ms", { kind: "strict" }],
  ["phase-swept", { kind: "swept", seed: 1 }],
];

for (const [label, cadence] of CADENCES) {
  test(`positive control (${label}): bare contenteditable >= 99% on time over 300 keystrokes`, async ({ page }) => {
    test.setTimeout(120_000);
    const { scores, summary, refreshIntervalMs } = await measureArm(page, "positive", 300, cadence);
    expect(scores.length, "exactly 300 positive keys, none dropped").toBe(300);
    const on = percentOnTime(scores);
    console.log(`positive ${label}: n=300 R=${refreshIntervalMs.toFixed(2)} ${pct(on)} on time, p95 frames=${p95FramesToPaint(scores)}; ${fmt(summary)}; ${ENGINE(page.context().browser()?.version())}`);
    expect(on, `positive control ${pct(on)} < ${pct(POSITIVE_MIN_ON_TIME)}: the harness or machine cannot show a trivial editor keeping up; the subject result cannot be trusted`).toBeGreaterThanOrEqual(POSITIVE_MIN_ON_TIME);
  });
}

for (const arm of ["negative", "render2"] as ArmName[]) {
  for (const [label, cadence] of CADENCES) {
    test(`negative control (${arm}, ${label}): >= 95% late over 100 keystrokes`, async ({ page }) => {
      test.setTimeout(60_000);
      const { scores, summary, refreshIntervalMs } = await measureArm(page, arm, 100, cadence);
      expect(scores.length, "exactly 100 keys, none dropped").toBe(100);
      const late = percentLate(scores);
      console.log(`${arm} ${label}: n=100 R=${refreshIntervalMs.toFixed(2)} ${pct(late)} late; ${fmt(summary)}; ${ENGINE(page.context().browser()?.version())}`);
      expect(late, `${arm} control ${pct(late)} late < ${pct(NEGATIVE_MIN_LATE)}: the harness failed to detect a deliberately blocking subject`).toBeGreaterThanOrEqual(NEGATIVE_MIN_LATE);
    });
  }
}

test("mutation: forcing the verdict on time turns BOTH negative arms red", async ({ context }) => {
  test.setTimeout(60_000);
  const mutant = namespaceSource({ isLate: "function isLate(etEnd, deadlineMs) { return false; }" });
  for (const arm of ["negative", "render2"] as ArmName[]) {
    const fresh = await context.newPage(); // one document per capture install
    const { scores } = await measureArm(fresh, arm, 40, { kind: "strict" }, mutant);
    await fresh.close();
    const late = percentLate(scores);
    console.log(`mutant (isLate -> false) on ${arm}: ${pct(late)} late (gate needs >= ${pct(NEGATIVE_MIN_LATE)}, so it is red); ${ENGINE(context.browser()?.version())}`);
    expect(late, `the mutant must fail the ${arm} gate`).toBeLessThan(NEGATIVE_MIN_LATE);
  }
});

test("graded 1.5R (reported, not gated; expected band 85-95% late)", async ({ page }) => {
  test.setTimeout(90_000);
  const { scores, summary, refreshIntervalMs } = await measureArm(page, "graded", 200);
  const late = percentLate(scores);
  const inBand = late >= GRADED_BAND[0] && late <= GRADED_BAND[1];
  console.log(`graded: n=200 R=${refreshIntervalMs.toFixed(2)} ${pct(late)} late; ${fmt(summary)}; ${ENGINE(page.context().browser()?.version())}; band ${pct(GRADED_BAND[0])}-${pct(GRADED_BAND[1])}: ${inBand ? "in band" : "OUT OF BAND (informational)"}`);
  test.info().annotations.push({ type: "graded-late", description: pct(late) });
});
