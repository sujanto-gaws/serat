// FR-EDT-02, ADR-0006, architecture.md §9.3 "Decorations": "Syntax markers
// are Decoration.replaced except on lines that intersect any selection
// range. A selection change recomputes only the lines it enters and
// leaves, which is how 'within one frame' is kept."
//
// This is a `ViewPlugin` over `view.visibleRanges` (§9.3: "their cost
// follows the viewport rather than the note"), recomputed synchronously on
// every doc/selection/viewport change — no debounce, so the hide/reveal
// always lands in the same paint as the selection change that caused it.
//
// Two kinds of marker are hidden:
//   1. Standard `@lezer/markdown` delimiter nodes, whose names all end in
//      "Mark" (`HeaderMark`, `EmphasisMark`, `StrikethroughMark`,
//      `CodeMark`, `QuoteMark`, `ListMark`, `LinkMark`) — the whole node is
//      the marker, so it is hidden verbatim.
//   2. This codebase's own constructs from `mdExtensions.ts`
//      (`SeratWikiLink`, `SeratEmbed`, `SeratHighlight`,
//      `SeratFootnoteRef`), whose marker sub-ranges (`[[`/`]]`, `==`/`==`,
//      `[^`/`]`) are computed from the node's own fixed shape.
//
// `SeratBlockRef` and `SeratField` are excluded on purpose — components.md
// §3: "always visible (never hidden by the reveal rule — they are
// structurally load-bearing, not typographic decoration)".

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Range, Text } from "@codemirror/state";
import { Decoration, ViewPlugin, type DecorationSet, type EditorView, type ViewUpdate } from "@codemirror/view";

import { SERAT_REVEALABLE_NODE_TYPES } from "@/editor/mdExtensions";
import { CODE_BLOCK_LINE, MONO_SOURCE_LINE } from "@/editor/typography";

export interface Span {
  from: number;
  to: number;
}

/** Every 1-based line number touched by any selection range — "touched"
 * meaning the range's `[from, to]` overlaps the line, exactly matching
 * "lines that intersect any selection range" (§9.3). A collapsed cursor
 * touches exactly the one line it sits on. */
export function selectedLineNumbers(doc: Text, ranges: readonly Span[]): Set<number> {
  const lines = new Set<number>();
  for (const range of ranges) {
    const startLine = doc.lineAt(Math.min(range.from, range.to)).number;
    const endLine = doc.lineAt(Math.max(range.from, range.to)).number;
    for (let line = startLine; line <= endLine; line++) {
      lines.add(line);
    }
  }
  return lines;
}

/** The marker sub-spans within one of this module's custom node types —
 * the characters the reveal rule hides, derived from the construct's own
 * fixed shape rather than from a separate marker child node (`mdExtensions.ts`'s
 * header note explains why there is no child node to read this from). */
export function customMarkerSpans(typeName: string, from: number, to: number, nodeText: string): Span[] {
  switch (typeName) {
    case "SeratWikiLink":
      return [
        { from, to: from + 2 }, // "[["
        { from: to - 2, to }, // "]]"
      ];
    case "SeratEmbed":
      return [
        { from, to: from + 3 }, // "![["
        { from: to - 2, to }, // "]]"
      ];
    case "SeratHighlight":
      return [
        { from, to: from + 2 }, // "=="
        { from: to - 2, to }, // "=="
      ];
    case "SeratFootnoteRef": {
      const closeLocal = nodeText.lastIndexOf("]");
      const spans: Span[] = [{ from, to: from + 2 }]; // "[^"
      if (closeLocal >= 0) {
        spans.push({ from: from + closeLocal, to: from + closeLocal + 1 }); // "]"
      }
      return spans;
    }
    default:
      return [];
  }
}

function isStandardMarkNode(typeName: string): boolean {
  return typeName.endsWith("Mark");
}

