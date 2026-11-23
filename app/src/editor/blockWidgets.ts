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

import { syntaxTree, syntaxTreeAvailable } from "@codemirror/language";
import { EditorState, MapMode, RangeSetBuilder, StateEffect, StateField, Text, type Extension, type Transaction } from "@codemirror/state";
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
  /** Obsidian's optional marker directly after `]`: `-` = collapsed, `+` = expanded by default, none =
   * not foldable (`null`). Display state only; the marker stays in the file (C-01, C-04). */
  fold: "collapsed" | "expanded" | null;
}

/** Parses a blockquote's first line as an Obsidian-style callout header:
 * `[!type]`, `[!type] Title`, with an optional `-` / `+` fold marker directly after `]`. A `-` after
 * a space belongs to the title. Returns `null` for an ordinary blockquote. */
export function parseCalloutHeader(firstLine: string): CalloutHeader | null {
  const match = /^>\s?\[!([\w-]+)\]([+-])?\s*(.*)$/.exec(firstLine.trim());
  if (!match) return null;
  const type = match[1].toLowerCase();
  const title = match[3].trim() || type.charAt(0).toUpperCase() + type.slice(1);
  const fold = match[2] === "-" ? "collapsed" : match[2] === "+" ? "expanded" : null;
  return { type, title, fold };
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
    try {
      img.src = assetUrl(this.resolvedPath);
    } catch {
      return this.placeholder(); // a path the builder refuses is never requested
    }
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

// ---------------------------------------------------------------------------
// Callout fold state (#159; editor.md "Callouts and the fold marker"). VIEW state only: toggling dispatches
// an effect and never a document change, so nothing is patched or saved, the unsaved dot does not appear, the
// undo stack is untouched and the `-` / `+` marker stays as written (C-01, C-04).
// ---------------------------------------------------------------------------

export type CalloutFold = "collapsed" | "expanded";

interface FoldToggle {
  /** The callout's block start (the widget's range start, `view.posAtDOM`). */
  from: number;
  fold: CalloutFold;
}

const foldToggled = StateEffect.define<FoldToggle>();

/** Toggled states, keyed by the callout's start position mapped through edits (assoc 1: a line inserted
 * above carries the callout's state with it). Survives edits elsewhere and the caret revealing and
 * re-hiding the source; a fresh editor state (reopening a note) starts empty, so the marker decides again.
 * Not written to the file, the index or the vault. */
export const calloutFoldField = StateField.define<ReadonlyMap<number, CalloutFold>>({
  create: () => new Map(),
  update(value, tr) {
    let next: Map<number, CalloutFold> | null = null;
    if (tr.docChanged && value.size > 0) {
      next = new Map();
      for (const [pos, fold] of value) {
        // TrackAfter: the callout's first character is after its start; if it was deleted the entry is dropped, not handed to whatever now starts at the deletion point.
        const mapped = tr.changes.mapPos(pos, 1, MapMode.TrackAfter);
        if (mapped !== null) next.set(mapped, fold);
      }
    }
    for (const effect of tr.effects) {
      if (effect.is(foldToggled)) {
        next = new Map(next ?? value);
        next.set(effect.value.from, effect.value.fold);
      }
    }
    return next ?? value;
  },
});

/** The block starts a transaction toggled (their widgets must be rebuilt). */
const NO_STARTS: readonly number[] = [];
function toggledStarts(tr: Transaction): readonly number[] {
  if (tr.effects.length === 0) return NO_STARTS; // every keystroke: no allocation
  let out: number[] | null = null;
  for (const effect of tr.effects) if (effect.is(foldToggled)) (out ??= []).push(effect.value.from);
  return out ?? NO_STARTS;
}

/** Non-blank source lines of a callout body: the number the collapsed title reports. */
export function countBodyLines(body: string): number {
  return body.split("\n").filter((line) => line.trim() !== "").length;
}

const GLYPH_CLASS = "serat-callout-glyph";

class CalloutWidget extends WidgetType {
  /** Foldable = has a `-`/`+` marker AND a non-empty body (editor.md). */
  private readonly lines: number;
  private readonly foldable: boolean;

  constructor(
    private readonly header: CalloutHeader,
    private readonly bodyText: string,
    /** The effective state: a toggle in this session, else the marker's. `null` for a non-foldable callout. */
    private readonly fold: CalloutFold | null,
    /** The block start at build time: only seeds a stable DOM id (not part of `eq`). */
    private readonly from: number = 0,
  ) {
    super();
    this.lines = countBodyLines(bodyText);
    this.foldable = header.fold !== null && this.lines > 0;
  }

  eq(other: CalloutWidget): boolean {
    return (
      this.header.type === other.header.type &&
      this.header.title === other.header.title &&
      this.header.fold === other.header.fold &&
      this.foldState() === other.foldState() &&
      this.bodyText === other.bodyText
    );
  }

  private foldState(): CalloutFold | null {
    return this.foldable ? (this.fold ?? this.header.fold) : null;
  }

  /** Everything except the fold state: if it differs the DOM is rebuilt, if not it is patched in place so the
   * glyph's turn can transition. */
  private signature(): string {
    return JSON.stringify([this.header.type, this.header.title, this.header.fold, this.bodyText]);
  }

  private countText(): string {
    return `${this.lines} ${this.lines === 1 ? "line" : "lines"} hidden`;
  }

  private applyFold(dom: HTMLElement): void {
    const state = this.foldState();
    if (state === null) return;
    const collapsed = state === "collapsed";
    dom.setAttribute("data-fold", state);
    const row = dom.querySelector<HTMLElement>(".serat-callout-title");
    row?.setAttribute("aria-expanded", collapsed ? "false" : "true");
    const body = dom.querySelector<HTMLElement>(".serat-callout-body");
    if (body) body.hidden = collapsed; // never animated: a height transition would run layout in the frame
    const count = dom.querySelector<HTMLElement>(".serat-callout-count");
    if (count) count.hidden = !collapsed;
  }

  toDOM(view?: EditorView): HTMLElement {
    const container = document.createElement("div");
    container.className = `serat-callout serat-callout-${this.header.type}`;
    container.setAttribute("data-callout", this.header.type);
    container.setAttribute("data-sig", this.signature());
    if (this.foldable) container.classList.add("serat-callout-foldable");
    const title = document.createElement("div");
    title.className = "serat-callout-title";
    const titleText = document.createElement("span");
    titleText.className = "serat-callout-title-text";
    titleText.textContent = this.header.title;

    let body: HTMLElement | null = null;
    if (this.lines > 0) {
      body = document.createElement("div");
      body.className = "serat-callout-body";
      // Stable across rebuilds (derived from the block start, not a counter); a suffix only if a live widget
      // already holds it (a widget kept through edits can carry a stale start).
      let id = `serat-callout-body-${this.from}`;
      for (let n = 2; document.getElementById(id); n++) id = `serat-callout-body-${this.from}-${n}`;
      body.id = id;
      // Text nodes and <br>, no HTML parsing at all: full inline-Markdown
      // rendering inside a callout body is split-preview's job for now, the
      // same trade-off `readTableModel` documents for table cells.
      this.bodyText
        .trim()
        .split("\n")
        .forEach((line, index) => {
          if (index > 0) body!.appendChild(document.createElement("br"));
          body!.appendChild(document.createTextNode(line));
        });
    }

    if (this.foldable && body) {
      title.setAttribute("role", "button");
      title.tabIndex = 0;
      title.setAttribute("aria-controls", body.id);
      const glyph = document.createElement("span");
      glyph.className = GLYPH_CLASS;
      glyph.setAttribute("aria-hidden", "true");
      const chevron = document.createElement("span");
      chevron.className = "serat-callout-chevron";
      glyph.appendChild(chevron);
      title.appendChild(glyph);
      title.appendChild(titleText);
      // The accessible name is the title plus the count while collapsed ("Remember to rebase, 3 lines
      // hidden"): the separator is visually hidden text, so no aria-label overrides the content.
      const count = document.createElement("span");
      count.className = "serat-callout-count";
      const sep = document.createElement("span");
      sep.className = "serat-callout-sep";
      sep.textContent = ", ";
      count.appendChild(sep);
      count.appendChild(document.createTextNode(this.countText()));
      title.appendChild(count);

      const toggle = (): void => {
        if (!view) return;
        const current = container.getAttribute("data-fold") === "collapsed" ? "collapsed" : "expanded";
        view.dispatch({
          effects: foldToggled.of({ from: view.posAtDOM(container), fold: current === "collapsed" ? "expanded" : "collapsed" }),
        });
      };
      // The glyph box toggles the fold in the preview and does nothing else: the mousedown default (focus,
      // caret placement) is cancelled, and `ignoreEvent` keeps CodeMirror from turning the click into a selection.
      glyph.addEventListener("mousedown", (event) => event.preventDefault());
      glyph.addEventListener("click", (event) => {
        event.preventDefault();
        toggle();
      });
      title.addEventListener("keydown", (event) => {
        if (event.target !== title) return;
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          toggle();
        } else if (event.key === "Escape") {
          event.preventDefault();
          view?.focus();
        }
      });
    } else {
      title.appendChild(titleText);
    }
    container.appendChild(title);
    if (body) container.appendChild(body);
    this.applyFold(container);
    return container;
  }

  updateDOM(dom: HTMLElement): boolean {
    if (dom.getAttribute("data-sig") !== this.signature()) return false;
    this.applyFold(dom);
    return true;
  }

  ignoreEvent(event: Event): boolean {
    const target = event.target instanceof Element ? event.target : null;
    if (!target) return false;
    // Inside the glyph box the widget owns the event; everywhere else the caret lands in the block and the
    // source is revealed (FR-EDT-02).
    if (target.closest(`.${GLYPH_CLASS}`)) return true;
    if (event instanceof KeyboardEvent && target.matches(".serat-callout-title[role=button]")) {
      return event.key === "Enter" || event.key === " " || event.key === "Tab" || event.key === "Escape";
    }
    return false;
  }
}

