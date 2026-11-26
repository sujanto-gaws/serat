// NFR-PRF-02 budget job, the other scenarios (task #61, rule E0, see measure.ts).
// Serves NFR-PRF-02, FR-EDT-01, US-35, C-06, architecture.md 9.3 "Scenarios".
//
// Playwright project "scenarios" (depends on "controls": if a control fails these do not run). The five
// scenarios 9.3 names beyond start/middle/end typing, plus the CRLF and BOM+CRLF fixture variants:
//
//   table-cell      typing inside a pipe-table cell of a table near the middle of the note
//   link-dense      typing between the wikilinks of a link-dense paragraph
//   enter-mid       Enter at the end of a paragraph in the middle of the note (the document grows by a line a key)
//   backspace-bold  Backspace from after a bold span's closing delimiter, through the delimiter (reveal/hide) and on
//   split-preview   continuous typing with live preview off: the source pane plus the rendered preview pane
//   variant-crlf / variant-bom-crlf   the same 5,000 lines loaded as CRLF, and as BOM + CRLF; typing mid-note
//
// Each scenario is 500 keystrokes per cadence (strict and phase-swept), the caret placed by a real click and
// ASSERTED (caret.ts), the note reloaded before each cadence so the second run starts from the same text.
// REPORT ONLY: headless Chromium is not gate-eligible, and this spec is not part of any gate until the reference
// machine exists. A red here is a finding about the editor or about this harness; it is not weakened to go green.
// Every result line carries the engine version, binary and mode, and R. The IPC client is still the stub of
// subject.ts (instant answers); the real-IPC arm is nfr-prf-02-real-ipc.spec.ts.
import os from "node:os";
import { test, expect, type Page } from "@playwright/test";
import { installCapture, runKeystrokes, percentOnTime, p95FramesToPaint, latencyDiagnostic, type Cadence } from "./measure";
import { SUBJECT_MIN_ON_TIME, SUBJECT_KEYS } from "./arms";
import { openNoteForLatencyTest, buildFixtureNote, fixtureVariant, type FixtureVariant, type OpenOptions } from "./subject";
import { goToLine, placeOnLine, move, findLine, caretLineText } from "./caret";
import { ENGINE, RULE_STATEMENT, fmt, pct } from "./report";

const SUBJECT_LABEL = process.env.SERAT_SUBJECT_LABEL ?? "unlabelled build";
const LINES = buildFixtureNote().split("\n");

test.beforeAll(async ({ browser }) => { console.log(RULE_STATEMENT(browser.version())); });

interface Placed { caret: string; key?: string; expectInputs?: boolean }
interface Scenario {
  id: string;
  title: string;
  variant?: FixtureVariant;
  open?: OpenOptions;
  /** Places the caret (asserted) and does any unmeasured preparation. */
  place: (page: Page, editor: import("@playwright/test").Locator) => Promise<Placed>;
}

