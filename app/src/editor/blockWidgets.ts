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
# tables cost.

import { syntaxTree } from "@codemirror/language";
import { EditorState, RangeSetBuilder, StateField, Text, type Extension, type Transaction } from "@codemirror/state";
import { Decoration, EditorView, WidgetType, type DecorationSet } from "@codemirror/view";
import type { SyntaxNode } from "@lezer/common";

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

/** Builds the `serat-asset:` URL for a vault-relative path (ADR-0017):
 * `serat-asset://localhost/<percent-encoded path>`. Percent-encodes each
 * path segment separately so a literal `/` in a file name (which cannot
 * occur on any of the three platforms) is never produced by encoding, and a
 * real `/` path separator is never encoded away. */
export function seratAssetUrl(vaultRelativePath: string): string {
  const segments = vaultRelativePath.split("/").map((segment) => encodeURIComponent(segment));
  return `serat-asset://localhost/${segments.join("/")}`;
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

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
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

class ImageEmbedWidget extends WidgetType {
  constructor(
    private readonly target: string,
    private readonly widthAttr: string | null,
  ) {
    super();
  }

  eq(other: ImageEmbedWidget): boolean {
    return this.target === other.target && this.widthAttr === other.widthAttr;
  }

  toDOM(): HTMLElement {
    // components.md §4's rule, restated once and enforced everywhere: "a
    // plain <img src='serat-asset://…'>, and nothing else". Never
    // <object>/<iframe>/inline SVG/v-html — a real DOM `<img>` element,
    // built directly, is the only path this widget has.
    const img = document.createElement("img");
    img.src = seratAssetUrl(this.target);
    img.alt = this.target;
    img.className = "serat-embed-widget";
    if (this.widthAttr && /^\d+$/.test(this.widthAttr)) {
      img.width = Number(this.widthAttr);
    }
    return img;
  }

  ignoreEvent(): boolean {
    return true;
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
      // Escaped plain text, newlines as <br> — a synchronous, sanitiser-free
      // render (the string is built entirely by this widget from escaped
      // source text, never from untrusted markup): full inline-Markdown
      // rendering inside a callout body is split-preview's job for now, the
      // same trade-off `readTableModel` documents for table cells.
      body.innerHTML = escapeHtml(this.bodyText.trim()).replace(/\n/g, "<br>");
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

function decorationForNode(doc: Text, node: SyntaxNode): BlockDecoration | null {
  if (node.type.name === "Table") {
    const model = readTableModel(doc, node);
    return {
      from: node.from,
      to: node.to,
      decoration: Decoration.replace({ widget: new TableWidget(model), block: true }),
    };
  }
  if (node.type.name === "SeratEmbed") {
    const text = doc.sliceString(node.from, node.to);
    const match = /^!\[\[([^\]|]+)(?:\|(\d+))?\]\]$/.exec(text);
    if (match && isImageTarget(match[1])) {
      return {
        from: node.from,
        to: node.to,
        decoration: Decoration.replace({ widget: new ImageEmbedWidget(match[1].trim(), match[2] ?? null) }),
      };
    }
    return null;
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
      const lineFrom = state.doc.lineAt(node.from).number;
      const lineTo = state.doc.lineAt(node.to).number;
      for (let line = lineFrom; line <= lineTo; line++) {
        if (selectedLines.has(line)) {
          return; // any touched line reveals the whole construct's source
        }
      }
      const built = decorationForNode(state.doc, node.node);
      if (built) {
        results.push(built);
        return false; // do not also decorate nested nodes (e.g. a table's own cells)
      }
      return undefined;
    },
  });
  return results;
}

/** The full-document build, used only for the field's initial `create` —
 * every subsequent update is the bounded, per-region recompute below. */
function buildAll(state: EditorState): DecorationSet {
  const selectedLines = selectedLineNumbers(
    state.doc,
    state.selection.ranges.map((r) => ({ from: r.from, to: r.to })),
  );
  const found = decorationsInRange(state, 0, state.doc.length, selectedLines).sort((a, b) => a.from - b.from);
  const builder = new RangeSetBuilder<Decoration>();
  for (const { from, to, decoration } of found) {
    builder.add(from, to, decoration);
  }
  return builder.finish();
}

/** The top-level block node (a direct child of `Document`) enclosing `pos`,
 * or `null` if `pos` sits directly under `Document` with no such ancestor
 * (e.g. a boundary position). Bounds every recompute to "one block", which
 * is what keeps this field's per-keystroke cost independent of note size. */
function enclosingTopLevelBlock(state: EditorState, pos: number): Span | null {
  let node: SyntaxNode | null = syntaxTree(state).resolveInner(pos, -1);
  while (node && node.parent && node.parent.type.name !== "Document") {
    node = node.parent;
  }
  if (!node || node.type.name === "Document") {
    return null;
  }
  return { from: node.from, to: node.to };
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

function computeDirtyRegions(tr: Transaction): Span[] {
  const regions: Span[] = [];
  if (tr.docChanged) {
    tr.changes.iterChanges((_fromA, _toA, fromB, toB) => {
      const region = enclosingTopLevelBlock(tr.state, fromB);
      regions.push(region ?? { from: fromB, to: Math.max(toB, fromB + 1) });
    });
  }
  if (!tr.startState.selection.eq(tr.state.selection)) {
    for (const range of tr.startState.selection.ranges) {
      const mapped = tr.changes.mapPos(range.head);
      const region = enclosingTopLevelBlock(tr.state, mapped);
      if (region) regions.push(region);
    }
    for (const range of tr.state.selection.ranges) {
      const region = enclosingTopLevelBlock(tr.state, range.head);
      if (region) regions.push(region);
    }
  }
  return mergeSpans(regions);
}

/** The block-widget `StateField` (architecture.md §9.3). Toggle its effect
 * on/off per `livePreview` the same way `reveal.ts`'s plugin is toggled —
 * `CodeMirrorEditor.vue` wraps both in the same `Compartment` so split mode
 * (no hide/reveal at all, per FR-EDT-10) drops both together. */
export const blockWidgetsField = StateField.define<DecorationSet>({
  create(state) {
    return buildAll(state);
  },
  update(value, tr) {
    if (!tr.docChanged && tr.startState.selection.eq(tr.state.selection)) {
      return value;
    }
    let mapped = value.map(tr.changes);
    const regions = computeDirtyRegions(tr);
    if (regions.length === 0) {
      return mapped;
    }
    const selectedLines = selectedLineNumbers(
      tr.state.doc,
      tr.state.selection.ranges.map((r) => ({ from: r.from, to: r.to })),
    );
    for (const region of regions) {
      mapped = replaceRegion(mapped, tr.state, region, selectedLines);
    }
    return mapped;
  },
  provide: (field) => EditorView.decorations.from(field),
});

export function blockWidgetsExtension(): Extension {
  return blockWidgetsField;
}
