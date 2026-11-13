// The front-end tree model (ADR-0015 §1, §4; FR-VLT-01, FR-VLT-08, FR-BIN-03).
//
// Messages are applied strictly in arrival order and none is dropped: the
// stream is lossless, so the model is a pure fold over it. The model never
// sorts and never compares names. A listing is a directory's complete,
// final-order contents; `Upsert` names the sibling it follows. A directory with
// no listing yet is *pending*; one whose listing is empty is *confirmed empty*
// (file-tree.md: the two are never conflated).
//
// The model holds plain objects and is deliberately not reactive; the component
// bumps a version counter after a batch, so a 20k-entry tree costs Vue nothing
// per entry.

import type { TreeEntry } from "@/ipc/generated/TreeEntry";
import type { TreeEntryKind } from "@/ipc/generated/TreeEntryKind";
import type { TreeMessage } from "@/ipc/generated/TreeMessage";
import type { TreeSource } from "@/ipc/generated/TreeSource";

export type RowState = "leaf" | "pending" | "empty" | "collapsed" | "expanded";

export interface TreeRow {
  /** Vault-relative path (`dir/name`; the root listing's entries have no prefix). */
  path: string;
  dir: string;
  name: string;
  kind: TreeEntryKind;
  /** An asset paired with a sidecar note (folded into this one row). */
  sidecar: boolean;
  depth: number;
  state: RowState;
}

export function childPath(dir: string, name: string): string {
  return dir === "" ? name : `${dir}/${name}`;
}

export class TreeModel {
  private readonly dirs = new Map<string, TreeEntry[]>();
  private readonly expandedDirs = new Set<string>();
  private rowsCache: TreeRow[] | null = null;
  private rowsCacheFilter = "";
  /** Count of non-directory entries in the model (what `TreeComplete.entries` counts). */
  fileCount = 0;
  complete: { entries: number; source: TreeSource } | null = null;
  /** Set once by `WalkSettled`: how many empty folders the verifying walk found. */
  emptyFolders: number | null = null;
  /** Messages applied; lets tests and the mark logic assert nothing was dropped. */
  applied = 0;
  /** Upserts whose `after` sibling was missing (Rust and the model disagreeing); appended, counted. */
  misplacedUpserts = 0;

  /** Forgets everything: a different vault is being opened (#151); nothing of the old one may stay. */
  clear(): void {
    this.dirs.clear();
    this.expandedDirs.clear();
    this.rowsCache = null;
    this.rowsCacheFilter = "";
    this.fileCount = 0;
    this.complete = null;
    this.emptyFolders = null;
    this.applied = 0;
    this.misplacedUpserts = 0;
  }

  // ---- applying messages -------------------------------------------------

  apply(message: TreeMessage): void {
    this.applied += 1;
    this.rowsCache = null;
    switch (message.kind) {
      case "listing":
        this.applyListing(message.dir, message.entries);
        break;
      case "upsert":
        this.applyUpsert(message.dir, message.entry, message.after);
        break;
      case "remove":
        this.applyRemove(message.dir, message.name);
        break;
      case "complete":
        this.complete = { entries: message.entries, source: message.source };
        break;
      case "walk_settled":
        this.emptyFolders = message.empty_folders;
        break;
    }
  }

  private applyListing(dir: string, entries: readonly TreeEntry[]): void {
    const old = this.dirs.get(dir);
    if (old) {
      const keep = new Set(entries.filter((e) => e.kind === "dir").map((e) => e.name));
      for (const e of old) {
        if (e.kind === "dir" && !keep.has(e.name)) this.dropSubtree(childPath(dir, e.name));
      }
      this.fileCount -= countFiles(old);
    }
    this.dirs.set(dir, entries.slice());
    this.fileCount += countFiles(entries);
  }

  private applyUpsert(dir: string, entry: TreeEntry, after: string | null): void {
    const list = this.dirs.get(dir);
    // A directory without a listing yet gets everything in its own (complete)
    // listing later; there is nothing to insert into.
    if (!list) return;
    const existing = list.findIndex((e) => e.name === entry.name);
    if (existing >= 0) {
      const previous = list[existing];
      if (previous.kind === "dir" && entry.kind !== "dir") {
        this.dropSubtree(childPath(dir, previous.name));
      }
      if (previous.kind !== "dir") this.fileCount -= 1;
      list.splice(existing, 1);
    }
    let at = 0;
    if (after !== null) {
      const index = list.findIndex((e) => e.name === after);
      if (index < 0) {
        this.misplacedUpserts += 1;
        at = list.length;
      } else {
        at = index + 1;
      }
    }
    list.splice(at, 0, entry);
    if (entry.kind !== "dir") this.fileCount += 1;
  }

  private applyRemove(dir: string, name: string): void {
    const list = this.dirs.get(dir);
    if (!list) return;
    const index = list.findIndex((e) => e.name === name);
    if (index < 0) return;
    const [removed] = list.splice(index, 1);
    if (removed.kind === "dir") this.dropSubtree(childPath(dir, name));
    else this.fileCount -= 1;
  }

