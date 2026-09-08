// Session state for the P0 shell: which vault is open, what the index is doing, and
// the write-back state for the one buffer P0 can hold. Kept in Pinia rather than
// component state because startup.ts (which runs before any component template
// renders) and App.vue both need to read and write it.
//
// P0 has one buffer and no navigation (contracts/ipc.rs: `list_notes` is P1), so this
// store deliberately does not yet model a document collection — that shape is a P1
// decision, not one to guess at here.

import { defineStore } from "pinia";

import type { IndexState } from "@/ipc/generated/IndexState";
import type { RebuildReason } from "@/ipc/generated/RebuildReason";

/** §3.4: a rebuild is a status line, never a modal, never a blocking spinner. */
export function describeIndexState(state: IndexState): string {
  switch (state.kind) {
    case "opening":
      return "index: opening";
    case "ready":
      return `index: ready · ${state.note_count} notes`;
    case "rebuilding":
      return `${describeRebuildReason(state.reason)} — search and queries are incomplete until this finishes · ${
        state.walk_complete ? `${state.done}/${state.total}` : `${state.done}`
      }`;
    case "unavailable":
      return `index: unavailable — ${state.message}`;
    /* c8 ignore next 2 -- IndexState is exhaustive; this is a compile-time backstop */
    default:
      return `index: ${(state as { kind: string }).kind}`;
  }
}

function describeRebuildReason(reason: RebuildReason): string {
  return `Rebuilding index (${reason})`;
}

export const useVaultStore = defineStore("vault", {
  state: () => ({
    /** Canonical vault root, for the title bar. `null` before `open_vault` returns. */
    vaultRoot: null as string | null,
    /** Rendered by `describeIndexState`, updated from the poll and the event both. */
    indexStatusText: "index: opening",
    /** The note the open buffer holds, vault-relative. `null` when nothing is open. */
    openNote: null as string | null,
    /** The hash the buffer was read at — the optimistic lock of architecture §6.1. */
    baseHash: null as string | null,
    /** Diagnostic status-bar text: paint/usable/save timings. Not a product feature. */
    marksText: "",
  }),
  actions: {
    setVaultRoot(root: string | null) {
      this.vaultRoot = root;
    },
    setIndexState(state: IndexState) {
      this.indexStatusText = describeIndexState(state);
    },
    setOpenBuffer(note: string | null, baseHash: string | null) {
      this.openNote = note;
      this.baseHash = baseHash;
    },
    setBaseHash(hash: string) {
      this.baseHash = hash;
    },
    appendMark(text: string) {
      this.marksText = this.marksText ? `${this.marksText} · ${text}` : text;
    },
    setMark(text: string) {
      this.marksText = text;
    },
  },
});
