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
import type { IndexStatus } from "@/ipc/generated/IndexStatus";
import type { VaultOpened } from "@/ipc/generated/VaultOpened";
import type { RebuildReason } from "@/ipc/generated/RebuildReason";

import type { OpeningReason } from "@/ipc/generated/OpeningReason";

/** Copy brief index-state-copy.md: each known `reason` has its own text and tooltip. An absent, null or
 * unknown (future) reason is NOT an error and renders as the plain opening text (goal 5); the field is the
 * only source, never elapsed time. Neither text carries a count or percentage ("No fake progress"). */
const OPENING_COPY: Record<OpeningReason, { text: string; detail: string }> = {
  previous_index_still_closing: {
    text: "Finishing the previous session's index — search will start when it's done",
    detail:
      "This vault was closed while its index was still being built. Serat finishes that first so two builds never write the same index. You can keep editing.",
  },
  index_held_by_another_process: {
    text: "Another Serat session is using this vault's index — search will start when it's free",
    detail:
      "Only one Serat session can update a vault's index at a time. Serat keeps trying for a while and tells you if it cannot get it. You can keep editing.",
  },
};

function openingCopy(state: Extract<IndexState, { kind: "opening" }>): { text: string; detail: string } | null {
  const reason = state.reason;
  return typeof reason === "string" && Object.hasOwn(OPENING_COPY, reason) ? OPENING_COPY[reason] : null;
}

/** Tooltip / aria-description for a waiting state; `null` for every other state. */
export function describeIndexDetail(state: IndexState): string | null {
  return state.kind === "opening" ? (openingCopy(state)?.detail ?? null) : null;
}

/** Copy brief index-state-copy.md, "Index check found problems". No number in the visible text. */
export const INDEX_CHECK_TEXT = "Index check found problems — rebuilding";

/** Tooltip / accessible description of that fact: the count lives only here. */
export function describeIndexCheckDetail(count: number): string {
  return `Serat's check of the index after an unclean shutdown found ${count} ${
    count === 1 ? "problem" : "problems"
  }, so it is rebuilding the index from your notes. Your notes are not affected. You can keep editing.`;
}

