// ADR-0016 §1/§2: the one coordinate system, both directions. A position is
// `{ line, col }` — `line` 0-based, `col` in UTF-16 code units within that
// line — mapped against a *base* `Text`. Lines split on `\r\n`, `\n` and a
// lone `\r`, exactly as CodeMirror splits them: this module leans on that
// fact rather than re-implementing it, because `@codemirror/state`'s own
// `Text` already stores documents this way (see `DefaultSplit` in
// `@codemirror/state`'s source: `/\r\n?|\n/`, applied whenever no
// `EditorState.lineSeparator` facet is configured — and nothing here ever
// configures one, so the front end always builds/patches in CM6's own line
// model, which is ADR-0016's requirement stated as a fact about the library
// rather than a rule this file has to enforce separately).
//
// JS strings are UTF-16 already, so `col` is exactly a JS string index minus
// the containing line's start offset — no separate UTF-16 accounting is
// needed on this side of the boundary.
//
// A BOM (U+FEFF), if present, is part of line 0's text here, exactly as
// ADR-0016 §1 requires — *because* the document is always built from
// `read_file`'s `String` (see `src/startup.ts`'s `hydrateBuffer`) and never
// run through a default `TextDecoder`, which strips it. This module does not
// special-case the BOM at all: it is simply character U+FEFF at offset 0 of
// line 0, and every offset downstream of it is already correct as long as
// nothing upstream stripped it.

import { ChangeSet, Text } from "@codemirror/state";

import type { Patch } from "@/ipc/generated/Patch";
import type { Pos } from "@/ipc/generated/Pos";

/**
 * CM6's own default line-split regex (`@codemirror/state`'s unexported
 * `DefaultSplit`), duplicated here rather than imported because it is not
 * part of the package's public API. It is what `EditorState.create({ doc })`
 * uses whenever no `EditorState.lineSeparator` facet is configured — which
 * this codebase never configures (see the module note above) — so building a
 * base `Text` with it is building it exactly the way the live document is
 * built.
 */
const CM_DEFAULT_SPLIT = /\r\n?|\n/;

/**
 * Builds a base `Text` from a raw string (a `read_file` result, never a
 * `TextDecoder`-decoded byte buffer — see the module note above) using CM6's
 * own line-splitting rule, so a base built here and a live `EditorView`'s
 * document always agree on where line boundaries are.
 */
export function textFromString(text: string): Text {
  return Text.of(text.split(CM_DEFAULT_SPLIT));
}

/** Converts a UTF-16 code-unit offset into `doc` to ADR-0016's `{ line, col }`. */
export function posAt(doc: Text, offset: number): Pos {
  const line = doc.lineAt(offset);
  return { line: line.number - 1, col: offset - line.from };
}

/** Raised by {@link offsetAt} for a position that does not name a place in
 * `doc`. A caller holding externally supplied positions (e.g. an
 * `ExternalChange` diff) must treat this as "the positions are unusable" and
 * request the external diff / reload rather than clamp or guess. */
export class InvalidPositionError extends RangeError {
  constructor(message: string) {
    super(message);
    this.name = "InvalidPositionError";
  }
}

/**
 * The inverse of {@link posAt}: `{ line, col }` back to an offset into `doc`.
 * Throws {@link InvalidPositionError} when the line does not exist, `col`
 * lies past the end of its line, or `col` falls between the two halves of a
 * surrogate pair (ADR-0016 §1: columns are UTF-16 units but a patch may not
 * split a scalar value). Silently running into the next line, or splitting
 * an emoji, would corrupt the note.
 */
export function offsetAt(doc: Text, pos: Pos): number {
  if (!Number.isInteger(pos.line) || !Number.isInteger(pos.col) || pos.line < 0 || pos.col < 0) {
    throw new InvalidPositionError(`invalid position ${pos.line}:${pos.col}`);
  }
  if (pos.line + 1 > doc.lines) {
    throw new InvalidPositionError(`line ${pos.line} is past the end of the document (${doc.lines} lines)`);
  }
  const line = doc.line(pos.line + 1);
  if (pos.col > line.length) {
    throw new InvalidPositionError(`column ${pos.col} is past the end of line ${pos.line} (length ${line.length})`);
  }
  if (pos.col > 0 && pos.col < line.length) {
    const before = line.text.charCodeAt(pos.col - 1);
    const after = line.text.charCodeAt(pos.col);
    if (before >= 0xd800 && before <= 0xdbff && after >= 0xdc00 && after <= 0xdfff) {
      throw new InvalidPositionError(`column ${pos.col} on line ${pos.line} splits a surrogate pair`);
    }
  }
  return line.from + pos.col;
}

/**
 * Converts a `ChangeSet` composed against `base` into ADR-0016 §2's patch
 * set: `[{ from, to, insert }]` in base coordinates, ordered and disjoint —
 * which is exactly what `ChangeSet.iterChanges` already yields, because a
 * `ChangeSet`'s changes are stored in ascending, non-overlapping order
 * against the document it was composed from.
 *
 * `insert` always uses `\n`, never `\r`: `Text#toString()` — what
 * `inserted.toString()` calls here — joins with `\n` unconditionally, and
 * step 3 of ADR-0016 §2 (serat-vault) is what turns that into the note's own
 * line ending. This module writes no other separator.
 */
export function patchesFromChangeSet(base: Text, changes: ChangeSet): Patch[] {
  const patches: Patch[] = [];
  changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
    patches.push({
      from: posAt(base, fromA),
      to: posAt(base, toA),
      insert: inserted.toString(),
    });
  });
  return patches;
}

/** Composes an ordered list of `ChangeSet`s, each relative to the document the
 * previous one produced, into one `ChangeSet` relative to `base`. Used to
 * turn a run of per-transaction changesets since the last save into the one
 * patch set ADR-0016 §2 sends. */
export function composeSince(base: Text, changesets: readonly ChangeSet[]): ChangeSet {
  return changesets.reduce((acc, cs) => acc.compose(cs), ChangeSet.empty(base.length));
}
