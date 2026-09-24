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
// are task #23's — this module exposes the hooks they need (`applyExternal`,
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
  | { kind: "stale"; error: unknown }; // caller must call requestExternalDiff (ADR-0016 §2)

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
  async save(ipc: EditorIpcAdapter): Promise<SaveOutcome> {
    if (this.saving) {
      this.saveAgainRequested = true;
      return { kind: "coalesced" };
    }
    return this.runSave(ipc);
  }

  private async runSave(ipc: EditorIpcAdapter): Promise<SaveOutcome> {
    if (this.pending.length === 0) {
      return { kind: "noop" };
    }
    this.saving = true;
    const sentCount = this.pending.length;
    const composed = composeSince(this.baseTextValue, this.pending.slice(0, sentCount));
    const patches: Patch[] = patchesFromChangeSet(this.baseTextValue, composed);
    const baseHashAtSend = this.baseHashValue;

    let outcome: SaveOutcome;
    try {
      const written = await ipc.savePatch({ path: this.path, base_hash: baseHashAtSend, patches });
      // The base becomes the document as it was when the save was taken
      // (ADR-0016 §2): apply exactly the prefix that was sent, not
      // whatever `pending` holds by the time the promise resolves.
      this.baseTextValue = composed.apply(this.baseTextValue);
      this.baseHashValue = written.hash;
      this.pending = this.pending.slice(sentCount);
      outcome = { kind: "saved", written };
    } catch (error) {
      // A refused save (StaleHash or otherwise) leaves the buffer dirty and
      // is never retried blindly — the caller (App-level buffer manager,
      // task #23) is responsible for calling `requestExternalDiff`.
      outcome = { kind: "stale", error };
    } finally {
      this.saving = false;
    }

    if (this.saveAgainRequested) {
      this.saveAgainRequested = false;
      if (this.pending.length > 0) {
        // Fire-and-continue: the coalesced trigger's caller already got its
        // `{ kind: "coalesced" }` answer; this run's result is what the
        // *original* caller of `save` observes.
        void this.runSave(ipc);
      }
    }
    return outcome;
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
  }
}
