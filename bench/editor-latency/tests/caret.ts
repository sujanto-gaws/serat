// Caret placement for the NFR-PRF-02 scenarios (task #61). Serves NFR-PRF-02, FR-EDT-01, C-06.
//
// The editor virtualises its lines and lives in an open shadow root, so a scenario that must type at a
// SPECIFIC line (a table row, a link-dense paragraph, a bold span) first scrolls that line into the viewport,
// clicks it with the real mouse, and then ASSERTS where the caret landed by reading the caret's own line from
// the shadow root's selection. A scenario whose caret is not where it claims fails before any key is scored:
// a latency figure for "typing in a table cell" taken with the caret in a paragraph would be worse than none.
import type { Locator, Page } from "@playwright/test";

const EDITOR = '[data-testid="serat-editor"]';

/** The text of the line holding the caret ("" when there is none), read from the shadow root. */
export async function caretLineText(editor: Locator): Promise<string> {
  return editor.evaluate((el) => {
    const sel = (el.getRootNode() as unknown as { getSelection?: () => Selection | null }).getSelection?.() ?? null;
    const n = sel?.focusNode ?? null;
    const e = n && (n.nodeType === 1 ? (n as Element) : n.parentElement);
    return e?.closest(".cm-line")?.textContent ?? "";
  });
}

async function renderedLines(editor: Locator): Promise<string[]> {
  return editor.evaluate((el) => [...(el.getRootNode() as ShadowRoot).querySelectorAll(".cm-line")].map((l) => l.textContent ?? ""));
}

/** Source line index of a rendered "Paragraph n ..." line, or -1. */
function paragraphIndex(lines: string[], rendered: string): number {
  const m = /^Paragraph (\d+) /.exec(rendered);
  return m ? lines.findIndex((l) => l.startsWith(`Paragraph ${m[1]} `)) : -1;
}

/** Scrolls until a rendered line contains `needle`, steering by the visible numbered paragraphs, and returns it.
 * `lines` are the LF source lines of the loaded note (the CRLF and BOM variants hold the same lines). */
export async function goToLine(page: Page, editor: Locator, lines: string[], target: number, needle: string): Promise<Locator> {
  const scroller = page.locator(`${EDITOR} .cm-scroller`);
  const match = page.locator(`${EDITOR} .cm-line`, { hasText: needle });
  const pxPerLine = await scroller.evaluate((s, n) => s.scrollHeight / n, lines.length);
  await scroller.evaluate((s, top) => { s.scrollTop = top; }, Math.max(0, target * pxPerLine - 200));
  for (let attempt = 0; attempt < 25; attempt++) {
    await new Promise((r) => setTimeout(r, 150));
    if ((await match.count()) >= 1) {
      await match.first().scrollIntoViewIfNeeded();
      return match.first();
    }
    const seen = (await renderedLines(editor)).map((t) => paragraphIndex(lines, t)).filter((i) => i >= 0);
    if (seen.length === 0) throw new Error(`goToLine(${target}): no numbered paragraph is rendered to steer by`);
    const here = seen[Math.floor(seen.length / 2)];
    await scroller.evaluate((s, delta) => { s.scrollTop += delta; }, (target - here) * pxPerLine * 0.9);
  }
  throw new Error(`goToLine(${target}): "${needle}" never rendered`);
}

/** Clicks the line's start with the real mouse and asserts the caret line. For `at: "end"` it then walks ArrowRight
 * across the line's raw source (`rawLength` characters), because End stops at the end of the first VISUAL line of a
 * wrapped paragraph, and a click on the right-hand blank area does not leave a caret CodeMirror's End key respects. */
export async function placeOnLine(page: Page, editor: Locator, line: Locator, needle: string, at: "start" | "end", rawLength = 0): Promise<void> {
  // Layout can shift between the measurement and the click (widgets re-measure after a scroll), so a click that
  // lands on a neighbouring line is retried; a caret that never reaches the wanted line fails the scenario.
  let text = "";
  for (let attempt = 0; attempt < 4; attempt++) {
    await line.scrollIntoViewIfNeeded();
    await new Promise((r) => setTimeout(r, 250));
    const box = await line.boundingBox();
    if (!box) throw new Error(`line "${needle}" has no bounding box`);
    await page.mouse.click(box.x + 2, box.y + 6);
    await page.keyboard.press("Home");
    text = await caretLineText(editor);
    if (text.includes(needle)) break;
    if (attempt === 3) throw new Error(`caret is on ${JSON.stringify(text.slice(0, 60))}, wanted a line containing "${needle}"`);
  }
  if (at === "end") {
    if (rawLength <= 0) throw new Error("placeOnLine(end) needs the raw source length of the line");
    await move(page, "ArrowRight", rawLength);
    const after = await caretLineText(editor);
    if (!after.includes(needle)) throw new Error(`after walking to the end the caret is on ${JSON.stringify(after.slice(0, 60))}`);
  }
}

export async function move(page: Page, key: "ArrowRight" | "ArrowLeft", times: number): Promise<void> {
  for (let i = 0; i < times; i++) await page.keyboard.press(key);
}

/** First source line index at or after `fromFraction` of the note that satisfies `test`. */
export function findLine(lines: string[], fromFraction: number, test: (l: string) => boolean): number {
  const from = Math.floor(lines.length * fromFraction);
  const i = lines.findIndex((l, k) => k >= from && test(l));
  if (i < 0) throw new Error(`no fixture line at or after ${fromFraction} satisfies the scenario's predicate`);
  return i;
}

