// ADR-0016 §2: "The front end keeps, per open note, the base — the Text as
// last loaded or saved, and its hash — and the ChangeSet composed from every
// document change since the base." This module is that state, plus the
// save orchestration rules §2 spells out:
//
//   - "At most one save per note is in flight."
//   - "A save triggered during another save does not start; triggers
//     coalesce into one save issued when the in-flight save resolves,
//     carrying every change made meanwhile against the base that save
//     produced."
//   - `write_file` is never called for an existing note; only `save_patch`.
//
// External-change reconciliation (§3/§4) and *Keep mine*/`rebase_buffer`
// are reconciled by `reconcile.ts` and the component glue; this module holds the state (`adoptExternal`,
// `rebaseOnto`) without implementing the conflict UI, so #23 does not have
// to redesign the buffer to get them.

import { ChangeSet, Text } from "@codemirror/state";

import { composeSince, patchesFromChangeSet, textFromString } from "@/editor/patches";
import type { EditorIpcAdapter } from "@/ipc/editorAdapter";
import type { FileWritten } from "@/ipc/generated/FileWritten";
import type { Patch } from "@/ipc/generated/Patch";

export type SaveOutcome =
  | { kind: "noop" } // nothing dirty
  | { kind: "coalesced" } // a save was already in flight; this one will ride the next round
  | { kind: "saved"; written: FileWritten }
  // The file changed on disk since the base (`StaleHash`), or an earlier save
  // already found that and nothing has reconciled it yet. No IPC call is made
  // while stale; the caller must call `requestExternalDiff` and then
  // `applyExternal`/`rebaseOnto` (ADR-0016 §2/§4). Never retried blindly.
  | { kind: "stale"; error: unknown }
  // The patch itself was refused (`InvalidPatch`, `NotWritable`, `Io`, a
  // transport failure). This is not a disk-divergence: the buffer stays dirty
  // and the next trigger may try again, but it is reported distinctly so the
  // UI does not offer a merge for something a merge cannot fix.
  | { kind: "rejected"; error: unknown }
  // `LineEndingsWouldFuse`: the save was refused because applying it would
  // fuse a lone CR with an LF in a note that mixes line endings; nothing was
  // written. The buffer stays dirty and intact, and NO automatic trigger
  // (idle, blur, note switch) retries it: only an explicit save command does.
  | { kind: "refused"; error: unknown };

function isLineEndingsWouldFuse(error: unknown): boolean {
  return (
    typeof error === "object" && error !== null && (error as { kind?: unknown }).kind === "line_endings_would_fuse"
  );
}

function isStaleHash(error: unknown): boolean {
  return typeof error === "object" && error !== null && (error as { kind?: unknown }).kind === "stale_hash";
}

export class EditorBuffer {
  readonly path: string;
  private baseTextValue: Text;
  private baseHashValue: string;
  /** Per-transaction changesets since `baseTextValue`, in order. Kept as an
   * array rather than one pre-composed `ChangeSet` so a save in flight can
   * remember exactly which prefix it sent (§2's coalescing rule) without
   * needing to "subtract" a `ChangeSet`, which is not a supported operation. */
  private pending: ChangeSet[] = [];
  private saving = false;
  private saveAgainRequested = false;
  /** Resolves when the whole current save chain (first save plus any
   * coalesced follow-up) has finished. `null` when idle. */
  private inflight: Promise<void> | null = null;
  /** Set by a `StaleHash` refusal; blocks every save until reconciled. */
  private staleError: { error: unknown } | null = null;
  /** Set by a `LineEndingsWouldFuse` refusal; blocks automatic saves only. */
  private refusedError: { error: unknown } | null = null;

  constructor(path: string, baseText: string, baseHash: string) {
    this.path = path;
    this.baseTextValue = textFromString(baseText);
    this.baseHashValue = baseHash;
  }

  get baseText(): Text {
    return this.baseTextValue;
  }

  get baseHash(): string {
    return this.baseHashValue;
  }

  get isDirty(): boolean {
    return this.pending.length > 0;
  }

  /** True after a `StaleHash` refusal until `applyExternal`/`rebaseOnto`. */
  get isStale(): boolean {
    return this.staleError !== null;
  }

  /** Records one transaction's changes (relative to the document they were
   * applied to, i.e. base + every previous entry of `pending`). A no-op
   * transaction (selection-only) is not recorded — it has nothing to save. */
  recordChange(changes: ChangeSet): void {
    if (changes.empty) {
      return;
    }
    this.pending.push(changes);
  }

  /** Every change since the base, composed into one `ChangeSet` — what a
   * save would send right now. Exposed for callers that need to preview or
   * inspect pending patches without triggering a save (e.g. a future crash
   * recovery journal, architecture.md §10.1). */
  composedPending(): ChangeSet {
    return composeSince(this.baseTextValue, this.pending);
  }