  /** Removes a directory's listing and every descendant's (a rename or delete of a subtree). */
  private dropSubtree(path: string): void {
    const prefix = `${path}/`;
    for (const key of [...this.dirs.keys()]) {
      if (key === path || key.startsWith(prefix)) {
        this.fileCount -= countFiles(this.dirs.get(key) ?? []);
        this.dirs.delete(key);
      }
    }
    for (const key of [...this.expandedDirs]) {
      if (key === path || key.startsWith(prefix)) this.expandedDirs.delete(key);
    }
  }

  // ---- queries -----------------------------------------------------------

  isListed(dir: string): boolean {
    return this.dirs.has(dir);
  }

  listing(dir: string): readonly TreeEntry[] | undefined {
    return this.dirs.get(dir);
  }

  /** Every listing held, for whole-model scans that are not on a hot path. */
  allListings(): IterableIterator<readonly TreeEntry[]> {
    return this.dirs.values();
  }

  get rootListed(): boolean {
    return this.dirs.has("");
  }

  /** Number of directories with a listing (including the root). */
  get listedDirCount(): number {
    return this.dirs.size;
  }

  isExpanded(path: string): boolean {
    return this.expandedDirs.has(path);
  }

  expand(path: string): void {
    // Only a listed, non-empty directory can expand; nothing is offered otherwise.
    const list = this.dirs.get(path);
    if (list && list.length > 0 && !this.expandedDirs.has(path)) {
      this.expandedDirs.add(path);
      this.rowsCache = null;
    }
  }

  collapse(path: string): void {
    if (this.expandedDirs.delete(path)) this.rowsCache = null;
  }

  toggle(path: string): void {
    if (this.expandedDirs.has(path)) this.collapse(path);
    else this.expand(path);
  }

  /**
   * The visible rows, depth-first through expanded directories. Cached until the
   * next change. With a non-empty `filter` (the tree's own filter box,
   * file-tree.md) only entries whose display name contains it (case-insensitive)
   * and the folders on the way to them are kept; those ancestor folders show
   * expanded. Only directories already listed can be searched.
   */
  rows(filter = ""): readonly TreeRow[] {
    const needle = filter.trim().toLowerCase();
    if (this.rowsCache && this.rowsCacheFilter === needle) return this.rowsCache;
    const keep = needle ? this.filterKeepSet(needle) : null;
    const rows: TreeRow[] = [];
    const root = this.dirs.get("");
    if (root) {
      // Explicit stack: a deep vault must not overflow the call stack.
      const stack: Array<{ dir: string; entries: readonly TreeEntry[]; index: number; depth: number }> = [
        { dir: "", entries: root, index: 0, depth: 0 },
      ];
      while (stack.length > 0) {
        const frame = stack[stack.length - 1];
        if (frame.index >= frame.entries.length) {
          stack.pop();
          continue;
        }
        const entry = frame.entries[frame.index];
        frame.index += 1;
        const path = childPath(frame.dir, entry.name);
        if (keep && !keep.has(path)) continue;
        let state: RowState = "leaf";
        let children: readonly TreeEntry[] | undefined;
        if (entry.kind === "dir") {
          children = this.dirs.get(path);
          if (!children) state = "pending";
          else if (children.length === 0) state = "empty";
          else if (keep) state = keep.get(path) === "ancestor" ? "expanded" : "collapsed";
          else state = this.expandedDirs.has(path) ? "expanded" : "collapsed";
        }
        rows.push({
          path,
          dir: frame.dir,
          name: entry.name,
          kind: entry.kind,
          sidecar: entry.sidecar,
          depth: frame.depth,
          state,
        });
        if (state === "expanded" && children) {
          stack.push({ dir: path, entries: children, index: 0, depth: frame.depth + 1 });
        }
      }
    }
    this.rowsCache = rows;
    this.rowsCacheFilter = needle;
    return rows;
  }

  /** Paths to keep under a filter: matches, and the ancestors of matches (marked "ancestor"). */
  private filterKeepSet(needle: string): Map<string, "match" | "ancestor"> {
    const keep = new Map<string, "match" | "ancestor">();
    for (const [dir, entries] of this.dirs) {
      for (const entry of entries) {
        if (!displayName(entry.name, entry.kind).toLowerCase().includes(needle)) continue;
        const path = childPath(dir, entry.name);
        if (!keep.has(path)) keep.set(path, "match");
        // Walk up the path; stop at the first ancestor already recorded.
        let at = dir;
        while (at !== "") {
          if (keep.get(at) === "ancestor") break;
          keep.set(at, "ancestor");
          const slash = at.lastIndexOf("/");
          at = slash < 0 ? "" : at.slice(0, slash);
        }
      }
    }
    return keep;
  }
}

/** What a row shows: a note without its `.md`. */
export function displayName(name: string, kind: TreeEntryKind): string {
  return kind === "note" ? name.replace(/\.md$/i, "") : name;
}

/** The first case-insensitive occurrence of `needle` in `text`, for bolding; null when absent. */
export function matchRange(text: string, needle: string): [number, number] | null {
  const n = needle.trim().toLowerCase();
  if (!n) return null;
  const at = text.toLowerCase().indexOf(n);
  return at < 0 ? null : [at, at + n.length];
}

function countFiles(entries: readonly TreeEntry[]): number {
  let n = 0;
  for (const e of entries) if (e.kind !== "dir") n += 1;
  return n;
}