const SCENARIOS: Scenario[] = [
  {
    id: "table-cell",
    title: "typing inside a pipe-table cell",
    async place(page, editor) {
      const head = findLine(LINES, 0.5, (l) => l.startsWith("| key |"));
      const m = /^\| row 1 \| (\d+) \|/.exec(LINES[head + 3]); // header, rule, row 0, row 1
      if (!m) throw new Error(`fixture row is not what the scenario expects: ${LINES[head + 3]}`);
      // A pipe table is a block widget while the caret is outside it, so steer to the paragraph before it, click a
      // cell of the widget, and let the editor reveal the table's source (the caret lands in the last row).
      const before = LINES.slice(0, head).map((l, i) => [l, i] as const).reverse().find(([l]) => /^Paragraph \d+ links/.test(l))!;
      const n = /^Paragraph (\d+) /.exec(before[0])![1];
      await goToLine(page, editor, LINES, before[1], `Paragraph ${n} links`);
      const widget = page.locator('[data-testid="serat-editor"] .serat-table-widget', { hasText: m[1] }).first();
      await widget.locator("tbody tr").nth(1).locator("td").first().click();
      await new Promise((r) => setTimeout(r, 300));
      const at = await caretLineText(editor);
      if (!at.startsWith("| row 3 |")) throw new Error(`the caret is on ${JSON.stringify(at.slice(0, 40))}, not in the revealed table source`);
      await page.keyboard.press("Home");
      await move(page, "ArrowRight", "| row 3".length); // inside the first cell of the last row, after its text
      return { caret: `revealed table source near line ${head}, first cell of its last row` };
    },
  },
  {
    id: "link-dense",
    title: "typing between the wikilinks of a link-dense paragraph",
    async place(page, editor) {
      const at = findLine(LINES, 0.6, (l) => /^Paragraph \d+ links/.test(l));
      const n = /^Paragraph (\d+) /.exec(LINES[at])![1];
      const line = await goToLine(page, editor, LINES, at, `Paragraph ${n} links`);
      await placeOnLine(page, editor, line, `Paragraph ${n} links`, "start");
      await move(page, "ArrowRight", LINES[at].indexOf("]], ") + 4); // after the first link and its comma
      return { caret: `source line ${at}, after the first of three wikilinks` };
    },
  },
  {
    id: "enter-mid",
    title: "Enter at the end of a mid-note paragraph",
    async place(page, editor) {
      const at = findLine(LINES, 0.4, (l) => /^Paragraph \d+ links/.test(l));
      const n = /^Paragraph (\d+) /.exec(LINES[at])![1];
      const line = await goToLine(page, editor, LINES, at, `Paragraph ${n} links`);
      await placeOnLine(page, editor, line, `Paragraph ${n} links`, "end", LINES[at].length);
      return { caret: `end of source line ${at}`, key: "Enter", expectInputs: false };
    },
  },
  {
    id: "backspace-bold",
    title: "Backspace through a bold span's closing delimiter",
    async place(page, editor) {
      const at = findLine(LINES, 0.55, (l) => /\*\*bold \d+\*\*/.test(l));
      const bold = /\*\*bold (\d+)\*\*/.exec(LINES[at])!;
      const line = await goToLine(page, editor, LINES, at, `bold ${bold[1]}`);
      await placeOnLine(page, editor, line, `bold ${bold[1]}`, "start");
      // Unmeasured preparation: grow the span to 520 characters so 500 Backspaces never run out of it, then step
      // over the closing "**". Measured keys start after this (runKeystrokes counts only keydowns after its start).
      await move(page, "ArrowRight", LINES[at].indexOf(`**bold ${bold[1]}**`) + `**bold ${bold[1]}`.length);
      await page.keyboard.type("y".repeat(520));
      await move(page, "ArrowRight", 2);
      return { caret: `source line ${at}, just after the closing ** of a 526-character bold span`, key: "Backspace", expectInputs: false };
    },
  },
  {
    id: "split-preview",
    title: "continuous typing in split-preview mode (live preview off)",
    open: { uiSettings: { live_preview: false, keybindings: [] } },
    async place(page, editor) {
      const at = findLine(LINES, 0.5, (l) => /^Paragraph \d+ links/.test(l));
      const n = /^Paragraph (\d+) /.exec(LINES[at])![1];
      const line = await goToLine(page, editor, LINES, at, `Paragraph ${n} links`);
      await placeOnLine(page, editor, line, `Paragraph ${n} links`, "end", LINES[at].length);
      expect(await page.locator(".serat-editor-split-preview").count(), "the preview pane is mounted").toBe(1);
      return { caret: `end of source line ${at}, preview pane mounted` };
    },
  },
  ...(["crlf", "bom+crlf"] as FixtureVariant[]).map((variant): Scenario => ({
    id: `variant-${variant.replace("+", "-")}`,
    title: `the fixture loaded as ${variant.toUpperCase()}, typing mid-note`,
    variant,
    async place(page, editor) {
      const at = findLine(LINES, 0.5, (l) => /^Paragraph \d+ links/.test(l));
      const n = /^Paragraph (\d+) /.exec(LINES[at])![1];
      const line = await goToLine(page, editor, LINES, at, `Paragraph ${n} links`);
      await placeOnLine(page, editor, line, `Paragraph ${n} links`, "end", LINES[at].length);
      return { caret: `end of source line ${at}` };
    },
  })),
];

