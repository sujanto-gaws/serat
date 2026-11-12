// Task #139: the foldable callout `> [!tip]- Title` from the compat corpus
// (bench/compat/corpus/links/callouts.md) is rendered as a widget or left as
// source text, and is never rewritten — C-01, NFR-CMP-01, FR-EDT-01/02,
// architecture §9.3. Widgets only change what is displayed; the document is
// never touched (C-04).
//
// The property asserted is the one that can silently regress: no transaction the
// widget machinery takes part in (cursor into the callout, out of it, the widget
// being built) changes a byte of the note. How the fold marker is *drawn* is not
// pinned beyond "it is the tip callout, or it is source".

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { ensureSyntaxTree } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import { blockWidgetsExtension, blockWidgetsField, parseCalloutHeader } from "@/editor/blockWidgets";
import { seratMarkdown } from "@/editor/markdownLanguage";

const FIXTURE = readFileSync(
  resolve(__dirname, "../../../bench/compat/corpus/links/callouts.md"),
  "utf8",
);
const TIP_HEADER = "> [!tip]- Collapsed by default";

function stateAt(anchor: number): EditorState {
  const state = EditorState.create({
    doc: FIXTURE,
    selection: { anchor },
    extensions: [seratMarkdown(), blockWidgetsExtension()],
  });
  ensureSyntaxTree(state, state.doc.length, 5000);
  return state.update({ selection: { anchor } }).state;
}

function widgetsOf(state: EditorState): Array<{ from: number; to: number; widget: unknown }> {
  const out: Array<{ from: number; to: number; widget: unknown }> = [];
  const iter = state.field(blockWidgetsField).deco.iter();
  while (iter.value) {
    out.push({ from: iter.from, to: iter.to, widget: (iter.value.spec as { widget?: unknown }).widget });
    iter.next();
  }
  return out;
}

describe("foldable callout `[!tip]-` (C-01)", () => {
  const tipFrom = FIXTURE.indexOf(TIP_HEADER);

  it("the fixture is the one the compat corpus ships", () => {
    expect(tipFrom).toBeGreaterThan(0);
  });

  it("is recognised as a tip callout header, the trailing `-` kept out of the type", () => {
    const header = parseCalloutHeader(TIP_HEADER);
    expect(header?.type).toBe("tip");
    expect(header?.title).toContain("Collapsed by default");
  });

  it("is a widget over exactly its own blockquote, or no widget at all, never something else", () => {
    const state = stateAt(0);
    const covering = widgetsOf(state).filter((w) => w.from <= tipFrom && w.to > tipFrom);
    expect(covering.length).toBeLessThanOrEqual(1);
    for (const w of covering) {
      expect(w.from).toBe(tipFrom);
      const dom = (w.widget as { toDOM(): HTMLElement }).toDOM();
      expect(dom.getAttribute("data-callout")).toBe("tip");
    }
  });

  it("is never rewritten: building widgets and moving the cursor through it change no byte", () => {
    const inside = tipFrom + 4;
    for (const anchor of [0, inside, tipFrom, FIXTURE.length, 0]) {
      const before = stateAt(anchor);
      expect(before.doc.toString()).toBe(FIXTURE);
      for (const widget of widgetsOf(before)) {
        (widget.widget as { toDOM(): HTMLElement }).toDOM();
      }
      const moved = before.update({ selection: { anchor: inside } });
      expect(moved.docChanged).toBe(false);
      expect(moved.state.doc.toString()).toBe(FIXTURE);
    }
  });
});
