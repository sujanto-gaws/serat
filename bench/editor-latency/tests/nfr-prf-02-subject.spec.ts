// NFR-PRF-02 budget job, subject (rule E0, see measure.ts). Serves NFR-PRF-02, NFR-PRF-08, US-35, C-06.
//
// Playwright project "subject" (depends on "controls": if a control fails these do not run).
// The real editor build, PARTIAL COVERAGE: 3 of the 8 §9.3 scenarios (typing at document start, middle,
// end). Missing: inside a table cell, link-dense paragraph, Enter mid-note, Backspace across a bold
// delimiter, split-preview continuous typing, CRLF/BOM fixture variants, and real-binary IPC latencies
// (the IPC client is an instant null stub here). A PASS here is therefore not a PASS of NFR-PRF-02.
// SERAT_BENCH_DIST selects the build (`npm run build:bench`, today only on task/p1-21-editor);
// SERAT_SUBJECT_LABEL names it in the output; SERAT_DUMP_DIR (optional) receives one JSON file per
// position x cadence with the per-key scores, for diagnosis (record/subject-report.mjs).
// The positions are independent tests: one failing does not hide the others.
import { test, expect, type Locator, type Page } from "@playwright/test";
import { installCapture, runKeystrokes, percentOnTime, p95FramesToPaint, latencyDiagnostic, type Cadence } from "./measure";
import { SUBJECT_MIN_ON_TIME, SUBJECT_KEYS } from "./arms";
import { openNoteForLatencyTest } from "./subject";
import { buildFixtureNote } from "./subject";
import { ENGINE, RULE_STATEMENT, fmt, pct } from "./report";

test.beforeAll(() => { console.log(RULE_STATEMENT); });

type Position = "start" | "middle" | "end";
const POSITIONS: Position[] = ["start", "middle", "end"];
const SUBJECT_LABEL = process.env.SERAT_SUBJECT_LABEL ?? "unlabelled build";
const COVERAGE = "partial: 3/8 scenarios (start/middle/end typing only)";

/** Puts the caret at the requested place and ASSERTS where it landed. Returns a description.
 * start: Control+Home, active line is the first heading. end: Control+End, scroller at the bottom.
 * middle: scroll the editor to its midpoint, click the centre of the viewport (a real mouse click),
 * then step down to a "Paragraph n" line and require its source line index to be within 45-55% of the note. */
async function placeCaret(page: Page, editor: Locator, where: Position): Promise<string> {
  const scroller = page.locator('[data-testid="serat-editor"] .cm-scroller');
  const active = page.locator('[data-testid="serat-editor"] .cm-activeLine');
  const lines = buildFixtureNote().split("\n");
  await editor.focus();
  await page.keyboard.press("Control+Home");
  let desc = "";
  if (where === "start") {
    await expect(active.first(), "caret at document start: active line is the first heading").toContainText("Section 0");
    desc = "line 0 (Section 0)";
  } else if (where === "end") {
    await page.keyboard.press("Control+End");
    await new Promise((r) => setTimeout(r, 300));
    const atBottom = await scroller.evaluate((s) => s.scrollTop + s.clientHeight >= s.scrollHeight - 2);
    expect(atBottom, "caret at document end: the editor is scrolled to the bottom").toBe(true);
    desc = `end of note (line ${lines.length - 1})`;
  } else {
    await scroller.evaluate((s) => { s.scrollTop = (s.scrollHeight - s.clientHeight) / 2; });
    await new Promise((r) => setTimeout(r, 400));
    const box = await scroller.boundingBox();
    if (!box) throw new Error("editor scroller has no bounding box");
    await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
    const lineOf = new Map<number, number>();
    lines.forEach((l, i) => { const m = /^Paragraph (\d+) /.exec(l); if (m) lineOf.set(Number(m[1]), i); });
    let idx = -1;
    for (let step = 0; step < 30 && idx < 0; step++) {
      const text = (await active.first().textContent()) ?? "";
      const m = /Paragraph (\d+) /.exec(text);
      if (m && lineOf.has(Number(m[1]))) idx = lineOf.get(Number(m[1])) as number;
      else await page.keyboard.press("ArrowDown");
    }
    expect(idx, "middle: caret landed on a recognisable Paragraph line").toBeGreaterThanOrEqual(0);
    const frac = idx / lines.length;
    expect(frac, `middle: caret at source line ${idx} of ${lines.length}`).toBeGreaterThan(0.45);
    expect(frac).toBeLessThan(0.55);
    desc = `source line ${idx} of ${lines.length} (${pct(frac)})`;
  }
  await new Promise((r) => setTimeout(r, 600)); // let scroll/measure settle before the first scored key
  return desc;
}

test.describe("subject positions", () => {
  for (const where of POSITIONS) {
    test(`NFR-PRF-02 subject [${where}]: >= 95% painted in the first frame, 5,000-line note, real editor build`, async ({ page }) => {
      test.setTimeout(420_000);
      const subject = await openNoteForLatencyTest(page);
      try {
        const { refreshIntervalMs } = await installCapture(page);
        const rows: string[] = [];
        let worst = 1;
        const verdicts: string[] = [];
        const cadences: [string, Cadence][] = [
          ["strict", { kind: "strict" }],
          ["swept", { kind: "swept", seed: 1 }],
        ];
        for (const [label, cadence] of cadences) {
          const caret = await placeCaret(page, subject.editor, where);
          const { scores, summary } = await runKeystrokes(page, subject.editor, SUBJECT_KEYS, { cadence, bar: SUBJECT_MIN_ON_TIME });
          verdicts.push(summary.verdict);
          const on = percentOnTime(scores);
          worst = Math.min(worst, on);
          const d = latencyDiagnostic(scores);
          rows.push(`${label} (caret at ${caret}): ${fmt(summary)}; ${pct(on)} on time, p95 frames=${p95FramesToPaint(scores)}, ${d.withEntry}/${SUBJECT_KEYS} with ET entry (mean ${d.meanMs.toFixed(1)} ms, max ${d.maxMs.toFixed(1)} ms)`);
          const dump = process.env.SERAT_DUMP_DIR;
          if (dump) {
            const fs = await import("node:fs");
            fs.mkdirSync(dump, { recursive: true });
            fs.writeFileSync(`${dump}/${SUBJECT_LABEL}-${where}-${label}.json`, JSON.stringify({ build: SUBJECT_LABEL, where, cadence: label, R: refreshIntervalMs, summary, scores }));
          }
        }
        console.log(`SUBJECT ${SUBJECT_LABEL} [${where}] ${COVERAGE}; n=${SUBJECT_KEYS} per cadence, R=${refreshIntervalMs.toFixed(2)}, ${ENGINE()}: ${rows.join(" | ")}`);
        expect(verdicts, "every cadence must be PASS; INDETERMINATE (95% between the -4 and +4 ms shares) is not a pass").toEqual(["PASS", "PASS"]);
        expect(worst).toBeGreaterThanOrEqual(SUBJECT_MIN_ON_TIME);
      } finally {
        await subject.close();
      }
    });
  }
});