/** The palette command "Toggle callout fold": toggles the foldable callout containing the caret, rendered
 * or revealed, with no focus change. `false` when the caret is in no foldable callout. */
export function toggleCalloutFoldAtCaret(view: EditorView): boolean {
  const state = view.state;
  const folds = state.field(calloutFoldField, false);
  if (!folds) return false;
  const head = state.selection.main.head;
  for (const span of touchedTopLevelBlocks(state, head)) {
    const block = state.doc.sliceString(span.from, span.to);
    const firstLine = block.split("\n", 1)[0];
    const header = parseCalloutHeader(firstLine);
    if (!header || header.fold === null) continue;
    const body = stripBlockquoteMarkers(block.slice(firstLine.length).replace(/^\n/, ""));
    if (countBodyLines(body) === 0) continue;
    const current = folds.get(span.from) ?? header.fold;
    view.dispatch({ effects: foldToggled.of({ from: span.from, fold: current === "collapsed" ? "expanded" : "collapsed" }) });
    return true;
  }
  return false;
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
      decoration: Decoration.replace({ widget: new CalloutWidget(header, body, state.field(calloutFoldField, false)?.get(node.from) ?? null, node.from), block: true }),
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
  return { deco: builder.finish(), examined: reliableParsedTo(state) };
}