  /**
   * Triggers a save. Per ADR-0016 §2: at most one save is in flight for this
   * buffer; a call while one is already running does not start a second —
   * it coalesces, and the run already in flight will, on resolving, issue
   * exactly one more save covering everything typed meanwhile.
   */
  async save(ipc: EditorIpcAdapter, options: { explicit?: boolean } = {}): Promise<SaveOutcome> {
    if (this.staleError) {
      return { kind: "stale", error: this.staleError.error };
    }
    if (this.refusedError) {
      if (!options.explicit) return { kind: "refused", error: this.refusedError.error };
      this.refusedError = null; // the user asked again: one attempt
    }
    if (this.saving) {
      this.saveAgainRequested = true;
      return { kind: "coalesced" };
    }
    if (this.pending.length === 0) {
      return { kind: "noop" };
    }
    this.saving = true;
    const first = this.runSave(ipc);
    this.inflight = (async () => {
      try {
        let outcome = await first;
        while (outcome.kind === "saved" && this.saveAgainRequested && this.pending.length > 0) {
          this.saveAgainRequested = false;
          outcome = await this.runSave(ipc);
        }
      } finally {
        this.saveAgainRequested = false;
        this.saving = false;
        this.inflight = null;
      }
    })();
    return first;
  }

  /**
   * Resolves once every pending edit is on disk (or cannot be written):
   * waits for any in-flight save, then saves what remains, repeating until
   * clean. Returns `true` only when the buffer is clean — `false` when it is
   * stale or the write was rejected, in which case the edits are still held
   * here and the caller must not discard this buffer.
   */
  async flush(ipc: EditorIpcAdapter): Promise<boolean> {
    for (;;) {
      if (this.inflight) {
        await this.inflight;
        continue;
      }
      if (!this.isDirty) return true;
      const outcome = await this.save(ipc);
      if (outcome.kind !== "saved") {
        if (this.inflight) continue;
        return !this.isDirty;
      }
    }
  }

  private async runSave(ipc: EditorIpcAdapter): Promise<SaveOutcome> {
    const sentCount = this.pending.length;
    const composed = composeSince(this.baseTextValue, this.pending.slice(0, sentCount));
    if (composed.empty) {
      // Edits that cancelled out (type, then undo): the document equals the
      // base, so there is nothing to send and no reason to touch the disk.
      this.pending = this.pending.slice(sentCount);
      return { kind: "noop" };
    }
    const patches: Patch[] = patchesFromChangeSet(this.baseTextValue, composed);
    const baseHashAtSend = this.baseHashValue;

    try {
      const written = await ipc.savePatch({ path: this.path, base_hash: baseHashAtSend, patches });
      // The base becomes the document as it was when the save was taken
      // (ADR-0016 §2): apply exactly the prefix that was sent, not
      // whatever `pending` holds by the time the promise resolves.
      this.baseTextValue = composed.apply(this.baseTextValue);
      this.baseHashValue = written.hash;
      this.pending = this.pending.slice(sentCount);
      return { kind: "saved", written };
    } catch (error) {
      // A refused save leaves the buffer dirty and the base unmoved. Only a
      // StaleHash means the disk diverged; that blocks all further saves
      // until reconciled. Anything else is a rejection of this patch set.
      if (isStaleHash(error)) {
        this.staleError = { error };
        return { kind: "stale", error };
      }
      if (isLineEndingsWouldFuse(error)) {
        this.refusedError = { error };
        return { kind: "refused", error };
      }
      return { kind: "rejected", error };
    }
  }

  /**
   * ADR-0016 §2 rule 3/4: the disk changed and the caller has reconciled it
   * (no unsaved edits to keep, or they were rebased). Adopts `diskText` /
   * `diskHash` as the base, drops pending changes and clears the stale
   * block. Use `rebaseOnto` to keep local edits on top.
   */
  applyExternal(diskText: string, diskHash: string): void {
    this.rebaseOnto(diskText, diskHash, null);
  }

  /**
   * ADR-0016 §4 outcomes 1 and 2 (and the `edits = []` case): the external
   * edit script `external` (base coordinates) has been applied to the live
   * document. The base becomes the disk version (`external` applied to the
   * old base, `newHash`); `rebasedLocal` is the unsaved work re-expressed on
   * that new base, or null when there is none. Clears the stale block.
   */
  adoptExternal(external: ChangeSet, newHash: string, rebasedLocal: ChangeSet | null): void {
    this.baseTextValue = external.apply(this.baseTextValue);
    this.baseHashValue = newHash;
    this.pending = rebasedLocal && !rebasedLocal.empty ? [rebasedLocal] : [];
    this.staleError = null;
    this.refusedError = null;
  }

  /** True while a save (including a coalesced follow-up) is in flight. */
  get isSaving(): boolean {
    return this.saving;
  }

  /** Resolves when no save is in flight. */
  async settled(): Promise<void> {
    while (this.inflight) await this.inflight;
  }

  /** Disk equals the base again after a `StaleHash`: the divergence is gone. */
  clearStale(): void {
    this.staleError = null;
  }

  /**
   * ADR-0016 §4's *Keep mine*, the `rebase_buffer` half: adopts `diskText`
   * (and `diskHash`) as the new base. This is the only place besides a
   * successful save where the base may change, and it never happens
   * implicitly. The pending changeset is reset here to `nextPending` —
   * built by the caller as `state.changes([...])` from `rebase_buffer`'s
   * returned patches, since constructing a `ChangeSet` from patch specs
   * needs a live `EditorState`, which this module intentionally does not
   * hold (it is the buffer's bookkeeping, not the editor). Task #23 wires
   * the conflict notice's *Keep mine* button to this.
   */
  rebaseOnto(diskText: string, diskHash: string, nextPending: ChangeSet | null = null): void {
    this.baseTextValue = textFromString(diskText);
    this.baseHashValue = diskHash;
    this.pending = nextPending && !nextPending.empty ? [nextPending] : [];
    this.staleError = null;
    this.refusedError = null;
  }
}
