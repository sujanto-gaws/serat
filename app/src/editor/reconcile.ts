// External-change reconciliation, the pure half (ADR-0016 §3 and §4; FR-VLT-05,
// NFR-REL-01, C-04). Everything here is a function of a base `Text`, the local
// `ChangeSet` composed since that base, and the external edit script, all in
// the same coordinates. The component glue (`CodeMirrorEditor.vue`) dispatches
// what these functions decide; none of it touches the document itself.

import { ChangeSet, type Text } from "@codemirror/state";

import { offsetAt } from "@/editor/patches";
import type { Patch } from "@/ipc/generated/Patch";

/** The external edit script as a `ChangeSet` against `base`. Throws
 * `InvalidPositionError` when a position does not exist in `base` (the caller
 * then has no usable edit script and takes the conflict path). */
export function externalChangeSet(base: Text, edits: readonly Patch[]): ChangeSet {
  return patchesToChangeSet(base, edits);
}

/** Patches (`{from,to,insert}` in `doc`'s `{line,col}` coordinates) as a `ChangeSet` against `doc`. */
export function patchesToChangeSet(doc: Text, patches: readonly Patch[]): ChangeSet {
  return ChangeSet.of(
    patches.map((p) => ({ from: offsetAt(doc, p.from), to: offsetAt(doc, p.to), insert: p.insert })),
    doc.length,
  );
}

function ranges(changes: ChangeSet): Array<[number, number]> {
  const out: Array<[number, number]> = [];
  changes.iterChangedRanges((fromA, toA) => out.push([fromA, toA]));
  return out;
}

/** ADR-0016 §4: ranges that touch count as overlapping (inclusive on both
 * ends), so two edits at the same insertion point conflict. Both change sets
 * are in the same base coordinates. Linear merge over two sorted lists. */
export function overlaps(local: ChangeSet, external: ChangeSet): boolean {
  const a = ranges(local);
  const b = ranges(external);
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    const [af, at] = a[i];
    const [bf, bt] = b[j];
    if (af <= bt && bf <= at) return true;
    if (at < bt) i += 1;
    else j += 1;
  }
  return false;
}

export type ReconcilePlan =
  /** No document change: only the base hash moves (e.g. a CRLF<->LF flip, `edits = []`). */
  | { kind: "hash-only" }
  /** No unsaved changes: apply `external` as is. */
  | { kind: "apply-clean"; external: ChangeSet }
  /** Disjoint unsaved changes: apply `externalOnDoc` to the live document; the
   * base becomes disk and the local changes become `localRebased`. */
  | { kind: "apply-rebased"; external: ChangeSet; externalOnDoc: ChangeSet; localRebased: ChangeSet }
  /** Overlap: apply nothing, ask the user. */
  | { kind: "conflict" };

/** Decides the §4 outcome. `local` is the composed pending change set, or
 * null/empty when there are no unsaved changes. */
export function planExternal(external: ChangeSet, local: ChangeSet | null): ReconcilePlan {
  if (external.empty) return { kind: "hash-only" };
  if (!local || local.empty) return { kind: "apply-clean", external };
  if (overlaps(local, external)) return { kind: "conflict" };
  return {
    kind: "apply-rebased",
    external,
    // External edits expressed on the live document (base + local).
    externalOnDoc: external.map(local),
    // Local edits expressed on the new base (disk).
    localRebased: local.map(external),
  };
}

/** The smallest single replacement turning `from` into `to` (common prefix and
 * suffix trimmed), so replacing the whole buffer still keeps a cursor outside
 * the changed middle where it was. */
export function minimalReplacement(from: string, to: string): { from: number; to: number; insert: string } {
  let start = 0;
  const max = Math.min(from.length, to.length);
  while (start < max && from.charCodeAt(start) === to.charCodeAt(start)) start += 1;
  // Do not split a surrogate pair.
  if (start > 0 && start < from.length) {
    const c = from.charCodeAt(start - 1);
    if (c >= 0xd800 && c <= 0xdbff) start -= 1;
  }
  let endFrom = from.length;
  let endTo = to.length;
  while (endFrom > start && endTo > start && from.charCodeAt(endFrom - 1) === to.charCodeAt(endTo - 1)) {
    endFrom -= 1;
    endTo -= 1;
  }
  if (endFrom < from.length && endFrom > start) {
    const c = from.charCodeAt(endFrom - 1);
    if (c >= 0xd800 && c <= 0xdbff) {
      endFrom += 1;
      endTo += 1;
    }
  }
  return { from: start, to: endFrom, insert: to.slice(start, endTo) };
}