export interface RevealScan {
  /** Marker spans to hide: every marker not on a selected line. */
  hidden: Span[];
  /** `from` of every selected line that carries at least one marker, i.e. a line showing raw syntax.
   * Those lines take the `mono-source` class (components.md section 3). A selected line with no syntax
   * on it stays in the prose face: switching a plain paragraph to mono because the caret entered it
   * would be a font change with nothing revealed. */
  revealedLines: Set<number>;
  /** `from` of every line inside a fenced or indented code block in `ranges`. */
  codeLines: Set<number>;
}

/** One walk of the syntax tree over `ranges`: the marker spans to hide, which selected lines are showing
 * raw syntax, and which lines are code. Pure and DOM-free, so it is unit tested directly against a plain
 * `EditorState` rather than only through a mounted `EditorView`. */
export function scanReveal(state: EditorState, ranges: readonly Span[], selectedLines: ReadonlySet<number>): RevealScan {
  const hidden: Span[] = [];
  const revealedLines = new Set<number>();
  const codeLines = new Set<number>();
  const tree = syntaxTree(state);
  for (const { from, to } of ranges) {
    tree.iterate({
      from,
      to,
      enter: (node) => {
        const name = node.type.name;
        if (name === "FencedCode" || name === "CodeBlock") {
          const last = state.doc.lineAt(Math.min(node.to, state.doc.length)).number;
          for (let n = state.doc.lineAt(node.from).number; n <= last; n++) codeLines.add(state.doc.line(n).from);
          // fall through: a fence's `CodeMark`s are still markers
        }
        const isCustom = SERAT_REVEALABLE_NODE_TYPES.has(name);
        const isStandard = isStandardMarkNode(name);
        if (!isCustom && !isStandard) {
          return;
        }
        const line = state.doc.lineAt(node.from);
        if (selectedLines.has(line.number)) {
          // this line is revealed: leave the raw syntax alone
          if (isStandard ? node.from < node.to : true) revealedLines.add(line.from);
          return;
        }
        if (isStandard) {
          if (node.from < node.to) {
            hidden.push({ from: node.from, to: node.to });
          }
        } else {
          const text = state.sliceDoc(node.from, node.to);
          for (const span of customMarkerSpans(name, node.from, node.to, text)) {
            if (span.from < span.to) {
              hidden.push(span);
            }
          }
        }
      },
    });
  }
  return { hidden, revealedLines, codeLines };
}

/** The marker spans `scanReveal` would hide (the original FR-EDT-02 entry point). */
export function collectHiddenMarkerSpans(
  state: EditorState,
  ranges: readonly Span[],
  selectedLines: ReadonlySet<number>,
): Span[] {
  return scanReveal(state, ranges, selectedLines).hidden;
}

const monoLine = Decoration.line({ class: MONO_SOURCE_LINE });
const codeLine = Decoration.line({ class: `${MONO_SOURCE_LINE} ${CODE_BLOCK_LINE}` });

function buildDecorations(view: EditorView): DecorationSet {
  const selectedLines = selectedLineNumbers(
    view.state.doc,
    view.state.selection.ranges.map((r) => ({ from: r.from, to: r.to })),
  );
  const scan = scanReveal(view.state, view.visibleRanges, selectedLines);
  const spans = scan.hidden.sort((a, b) => a.from - b.from || a.to - b.to);
  const decorations: Range<Decoration>[] = spans
    .filter((span, index) => index === 0 || span.from >= spans[index - 1].to) // never overlap
    .map((span) => Decoration.replace({}).range(span.from, span.to));
  for (const from of scan.codeLines) decorations.push(codeLine.range(from));
  for (const from of scan.revealedLines) if (!scan.codeLines.has(from)) decorations.push(monoLine.range(from));
  return Decoration.set(decorations, true);
}

/** The reveal-rule `ViewPlugin` (FR-EDT-02). Mounted once, in
 * `codemirrorSetup.ts`'s extension list. */
export const revealMarkersPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = buildDecorations(view);
    }
    update(update: ViewUpdate): void {
      if (update.docChanged || update.selectionSet || update.viewportChanged) {
        this.decorations = buildDecorations(update.view);
      }
    }
  },
  { decorations: (plugin) => plugin.decorations },
);