/** §3.4: a rebuild is a status line, never a modal, never a blocking spinner. */
export function describeIndexState(state: IndexState): string {
  switch (state.kind) {
    case "opening":
      return openingCopy(state)?.text ?? "Opening index…";
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
    indexStatusText: "Opening index…",
    /** Tooltip / aria-description of the waiting Opening state; `null` otherwise. */
    indexStatusDetail: null as string | null,
    /** Live-region text: a snapshot, never bound to `indexStatusText` (Rebuilding's done/total would be
     * re-announced on every event). Set once on entering the waiting state and once on leaving it;
     * cleared at the next transition. */
    indexAnnouncement: "",
    /** `kind` of the state the snapshot was taken in; the snapshot is kept while the kind holds. */
    indexAnnouncementKind: "" as string,
    /** A cleared reason just left the strip at a plain Opening: that state is not announced (it lasts until
     * the open check's first state and would be noise); the state after it is. */
    indexAfterWaiting: false,
    /** `IndexStatus.index_check_problems` (#69, #157): the post-unclean-open check found this many problems and
     * a rebuild is running. `null` = absent (never zero). The visible fact carries no number; the count is
     * only in the tooltip. */
    indexCheckProblems: null as number | null,
    /** The note the open buffer holds, vault-relative. `null` when nothing is open. */
    openNote: null as string | null,
    /** The hash the buffer was read at — the optimistic lock of architecture §6.1. */
    baseHash: null as string | null,
    /** The open vault's opaque `vault_id`; compared for equality only, never parsed. Set ONLY by
     * `adoptBoot`, `vaultOpened` and (restricted) `adoptFromIndexStatus`; `null` = no vault known. */
    vaultId: null as string | null,
    /** False only between a failed open (`openFailed`) and the next successful `vault://opened`: the
     * window then has NO vault open, so every event is dropped, including those carrying `vaultId`, which
     * is kept only to recognise them (contracts/ipc.rs `VAULT_OPENED_EVENT`, rule 2, task #103). Distinct
     * from `vaultId === null`, which means "not known yet, do not filter". */
    vaultOpen: true,
    /** `BootInfo.vault_id` was present at launch: the poll fallback is closed for good. */
    vaultIdFromBoot: false,
    /** `vault://opened` has been seen: the id is event-sourced and the poll can never change it. */
    announced: false,
    /** Distinct names the vault could not turn into a vault path (FR-VLT-01), as one status fact; `null` = absent (never zero). */
    refusedNames: null as number | null,
    /** An `index://status` event has been applied, so a late poll answer must not overwrite it. */
    statusEventSeen: false,
    /** Diagnostic status-bar text: paint/usable/save timings. Not a product feature. */
    marksText: "",
  }),
  actions: {
    /** Launch vault: `window.__SERAT_BOOT__.vault_id` (architecture 3.4). Absent/empty adopts nothing. */
    adoptBoot(id: string | undefined | null) {
      if (!id) return;
      this.vaultId = id;
      this.vaultOpen = true;
      this.vaultIdFromBoot = true;
    },
    /** `vault://opened`: the authoritative id of every later open. Replaces whatever was there. */
    vaultOpened(opened: VaultOpened) {
      this.announced = true; // from now on, no poll may set the id, even after a failed open
      if (!opened.vault_id) return; // never adopt an empty id
      if (opened.vault_id !== this.vaultId) this.refusedNames = null; // another vault: the old count is not its count
      this.vaultId = opened.vault_id;
      this.vaultOpen = true;
      this.vaultRoot = opened.root;
      // Seed the strip from the open itself: on the deferred re-open path the event stream never emits
      // Opening{reason}; its first index://status is the clear (#117). The reason arrives only here and
      // in the index_status poll. Only a reasoned Opening is seeded: every other state keeps arriving by
      // event/poll exactly as before (the #91/#103 drop rules are probed through the status text).
      if (opened.index?.kind === "opening" && opened.index.reason) this.setIndexState(opened.index);
    },
    /** `choose_vault` answered `Err` after `vault://opened`: the window has no vault open and no later
     * event carries the announced id. Task #103's rule (VAULT_OPENED_EVENT, rules 1-3): an `index://status`
     * for the announced id may be emitted before the `Err` and delivered after it, so after the `Err` EVERY
     * event carrying that id is dropped. `vaultId` is KEPT at the announced id only to recognise them
     * (nulling it would switch every consumer's filter off), `vaultOpen` goes false so consumers drop all,
     * and the previous vault's id is never re-adopted (it was closed before the emit). "No vault open" is
     * also `vaultRoot === null`. `announced` stays set so a stale poll cannot change the id either.
     *
     * This resets only the store. It cannot reset the file tree (stopped state, rows, no `open_tree`) or
     * the editor buffer: #80, the only caller, must do that on `choose_vault` Err BEFORE calling this. */
    openFailed() {
      this.vaultOpen = false;
      this.refusedNames = null; // no vault, nothing to count
      this.indexCheckProblems = null;
      this.vaultRoot = null;
      this.openNote = null;
      this.baseHash = null;
    },
    /** True when an `index_status` answer/event belongs to the current vault: never while no vault is open
     * (after a failed open, even for the announced id), never an empty id (a poll computed between a close
     * and the next install), never one that differs from a known current id. */
    statusBelongsHere(id: string): boolean {
      if (!this.vaultOpen) return false;
      return id !== "" && (this.vaultId === null || id === this.vaultId);
    },
    /** The `index_status` fallback (poll and event, one guarded entry): only at launch, only while
     * `BootInfo.vault_id` is absent, never once `vault://opened` has been seen, never an empty id. The
     * poll is not ordered against events: it can carry the OLD id or `""` after a switch began. */
    adoptFromIndexStatus(id: string) {
      if (!id || this.vaultIdFromBoot || this.announced || this.vaultId !== null) return;
      this.vaultId = id;
      this.vaultOpen = true;
    },
    setVaultRoot(root: string | null) {
      this.vaultRoot = root;
    },
    setIndexState(state: IndexState) {
      const wasWaiting = this.indexStatusDetail !== null;
      const text = describeIndexState(state);
      const detail = describeIndexDetail(state);
      const plainOpening = state.kind === "opening" && detail === null;
      if (detail !== null && (!wasWaiting || text !== this.indexStatusText)) {
        this.indexAnnouncement = text; // entered (or the reason changed): one snapshot
        this.indexAnnouncementKind = state.kind;
        this.indexAfterWaiting = false;
      } else if (detail === null && wasWaiting) {
        if (plainOpening) {
          // Opening{None} after a cleared reason: say nothing, announce the state after it instead.
          this.indexAnnouncement = "";
          this.indexAnnouncementKind = state.kind;
          this.indexAfterWaiting = true;
        } else {
          this.indexAnnouncement = text; // left: one snapshot
          this.indexAnnouncementKind = state.kind;
        }
      } else if (detail === null && this.indexAfterWaiting) {
        if (!plainOpening) {
          this.indexAnnouncement = text;
          this.indexAnnouncementKind = state.kind;
          this.indexAfterWaiting = false;
        }
      } else if (detail === null && state.kind !== this.indexAnnouncementKind) {
        // Cleared only on a change of kind, not on each progress event: a polite region can drop a
        // snapshot that is wiped milliseconds after it is written (worst on WebKit).
        this.indexAnnouncement = "";
        this.indexAnnouncementKind = "";
      }
      this.indexStatusText = text;
      this.indexStatusDetail = detail;
    },
    setRefusedNames(count: number | null) {
      this.refusedNames = count;
    },
    /** One entry for an `index://status` event and the late-subscriber poll alike (architecture 3.4): the state
     * text and the refused-names count, which is `null` when the status omits it (`None`: not counting yet, or
     * no vault open; never zero, FR-VLT-01). */
    applyIndexStatus(status: Pick<IndexStatus, "state" | "refused_names" | "index_check_problems">, source: "event" | "poll") {
      // The poll is not ordered against events: once an event has spoken, its (newer) facts win.
      if (source === "poll" && this.statusEventSeen) return;
      if (source === "event") this.statusEventSeen = true;
      this.setIndexState(status.state);
      this.setRefusedNames(status.refused_names ?? null);
      this.setIndexCheckProblems(status.index_check_problems ?? null);
    },
    /** The quiet "index check found problems" fact. Announced once when it appears (it replaces whatever the
     * state change just queued: it is the more specific news), never again on progress events; cleared when
     * the field goes absent. A count of 0 is treated as absent (the contract never sends it). */
    setIndexCheckProblems(count: number | null) {
      const next = count !== null && count > 0 ? count : null;
      if (next !== null && this.indexCheckProblems === null) {
        this.indexAnnouncement = INDEX_CHECK_TEXT;
        this.indexAnnouncementKind = this.indexAnnouncementKind || "rebuilding";
        this.indexAfterWaiting = false;
      }
      this.indexCheckProblems = next;
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
