// Task #40: block widgets (tables, ![[image]] embeds, callouts) — FR-EDT-01/02,
// NFR-PRF-02, architecture.md §9.3. Widgets only change what is displayed;
// the document is never touched (C-04).

import { ensureSyntaxTree, syntaxTree, syntaxTreeAvailable } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { describe, expect, it, vi } from "vitest";

import {
  blockWidgetsExtension,
  blockWidgetsField,
  reliableParsedTo,
  growthRegion,
  isImageTarget,
  parseCalloutHeader,
  stripBlockquoteMarkers,
} from "@/editor/blockWidgets";
import { assetUrl, isSeratAssetUrl } from "@/editor/assetUrl";
import { setResolutions } from "@/editor/linkState";
import { seratMarkdown } from "@/editor/markdownLanguage";
import type { LinkResolution } from "@/ipc/generated/LinkResolution";

function resolution(target: string, path: string | null): LinkResolution {
  return { target, path, subpath_found: null, via: "index" };
}

function makeState(doc: string, anchor = 0, resolutions: LinkResolution[] = []): EditorState {
  const state = EditorState.create({
    doc,
    selection: { anchor },
    extensions: [seratMarkdown(), blockWidgetsExtension()],
  });
  ensureSyntaxTree(state, state.doc.length, 5000);
  // `create` may have parsed only a prefix; rebuild the field against the full tree.
  return state.update({ selection: { anchor }, effects: setResolutions.of(resolutions) }).state;
}

function widgets(state: EditorState): Array<{ from: number; to: number; widget: unknown; block: boolean }> {
  const out: Array<{ from: number; to: number; widget: unknown; block: boolean }> = [];
  const iter = state.field(blockWidgetsField).deco.iter();
  while (iter.value) {
    const spec = iter.value.spec as { widget?: unknown; block?: boolean };
    out.push({ from: iter.from, to: iter.to, widget: spec.widget, block: !!spec.block });
    iter.next();
  }
  return out;
}

const TABLE = "| a | b |\n| - | - |\n| 1 | 2 |";

describe("pure helpers", () => {
  it("recognises image targets by extension", () => {
    expect(isImageTarget("pic.PNG")).toBe(true);
    expect(isImageTarget("pic.png|200")).toBe(true);
    expect(isImageTarget("note")).toBe(false);
    expect(isImageTarget("doc.pdf")).toBe(false);
  });
  it("builds the platform's asset URL form, percent-encoded per segment", () => {
    expect(assetUrl("a b/c#d.png", "other")).toBe("serat-asset://localhost/a%20b/c%23d.png");
    expect(assetUrl("a b/c#d.png", "windows")).toBe("http://serat-asset.localhost/a%20b/c%23d.png");
  });
  it("sanitize accepts both asset origins and nothing that merely starts like one", () => {
    expect(isSeratAssetUrl("serat-asset://localhost/x.png")).toBe(true);
    expect(isSeratAssetUrl("http://serat-asset.localhost/x.png")).toBe(true);
    expect(isSeratAssetUrl("http://serat-asset.localhost.evil.test/x.png")).toBe(false);
    expect(isSeratAssetUrl("https://example.com/x.png")).toBe(false);
  });
  it("parses callout headers and strips quote markers", () => {
    expect(parseCalloutHeader("> [!warning] Careful")).toEqual({ type: "warning", title: "Careful" });
    expect(parseCalloutHeader("> [!note]")).toEqual({ type: "note", title: "Note" });
    expect(parseCalloutHeader("> plain quote")).toBeNull();
    expect(stripBlockquoteMarkers("> a\n>b\n> > c")).toBe("a\nb\n> c");
  });
});

