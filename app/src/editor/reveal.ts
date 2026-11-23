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
//      `CodeMark`, `QuoteMark`, `LinkMark`) — the whole node is
//      the marker, so it is hidden verbatim.
//   2. This codebase's own constructs from `mdExtensions.ts`
//      (`SeratWikiLink`, `SeratEmbed`, `SeratHighlight`,
//      `SeratFootnoteRef`), whose marker sub-ranges (`[[`/`]]`, `==`/`==`,
//      `[^`/`]`) are computed from the node's own fixed shape.
//
// `ListMark` is not hidden: a bullet becomes a glyph widget off the selected line and an ordered numeral stays
// visible (editor.md, Block level). `scanReveal` also returns the per-line classes the design CSS keys on.
//
// `SeratBlockRef` and `SeratField` are excluded on purpose — components.md
// §3: "always visible (never hidden by the reveal rule — they are
// structurally load-bearing, not typographic decoration)".

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Range, Text } from "@codemirror/state";
import { Decoration, ViewPlugin, WidgetType, type DecorationSet, type EditorView, type ViewUpdate } from "@codemirror/view";

import { SERAT_REVEALABLE_NODE_TYPES } from "@/editor/mdExtensions";

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
  /** The custom-syntax markers (`[[`, `==`, `[^`) left showing on a selected line. They take the
   * `serat-marker` class (mono, text-secondary); the standard markers get it from the highlighter. */
  revealedMarkers: Span[];
  /** List markers (editor.md, Block level). A bullet off the selected line is replaced by a glyph widget
   * (`widget: true`, the span includes the space after the mark); on a revealed line, and for every ordered
   * numeral, the raw marker stays and takes the class (`serat-bullet` / `serat-list-number`). `indent` is
   * the whitespace before the marker, hidden so it does not double the hanging padding. */
  listMarks: Array<Span & { ordered: boolean; widget: boolean; indent: Span | null }>;
  /** Line classes by line start offset (one `Decoration.line` per line): headings, quotes, code, list
   * items, rules and the YAML properties block. */
  lines: Map<number, string>;
}

/** Deepest list nesting given its own class (`serat-depth-N`; CSS cannot be set per line from a style
 * attribute under the app's CSP, so the depth is a class). */
export const MAX_LIST_DEPTH = 8;

/** The last line of a leading `---` YAML properties block, or 0 when the note has none. Bounded: a
 * note that opens with a rule and never closes it is scanned for 200 lines at most. */
export function frontmatterEndLine(doc: Text): number {
  if (doc.lines < 2 || doc.line(1).text.trimEnd() !== "---") return 0;
  const last = Math.min(doc.lines, 200);
  for (let n = 2; n <= last; n++) {
    const text = doc.line(n).text.trimEnd();
    if (text === "---" || text === "...") return n;
  }
  return 0;
}

/** One walk of the syntax tree over `ranges`: the marker spans to hide, which markers stay visible and
 * the per-line classes. Cost follows the viewport, never the note (architecture.md section 9.3): a node
 * that starts above or runs below the viewport is clamped to `ranges`. Pure and DOM-free, so it is unit
 * tested directly against a plain `EditorState`. */
