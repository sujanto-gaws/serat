// FR-EDT-02: syntax markers hide except on lines that intersect the
// selection. Tested as pure functions against a plain `EditorState` (no
// mounted `EditorView`/DOM needed) — `collectHiddenMarkerSpans` is exactly
// what `revealMarkersPlugin`'s decorator calls on every update, so this is
// the same computation the running editor does, just invoked directly.

import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import {
  collectHiddenMarkerSpans,
  customMarkerSpans,
  selectedLineNumbers,
  type Span,
} from "@/editor/reveal";
import { seratMarkdown } from "@/editor/markdownLanguage";

function stateFor(doc: string, selection?: { anchor: number; head?: number }): EditorState {
  return EditorState.create({
    doc,
    selection: selection ? { anchor: selection.anchor, head: selection.head ?? selection.anchor } : undefined,
    extensions: [seratMarkdown()],
  });
}

function fullRange(state: EditorState): Span[] {
  return [{ from: 0, to: state.doc.length }];
}

describe("selectedLineNumbers", () => {
  it("a collapsed cursor touches exactly its own line", () => {
    const doc = "one\ntwo\nthree";
    const state = stateFor(doc);
    const lines = selectedLineNumbers(state.doc, [{ from: 5, to: 5 }]); // inside "two"
    expect(lines).toEqual(new Set([2]));
  });

  it("a multi-line selection touches every line it spans", () => {
    const state = stateFor("one\ntwo\nthree\nfour");
    // from inside "one" to inside "three"
    const lines = selectedLineNumbers(state.doc, [{ from: 1, to: 10 }]);
    expect(lines).toEqual(new Set([1, 2, 3]));
  });

  it("multiple selection ranges union their lines", () => {
    const state = stateFor("a\nb\nc\nd\ne");
    const lines = selectedLineNumbers(state.doc, [
      { from: 0, to: 0 },
      { from: 6, to: 6 },
    ]);
    expect(lines).toEqual(new Set([1, 4]));
  });
});

describe("customMarkerSpans", () => {
  it("SeratWikiLink hides the double brackets, not the target text", () => {
    const text = "[[note]]";
    const spans = customMarkerSpans("SeratWikiLink", 10, 10 + text.length, text);
    expect(spans).toEqual([
      { from: 10, to: 12 },
      { from: 16, to: 18 },
    ]);
  });

  it("SeratEmbed hides the bang and double brackets", () => {
    const text = "![[image.png]]";
    const spans = customMarkerSpans("SeratEmbed", 0, text.length, text);
    expect(spans).toEqual([
      { from: 0, to: 3 },
      { from: 12, to: 14 },
    ]);
  });

  it("SeratHighlight hides both == delimiters", () => {
    const text = "==important==";
    const spans = customMarkerSpans("SeratHighlight", 0, text.length, text);
    expect(spans).toEqual([
      { from: 0, to: 2 },
      { from: 11, to: 13 },
    ]);
  });

  it("an unrecognised node type hides nothing", () => {
    expect(customMarkerSpans("Paragraph", 0, 5, "hello")).toEqual([]);
  });
});

describe("collectHiddenMarkerSpans — the reveal rule end to end", () => {
  it("hides bold markers on a line the selection does not touch", () => {
    const state = stateFor("plain\n**bold**\nplain");
    const selectedLines = selectedLineNumbers(state.doc, [{ from: 0, to: 0 }]); // line 1 only
    const spans = collectHiddenMarkerSpans(state, fullRange(state), selectedLines);
    // Line 2 is "**bold**" at offset 6..14; the ** markers are at 6-8 and 12-14.
    expect(spans).toContainEqual({ from: 6, to: 8 });
    expect(spans).toContainEqual({ from: 12, to: 14 });
  });

  it("reveals (does not hide) markers on a line the selection touches", () => {
    const state = stateFor("plain\n**bold**\nplain");
    const cursorInsideBoldLine = selectedLineNumbers(state.doc, [{ from: 8, to: 8 }]); // inside "bold"
    const spans = collectHiddenMarkerSpans(state, fullRange(state), cursorInsideBoldLine);
    expect(spans).not.toContainEqual({ from: 6, to: 8 });
    expect(spans).not.toContainEqual({ from: 12, to: 14 });
  });

  it("hides wikilink brackets off the selected line and reveals them on it", () => {
    const state = stateFor("see [[Target Note]] here");
    const hidden = collectHiddenMarkerSpans(state, fullRange(state), new Set()); // nothing selected on line 1... wait line 1 is always line 1
    // With no selection lines recorded at all, everything on line 1 hides.
    expect(hidden.length).toBeGreaterThan(0);

    const revealed = collectHiddenMarkerSpans(state, fullRange(state), new Set([1]));
    expect(revealed).toEqual([]);
  });

  it("never hides a block reference marker, even off the selected line", () => {
    const state = stateFor("plain\nsome text ^block1\nplain");
    const spans = collectHiddenMarkerSpans(state, fullRange(state), new Set([1])); // line 2 not selected
    // "^block1" starts at offset 6+10=16... just assert nothing in that
    // region was hidden, by checking no span's `from` falls in line 2.
    const line2 = state.doc.line(2);
    const anyInLine2 = spans.some((s) => s.from >= line2.from && s.from < line2.to);
    expect(anyInLine2).toBe(false);
  });

  it("never hides an inline field marker (key::)", () => {
    const state = stateFor("status:: done\nplain");
    const spans = collectHiddenMarkerSpans(state, fullRange(state), new Set([2]));
    expect(spans).toEqual([]);
  });

  it("hides a highlight's == delimiters off the selected line", () => {
    const state = stateFor("plain\n==important==\nplain");
    const spans = collectHiddenMarkerSpans(state, fullRange(state), new Set([1]));
    const line2 = state.doc.line(2);
    expect(spans).toContainEqual({ from: line2.from, to: line2.from + 2 });
    expect(spans).toContainEqual({ from: line2.to - 2, to: line2.to });
  });

  it("restricting the walk to a range (simulating visibleRanges) only reports spans in that range", () => {
    const state = stateFor("**a**\n**b**\n**c**");
    const line2 = state.doc.line(2);
    const spans = collectHiddenMarkerSpans(state, [{ from: line2.from, to: line2.to }], new Set());
    expect(spans.every((s) => s.from >= line2.from && s.to <= line2.to)).toBe(true);
    expect(spans.length).toBeGreaterThan(0);
  });
});
