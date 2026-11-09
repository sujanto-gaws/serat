// Task #40 (follows task #21/ADR-0006): "tables and images as raw source in
// live mode is a real gap" for P1's gate ("an editor you could live in").
// architecture.md §9.3: "Block widgets — tables, embeds, callouts — come
// from a StateField, because CM6 permits block decorations only from
// state. Each widget implements eq so that an unchanged table is never
// rebuilt in the DOM." This module is that StateField.
//
// C-04 holds by construction here the same way it holds for saves: a
// widget only ever *replaces what is displayed*; it never touches the
// document. The underlying text is exactly what `getText()`/a save sees,
// with or without a widget rendered over it.
//
// Reveal semantics match FR-EDT-02's rule for inline syntax, extended to
// block constructs: a table/callout/image-embed whose source line range
// intersects the selection shows its raw Markdown (`mono-source`,
// editable) instead of the widget — "the widget yields to raw source when
// the selection enters its line" (§9.3, embeds).
//
// NFR-PRF-02 cost note (coordinator, task #40: "keep an eye on the
// per-keystroke cost of widget recomputation... the rule is still being
// settled"). A `StateField` has no `visibleRanges` to scope itself to the
// way the inline `ViewPlugin` does (`reveal.ts`), so this module bounds its
// own cost instead: `update` never re-walks the whole document. It maps the
// previous `DecorationSet` forward through the transaction (cheap,
// independent of document size) and only re-examines the *top-level block*
// enclosing each changed range and each old/new selection head — typically
// one table, one callout or one paragraph, never the whole note. A 5,000
// line note with 50 tables (the NFR-PRF-02 fixture's own shape) therefore
// costs this field roughly what one block costs, per keystroke, not what 50
// tables cost.

import { syntaxTree } from "@codemirror/language";
import { EditorState, RangeSetBuilder, StateField, Text, type Extension, type Transaction } from "@codemirror/state";
import { Decoration, EditorView, WidgetType, type DecorationSet } from "@codemirror/view";
import type { SyntaxNode } from "@lezer/common";

import { assetUrl } from "@/editor/assetUrl";
import { hasResolutionEffect, innerOfLinkNode, linkKey, parseLinkInner, resolutionsField } from "@/editor/linkState";
import { selectedLineNumbers, type Span } from "@/editor/reveal";

// ---------------------------------------------------------------------------
// Pure helpers — unit tested directly (blockWidgets.test.ts), independent of
// any mounted EditorView.
// ---------------------------------------------------------------------------

const IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "svg"]);

/** Whether a wikilink-embed target names an image, by extension — the only
 * signal available in P1 without an index lookup (matches ADR-0017's own
 * extension-table approach for `serat-asset:`). */