/**
 * How far the syntax tree can be trusted: the position up to which the parse has actually
 * completed. `syntaxTree(state).length` is NOT that: after an edit CodeMirror may hand over a
 * partial, time-sliced tree whose length is the document's (or longer than what is parsed),
 * and the parse finishes later in a transaction with no document change and the same length
 * (#109). The common case, a finished parse, is one call; a partial one binary-searches the
 * boundary (about log2(n) O(1) calls). Never walks the tree.
 */
export function reliableParsedTo(state: EditorState): number {
  const length = state.doc.length;
  if (syntaxTreeAvailable(state, length)) return length;
  let lo = 0;
  let hi = length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (syntaxTreeAvailable(state, mid)) lo = mid;
    else hi = mid - 1;
  }
  return lo;
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

/** The span covering every top-level block overlapping `[from, to]`: one tree walk bounded to the range
 * (it does not descend into a block), so its cost is the selection's, not the note's. */
function blocksIntersecting(state: EditorState, from: number, to: number): Span[] {
  let first = -1;
  let last = -1;
  syntaxTree(state).iterate({
    from,
    to,
    enter(node) {
      if (node.type.name === "Document") return true;
      if (node.node.parent?.type.name === "Document") {
        if (first < 0) first = node.from;
        last = node.to;
      }
      return false;
    },
  });
  // One span, not one per block: select-all on a 5,000-line note is one replaceRegion.
  return first < 0 ? [] : [{ from: first, to: last }];
}

/**
 * The position intervals whose blocks need re-examining for a selection change.
 * A line selected in both states is revealed in both, so with one range before and
 * after, no document change, and overlapping ranges, only the two ends that moved
 * matter (Shift-extending a select-all costs the extension, not the selection).
 * Anything else (several ranges, disjoint, or a doc change) takes every non-empty
 * range of both states, mapped.
 */
