// Pure logic behind the file tree component (file-tree.md): the virtual window,
// keyboard navigation, and the link text for "insert link to attachment". Kept
// separate from the component so each rule is unit-testable without a DOM.

import type { TreeModel, TreeRow } from "@/tree/treeModel";

export interface VisibleWindow {
  /** First row index to render. */
  start: number;
  /** One past the last row index to render. */
  end: number;
  /** Total scrollable height in px. */
  totalHeight: number;
  /** Top offset of row `start` in px. */
  offsetTop: number;
}

/** ADR-0015 §4: DOM rows are proportional to the viewport, never to the vault. */
export function visibleWindow(
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  total: number,
  overscan = 8,
): VisibleWindow {
  const first = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const last = Math.min(total, Math.ceil((scrollTop + viewportHeight) / rowHeight) + overscan);
  const start = Math.min(first, total);
  return { start, end: Math.max(start, last), totalHeight: total * rowHeight, offsetTop: start * rowHeight };
}

export type NavAction =
  | { kind: "focus"; path: string }
  | { kind: "expand"; path: string }
  | { kind: "collapse"; path: string }
  | { kind: "open"; path: string; newTab: boolean; keepFocus: boolean }
  | { kind: "rename"; path: string }
  | { kind: "context-menu"; path: string }
  | { kind: "insert-link"; path: string }
  | { kind: "none" };

export interface KeyInfo {
  key: string;
  ctrlOrMeta: boolean;
  shift: boolean;
}

/**
 * Maps a key press on the focused row to what the tree should do
 * (file-tree.md, Keyboard map). One meaning per key: Enter opens, F2 renames.
 */
export function navigate(rows: readonly TreeRow[], focusedPath: string | null, key: KeyInfo): NavAction {
  if (rows.length === 0) return { kind: "none" };
  const index = focusedPath === null ? -1 : rows.findIndex((r) => r.path === focusedPath);
  const row = index >= 0 ? rows[index] : null;
  const at = (i: number): NavAction => ({ kind: "focus", path: rows[Math.max(0, Math.min(rows.length - 1, i))].path });

  switch (key.key) {
    case "ArrowDown":
      return at(index < 0 ? 0 : index + 1);
    case "ArrowUp":
      return at(index < 0 ? 0 : index - 1);
    case "Home":
      return at(0);
    case "End":
      return at(rows.length - 1);
    case "ArrowRight":
      if (!row) return at(0);
      if (row.kind === "dir") {
        if (row.state === "collapsed") return { kind: "expand", path: row.path };
        if (row.state === "expanded") return at(index + 1);
        return { kind: "none" }; // pending or confirmed-empty: nothing to expand into
      }
      return { kind: "open", path: row.path, newTab: false, keepFocus: true };
    case "ArrowLeft": {
      if (!row) return at(0);
      if (row.kind === "dir" && row.state === "expanded") return { kind: "collapse", path: row.path };
      const parent = rows.findIndex((r) => r.path === row.dir && r.depth < row.depth);
      return parent >= 0 ? { kind: "focus", path: rows[parent].path } : { kind: "none" };
    }
    case "Enter":
      if (!row) return { kind: "none" };
      if (row.kind === "dir") {
        if (row.state === "collapsed") return { kind: "expand", path: row.path };
        if (row.state === "expanded") return { kind: "collapse", path: row.path };
        return { kind: "none" };
      }
      return { kind: "open", path: row.path, newTab: key.ctrlOrMeta, keepFocus: false };
    case "F2":
      return row ? { kind: "rename", path: row.path } : { kind: "none" };
    case "ContextMenu":
      return row ? { kind: "context-menu", path: row.path } : { kind: "none" };
    case "F10":
      return key.shift && row ? { kind: "context-menu", path: row.path } : { kind: "none" };
    case "L":
    case "l":
      return key.ctrlOrMeta && key.shift && row && row.kind !== "dir"
        ? { kind: "insert-link", path: row.path }
        : { kind: "none" };
    default:
      return { kind: "none" };
  }
}

const EMBED_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "svg", "pdf"]);

function extensionOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot < 0 ? "" : name.slice(dot + 1).toLowerCase();
}

/**
 * The link `Insert link` writes at the editor's cursor (file-tree.md,
 * architecture.md §7.4): `![[name.png]]` for an image or PDF, `[[name.ext]]`
 * for other assets, `[[stem]]` for a note. The shortest form that resolves:
 * the bare file name when no other listed entry shares it, else the vault path.
 */
export function insertLinkText(row: Pick<TreeRow, "path" | "name" | "kind">, nameIsUnique: boolean): string {
  const isNote = row.kind === "note";
  const written = nameIsUnique ? row.name : row.path;
  const target = isNote ? written.replace(/\.md$/i, "") : written;
  const embed = row.kind === "asset" && EMBED_EXTENSIONS.has(extensionOf(row.name));
  return embed ? `![[${target}]]` : `[[${target}]]`;
}

/** Whether `name` is carried by exactly one listed entry in the model. */
export function isNameUnique(model: TreeModel, name: string): boolean {
  let seen = 0;
  for (const dirRows of model.allListings()) {
    for (const entry of dirRows) {
      if (entry.kind !== "dir" && entry.name === name) {
        seen += 1;
        if (seen > 1) return false;
      }
    }
  }
  return true;
}