export function scanReveal(state: EditorState, ranges: readonly Span[], selectedLines: ReadonlySet<number>): RevealScan {
  const { doc } = state;
  const hidden: Span[] = [];
  const revealedMarkers: Span[] = [];
  const listMarks: RevealScan["listMarks"] = [];
  const lines = new Map<number, string>();
  const addClass = (lineFrom: number, cls: string) => {
    const have = lines.get(lineFrom);
    if (!have) lines.set(lineFrom, cls);
    else if (!have.split(" ").includes(cls)) lines.set(lineFrom, `${have} ${cls}`);
  };
  /** Adds `cls` to every line of [from, to] that lies in the clamp window [lo, hi]. */
  const classLines = (from: number, to: number, lo: number, hi: number, cls: string) => {
    const a = Math.max(from, lo);
    const b = Math.min(Math.max(to, from), hi);
    if (a > b) return;
    for (let n = doc.lineAt(a).number, end = doc.lineAt(b).number; n <= end; n++) addClass(doc.line(n).from, cls);
  };

  const tree = syntaxTree(state);
  for (const { from, to } of ranges) {
    let listDepth = 0;
    tree.iterate({
      from,
      to,
      enter: (node) => {
        const name = node.type.name;
        const level = /^(?:ATX|Setext)Heading([1-6])$/.exec(name);
        if (level) classLines(node.from, node.to, from, to, `serat-h${level[1]}`);
        else if (name === "Blockquote") classLines(node.from, node.to, from, to, "serat-quote");
        else if (name === "FencedCode" || name === "CodeBlock") classLines(node.from, node.to, from, to, "serat-code-line");
        else if (name === "HorizontalRule") classLines(node.from, node.to, from, to, "serat-hr");
        else if (name === "BulletList" || name === "OrderedList") listDepth++;
        else if (name === "ListItem") {
          classLines(node.from, node.from, from, to, `serat-li serat-depth-${Math.min(listDepth - 1, MAX_LIST_DEPTH)}`);
        } else if (name === "ListMark") {
          const line = doc.lineAt(node.from);
          const ordered = node.node.parent?.parent?.name === "OrderedList";
          const revealed = selectedLines.has(line.number);
          const lead = state.sliceDoc(line.from, node.from);
          const indent = lead.length > 0 && /^[ \t]+$/.test(lead) ? { from: line.from, to: node.from } : null;
          const spaced = state.sliceDoc(node.to, node.to + 1) === " ";
          listMarks.push({
            from: node.from,
            to: spaced ? node.to + 1 : node.to,
            ordered,
            widget: !ordered && !revealed,
            indent,
          });
          return;
        }
        const isCustom = SERAT_REVEALABLE_NODE_TYPES.has(name);
        // The `(url)` of `[text](url)` is syntax too: it shows only on a revealed line (editor.md, Inline).
        const isLinkUrl = name === "URL" && node.node.parent?.name === "Link";
        const isStandard = isStandardMarkNode(name) || isLinkUrl;
        if (!isCustom && !isStandard) {
          return;
        }
        const lineNumber = doc.lineAt(node.from).number;
        if (selectedLines.has(lineNumber)) {
          // this line is revealed: leave the raw syntax alone
          if (isCustom) {
            for (const span of customMarkerSpans(name, node.from, node.to, state.sliceDoc(node.from, node.to))) {
              if (span.from < span.to) revealedMarkers.push(span);
            }
          }
          return;
        }
        if (isStandard) {
          if (node.from < node.to) {
            // The space after `#` / `>` is part of the marker as far as the reader goes: left behind it
            // would indent the first word by a space's width.
            let end = node.to;
            if (name === "HeaderMark" || name === "QuoteMark") {
              const lineEnd = doc.lineAt(node.to).to;
              while (end < lineEnd && /[ \t]/.test(state.sliceDoc(end, end + 1))) end++;
            }
            hidden.push({ from: node.from, to: end });
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
      leave: (node) => {
        if (node.name === "BulletList" || node.name === "OrderedList") listDepth--;
      },
    });
  }

  // The properties block is raw, whatever the Markdown parse made of it (`---`, `key: v`, `---` reads as a
  // rule and a Setext heading): its lines carry only the frontmatter class and none of its syntax is hidden.
  const fmEnd = frontmatterEndLine(doc);
  if (fmEnd > 0) {
    const fmTo = doc.line(fmEnd).to;
    for (const { from, to } of ranges) {
      const a = Math.max(from, 0);
      const b = Math.min(to, fmTo);
      if (a > b) continue;
      for (let n = doc.lineAt(a).number, end = doc.lineAt(b).number; n <= end; n++) lines.set(doc.line(n).from, "serat-frontmatter");
    }
    const keep = <T extends Span>(spans: T[]) => spans.filter((span) => span.from > fmTo);
    return { hidden: keep(hidden), revealedMarkers: keep(revealedMarkers), listMarks: keep(listMarks), lines };
  }
  return { hidden, revealedMarkers, listMarks, lines };
}

/** The marker spans `scanReveal` would hide (the original FR-EDT-02 entry point). */
export function collectHiddenMarkerSpans(
  state: EditorState,
  ranges: readonly Span[],
  selectedLines: ReadonlySet<number>,
): Span[] {
  return scanReveal(state, ranges, selectedLines).hidden;
}

const lineDecorations = new Map<string, Decoration>();
function lineDecoration(cls: string): Decoration {
  let deco = lineDecorations.get(cls);
  if (!deco) {
    // Rendered headings carry heading semantics mirrored from the Markdown level (editor.md, Accessibility).
    const level = /\bserat-h([1-6])\b/.exec(cls)?.[1];
    const attributes = level ? { role: "heading", "aria-level": level } : undefined;
    lineDecorations.set(cls, (deco = Decoration.line({ class: cls, attributes })));
  }
  return deco;
}
const markerDeco = Decoration.mark({ class: "serat-marker" });
const bulletDeco = Decoration.mark({ class: "serat-bullet" });
const numberDeco = Decoration.mark({ class: "serat-list-number" });
const indentRawDeco = Decoration.mark({ class: "serat-indent-raw" });

/** The glyph a bullet marker (`-`, `*`, `+`) renders as when its line is not revealed. */
class BulletWidget extends WidgetType {
  eq(): boolean {
    return true;
  }
  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = "serat-bullet";
    span.setAttribute("aria-hidden", "true");
    span.textContent = "\u2022";
    return span;
  }
}
const bulletWidgetDeco = Decoration.replace({ widget: new BulletWidget() });
const hideDeco = Decoration.replace({});

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
  for (const [from, cls] of scan.lines) decorations.push(lineDecoration(cls).range(from));
  for (const span of scan.revealedMarkers) decorations.push(markerDeco.range(span.from, span.to));
  for (const mark of scan.listMarks) {
    if (mark.widget) decorations.push(bulletWidgetDeco.range(mark.from, mark.to));
    else decorations.push((mark.ordered ? numberDeco : bulletDeco).range(mark.from, mark.to));
    if (mark.indent) decorations.push((mark.widget || mark.ordered ? hideDeco : indentRawDeco).range(mark.indent.from, mark.indent.to));
  }
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
