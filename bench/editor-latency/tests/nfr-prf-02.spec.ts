// NFR-PRF-02 budget job, rule E0 (see measure.ts). Serves NFR-PRF-02, NFR-PRF-08, US-35, C-06.
//
// Gates, in order (serial: if a control fails, the later tests are skipped, because a
// subject result is meaningless on a harness or machine that fails its controls):
//   1. positive control   bare contenteditable, 300 keys, strict 100 ms AND phase-swept: >= 99% on time
//   2. negative control   2.2R busy-wait in the input handler, 100 keys: >= 95% late
//   3. negative control 2 2.2R busy-wait in a rAF queued by the input handler, 100 keys: >= 95% late
//   4. mutation           verdict forced "on time" must turn BOTH negative arms red
//   5. subject            5,000-line note in the real editor build: >= 95% on time
// Reported, never gated: graded 1.5R (expected band 85-95% late) and the keydown->presentation diagnostic.
//
// Engine label: this runs in headless Chromium. Presentation timing there is a proxy for a
// real compositor, so a result is NOT gate-eligible for NFR-PRF-02 until it is reproduced on a
// reference machine (WebView2). Event Timing is Chromium/WebView2 only.
import { test, expect } from "@playwright/test";
import {
  namespaceSource,
  percentLate,
  percentOnTime,
  p95FramesToPaint,
  latencyDiagnostic,
  installCapture,
  runKeystrokes,
  type Cadence,
} from "./measure";
import {
  measureArm,
  POSITIVE_MIN_ON_TIME,
  NEGATIVE_MIN_LATE,
  SUBJECT_MIN_ON_TIME,
  GRADED_BAND,
  type ArmName,
} from "./arms";
import { openNoteForLatencyTest } from "./subject";


const pct = (x: number) => (x * 100).toFixed(1) + "%";
const ENGINE = () => "headless Chromium (not gate-eligible)";
const RULE_STATEMENT =
  "NFR-PRF-02 rule E0: key LATE iff EventTimingEnd > D = ts_j + 2R (R = median rAF delta over 120 frames, measured per run and printed as R=); " +
  "VALIDITY: a run with R < 16 ms is INVALID (throws), because an absent Event Timing entry (duration < 16 ms) is only provably on time when D - t > R >= 16; " +
  "ABSENT KEY: no Event Timing entry >= 16 ms means presented in < 16 ms, scored ON TIME and counted in 'on time by absence', with the worst margin (D - t - 16) reported; " +
  "VERDICT: PASS iff on-time share >= bar at -4, 0 and +4 ms threshold shifts, FAIL iff < bar at all, else INDETERMINATE (not a pass).";
test.beforeAll(() => { console.log(RULE_STATEMENT); });
const fmt = (m: import("./measure").RunSummary) =>
  `on time @-4/0/+4 ms = ${pct(m.onTimeAtMinus4)}/${pct(m.onTimeAtZero)}/${pct(m.onTimeAtPlus4)}; ` +
  `${m.scoredOnTimeByAbsence}/${m.n} on time by absence of an Event Timing entry; ${m.staleAnchors} stale anchors (phase >= 1R); worst absent-key margin ${m.worstAbsentMarginMs === null ? "n/a" : m.worstAbsentMarginMs.toFixed(1) + " ms"}; verdict ${m.verdict}`;

test.describe("controls and mutation (serial: a failing control skips the rest of this block)", () => {
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
    console.log(`positive ${label}: n=300 R=${refreshIntervalMs.toFixed(2)} ${pct(on)} on time, p95 frames=${p95FramesToPaint(scores)}; ${fmt(summary)}; ${ENGINE()}`);
    expect(on, `positive control ${pct(on)} < ${pct(POSITIVE_MIN_ON_TIME)}: the harness or machine cannot show a trivial editor keeping up; the subject result cannot be trusted`).toBeGreaterThanOrEqual(POSITIVE_MIN_ON_TIME);
  });
}

for (const arm of ["negative", "render2"] as ArmName[]) {
  test(`negative control (${arm}): >= 95% late over 100 keystrokes`, async ({ page }) => {
    test.setTimeout(60_000);
    const { scores, summary, refreshIntervalMs } = await measureArm(page, arm, 100);
    expect(scores.length, "at least 100 keys, none dropped").toBeGreaterThanOrEqual(100);
    const late = percentLate(scores);
    console.log(`${arm}: n=100 R=${refreshIntervalMs.toFixed(2)} ${pct(late)} late; ${fmt(summary)}; ${ENGINE()}`);
    expect(late, `${arm} control ${pct(late)} late < ${pct(NEGATIVE_MIN_LATE)}: the harness failed to detect a deliberately blocking subject`).toBeGreaterThanOrEqual(NEGATIVE_MIN_LATE);
  });
}

