// Table tests for ADR-0016 §1/§2's coordinate mapping and patch-set
// generation. Task #21 (FR-EDT-01/02/03/10, NFR-PRF-02): CRLF, BOM,
// multi-range and UTF-16-column cases, per the task brief.

import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import { composeSince, InvalidPositionError, offsetAt, patchesFromChangeSet, posAt, textFromString } from "@/editor/patches";

describe("textFromString / posAt / offsetAt", () => {
  it("splits on LF", () => {
    const doc = textFromString("one\ntwo\nthree");
    expect(doc.lines).toBe(3);
    expect(posAt(doc, 0)).toEqual({ line: 0, col: 0 });
    expect(posAt(doc, 4)).toEqual({ line: 1, col: 0 });
    expect(posAt(doc, 5)).toEqual({ line: 1, col: 1 });
  });

  it("splits on CRLF, and the terminator is never part of either line's text", () => {
    const doc = textFromString("one\r\ntwo\r\nthree");
    expect(doc.lines).toBe(3);
    expect(doc.line(1).text).toBe("one");
    expect(doc.line(2).text).toBe("two");
    expect(doc.line(3).text).toBe("three");
    // Offset 3 is the position right after "one", the end of line 0.
    expect(posAt(doc, 3)).toEqual({ line: 0, col: 3 });
    // Whatever the terminator's own width in the source bytes, CodeMirror's
    // line model places exactly one addressable position (a line boundary)
    // between two lines: there is no "inside the terminator" position, which
    // is the coordinate-model form of ADR-0016 §1's "no position can fall
    // inside a \r\n" — the terminator is not addressable at all, not a
    // two-wide gap.
    expect(offsetAt(doc, { line: 1, col: 0 })).toBe(doc.line(2).from);
    expect(posAt(doc, doc.line(2).from)).toEqual({ line: 1, col: 0 });
  });

  it("splits on a lone CR", () => {
    const doc = textFromString("one\rtwo\rthree");
    expect(doc.lines).toBe(3);
    expect(doc.line(1).text).toBe("one");
    expect(doc.line(2).text).toBe("two");
    expect(doc.line(3).text).toBe("three");
    expect(posAt(doc, 4)).toEqual({ line: 1, col: 0 });
  });

  it("keeps a BOM as the first character of line 0's text", () => {
    const doc = textFromString("﻿title\nbody");
    expect(doc.line(1).text).toBe("﻿title");
    // The BOM occupies column 0; "title" starts at column 1, not 0 — every
    // downstream column on line 0 is shifted by exactly one code unit,
    // which is the property ADR-0016 §1 calls out as depending on never
    // stripping it.
    expect(posAt(doc, 1)).toEqual({ line: 0, col: 1 });
    expect(offsetAt(doc, { line: 0, col: 1 })).toBe(1);
  });

  it("counts UTF-16 code units, not code points, for astral characters", () => {
    // U+1F600 (😀) is a surrogate pair: two UTF-16 code units.
    const doc = textFromString("a😀b");
    expect(doc.line(1).text.length).toBe(4);
    expect(posAt(doc, 1)).toEqual({ line: 0, col: 1 }); // right after "a", before the pair
    expect(posAt(doc, 3)).toEqual({ line: 0, col: 3 }); // between the pair and "b"
    expect(offsetAt(doc, { line: 0, col: 3 })).toBe(3);
  });

  it("offsetAt is the exact inverse of posAt across a whole document, mixed terminators included", () => {
    const doc = textFromString("alpha\r\nbeta\rgamma\ndelta");
    for (let offset = 0; offset <= doc.length; offset++) {
      expect(offsetAt(doc, posAt(doc, offset))).toBe(offset);
    }
  });
});