// ---- Middle-of-note placement (task #202) ----------------------------------------------------------------
// Serves NFR-PRF-02, NFR-MNT-01. Budgets run 442 threw from the subject's middle placement: the old code set
// scrollTop to half of CodeMirror's ESTIMATED height, slept a fixed 400 ms and clicked once; on a starved runner
// the viewport re-measured after the click target was chosen, the caret landed outside 45-55% (or on no
// Paragraph line), and an assertion threw before any key was scored. That is a harness exception, not a verdict.
// Placement now waits for a positive signal (scroll geometry unchanged for several frames), asserts where the
// caret landed, steers from what it observed and retries until a bounded deadline, then fails with every attempt.

export interface Attempt { ok: boolean; note: string }

/** Runs `attempt` until it reports ok or `deadlineMs` elapses (measured by `now`). Always makes at least one
 * attempt; never starts one after the deadline. Throws with the notes of every attempt. `label` names the goal. */
export async function retryUntilDeadline(
  label: string,
  attempt: (n: number, remainingMs: number) => Promise<Attempt>,
  deadlineMs: number,
  now: () => number = () => Date.now(),
): Promise<string> {
  const start = now();
  const notes: string[] = [];
  for (let n = 0; n === 0 || now() - start < deadlineMs; n++) {
    const r = await attempt(n, Math.max(1, deadlineMs - (now() - start)));
    notes.push(`#${n + 1}: ${r.note}`);
    if (r.ok) return `${r.note} after ${n + 1} attempt(s)`; // the count is on the run log: each retry warms the height map
  }
  throw new Error(`${label}: not reached within ${deadlineMs} ms (harness could not place the caret; this is not a latency verdict). Attempts: ${notes.join(" | ")}`);
}

/** Where the caret landed relative to the wanted band, as an Attempt. */
export function judgePlacement(idx: number, total: number, lo: number, hi: number): Attempt {
  if (idx < 0) return { ok: false, note: "caret is on no recognisable Paragraph line" };
  const frac = idx / total;
  const ok = frac > lo && frac < hi;
  return { ok, note: `source line ${idx} of ${total} (${(frac * 100).toFixed(1)}%)${ok ? "" : ` outside ${lo * 100}-${hi * 100}%`}` };
}

/** Resolves once the scroller's scrollTop and scrollHeight have not changed for `frames` consecutive animation
 * frames: CodeMirror has finished re-measuring. Rejects (with the geometry) when `timeoutMs` passes first. */
async function stableOrMiss(scroller: Locator, timeoutMs: number): Promise<Attempt | null> {
  try { await waitForStableScroll(scroller, 8, timeoutMs); return null; } catch (e) { return { ok: false, note: String((e as Error).message ?? e) }; }
}

export async function waitForStableScroll(scroller: Locator, frames = 8, timeoutMs = 10_000): Promise<void> {
  await scroller.evaluate((s, args) => new Promise<void>((resolve, reject) => {
    const t0 = performance.now();
    let last = "", same = 0;
    const tick = () => {
      const g = `${s.scrollTop}/${s.scrollHeight}`;
      same = g === last ? same + 1 : 0;
      last = g;
      if (same >= args.frames) return resolve();
      if (performance.now() - t0 > args.timeoutMs) return reject(new Error(`scroll geometry never settled (last ${g})`));
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }), { frames, timeoutMs });
}

/** Clicks into the middle of the note and returns a description of the Paragraph line the caret is on, which is
 * asserted to sit strictly between `lo` and `hi` of the note's lines. Bounded by `deadlineMs`. */
export async function placeCaretMiddle(page: Page, editor: Locator, lines: string[], lo = 0.45, hi = 0.55, deadlineMs = 30_000): Promise<string> {
  const scroller = page.locator(`${EDITOR} .cm-scroller`);
  const lineOf = new Map<number, number>();
  lines.forEach((l, i) => { const m = /^Paragraph (\d+) /.exec(l); if (m) lineOf.set(Number(m[1]), i); });
  const wanted = Math.round(lines.length * (lo + hi) / 2);
  let steer = 0; // px correction learnt from the last miss
  const note = await retryUntilDeadline("middle: caret within the band", async (_n, remaining) => {
    const t0 = Date.now(), left = () => Math.max(1, remaining - (Date.now() - t0));
    let miss = await stableOrMiss(scroller, left());
    if (miss) return miss;
    await scroller.evaluate((s, d) => { s.scrollTop = (s.scrollHeight - s.clientHeight) / 2 + d; }, steer);
    miss = await stableOrMiss(scroller, left());
    if (miss) return miss;
    const box = await scroller.boundingBox();
    if (!box) return { ok: false, note: "editor scroller has no bounding box" };
    await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
    let idx = -1;
    for (let step = 0; step < 30 && idx < 0; step++) {
      const m = /Paragraph (\d+) /.exec(await caretLineText(editor));
      if (m && lineOf.has(Number(m[1]))) idx = lineOf.get(Number(m[1])) as number;
      else await page.keyboard.press("ArrowDown");
    }
    const r = judgePlacement(idx, lines.length, lo, hi);
    if (!r.ok && idx >= 0) {
      const pxPerLine = await scroller.evaluate((s, n) => s.scrollHeight / n, lines.length);
      steer += (wanted - idx) * pxPerLine;
    }
    return r;
  }, deadlineMs);
  // A final settle before the first scored key; it is a positive signal and the caller's fixed pause stays for start/end.
  const last = await stableOrMiss(scroller, 10_000);
  if (last) throw new Error(`middle: caret placed (${note}) but the editor did not settle: ${last.note} (harness could not settle; this is not a latency verdict)`);
  return note;
}