describe("blockWidgetsField", () => {
  it("replaces a table, a callout and an image embed with widgets when the cursor is elsewhere", () => {
    const doc = `intro\n\n${TABLE}\n\n> [!note] Hi\n> body\n\nx ![[pic.png]] y\n\nend`;
    const found = widgets(makeState(doc, 0, [resolution("pic.png", "assets/pic.png")]));
    expect(found).toHaveLength(3);
    expect(found.filter((w) => w.block)).toHaveLength(2);
    // The text itself is untouched (C-04).
    expect(makeState(doc, 0).doc.toString()).toBe(doc);
  });

  it("does not widget a non-image embed or an ordinary blockquote", () => {
    expect(widgets(makeState("![[note]]\n\n> just a quote", 0))).toHaveLength(0);
  });

  it("yields to raw source on a line the selection touches", () => {
    const doc = `intro\n\n${TABLE}\n\nend`;
    expect(widgets(makeState(doc, doc.indexOf("| 1")))).toHaveLength(0);
    expect(widgets(makeState(doc, 0))).toHaveLength(1);
  });

  it("an unchanged table keeps its widget object when typing elsewhere (no rebuild)", () => {
    const doc = `intro\n\n${TABLE}\n\nend`;
    const s0 = makeState(doc, 0);
    const before = widgets(s0)[0].widget;
    const s1 = s0.update({ changes: { from: 0, insert: "x" }, selection: { anchor: 1 } }).state;
    const after = widgets(s1);
    expect(after).toHaveLength(1);
    expect(after[0].widget).toBe(before);
  });

  it("editing a table's cell rebuilds only that table", () => {
    const doc = `${TABLE}\n\nmid\n\n${TABLE.replace("1", "7")}`;
    const s0 = makeState(doc, doc.indexOf("mid"));
    const [first0, second0] = widgets(s0);
    // Move cursor into table 1 (reveals it), edit, move out.
    const s1 = s0.update({ selection: { anchor: 2 } }).state;
    expect(widgets(s1)).toHaveLength(1);
    const s2 = s1.update({ changes: { from: 2, to: 3, insert: "Z" }, selection: { anchor: doc.indexOf("mid") } }).state;
    const [first2, second2] = widgets(s2);
    expect(first2.widget).not.toBe(first0.widget);
    expect(second2.widget).toBe(second0.widget);
  });

  it("a 5,000-line note: a keystroke in a paragraph reuses every one of 50 table widgets", () => {
    const parts: string[] = [];
    for (let i = 0; i < 50; i += 1) {
      parts.push(TABLE.replace("1", String(i)));
      for (let j = 0; j < 90; j += 1) parts.push(`paragraph ${i}.${j}`, "");
    }
    const doc = parts.join("\n");
    const s0 = makeState(doc, doc.indexOf("paragraph 0.45"));
    const before = widgets(s0);
    expect(before.length).toBe(50);
    const s1 = s0.update({
      changes: { from: doc.length - 1, insert: "x" },
      selection: { anchor: doc.length },
    }).state;
    const after = widgets(s1);
    expect(after.length).toBe(before.length);
    // Every pre-existing widget (those not near the edit/selection) is the same object.
    const reused = after.filter((w, i) => w.widget === before[i].widget).length;
    expect(reused).toBe(before.length);
    // NFR-PRF-02: the property is REUSE, asserted above by object identity, not wall time (a 100 ms
    // single-sample bound failed 1/389 under load, #99). A whole-document rebuild replaces every
    // widget, so identity catches it deterministically.
  });

  it("a table's FIRST character (offset 0): leaving it brings the widget back, same count as a fresh build", () => {
    const doc = `${TABLE}\n\nmid\n\n${TABLE.replace("1", "7")}`;
    const s0 = makeState(doc, 0); // cursor on the first character: table 1 is revealed
    expect(widgets(s0)).toHaveLength(1);
    const away = doc.indexOf("mid");
    const s1 = s0.update({ selection: { anchor: away } }).state;
    expect(widgets(s1)).toHaveLength(widgets(makeState(doc, away)).length);
    expect(widgets(s1)).toHaveLength(2);
  });

  it("the first character of a block in the middle of the note behaves the same", () => {
    const doc = `intro\n\n${TABLE}\n\nmid\n\n${TABLE.replace("1", "7")}\n\nend`;
    const first = doc.indexOf("| a |");
    const s0 = makeState(doc, first);
    expect(widgets(s0)).toHaveLength(1);
    const s1 = s0.update({ selection: { anchor: doc.length } }).state;
    expect(widgets(s1)).toHaveLength(widgets(makeState(doc, doc.length)).length);
    expect(widgets(s1)).toHaveLength(2);
    // And entering from elsewhere onto a first character reveals it.
    const s2 = s1.update({ selection: { anchor: first } }).state;
    expect(widgets(s2)).toHaveLength(1);
  });

  it("a table's LAST character: leaving it brings the widget back, same as a fresh build", () => {
    const doc = `${TABLE}\n\nmid paragraph\n\nend`;
    const last = TABLE.length; // offset just past the final character, the block's end
    for (const at of [last - 1, last]) {
      const s0 = makeState(doc, at);
      expect(widgets(s0)).toHaveLength(0);
      const away = doc.indexOf("mid");
      const s1 = s0.update({ selection: { anchor: away } }).state;
      expect(widgets(s1)).toHaveLength(1);
      expect(widgets(s1).map((w) => [w.from, w.to])).toEqual(widgets(makeState(doc, away)).map((w) => [w.from, w.to]));
    }
  });

  it("property: from every cursor position, moving to 0, a middle paragraph or the end equals a fresh build (FR-EDT-02)", () => {
    const t = (n: string) => TABLE.replace("1", n);
    const doc = `${t("a")}\n\nfirst paragraph\n\n${t("b")}\n\n> [!note] Title\n> body line\n\nmiddle paragraph\n\n${t("c")}\n\nlast paragraph`;
    const shape = (st: EditorState) => widgets(st).map((w) => [w.from, w.to]);
    const targets = [0, doc.indexOf("middle paragraph") + 3, doc.length];
    for (let from = 0; from <= doc.length; from += 1) {
      const s0 = makeState(doc, from);
      for (const to of targets) {
        const s1 = s0.update({ selection: { anchor: to } }).state;
        expect(shape(s1), `from ${from} to ${to}`).toEqual(shape(makeState(doc, to)));
      }
    }
  });

  it("the examined watermark maps through edits and never moves back", () => {
    const doc = `intro\n\n${TABLE}\n\nend`;
    const s0 = makeState(doc, 0);
    const w0 = s0.field(blockWidgetsField).examined;
    expect(w0).toBe(doc.length);
    const s1 = s0.update({ changes: { from: 0, insert: "xx" }, selection: { anchor: 2 } }).state;
    expect(s1.field(blockWidgetsField).examined).toBe(doc.length + 2);
    // Deleting before the watermark pulls it back by exactly that much, not further.
    const s2 = s1.update({ changes: { from: 0, to: 2 } }).state;
    expect(s2.field(blockWidgetsField).examined).toBe(doc.length);
  });

  it("growth re-examines only never-examined territory; a cut-back tree causes none", () => {
    expect(growthRegion(1000, 400)).toBeNull(); // tree cut back to the viewport: nothing to do
    expect(growthRegion(1000, 1000)).toBeNull();
    expect(growthRegion(400, 1000)).toEqual({ from: 400, to: 1000 });
  });

  it("leaves a table nested in a blockquote or list as raw source (no mid-line block replace)", () => {
    const quoted = `> ${TABLE.split("\n").join("\n> ")}\n\nend`;
    expect(widgets(makeState(quoted, quoted.length))).toHaveLength(0);
    const listed = `- item\n  ${TABLE.split("\n").join("\n  ")}\n\nend`;
    expect(widgets(makeState(listed, listed.length))).toHaveLength(0);
  });

  it("an image embed is a labelled placeholder until a resolved path exists, never an <img> from the name", () => {
    const state = makeState("x\n\n![[pic.png|200]]\n\nend", 0);
    const [found] = widgets(state);
    const dom = (found.widget as { toDOM(): HTMLElement }).toDOM();
    expect(dom.tagName).toBe("SPAN");
    expect(dom.textContent).toContain("pic.png");
    expect(dom.querySelector("img")).toBeNull();
  });

  it("a resolved image embed builds its serat-asset URL from the RESOLVED path, not the written name", () => {
    const doc = "x\n\n![[pic.png|200]]\n\nend";
    const state = makeState(doc, 0, [resolution("pic.png", "deep/folder/pic.png")]);
    const [found] = widgets(state);
    const img = (found.widget as { toDOM(): HTMLElement }).toDOM() as HTMLImageElement;
    expect(img.tagName).toBe("IMG");
    expect(img.getAttribute("src")).toBe(assetUrl("deep/folder/pic.png"));
    expect(img.getAttribute("src")).not.toContain("localhost/pic.png");
    expect(img.width).toBe(200);
    // A load failure degrades to the labelled placeholder, not a broken image.
    img.dispatchEvent(new Event("error"));
  });

  it("an embed that resolves to nothing stays a placeholder", () => {
    const state = makeState("x\n\n![[ghost.png]]\n\nend", 0, [resolution("ghost.png", null)]);
    const dom = (widgets(state)[0].widget as { toDOM(): HTMLElement }).toDOM();
    expect(dom.tagName).toBe("SPAN");
  });

  it("a callout body is built from text nodes: markup in it stays text", () => {
    const state = makeState("x\n\n> [!note] T\n> <img src=x onerror=alert(1)>\n\nend", 0);
    const [found] = widgets(state);
    const dom = (found.widget as { toDOM(): HTMLElement }).toDOM();
    expect(dom.querySelector("img")).toBeNull();
    expect(dom.textContent).toContain("<img src=x onerror=alert(1)>");
  });

  // ---- range selections (task #102): dirty regions cover the whole range, not just its head ----

  function makeRange(doc: string, anchor: number, head: number): EditorState {
    const state = EditorState.create({
      doc,
      selection: { anchor, head },
      extensions: [seratMarkdown(), blockWidgetsExtension()],
    });
    ensureSyntaxTree(state, state.doc.length, 5000);
    return state.update({ selection: { anchor, head }, effects: setResolutions.of([]) }).state;
  }
  const shapeOf = (st: EditorState) => widgets(st).map((w) => [w.from, w.to]);

  it("dragging a selection from a paragraph across a table reveals the table raw, equal to a fresh build", () => {
    const doc = `intro paragraph\n\n${TABLE}\n\nend paragraph`;
    const s0 = makeState(doc, 2);
    expect(widgets(s0)).toHaveLength(1);
    const head = doc.indexOf("end") + 2;
    const s1 = s0.update({ selection: { anchor: 2, head } }).state;
    expect(widgets(s1)).toHaveLength(0);
    expect(shapeOf(s1)).toEqual(shapeOf(makeRange(doc, 2, head)));
  });

  it("collapsing the selection away brings the widget back", () => {
    const doc = `intro paragraph\n\n${TABLE}\n\nend paragraph`;
    const head = doc.indexOf("end") + 2;
    const s1 = makeState(doc, 2).update({ selection: { anchor: 2, head } }).state;
    const s2 = s1.update({ selection: { anchor: 2 } }).state;
    expect(widgets(s2)).toHaveLength(1);
    expect(shapeOf(s2)).toEqual(shapeOf(makeState(doc, 2)));
  });

  it("select-all on a three-table note: zero widgets; collapse: all three back", () => {
    const t = (n: string) => TABLE.replace("1", n);
    const doc = `${t("a")}\n\nmid\n\n${t("b")}\n\nmid2\n\n${t("c")}\n\nend`;
    const s0 = makeState(doc, doc.length);
    expect(widgets(s0)).toHaveLength(3);
    const all = s0.update({ selection: { anchor: 0, head: doc.length } }).state;
    expect(widgets(all)).toHaveLength(0);
    expect(shapeOf(all)).toEqual(shapeOf(makeRange(doc, 0, doc.length)));
    const back = all.update({ selection: { anchor: doc.length } }).state;
    expect(widgets(back)).toHaveLength(3);
  });

  it("collapsed-cursor moves do not walk the tree over a range (only the selection-sized path walks)", () => {
    // The collapsed path takes touchedTopLevelBlocks only; assert by behaviour: a cursor move
    // across a 50-table note equals a fresh build and reuses untouched widget objects.
    const t = (n: number) => TABLE.replace("1", String(n));
    const doc = Array.from({ length: 50 }, (_, i) => `${t(i)}\n\np${i}`).join("\n\n");
    const s0 = makeState(doc, doc.indexOf("p0"));
    const before = widgets(s0);
    const s1 = s0.update({ selection: { anchor: doc.indexOf("p1") } }).state;
    const after = widgets(s1);
    expect(shapeOf(s1)).toEqual(shapeOf(makeState(doc, doc.indexOf("p1"))));
    expect(after.filter((w) => before.some((b) => b.widget === w.widget)).length).toBeGreaterThanOrEqual(48);
  });

  it("property: for every (anchor, head) pair in a small note the incremental decorations equal a fresh build", () => {
    const doc = `p one\n\n${TABLE}\n\np two\n\n> [!note] T\n> body\n\nend`;
    const mid = doc.indexOf("p two") + 2;
    // Start states hoisted: collapsed cursors, a range across the table, and a select-all.
    const starts: Array<[number, number]> = [
      [0, 0],
      [mid, mid],
      [doc.length, doc.length],
      [2, mid],
      [0, doc.length],
    ];
    const startStates = starts.map(([a, h]) => makeRange(doc, a, h));
    const base = makeState(doc, 0);
    const rebuild = setResolutions.of([]); // the effect forces a full buildAll: the fresh reference
    // Every 8th position plus every block boundary (blank-line edges) and both ends.
    const positions = new Set<number>([0, 1, doc.length - 1, doc.length]);
    for (let i = 0; i <= doc.length; i += 8) positions.add(i);
    for (let i = doc.indexOf("\n\n"); i >= 0; i = doc.indexOf("\n\n", i + 1)) [i, i + 2].forEach((p) => positions.has(p) || (p >= 0 && p <= doc.length && positions.add(p)));
    for (const anchor of positions) {
      for (const head of positions) {
        const target = shapeOf(base.update({ selection: { anchor, head }, effects: rebuild }).state);
        startStates.forEach((st, i) => {
          const got = shapeOf(st.update({ selection: { anchor, head } }).state);
          expect(got, `from ${starts[i]} to ${anchor}-${head}`).toEqual(target);
        });
      }
    }
  });

  it("Shift-extending a select-all on a 5,000-line note equals a fresh build", () => {
    // Informational: re-measured at ~0.5 ms, was ~20 ms with one replaceRegion per block.
    const t = (n: number) => TABLE.replace("1", String(n));
    const doc = Array.from({ length: 700 }, (_, i) => `${t(i)}\n\npara ${i}\n\nmore text ${i}`).join("\n\n");
    const all = makeRange(doc, 0, doc.length - 1);
    const t0 = performance.now();
    const next = all.update({ selection: { anchor: 0, head: doc.length } }).state;
    const ms = performance.now() - t0;
    console.log(`select-all extend by one char on ${doc.split("\n").length} lines: ${ms.toFixed(1)} ms`);
    expect(shapeOf(next)).toEqual(shapeOf(makeRange(doc, 0, doc.length)));
  });

  // ---- document changes (task #104): both ends, both sides, and the inserted span ----

  // Timing (informational; the 5 s default timeout is unchanged): unloaded, the deletion sweep
  // (506 edits) takes ~0.3 s and the block-end sweep (804 edits) ~0.35 s. Under 6 busy loops in
  // the full suite they took 0.5-2.1 s and 0.3-3.3 s; fresh references are cached by resulting
  // document, and the sweeps use 2 start states and a stride of 8.
  /** Fresh-build shapes keyed by (resulting doc, selection): many edits produce the same result. */
  const freshCache = new Map<string, string>();
  interface SweepTally {
    total: number;
    skipped: number;
  }

  /**
   * Applies `spec` to `state` incrementally and compares with a fresh build of the result.
   *
   * Under heavy CPU load CodeMirror's incremental reparse of a changed doc can be partial
   * (time-sliced; the partial tree still reports the full length) and the field cannot tell
   * until a later edit (#109). So the SAME update is retried from the same start state up to
   * 3 times while the tree is partial. Only the property sweeps pass `sweep`, which lets a
   * still-partial tree be counted as skipped (they assert a skip budget); everywhere else a
   * partial tree fails with the label.
   */
  function expectChangeMatchesFresh(
    state: EditorState,
    spec: { from: number; to?: number; insert?: string },
    label: string,
    sweep?: SweepTally,
  ): EditorState {
    if (sweep) sweep.total += 1;
    let next = state.update({ changes: spec }).state;
    for (let retry = 0; retry < 3 && !syntaxTreeAvailable(next, next.doc.length); retry += 1) {
      next = state.update({ changes: spec }).state;
    }
    if (!syntaxTreeAvailable(next, next.doc.length)) {
      if (!sweep) throw new Error(`${label}: the reparse stayed partial after 3 retries (machine too loaded?)`);
      // TODO(#109): remove this skip when the field notices a completed parse by itself.
      sweep.skipped += 1;
      return next;
    }
    const sel = next.selection.main;
    const docText = next.doc.toString();
    const key = `${sel.anchor}:${sel.head}:${docText}`;
    let fresh = freshCache.get(key);
    if (fresh === undefined) {
      fresh = JSON.stringify(shapeOf(makeRange(docText, sel.anchor, sel.head)));
      freshCache.set(key, fresh);
    }
    expect(JSON.stringify(shapeOf(next)), label).toEqual(fresh);
    return next;
  }

  function expectSkipBudget(sweep: SweepTally, name: string): void {
    console.info(`${name}: ${sweep.total} edits, ${sweep.skipped} skipped (partial reparse)`);
    expect(sweep.skipped).toBeLessThanOrEqual(Math.floor(sweep.total * 0.01));
  }

  it("deleting the text before a table brings it back to a widget/raw state equal to a fresh build", () => {
    const doc = `intro\n\n${TABLE}\n\nend`;
    const s0 = makeState(doc, doc.length);
    expect(widgets(s0)).toHaveLength(1);
    const s1 = expectChangeMatchesFresh(s0, { from: 0, to: "intro\n".length }, "delete `intro\\n`");
    expect(widgets(s1)).toHaveLength(1);
    expectChangeMatchesFresh(s0, { from: 0, to: "intro\n\n".length }, "delete the whole line + blank");
  });

  it("pasting a table over a selection equals a fresh build", () => {
    const doc = `one\n\ntwo\n\nthree`;
    const s0 = makeRange(doc, 0, doc.indexOf("two") + 3);
    const s1 = expectChangeMatchesFresh(s0, { from: 0, to: doc.indexOf("two") + 3, insert: TABLE }, "paste over selection");
    expect(widgets(s1)).toHaveLength(0); // the cursor sits at the end of the pasted table: revealed
    const s2 = s1.update({ selection: { anchor: s1.doc.length } }).state;
    expect(widgets(s2)).toHaveLength(1);
  });

  it("single-character typing in a paragraph reuses every widget object (no range walk)", () => {
    const t = (n: number) => TABLE.replace("1", String(n));
    const doc = Array.from({ length: 50 }, (_, i) => `${t(i)}\n\np${i}`).join("\n\n");
    const at = doc.indexOf("p25") + 1;
    const s0 = makeState(doc, at);
    const before = widgets(s0);
    const s1 = s0.update({ changes: { from: at, insert: "x" }, selection: { anchor: at + 1 } }).state;
    const after = widgets(s1);
    expect(after.length).toBe(before.length);
    expect(after.every((w) => before.some((b) => b.widget === w.widget))).toBe(true);
  });

  // The sweeps are split per start state (deletion) and per block end x start state (block end)
  // so each case is short under load, with its own tally and skip budget.
  const DEL_DOC = `p one\n\n${TABLE}\n\np two\n\n> [!note] T\n> body\n\nend`;
  it.each([0, DEL_DOC.length])("property: deleting [min, max] of each sampled pair, from a cursor at %i, equals a fresh build of the result", (cursor) => {
    const doc = DEL_DOC;
    const sweep: SweepTally = { total: 0, skipped: 0 };
    const start = makeState(doc, cursor);
    const positions = new Set<number>([0, 1, doc.length - 1, doc.length]);
    for (let i = 0; i <= doc.length; i += 8) positions.add(i);
    for (let i = doc.indexOf("\n\n"); i >= 0; i = doc.indexOf("\n\n", i + 1)) [i, i + 1, i + 2].forEach((p) => p <= doc.length && positions.add(p));
    const list = [...positions];
    for (const a of list) {
      for (const b of list) {
        if (b <= a) continue;
        expectChangeMatchesFresh(start, { from: a, to: b }, `delete ${a}-${b} from cursor ${cursor}`, sweep);
      }
    }
    expectSkipBudget(sweep, `deletion sweep (cursor ${cursor})`);
  });

  it("deleting a table's last row (cursor elsewhere) keeps a widget for the shortened table", () => {
    const doc = `intro\n\n${TABLE}\n\nend`;
    const s0 = makeState(doc, 0);
    const lastRow = doc.indexOf("| 1 | 2 |");
    const s1 = expectChangeMatchesFresh(s0, { from: lastRow - 1, to: lastRow + "| 1 | 2 |".length }, "delete last row");
    expect(widgets(s1)).toHaveLength(1);
  });

  it("inserting a newline at the start of a table's last row, or text after the block, equals a fresh build", () => {
    const doc = `intro\n\n${TABLE}\n\nend`;
    const s0 = makeState(doc, 0);
    const lastRow = doc.indexOf("| 1 | 2 |");
    expectChangeMatchesFresh(s0, { from: lastRow, insert: "\n" }, "newline at last row start");
    const blockEnd = lastRow + "| 1 | 2 |".length;
    expectChangeMatchesFresh(s0, { from: blockEnd, insert: "\n" }, "newline at block end");
    expectChangeMatchesFresh(s0, { from: blockEnd, insert: "\n> [!tip] x\n" }, "callout header at block end");
  });

  const BE_DOC = DEL_DOC;
  const BE_ENDS: number[] = [];
  for (let i = BE_DOC.indexOf("\n\n"); i >= 0; i = BE_DOC.indexOf("\n\n", i + 1)) BE_ENDS.push(i);
  BE_ENDS.push(BE_DOC.length);
  const BE_CASES = BE_ENDS.flatMap((end) => [0, BE_DOC.length].map((cursor) => [end, cursor] as const));
  it.each(BE_CASES)("property: every deletion and insertion near the block end at %i, from a cursor at %i, equals a fresh build", (end, cursor) => {
    const doc = BE_DOC;
    const list: number[] = [];
    for (let d = -12; d <= 3; d += 1) if (end + d >= 0 && end + d <= doc.length) list.push(end + d);
    const sweep: SweepTally = { total: 0, skipped: 0 };
    const st = makeState(doc, cursor);
    for (const a of list) {
      for (const ins of ["\n", "\n> [!tip] x\n"]) expectChangeMatchesFresh(st, { from: a, insert: ins }, `insert ${JSON.stringify(ins)} at ${a}`, sweep);
      for (const b of list) if (b > a && b - a <= 8) expectChangeMatchesFresh(st, { from: a, to: b }, `delete ${a}-${b}`, sweep);
    }
    expectSkipBudget(sweep, `block-end sweep (end ${end}, cursor ${cursor})`);
  });

  // ---- partial reparses (task #109) ----

  /** Applies `spec` with CodeMirror's parse budget forced to expire (Date.now jumps 50 ms per call). */
  function updateWithPartialParse(state: EditorState, spec: { from: number; to?: number; insert?: string }): EditorState {
    let clock = 0;
    const spy = vi.spyOn(Date, "now").mockImplementation(() => (clock += 50));
    try {
      return state.update({ changes: spec }).state;
    } finally {
      spy.mockRestore();
    }
  }

  it("reliableParsedTo: the document end for a finished parse, a boundary short of it for a partial one", () => {
    const doc = Array.from({ length: 30 }, (_, i) => `${TABLE}\n\npara ${i}`).join("\n\n");
    const done = makeState(doc, 0);
    expect(reliableParsedTo(done)).toBe(doc.length);
    const partial = updateWithPartialParse(done, { from: 3, insert: "z" });
    expect(syntaxTreeAvailable(partial, partial.doc.length)).toBe(false);
    expect(reliableParsedTo(partial)).toBeLessThan(partial.doc.length);
    expect(syntaxTreeAvailable(partial, reliableParsedTo(partial))).toBe(true);
  });

  // Recorded gap: the partial parse here is forced on a bare EditorState by expiring CodeMirror's
  // work budget; no test drives it through a mounted EditorView and its idle parse worker.
  const PARTIAL_DOC = Array.from({ length: 30 }, (_, i) => `${TABLE}\n\npara ${i}`).join("\n\n");
  const PARTIAL_INSERTS = ["```\n", "\n", "x", "> "];

  it.each(PARTIAL_INSERTS)("an edit (%j) applied during a partial parse: once the parse completes the widgets equal a fresh build", (insert) => {
    const doc = PARTIAL_DOC;
    const base = makeState(doc, 0);
    let partials = 0;
    let total = 0;
    for (let at = 0; at < doc.length; at += 7) {
      total += 1;
      const s2 = updateWithPartialParse(base, { from: at, insert });
      if (!syntaxTreeAvailable(s2, s2.doc.length)) partials += 1;
      ensureSyntaxTree(s2, s2.doc.length, 5000);
      const s3 = s2.update({}).state; // the completion transaction: no change, tree identity differs
      const fresh = s3.update({ effects: setResolutions.of([]) }).state;
      expect(shapeOf(s3), `insert ${JSON.stringify(insert)} at ${at}`).toEqual(shapeOf(fresh));
    }
    expect(partials).toBeGreaterThan(total / 2); // the forced budget really produced partial parses
  });

  it.each(PARTIAL_INSERTS)("a resolutions rebuild (buildAll) arriving mid partial parse (edit %j) does not mark unparsed territory examined", (insert) => {
    // buildAll also runs on a setResolutions effect, which can land while the parse is partial.
    // Its watermark must be the parse boundary, not the (possibly full-length) partial tree's length.
    const doc = PARTIAL_DOC;
    const base = makeState(doc, 0);
    let partials = 0;
    let total = 0;
    for (let at = 0; at < doc.length; at += 7) {
      total += 1;
      const s2 = updateWithPartialParse(base, { from: at, insert });
      if (!syntaxTreeAvailable(s2, s2.doc.length)) partials += 1;
      const rebuilt = s2.update({ effects: setResolutions.of([]) }).state;
      ensureSyntaxTree(rebuilt, rebuilt.doc.length, 5000);
      const s3 = rebuilt.update({}).state;
      const fresh = s3.update({ effects: setResolutions.of([]) }).state;
      expect(shapeOf(s3), `insert ${JSON.stringify(insert)} at ${at}`).toEqual(shapeOf(fresh));
    }
    expect(partials).toBeGreaterThan(total / 2);
  });

  it("buildAll's watermark is the parse boundary, not the partial tree's length (create and a resolutions rebuild)", () => {
    // Pins `examined: reliableParsedTo(state)` in buildAll. A mutant using syntaxTree(state).length
    // there is not observable through the decorations (the nodes inside the partial tree are
    // correct), so the watermark itself is asserted. Guarded: the partial tree must really be
    // longer than the parsed boundary for the assertion to mean anything.
    const T = (n: number) => `${TABLE}\n\npara ${n}`;
    const doc = Array.from({ length: 100 }, (_, i) => T(i)).join("\n\n");
    const created = EditorState.create({ doc, selection: { anchor: 0 }, extensions: [seratMarkdown(), blockWidgetsExtension()] });
    expect(syntaxTreeAvailable(created, created.doc.length)).toBe(false);
    expect(syntaxTree(created).length).toBeGreaterThan(reliableParsedTo(created));
    expect(created.field(blockWidgetsField).examined).toBe(reliableParsedTo(created));
    const rebuilt = created.update({ effects: setResolutions.of([]) }).state;
    if (!syntaxTreeAvailable(rebuilt, rebuilt.doc.length)) {
      expect(rebuilt.field(blockWidgetsField).examined).toBe(reliableParsedTo(rebuilt));
    }
  });

  it("a finished parse keeps examined at the document end", () => {
    const doc = `intro\n\n${TABLE}\n\nend`;
    const s0 = makeState(doc, 0);
    const s1 = s0.update({ changes: { from: 2, insert: "x" }, selection: { anchor: 3 } }).state;
    expect(s1.field(blockWidgetsField).examined).toBe(s1.doc.length);
  });
});
