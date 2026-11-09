// NFR-PRF-02 budget job, subject (rule E0, see measure.ts). Serves NFR-PRF-02, NFR-PRF-08, US-35, C-06.
//
// Playwright project "subject" (depends on "controls": if a control fails these do not run).
// The real editor build, PARTIAL COVERAGE: 3 of the 8 §9.3 scenarios (typing at document start, middle,
// end). Missing: inside a table cell, link-dense paragraph, Enter mid-note, Backspace across a bold
// delimiter, split-preview continuous typing, CRLF/BOM fixture variants, and real-binary IPC latencies
// (the IPC client is an instant null stub here). A PASS here is therefore not a PASS of NFR-PRF-02.
// SERAT_BENCH_DIST selects the build (`npm run build:bench`, on the branch carrying it, task/p1-42-prf02-stalls; not yet on integration);
// SERAT_SUBJECT_LABEL names it in the output; SERAT_DUMP_DIR (optional) receives one JSON file per
// position x cadence with the per-key scores, for diagnosis (record/subject-report.mjs).
// The positions are independent tests: one failing does not hide the others.
import { test, expect, type Locator, type Page } from "@playwright/test";
import { installCapture, runKeystrokes, percentOnTime, p95FramesToPaint, latencyDiagnostic, type Cadence } from "./measure";
import { SUBJECT_MIN_ON_TIME, SUBJECT_KEYS } from "./arms";
import { openNoteForLatencyTest } from "./subject";
import { buildFixtureNote } from "./subject";
import { ENGINE, RULE_STATEMENT, fmt, pct } from "./report";

test.beforeAll(async ({ browser }) => { console.log(RULE_STATEMENT(browser.version())); });

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
  // The editor has no active-line decoration and lives in an open shadow root: read the caret's own
  // line from the shadow root's selection (Chromium ShadowRoot.getSelection).
  const activeLineText = () => editor.evaluate((el) => {
    const sel = ((el.getRootNode() as unknown) as { getSelection?: () => Selection | null }).getSelection?.() ?? null;
    const n = sel?.focusNode ?? null;
    const e = n && (n.nodeType === 1 ? (n as Element) : n.parentElement);
    return e?.closest(".cm-line")?.textContent ?? "";
  });
  const lines = buildFixtureNote().split("\n");
  await editor.focus();
  await page.keyboard.press("Control+Home");
  await expect.poll(() => scroller.evaluate((s) => s.scrollTop), { message: "Control+Home scrolled to the top" }).toBe(0);
  let desc = "";
  if (where === "start") {
    await expect.poll(activeLineText, { message: "caret at document start: caret line is the first heading" }).toContain("Section 0");
    desc = "line 0 (Section 0)";
  } else if (where === "end") {
    await page.keyboard.press("Control+End");
    // Control+End scrolls the caret line into view but need not reach the maximum scrollTop (CodeMirror's
    // height estimate), so assert the caret itself: it sits on the LAST rendered line (the viewport holds the
    // document end) and that line is inside the scroller's visible rectangle.
    // Require the four source lines before the final one to be the four rendered .cm-line siblings before the
    // caret line, and at least one of them to be a unique line (a numbered Paragraph or `fn f_n`), so the
    // document end is proven by content and not only by CodeMirror's viewport margin.
    const prevText = lines.slice(-5, -1);
    expect(prevText.some((l) => /^(Paragraph \d+ |fn f_\d+\()/.test(l)), "fixture: the tail holds a unique line").toBe(true);
    await expect.poll(() => editor.evaluate((el, prev: string[]) => {
      const root = el.getRootNode() as ShadowRoot;
      const sel = (root as unknown as { getSelection?: () => Selection | null }).getSelection?.() ?? null;
      const n = sel?.focusNode ?? null;
      const e = n && (n.nodeType === 1 ? (n as Element) : n.parentElement);
      const line = e?.closest(".cm-line");
      const all = root.querySelectorAll(".cm-line");
      const sc = root.querySelector(".cm-scroller");
      if (!line || !sc) return "no caret line or scroller";
      if (all[all.length - 1] !== line) return "caret line is not the last rendered line: " + JSON.stringify((line.textContent ?? "").slice(0, 30));
      // not only the viewport margin: the previous sibling is the fixture's second-to-last line, and the
      // caret line itself is empty or only the x's typed by an earlier run
      let sib = line.previousElementSibling;
      for (let q = prev.length - 1; q >= 0; q--, sib = sib?.previousElementSibling ?? null) {
        // a code-fence line is rendered hidden (empty), so only its presence is checked, not its text
        if (!sib || (sib.textContent !== prev[q] && !(prev[q].startsWith("```") && sib.textContent === ""))) return "sibling " + q + " is " + JSON.stringify((sib?.textContent ?? "<none>").slice(0, 40)) + ", want " + JSON.stringify(prev[q].slice(0, 40));
      }
      if (!/^x*$/.test(line.textContent ?? "")) return "caret line text is not empty/x: " + JSON.stringify((line.textContent ?? "").slice(0, 30));
      const a = line.getBoundingClientRect(), b = sc.getBoundingClientRect();
      return a.top >= b.top - 1 && a.bottom <= b.bottom + 1 ? "ok" : "caret line outside the viewport";
    }, prevText), { message: "caret at document end: caret is on the last line and that line is visible" }).toBe("ok");
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
      const text = await activeLineText();
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
        const chromiumVersion = page.context().browser()?.version();
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
            fs.writeFileSync(`${dump}/${SUBJECT_LABEL}-${where}-${label}.json`, JSON.stringify({ build: SUBJECT_LABEL, chromium: chromiumVersion, where, cadence: label, R: refreshIntervalMs, summary, scores }));
          }
        }
        console.log(`SUBJECT ${SUBJECT_LABEL} [${where}] ${COVERAGE}; n=${SUBJECT_KEYS} per cadence, R=${refreshIntervalMs.toFixed(2)}, ${ENGINE(chromiumVersion)}: ${rows.join(" | ")}`);
        expect(verdicts, "every cadence must be PASS; INDETERMINATE (95% between the -4 and +4 ms shares) is not a pass").toEqual(["PASS", "PASS"]);
        expect(worst).toBeGreaterThanOrEqual(SUBJECT_MIN_ON_TIME);
      } finally {
        await subject.close();
      }
    });
  }
});
