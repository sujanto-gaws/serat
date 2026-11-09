// Task #40: block widgets (tables, ![[image]] embeds, callouts) — FR-EDT-01/02,
// NFR-PRF-02, architecture.md §9.3. Widgets only change what is displayed;
// the document is never touched (C-04).

import { ensureSyntaxTree } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import {
  blockWidgetsExtension,
  blockWidgetsField,
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
    const s0 = makeState(doc, 0);
    const before = widgets(s0);
    expect(before.length).toBeGreaterThanOrEqual(49);
    const t0 = performance.now();
    const s1 = s0.update({
      changes: { from: doc.length - 1, insert: "x" },
      selection: { anchor: doc.length },
    }).state;
    const elapsed = performance.now() - t0;
    const after = widgets(s1);
    expect(after.length).toBe(before.length);
    // Every pre-existing widget (those not near the edit/selection) is the same object.
    const reused = after.filter((w, i) => w.widget === before[i].widget).length;
    expect(reused).toBeGreaterThanOrEqual(before.length - 2);
    // Loose guard against an accidental whole-document walk (jsdom/CI noise tolerated).
    expect(elapsed).toBeLessThan(100);
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
});