describe("patchesFromChangeSet", () => {
  function change(doc: string, ...specs: Array<{ from: number; to?: number; insert?: string }>) {
    const state = EditorState.create({ doc });
    const tr = state.update({ changes: specs });
    return { base: state.doc, changes: tr.changes };
  }

  it("emits a single insert patch in base coordinates", () => {
    const { base, changes } = change("hello world", { from: 5, insert: "," });
    const patches = patchesFromChangeSet(base, changes);
    expect(patches).toEqual([{ from: { line: 0, col: 5 }, to: { line: 0, col: 5 }, insert: "," }]);
  });

  it("emits a replacement patch (from !== to)", () => {
    const { base, changes } = change("hello world", { from: 0, to: 5, insert: "goodbye" });
    const patches = patchesFromChangeSet(base, changes);
    expect(patches).toEqual([{ from: { line: 0, col: 0 }, to: { line: 0, col: 5 }, insert: "goodbye" }]);
  });

  it("emits ordered, disjoint patches for a multi-range edit", () => {
    const { base, changes } = change("line one\nline two\nline three", {
      from: 0,
      to: 4,
      insert: "LINE",
    }, {
      from: 18,
      to: 22,
      insert: "LINE",
    });
    const patches = patchesFromChangeSet(base, changes);
    expect(patches).toHaveLength(2);
    expect(patches[0]).toEqual({ from: { line: 0, col: 0 }, to: { line: 0, col: 4 }, insert: "LINE" });
    expect(patches[1]).toEqual({ from: { line: 2, col: 0 }, to: { line: 2, col: 4 }, insert: "LINE" });
    // Ordered: every patch's `from` strictly precedes the next patch's `from`.
    expect(patches[0].from.line).toBeLessThanOrEqual(patches[1].from.line);
  });

  it("a multi-line insert always uses \\n, never \\r, regardless of the base's line ending", () => {
    const { base, changes } = change("one\r\ntwo", { from: 3, insert: "\r\nINSERTED\r\n" });
    const patches = patchesFromChangeSet(base, changes);
    expect(patches).toHaveLength(1);
    expect(patches[0].insert).toBe("\nINSERTED\n");
    expect(patches[0].insert).not.toContain("\r");
  });

  it("maps offsets correctly against a CRLF base", () => {
    const { base, changes } = change("first\r\nsecond\r\nthird", { from: 6, to: 12, insert: "SECOND" });
    const patches = patchesFromChangeSet(base, changes);
    expect(patches).toEqual([{ from: { line: 1, col: 0 }, to: { line: 1, col: 6 }, insert: "SECOND" }]);
  });

  it("maps UTF-16 columns correctly when the edit sits after astral characters", () => {
    const { base, changes } = change("😀😀rest", { from: 4, insert: "X" });
    const patches = patchesFromChangeSet(base, changes);
    // Two astral pairs = 4 UTF-16 code units before "rest".
    expect(patches).toEqual([{ from: { line: 0, col: 4 }, to: { line: 0, col: 4 }, insert: "X" }]);
  });

  it("keeps a BOM's column offset correct for an edit later on line 0", () => {
    const { base, changes } = change("﻿# Title\nbody", { from: 8, insert: "!" });
    const patches = patchesFromChangeSet(base, changes);
    expect(patches).toEqual([{ from: { line: 0, col: 8 }, to: { line: 0, col: 8 }, insert: "!" }]);
  });
});

describe("composeSince", () => {
  it("composes a run of per-transaction changesets into one patch set against the original base", () => {
    const state0 = EditorState.create({ doc: "abc" });
    const tr1 = state0.update({ changes: { from: 3, insert: "d" } }); // "abcd"
    const tr2 = tr1.state.update({ changes: { from: 4, insert: "e" } }); // "abcde"
    const composed = composeSince(state0.doc, [tr1.changes, tr2.changes]);
    expect(composed.apply(state0.doc).toString()).toBe("abcde");
    const patches = patchesFromChangeSet(state0.doc, composed);
    // Two adjacent single-character inserts compose into one contiguous
    // patch, not two — `ChangeSet.compose` merges touching ranges, which is
    // still "ordered and disjoint" (ADR-0016 §2), just coalesced.
    expect(patches).toEqual([{ from: { line: 0, col: 3 }, to: { line: 0, col: 3 }, insert: "de" }]);
  });

  it("returns an empty changeset for an empty list", () => {
    const doc = textFromString("abc");
    const composed = composeSince(doc, []);
    expect(composed.empty).toBe(true);
  });
});

describe("offsetAt validation", () => {
  const doc = textFromString("ab\n\u{1F600}x");
  it("accepts the end of a line", () => {
    expect(offsetAt(doc, { line: 0, col: 2 })).toBe(2);
  });
  it("throws for a column past the end of the line", () => {
    expect(() => offsetAt(doc, { line: 0, col: 3 })).toThrow(InvalidPositionError);
  });
  it("throws for a line past the end of the document and for negatives", () => {
    expect(() => offsetAt(doc, { line: 2, col: 0 })).toThrow(InvalidPositionError);
    expect(() => offsetAt(doc, { line: 0, col: -1 })).toThrow(InvalidPositionError);
  });
  it("throws for a column inside a surrogate pair, accepts both sides of it", () => {
    expect(() => offsetAt(doc, { line: 1, col: 1 })).toThrow(InvalidPositionError);
    expect(offsetAt(doc, { line: 1, col: 0 })).toBe(3);
    expect(offsetAt(doc, { line: 1, col: 2 })).toBe(5);
  });
});
