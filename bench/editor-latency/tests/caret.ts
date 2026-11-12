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

/** Clicks the line (real mouse, trusted) at its start or end, then normalises with Home or End, and asserts the caret line. */
export async function placeOnLine(page: Page, editor: Locator, line: Locator, needle: string, at: "start" | "end"): Promise<void> {
  const box = await line.boundingBox();
  if (!box) throw new Error(`line "${needle}" has no bounding box`);
  if (at === "start") await page.mouse.click(box.x + 2, box.y + 6);
  else await page.mouse.click(box.x + box.width - 2, box.y + box.height - 6);
  await page.keyboard.press(at === "start" ? "Home" : "End");
  const text = await caretLineText(editor);
  if (!text.includes(needle)) throw new Error(`caret is on ${JSON.stringify(text.slice(0, 60))}, wanted a line containing "${needle}"`);
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