test.describe("scenarios", () => {
  for (const sc of SCENARIOS) {
    test(`NFR-PRF-02 scenario [${sc.id}]: ${sc.title}; >= 95% painted in the first frame, 5,000-line note`, async ({ page }) => {
      // Task #221: normal duration is about 2.5 min (17 min step / 7 tests, minus unit and controls); 300 s is 2x.
      // 420 s x 7 made a stalled run last 49+ min (budgets run 465).
      test.setTimeout(300_000);
      const startedAt = Date.now();
      console.log(`SCENARIO-PROGRESS [${sc.id}] started`);
      const text = fixtureVariant(buildFixtureNote(), sc.variant ?? "lf");
      const subject = await openNoteForLatencyTest(page, { ...sc.open, text });
      try {
        const chromiumVersion = page.context().browser()?.version();
        const rs: number[] = [];
        const rows: string[] = [];
        const verdicts: string[] = [];
        let worst = 1;
        const cadences: [string, Cadence][] = [["strict", { kind: "strict" }], ["swept", { kind: "swept", seed: 1 }]];
        for (const [label, cadence] of cadences) {
          // Every cadence starts from a fresh document with the same text (Enter, Backspace and typed keys change it,
          // and re-opening the same note path would not reset the buffer). The init scripts re-run on reload.
          await page.reload();
          await page.waitForFunction(() => typeof (window as unknown as { __seratBenchOpenNote?: unknown }).__seratBenchOpenNote === "function", null, { timeout: 10_000 });
          await page.evaluate((t) => (window as unknown as { __seratBenchOpenNote: (s: string) => void }).__seratBenchOpenNote(t), text);
          await subject.editor.getByText("Section 0").first().waitFor({ timeout: 10_000 });
          const { refreshIntervalMs: R } = await installCapture(page);
          rs.push(R);
          await subject.editor.focus();
          const placed = await sc.place(page, subject.editor);
          expect(await caretLineText(subject.editor), "the caret is on a line after placement").not.toBeNull();
          await new Promise((r) => setTimeout(r, 600));
          const { scores, summary } = await runKeystrokes(page, subject.editor, SUBJECT_KEYS, {
            cadence, bar: SUBJECT_MIN_ON_TIME, key: placed.key, expectInputs: placed.expectInputs,
          });
          verdicts.push(summary.verdict);
          const on = percentOnTime(scores);
          worst = Math.min(worst, on);
          const d = latencyDiagnostic(scores);
          rows.push(`${label} (caret at ${placed.caret}; key ${placed.key ?? "x"}): ${fmt(summary)}; ${pct(on)} on time, p95 frames=${p95FramesToPaint(scores)}, ${d.withEntry}/${SUBJECT_KEYS} with ET entry (mean ${d.meanMs.toFixed(1)} ms, max ${d.maxMs.toFixed(1)} ms)`);
          const dump = process.env.SERAT_DUMP_DIR;
          if (dump) {
            const fs = await import("node:fs");
            fs.mkdirSync(dump, { recursive: true });
            fs.writeFileSync(`${dump}/${SUBJECT_LABEL}-scenario-${sc.id}-${label}.json`, JSON.stringify({ build: SUBJECT_LABEL, chromium: chromiumVersion, scenario: sc.id, cadence: label, R: rs[rs.length - 1], summary, scores }));
          }
        }
        console.log(`SCENARIO ${SUBJECT_LABEL} [${sc.id}] report-only; n=${SUBJECT_KEYS} per cadence, R=${rs.map((r) => r.toFixed(2)).join("/")}, load average ${os.loadavg().map((x) => x.toFixed(1)).join("/")} on ${os.cpus().length} CPUs, ${ENGINE(chromiumVersion)}: ${rows.join(" | ")}`);
        expect(verdicts, "every cadence must be PASS; INDETERMINATE (95% between the -4 and +4 ms shares) is not a pass").toEqual(["PASS", "PASS"]);
        expect(worst).toBeGreaterThanOrEqual(SUBJECT_MIN_ON_TIME);
      } finally {
        console.log(`SCENARIO-PROGRESS [${sc.id}] finished in ${Math.round((Date.now() - startedAt) / 1000)} s`);
        await subject.close();
      }
    });
  }
});