export function isImageTarget(target: string): boolean {
  const clean = target.split(/[|#]/, 1)[0]?.trim() ?? "";
  const dot = clean.lastIndexOf(".");
  if (dot < 0) return false;
  return IMAGE_EXTENSIONS.has(clean.slice(dot + 1).toLowerCase());
}

export interface CalloutHeader {
  type: string;
  title: string;
}

/** Parses a blockquote's first line as an Obsidian-style callout header:
 * `[!type]` or `[!type] Title`. Returns `null` for an ordinary blockquote. */
export function parseCalloutHeader(firstLine: string): CalloutHeader | null {
  const match = /^>\s?\[!([\w-]+)\]\s*(.*)$/.exec(firstLine.trim());
  if (!match) return null;
  const type = match[1].toLowerCase();
  const title = match[2].trim() || type.charAt(0).toUpperCase() + type.slice(1);
  return { type, title };
}

/** Strips each line's leading blockquote marker (`> ` or `>`), the same way
 * every Markdown renderer reads blockquote content — never touches the
 * document, only a string used to build a widget's rendered text. */
export function stripBlockquoteMarkers(source: string): string {
  return source
    .split("\n")
    .map((line) => line.replace(/^>\s?/, ""))
    .join("\n");
}

export interface TableModel {
  header: string[];
  rows: string[][];
}

/** Reads a parsed `Table` node's cells into a plain model, from the
 * document text directly — no re-parsing of pipe syntax, so it can never
 * disagree with what Lezer already decided is a table. Cell text is taken
 * verbatim (inline Markdown *inside* a cell is not further rendered in
 * P1 — a documented scope trim, not a data loss: the source is unaffected
 * and split-preview renders it in full via `markdown-it`). */
export function readTableModel(doc: Text, tableNode: SyntaxNode): TableModel {
  const header: string[] = [];
  const rows: string[][] = [];
  for (let row = tableNode.firstChild; row; row = row.nextSibling) {
    if (row.type.name !== "TableHeader" && row.type.name !== "TableRow") {
      continue; // TableDelimiter (the --- separator line)
    }
    const cells: string[] = [];
    for (let cell = row.firstChild; cell; cell = cell.nextSibling) {
      if (cell.type.name !== "TableCell") continue;
      cells.push(doc.sliceString(cell.from, cell.to).trim());
    }
    if (row.type.name === "TableHeader") {
      header.push(...cells);
    } else {
      rows.push(cells);
    }
  }
  return { header, rows };
}

// ---------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------

class TableWidget extends WidgetType {
  constructor(private readonly model: TableModel) {
    super();
  }

  eq(other: TableWidget): boolean {
    return JSON.stringify(this.model) === JSON.stringify(other.model);
  }

  toDOM(): HTMLElement {
    const table = document.createElement("table");
    table.className = "serat-table-widget";
    if (this.model.header.length > 0) {
      const thead = document.createElement("thead");
      const tr = document.createElement("tr");
      for (const cell of this.model.header) {
        const th = document.createElement("th");
        th.textContent = cell;
        tr.appendChild(th);
      }
      thead.appendChild(tr);
      table.appendChild(thead);
    }
    const tbody = document.createElement("tbody");
    for (const row of this.model.rows) {
      const tr = document.createElement("tr");
      for (const cell of row) {
        const td = document.createElement("td");
        td.textContent = cell;
        tr.appendChild(td);
      }
      tbody.appendChild(tr);
    }
    table.appendChild(tbody);
    return table;
  }

  ignoreEvent(): boolean {
    return false; // clicking a cell should still be able to move the cursor there
  }
}

/** An image embed. With a RESOLVED vault path it renders a plain
 * `<img src=serat-asset…>` (platform-aware URL, ADR-0017); with none (not
 * resolved yet, or the scheme handler is not there) or when the image fails
 * to load, it renders a labelled placeholder, never a broken image. */
class ImageEmbedWidget extends WidgetType {
  constructor(
    private readonly label: string,
    private readonly resolvedPath: string | null,
    private readonly widthAttr: string | null,
  ) {
    super();
  }

  eq(other: ImageEmbedWidget): boolean {
    return (
      this.label === other.label && this.resolvedPath === other.resolvedPath && this.widthAttr === other.widthAttr
    );
  }

  private placeholder(): HTMLElement {
    const span = document.createElement("span");
    span.className = "serat-embed-placeholder";
    span.textContent = `[image not loaded: ${this.label}]`;
    return span;
  }

  toDOM(): HTMLElement {
    // components.md §4's rule, restated once and enforced everywhere: "a
    // plain <img src='serat-asset://…'>, and nothing else". Never
    // <object>/<iframe>/inline SVG/v-html.
    if (this.resolvedPath === null) return this.placeholder();
    const img = document.createElement("img");
    img.src = assetUrl(this.resolvedPath);
    img.alt = this.label;
    img.className = "serat-embed-widget";
    if (this.widthAttr && /^\d+$/.test(this.widthAttr)) {
      img.width = Number(this.widthAttr);
    }
    img.addEventListener("error", () => img.replaceWith(this.placeholder()), { once: true });
    return img;
  }

  ignoreEvent(): boolean {
    return false; // a click may place the cursor on the embed (revealing its source)
  }
}

class CalloutWidget extends WidgetType {
  constructor(
    private readonly header: CalloutHeader,
    private readonly bodyText: string,
  ) {
    super();
  }

  eq(other: CalloutWidget): boolean {
    return (
      this.header.type === other.header.type &&
      this.header.title === other.header.title &&
      this.bodyText === other.bodyText
    );
  }

  toDOM(): HTMLElement {
    const container = document.createElement("div");
    container.className = `serat-callout serat-callout-${this.header.type}`;
    container.setAttribute("data-callout", this.header.type);
    const title = document.createElement("div");
    title.className = "serat-callout-title";
    title.textContent = this.header.title;
    container.appendChild(title);
    if (this.bodyText.trim()) {
      const body = document.createElement("div");
      body.className = "serat-callout-body";
      // Text nodes and <br>, no HTML parsing at all: full inline-Markdown
      // rendering inside a callout body is split-preview's job for now, the
      // same trade-off `readTableModel` documents for table cells.
      this.bodyText
        .trim()
        .split("\n")
        .forEach((line, index) => {
          if (index > 0) body.appendChild(document.createElement("br"));
          body.appendChild(document.createTextNode(line));
        });
      container.appendChild(body);
    }
    return container;
  }

  ignoreEvent(): boolean {
    return false;
  }
}

// ---------------------------------------------------------------------------
// Node -> decoration
// ---------------------------------------------------------------------------

interface BlockDecoration {
  from: number;
  to: number;
  decoration: ReturnType<typeof Decoration.replace>;
}

function decorationForNode(state: EditorState, node: SyntaxNode): BlockDecoration | null {
  const doc = state.doc;
  if (node.type.name === "Table") {
    const model = readTableModel(doc, node);
    return {
      from: node.from,
      to: node.to,
      decoration: Decoration.replace({ widget: new TableWidget(model), block: true }),
    };
  }
  if (node.type.name === "SeratEmbed") {
    // The URL is built from the RESOLVED vault path only (ADR-0017), never
    // from the name as written: `![[pic.png]]` may live in any folder. Until
    // the resolver has answered (or if it cannot find the file) the widget
    // is a labelled placeholder, not a guessed or broken <img>.
    const inner = innerOfLinkNode(doc.sliceString(node.from, node.to));
    if (inner === null) return null;
    const link = parseLinkInner(inner);
    const resolution = state.field(resolutionsField, false)?.get(linkKey(link.target));
    const resolvedPath = resolution?.path ?? null;
    if (!isImageTarget(link.note) && !(resolvedPath !== null && isImageTarget(resolvedPath))) return null;
    const width = link.display !== null && /^\d+$/.test(link.display.trim()) ? link.display.trim() : null;
    return {
      from: node.from,
      to: node.to,
      decoration: Decoration.replace({
        widget: new ImageEmbedWidget(link.note || link.target, resolvedPath, width),
      }),
    };
  }
  if (node.type.name === "Blockquote") {
    const source = doc.sliceString(node.from, node.to);
    const firstLine = source.split("\n", 1)[0];
    const header = parseCalloutHeader(firstLine);
    if (!header) return null;
    const rest = source.slice(firstLine.length).replace(/^\n/, "");
    const body = stripBlockquoteMarkers(rest);
    return {
      from: node.from,
      to: node.to,
      decoration: Decoration.replace({ widget: new CalloutWidget(header, body), block: true }),
    };
  }
  return null;
}

/** Every top-level block node (Table, SeratEmbed, Blockquote) within
 * `[from, to]`, decorated, unless its line range intersects `selectedLines`
 * — the reveal rule extended to block constructs. */
function decorationsInRange(state: EditorState, from: number, to: number, selectedLines: ReadonlySet<number>): BlockDecoration[] {
  const results: BlockDecoration[] = [];
  syntaxTree(state).iterate({
    from,
    to,
    enter: (node) => {
      if (node.type.name !== "Table" && node.type.name !== "SeratEmbed" && node.type.name !== "Blockquote") {
        return;
      }
      const startLine = state.doc.lineAt(node.from);
      const endLine = state.doc.lineAt(node.to);
      // A block widget replaces whole lines. A table or callout nested in a
      // list item or an outer blockquote starts after a marker, mid-line;
      // it stays raw source rather than getting a block replace that cuts a line.
      if (node.type.name !== "SeratEmbed" && (node.from !== startLine.from || node.to !== endLine.to)) {
        return undefined;
      }
      const lineFrom = startLine.number;
      const lineTo = endLine.number;
      for (let line = lineFrom; line <= lineTo; line++) {
        if (selectedLines.has(line)) {
          return; // any touched line reveals the whole construct's source
        }
      }
      const built = decorationForNode(state, node.node);
      if (built) {
        results.push(built);
        return false; // do not also decorate nested nodes (e.g. a table's own cells)
      }
      return undefined;
    },
  });
  return results;
}

/** Field value: the decorations plus a watermark. `examined` is the document
 * position up to which the decorations reflect the current text and parse
 * tree. It maps through edits and NEVER moves back when CodeMirror cuts the
 * tree back to the viewport after an edit, so a later "growth" of the tree
 * only re-examines territory that was never examined (or has since changed),
 * not the whole note on every keystroke. */
export interface BlockWidgetState {
  deco: DecorationSet;
  examined: number;
}

/** The full-document build, used for the field's initial `create` and when
 * link resolutions change; every other update is the bounded recompute below. */
function buildAll(state: EditorState): BlockWidgetState {
  const selectedLines = selectedLineNumbers(
    state.doc,
    state.selection.ranges.map((r) => ({ from: r.from, to: r.to })),
  );
  const found = decorationsInRange(state, 0, state.doc.length, selectedLines).sort((a, b) => a.from - b.from);
  const builder = new RangeSetBuilder<Decoration>();
  for (const { from, to, decoration } of found) {
    builder.add(from, to, decoration);
  }
  return { deco: builder.finish(), examined: syntaxTree(state).length };
}

/** The top-level block node (a direct child of `Document`) enclosing `pos`,
 * or `null` if `pos` sits directly under `Document` with no such ancestor
 * (e.g. a boundary position). Bounds every recompute to "one block", which
 * is what keeps this field's per-keystroke cost independent of note size. */
function enclosingTopLevelBlock(state: EditorState, pos: number, side: -1 | 1 = -1): Span | null {
  let node: SyntaxNode | null = syntaxTree(state).resolveInner(pos, side);
  while (node && node.parent && node.parent.type.name !== "Document") {
    node = node.parent;
  }
  if (!node || node.type.name === "Document") {
    return null;
  }
  return { from: node.from, to: node.to };
}

/** The block(s) a position touches: `resolveInner(pos, -1)` alone resolves a block's
 * FIRST character to the previous node (or to `Document`, so `null`, at offset 0), which
 * left that block unexamined when the cursor left it and a table stayed raw (FR-EDT-02).
 * Both sides, deduplicated; two O(depth) lookups, no walk. */
function touchedTopLevelBlocks(state: EditorState, pos: number): Span[] {
  const before = enclosingTopLevelBlock(state, pos, -1);
  const after = enclosingTopLevelBlock(state, pos, 1);
  if (before && after && before.from === after.from && before.to === after.to) return [before];
  return [before, after].filter((x): x is Span => x !== null);
}

/** Every top-level block overlapping `[from, to]`: one tree walk bounded to the range
 * (it does not descend into a block), so its cost is the selection's, not the note's. */
function blocksIntersecting(state: EditorState, from: number, to: number): Span[] {
  const out: Span[] = [];
  syntaxTree(state).iterate({
    from,
    to,
    enter(node) {
      if (node.type.name === "Document") return true;
      if (node.node.parent?.type.name === "Document") out.push({ from: node.from, to: node.to });
      return false;
    },
  });
  return out;
}

function mergeSpans(spans: Span[]): Span[] {
  if (spans.length === 0) return [];
  const sorted = [...spans].sort((a, b) => a.from - b.from);
  const merged: Span[] = [sorted[0]];
  for (const span of sorted.slice(1)) {
    const last = merged[merged.length - 1];
    if (span.from <= last.to) {
      last.to = Math.max(last.to, span.to);
    } else {
      merged.push({ ...span });
    }
  }
  return merged;
}

/** Recomputes decorations only within `region`, splicing the result into
 * `deco` (which must already have nothing else inside `region`). */
function replaceRegion(deco: DecorationSet, state: EditorState, region: Span, selectedLines: ReadonlySet<number>): DecorationSet {
  const kept = deco.update({
    filter: (from, to) => to <= region.from || from >= region.to,
  });
  const fresh = decorationsInRange(state, region.from, region.to, selectedLines);
  if (fresh.length === 0) {
    return kept;
  }
  return kept.update({
    add: fresh.map(({ from, to, decoration }) => decoration.range(from, to)),
  });
}

/** Territory the tree newly covers that was never examined. Pure, so the
 * "never moves back" rule is testable without provoking a partial parse. */
export function growthRegion(examined: number, treeLength: number): Span | null {
  return treeLength > examined ? { from: examined, to: treeLength } : null;
}

function computeDirtyRegions(tr: Transaction, examined: number): Span[] {
  const regions: Span[] = [];
  if (tr.docChanged) {
    tr.changes.iterChanges((_fromA, _toA, fromB, toB) => {
      const region = enclosingTopLevelBlock(tr.state, fromB);
      regions.push(region ?? { from: fromB, to: Math.max(toB, fromB + 1) });
    });
  }
  // CodeMirror parses a long note in time-sliced chunks, and cuts the tree
  // back to the viewport after an edit. Only territory beyond the watermark
  // has never been examined.
  const growth = growthRegion(examined, syntaxTree(tr.state).length);
  if (growth) {
    const start = enclosingTopLevelBlock(tr.state, Math.min(growth.from, tr.state.doc.length));
    regions.push({ from: start ? Math.min(start.from, growth.from) : growth.from, to: growth.to });
  }
  if (!tr.startState.selection.eq(tr.state.selection)) {
    // Both ends of every range, old (mapped) and new, plus every top-level block a
    // non-empty range spans: `selectedLines` (buildAll/update) comes from the full
    // ranges, so the dirty regions must too (FR-EDT-02). A collapsed cursor takes the
    // two O(depth) lookups only; cost grows with the selection, never the note.
    for (const range of tr.startState.selection.ranges) {
      const from = tr.changes.mapPos(range.from);
      const to = tr.changes.mapPos(range.to);
      regions.push(...touchedTopLevelBlocks(tr.state, tr.changes.mapPos(range.head)));
      if (range.anchor !== range.head) {
        regions.push(...touchedTopLevelBlocks(tr.state, tr.changes.mapPos(range.anchor)));
        regions.push(...blocksIntersecting(tr.state, from, to));
      }
    }
    for (const range of tr.state.selection.ranges) {
      regions.push(...touchedTopLevelBlocks(tr.state, range.head));
      if (range.anchor !== range.head) {
        regions.push(...touchedTopLevelBlocks(tr.state, range.anchor));
        regions.push(...blocksIntersecting(tr.state, range.from, range.to));
      }
    }
  }
  return mergeSpans(regions);
}

/** The block-widget `StateField` (architecture.md §9.3). Toggle its effect
 * on/off per `livePreview` the same way `reveal.ts`'s plugin is toggled —
 * `CodeMirrorEditor.vue` wraps both in the same `Compartment` so split mode
 * (no hide/reveal at all, per FR-EDT-10) drops both together. */
export const blockWidgetsField = StateField.define<BlockWidgetState>({
  create(state) {
    return buildAll(state);
  },
  update(value, tr) {
    // Resolved paths arrived (or were refreshed): embed widgets depend on them
    // and the answer can touch any embed in view, so rebuild once. This is an
    // IPC-arrival event, not a keystroke.
    if (hasResolutionEffect(tr.effects)) {
      return buildAll(tr.state);
    }
    if (
      !tr.docChanged &&
      tr.startState.selection.eq(tr.state.selection) &&
      syntaxTree(tr.startState) === syntaxTree(tr.state)
    ) {
      return value;
    }
    const deco = value.deco.map(tr.changes);
    const examined = tr.docChanged ? tr.changes.mapPos(value.examined, -1) : value.examined;
    const regions = computeDirtyRegions(tr, examined);
    const treeLength = syntaxTree(tr.state).length;
    const nextExamined = Math.max(examined, Math.min(treeLength, tr.state.doc.length));
    if (regions.length === 0) {
      return { deco, examined: nextExamined };
    }
    const selectedLines = selectedLineNumbers(
      tr.state.doc,
      tr.state.selection.ranges.map((r) => ({ from: r.from, to: r.to })),
    );
    let next = deco;
    for (const region of regions) {
      next = replaceRegion(next, tr.state, region, selectedLines);
    }
    return { deco: next, examined: nextExamined };
  },
  provide: (field) => EditorView.decorations.from(field, (value) => value.deco),
});

/** Minimal widget styling (serat-identity.md tokens, with fallbacks). Injected
 * by CodeMirror into the editor's shadow root, so it needs no CSP exception. */
const widgetTheme = EditorView.baseTheme({
  ".serat-table-widget": { borderCollapse: "collapse", margin: "0.4em 0" },
  ".serat-table-widget th, .serat-table-widget td": {
    border: "1px solid var(--serat-border-hairline, #ddd)",
    padding: "2px 8px",
    textAlign: "left",
  },
  ".serat-table-widget th": { fontWeight: "600" },
  ".serat-embed-widget": { maxWidth: "100%", verticalAlign: "middle" },
  ".serat-callout": {
    borderLeft: "3px solid var(--serat-accent, #4a6fa5)",
    background: "var(--serat-surface-raised, rgba(0,0,0,0.04))",
    padding: "4px 10px",
    margin: "0.4em 0",
  },
  ".serat-callout-title": { fontWeight: "600" },
});

export function blockWidgetsExtension(): Extension {
  return [resolutionsField, blockWidgetsField, widgetTheme];
}