test("mutation: forcing the verdict on time turns BOTH negative arms red", async ({ context }) => {
  test.setTimeout(60_000);
  const mutant = namespaceSource({ isLate: "function isLate(etEnd, deadlineMs) { return false; }" });
  for (const arm of ["negative", "render2"] as ArmName[]) {
    const fresh = await context.newPage(); // one document per capture install
    const { scores } = await measureArm(fresh, arm, 40, { kind: "strict" }, mutant);
    await fresh.close();
    const late = percentLate(scores);
    console.log(`mutant (isLate -> false) on ${arm}: ${pct(late)} late (gate needs >= ${pct(NEGATIVE_MIN_LATE)}, so it is red)`);
    expect(late, `the mutant must fail the ${arm} gate`).toBeLessThan(NEGATIVE_MIN_LATE);
  }
});

test("graded 1.5R (reported, not gated; expected band 85-95% late)", async ({ page }) => {
  test.setTimeout(90_000);
  const { scores, summary } = await measureArm(page, "graded", 200);
  const late = percentLate(scores);
  const inBand = late >= GRADED_BAND[0] && late <= GRADED_BAND[1];
  console.log(`graded: n=200 ${pct(late)} late; ${fmt(summary)}; band ${pct(GRADED_BAND[0])}-${pct(GRADED_BAND[1])}: ${inBand ? "in band" : "OUT OF BAND (informational)"}`);
  test.info().annotations.push({ type: "graded-late", description: pct(late) });
});

});

// NOTE: the subject positions are deliberately outside the serial block so that one failing position
// does not hide the others; a subject figure is only meaningful when the controls block also passed in
// the same run (CI runs the whole file and reads both).
// Subject: the real editor build, three §9.3 note positions. SERAT_BENCH_DIST selects the build;
// SERAT_SUBJECT_LABEL names it in the output; SERAT_DUMP_DIR (optional) receives one JSON file per
// position x cadence with the per-key scores (index, phase, ET end - D), for diagnosis.
type Position = "start" | "middle" | "end";
const POSITIONS: Position[] = ["start", "middle", "end"];
const SUBJECT_LABEL = process.env.SERAT_SUBJECT_LABEL ?? "unlabelled build";

async function placeCaret(page: import("@playwright/test").Page, editor: import("@playwright/test").Locator, where: Position) {
  await editor.focus();
  await page.keyboard.press("Control+Home");
  if (where === "end") await page.keyboard.press("Control+End");
  if (where === "middle") {
    // The fixture is 5,000 lines of source; 2,500 ArrowDowns lands on the middle line.
    for (let i = 0; i < 2500; i++) await page.keyboard.press("ArrowDown");
  }
  await new Promise((r) => setTimeout(r, 600)); // let scroll/measure settle before the first scored key
}

test.describe("subject positions (independent: one position failing must not hide the others)", () => {
for (const where of POSITIONS) {
  test(`NFR-PRF-02 subject [${where}]: >= 95% painted in the first frame, 5,000-line note, real editor build`, async ({ page }) => {
    test.setTimeout(300_000);
    const subject = await openNoteForLatencyTest(page);
    try {
      const { refreshIntervalMs } = await installCapture(page);
      const rows: string[] = [];
      let worst = 1;
      const verdicts: string[] = [];
      const subjectCadences: [string, Cadence][] = [
        ["strict", { kind: "strict" }],
        ["swept", { kind: "swept", seed: 1 }],
      ];
      for (const [label, cadence] of subjectCadences) {
        await placeCaret(page, subject.editor, where);
        const { scores, summary } = await runKeystrokes(page, subject.editor, 300, { cadence, bar: SUBJECT_MIN_ON_TIME });
        verdicts.push(summary.verdict);
        const on = percentOnTime(scores);
        worst = Math.min(worst, on);
        const d = latencyDiagnostic(scores);
        rows.push(`${label}: ${fmt(summary)}; ${pct(on)} on time, p95 frames=${p95FramesToPaint(scores)}, ${d.withEntry}/300 with ET entry (mean ${d.meanMs.toFixed(1)} ms, max ${d.maxMs.toFixed(1)} ms)`);
        const dump = process.env.SERAT_DUMP_DIR;
        if (dump) {
          const fs = await import("node:fs");
          fs.mkdirSync(dump, { recursive: true });
          fs.writeFileSync(
            `${dump}/${SUBJECT_LABEL}-${where}-${label}.json`,
            JSON.stringify({ build: SUBJECT_LABEL, where, cadence: label, R: refreshIntervalMs, summary, scores }),
          );
        }
      }
      console.log(`SUBJECT ${SUBJECT_LABEL} [${where}] n=300 per cadence, R=${refreshIntervalMs.toFixed(2)}, ${ENGINE()}: ${rows.join(" | ")}`);
      expect(verdicts, "every cadence must be PASS; INDETERMINATE (95% between the -4 and +4 ms shares) is not a pass").toEqual(["PASS", "PASS"]);
      expect(worst).toBeGreaterThanOrEqual(SUBJECT_MIN_ON_TIME);
    } finally {
      await subject.close();
    }
  });
}
});