function selectionSpansToRecompute(tr: Transaction): Array<[number, number]> {
  const oldRanges = tr.startState.selection.ranges;
  const newRanges = tr.state.selection.ranges;
  if (!tr.docChanged && oldRanges.length === 1 && newRanges.length === 1) {
    const o = oldRanges[0];
    const n = newRanges[0];
    if (o.from <= n.to && n.from <= o.to) {
      const spans: Array<[number, number]> = [];
      if (o.from !== n.from) spans.push([Math.min(o.from, n.from), Math.max(o.from, n.from)]);
      if (o.to !== n.to) spans.push([Math.min(o.to, n.to), Math.max(o.to, n.to)]);
      return spans;
    }
  }
  const spans: Array<[number, number]> = [];
  for (const r of oldRanges) {
    if (r.anchor !== r.head) spans.push([tr.changes.mapPos(r.from), tr.changes.mapPos(r.to)]);
  }
  for (const r of newRanges) {
    if (r.anchor !== r.head) spans.push([r.from, r.to]);
  }
  return spans;
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
function replaceRegion(deco: DecorationSet, state: EditorState, region0: Span, selectedLines: ReadonlySet<number>): DecorationSet {
  // Widen the region to every existing decoration it overlaps or touches before walking: a
  // block cut short at its end (a table's last row deleted) leaves the change point in the
  // blank line after it, so the walk over the bare region never reaches the shortened block
  // while the filter below still drops its mapped widget.
  const region = { ...region0 };
  deco.between(region0.from, region0.to, (from, to) => {
    if (from < region.from) region.from = from;
    if (to > region.to) region.to = to;
  });
  const fresh = decorationsInRange(state, region.from, region.to, selectedLines);
  // The tree walk also reports blocks that merely touch the region's edge. Such a block
  // may already be in `deco` (kept by the strict test below), so a fresh one at the same
  // span replaces it instead of doubling it. A widget whose text was deleted collapses to
  // an empty span when mapped; no widget is ever empty, so those are dropped too.
  const freshSpans = new Set(fresh.map((f) => `${f.from}:${f.to}`));
  // A widget that touches the region's edge is not trusted either: a change at that edge
  // may have turned it into something else (a callout losing its header), and the walk
  // above is edge-inclusive, so it is found again if it is still a widget.
  const kept = deco.update({
    filter: (from, to) => from < to && (to < region.from || from > region.to) && !freshSpans.has(`${from}:${to}`),
  });
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

function computeDirtyRegions(tr: Transaction, examined: number, reliable: number): Span[] {
  const regions: Span[] = [];
  if (tr.docChanged) {
    // Both ends of every change, both sides of each (a deletion before a block moves
    // that block to the change point; a first or last character resolves to the
    // neighbour on one side only), plus every block the inserted text spans (a pasted
    // table, a multi-block replace). A single-character insert (ordinary typing) skips
    // the range walk: it can only touch the blocks at its two ends, so its cost stays
    // O(depth) and every unrelated widget keeps its object (NFR-PRF-02).
    tr.changes.iterChanges((_fromA, _toA, fromB, toB) => {
      const touched = [...touchedTopLevelBlocks(tr.state, fromB), ...touchedTopLevelBlocks(tr.state, toB)];
      if (touched.length === 0) touched.push({ from: fromB, to: Math.max(toB, fromB + 1) });
      regions.push(...touched);
      if (toB - fromB > 1) regions.push(...blocksIntersecting(tr.state, fromB, toB));
    });
  }
  // CodeMirror parses a long note in time-sliced chunks, and cuts the tree
  // back to the viewport after an edit. Only territory beyond the watermark
  // has never been examined.
  const growth = growthRegion(examined, reliable);
  if (growth) {
    const start = enclosingTopLevelBlock(tr.state, Math.min(growth.from, tr.state.doc.length));
    regions.push({ from: start ? Math.min(start.from, growth.from) : growth.from, to: growth.to });
  }
  if (!tr.startState.selection.eq(tr.state.selection)) {
    // Both ends of every range, old (mapped) and new, plus the top-level blocks a
    // non-empty range spans: `selectedLines` (buildAll/update) comes from the full
    // ranges, so the dirty regions must too (FR-EDT-02). A collapsed cursor takes the
    // two O(depth) lookups only (no walk); cost grows with the selection, never the note.
    for (const range of tr.startState.selection.ranges) {
      regions.push(...touchedTopLevelBlocks(tr.state, tr.changes.mapPos(range.head)));
      if (range.anchor !== range.head) {
        regions.push(...touchedTopLevelBlocks(tr.state, tr.changes.mapPos(range.anchor)));
      }
    }
    for (const range of tr.state.selection.ranges) {
      regions.push(...touchedTopLevelBlocks(tr.state, range.head));
      if (range.anchor !== range.head) {
        regions.push(...touchedTopLevelBlocks(tr.state, range.anchor));
      }
    }
    for (const [from, to] of selectionSpansToRecompute(tr)) {
      regions.push(...blocksIntersecting(tr.state, from, to));
    }
  }
  return mergeSpans(regions);
}

/** The block-widget `StateField` (architecture.md §9.3). Toggle its effect
 * on/off per `livePreview` the same way `reveal.ts`'s plugin is toggled —
 * `CodeMirrorEditor.vue` wraps both in the same `Compartment` so split mode
 * (no hide/reveal at all, per FR-EDT-10) drops both together. */
export const blockWidgetsField: StateField<BlockWidgetState> = StateField.define<BlockWidgetState>({
  create(state) {
    return buildAll(state);
  },
  update(value, tr) {
    const result = updateBlockWidgets(value, tr);
    // A toggled callout's widget is rebuilt (its effective fold state changed); nothing else is.
    const toggled = toggledStarts(tr);
    if (toggled.length === 0) return result;
    const selectedLines = selectedLineNumbers(
      tr.state.doc,
      tr.state.selection.ranges.map((r) => ({ from: r.from, to: r.to })),
    );
    let deco = result.deco;
    for (const from of toggled) deco = replaceRegion(deco, tr.state, { from, to: from + 1 }, selectedLines);
    return { deco, examined: result.examined };
  },
  provide: (field) => EditorView.decorations.from(field, (value) => value.deco),
});

function updateBlockWidgets(value: BlockWidgetState, tr: Transaction): BlockWidgetState {
  {
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
    // What has been examined with a tree that could be trusted: the old watermark mapped
    // through the edit, but never beyond what is parsed now. Territory past the parse
    // boundary was (at best) examined against a partial tree, so it is re-examined once the
    // parse completes, in the follow-up transaction that carries no document change (#109).
    // With a finished parse (the normal case) `reliable` is the document end and nothing
    // here costs more than one `syntaxTreeAvailable` call.
    const reliable = reliableParsedTo(tr.state);
    const mapped = tr.docChanged ? tr.changes.mapPos(value.examined, -1) : value.examined;
    const examined = Math.min(mapped, reliable);
    const regions = computeDirtyRegions(tr, examined, reliable);
    const nextExamined = Math.max(examined, reliable);
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
  }
}

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
  // Fold glyph, count, focus (editor.md "Callouts and the fold marker"): tokens with fallbacks.
  ".serat-callout-foldable .serat-callout-title": { display: "flex", alignItems: "center", cursor: "default" },
  ".serat-callout-foldable .serat-callout-title:focus-visible": {
    outline: "2px solid var(--serat-border-focus, #24457e)",
    outlineOffset: "1px",
  },
  ".serat-callout-glyph": {
    flex: "0 0 auto",
    boxSizing: "border-box",
    width: "24px",
    height: "24px",
    margin: "-4px 0 -4px -10px", // the box reaches into the title row's leading padding: the text does not move
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    cursor: "pointer",
    color: "var(--serat-text-secondary, rgba(22, 25, 27, 0.7))",
    borderRadius: "var(--serat-radius-sm, 3px)",
  },
  ".serat-callout-glyph:hover, .serat-callout-title:focus-visible .serat-callout-glyph": {
    color: "var(--serat-accent-interactive, #24457e)",
  },
  ".serat-callout-glyph:hover": { background: "var(--serat-surface-sunken, rgba(22, 25, 27, 0.06))" },
  ".serat-callout-chevron": {
    display: "block",
    width: "7px",
    height: "7px",
    borderRight: "1.5px solid currentColor",
    borderBottom: "1.5px solid currentColor",
    transform: "rotate(-45deg)", // collapsed: points right
    transition: "transform var(--serat-duration-micro, 120ms) var(--serat-easing-enter, cubic-bezier(0.2, 0, 0, 1))",
  },
  '.serat-callout[data-fold="expanded"] .serat-callout-chevron': { transform: "rotate(45deg)" }, // points down
  ".serat-callout-count": {
    marginLeft: "auto",
    paddingLeft: "12px",
    fontSize: "12px",
    fontWeight: "400",
    color: "var(--serat-text-secondary, rgba(22, 25, 27, 0.7))",
  },
  ".serat-callout-sep": {
    position: "absolute",
    width: "1px",
    height: "1px",
    overflow: "hidden",
    clip: "rect(0 0 0 0)",
    whiteSpace: "nowrap",
  },
  "@media (prefers-reduced-motion: reduce)": {
    ".serat-callout-chevron": { transitionDuration: "1ms" },
  },
});

export function blockWidgetsExtension(): Extension {
  return [resolutionsField, calloutFoldField, blockWidgetsField, widgetTheme];
}
