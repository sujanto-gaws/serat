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
import type { VaultOpened } from "@/ipc/generated/VaultOpened";
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
    /** The open vault's opaque `vault_id`; compared for equality only, never parsed. Set ONLY by
     * `adoptBoot`, `vaultOpened` and (restricted) `adoptFromIndexStatus`; `null` = no vault known. */
    vaultId: null as string | null,
    /** `BootInfo.vault_id` was present at launch: the poll fallback is closed for good. */
    vaultIdFromBoot: false,
    /** `vault://opened` has been seen: the id is event-sourced and the poll can never change it. */
    announced: false,
    /** Names the vault refused to list (invalid on this platform), as one status fact; `null` = unknown. */
    refusedNames: null as number | null,
    /** Diagnostic status-bar text: paint/usable/save timings. Not a product feature. */
    marksText: "",
  }),
  actions: {
    /** Launch vault: `window.__SERAT_BOOT__.vault_id` (architecture 3.4). Absent/empty adopts nothing. */
    adoptBoot(id: string | undefined | null) {
      if (!id) return;
      this.vaultId = id;
      this.vaultIdFromBoot = true;
    },
    /** `vault://opened`: the authoritative id of every later open. Replaces whatever was there. */
    vaultOpened(opened: VaultOpened) {
      this.announced = true; // from now on, no poll may set the id, even after a failed open
      if (!opened.vault_id) return; // never adopt an empty id
      this.vaultId = opened.vault_id;
      this.vaultRoot = opened.root;
    },
    /** `choose_vault` answered `Err` after `vault://opened`: the window has no vault open and no later
     * event carries the announced id. `announced` stays set so a stale poll cannot bring it back. */
    openFailed() {
      this.vaultId = null;
      this.vaultRoot = null;
      this.openNote = null;
      this.baseHash = null;
    },
    /** The `index_status` fallback (poll and event, one guarded entry): only at launch, only while
     * `BootInfo.vault_id` is absent, never once `vault://opened` has been seen, never an empty id. The
     * poll is not ordered against events: it can carry the OLD id or `""` after a switch began. */
    adoptFromIndexStatus(id: string) {
      if (!id || this.vaultIdFromBoot || this.announced || this.vaultId !== null) return;
      this.vaultId = id;
    },
    setVaultRoot(root: string | null) {
      this.vaultRoot = root;
    },
    setIndexState(state: IndexState) {
      this.indexStatusText = describeIndexState(state);
    },
    setRefusedNames(count: number | null) {
      this.refusedNames = count;
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
