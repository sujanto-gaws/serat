<!--
  Task #21 (FR-EDT-01/02/03/10, NFR-PRF-02, NFR-PRF-01; ADR-0006, ADR-0016):
  the hybrid live-preview editor. Builds on task #98's P0 shell (see the
  history in this file's earlier revisions for the shadow-root/CSP
  reasoning, which is unchanged and restated briefly below) by adding:

    - GFM + custom inline Markdown (`markdownLanguage.ts`, `mdExtensions.ts`);
    - the reveal rule (`reveal.ts`, FR-EDT-02) — hides syntax markers except
      on lines the selection intersects, recomputed synchronously on every
      doc/selection/viewport change so it lands within one frame;
    - the single command registry and its one `keymap.of` call
      (`codemirrorSetup.ts`);
    - a per-note `EditorBuffer` (ADR-0016 §2) that turns local edits into
      patch-set saves through a mockable `EditorIpcAdapter`, debounced 1s
      idle / on blur / on unmount, with at most one save in flight;
    - split-preview mode (FR-EDT-10): `livePreview: false` turns off the
      reveal rule (source stays fully raw, `mono-source` throughout — no
      hide/reveal in this mode, which is the point of the fallback) and
      mounts a read-only `Preview.vue` beside the source.

  Still mounted synchronously in `onMounted`, with no `await` before the
  `EditorView` is constructed, and still into a shadow root — CodeMirror's
  runtime CSS (`style-mod`) needs `adoptedStyleSheets`, which only a shadow
  root gets, to satisfy `tauri.conf.json`'s CSP with no `'unsafe-inline'`.
  See `src/startup.ts` and `app/README.md` for why synchronous-mount is
  load-bearing, and task #98/#102's history for the CSP finding.
-->
<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { isolateHistory, redo, undo } from "@codemirror/commands";
import { ChangeSet, Compartment, EditorState, type Extension, Transaction } from "@codemirror/state";
import { EditorView, lineNumbers, placeholder } from "@codemirror/view";

import type { CommandRegistry } from "@/editor/commands";
import { buildEditorExtensions, createEditorCommandRegistry, editorScrollExtensions } from "@/editor/codemirrorSetup";
import { EditorBuffer, type SaveOutcome } from "@/editor/buffer";
import { seratMarkdown } from "@/editor/markdownLanguage";
import Preview from "@/editor/Preview.vue";
import ExternalCompare from "@/editor/ExternalCompare.vue";
import { blockWidgetsExtension } from "@/editor/blockWidgets";
import { minimalReplacement, patchesToChangeSet, planExternal, externalChangeSet } from "@/editor/reconcile";
import { textFromString } from "@/editor/patches";
import type { AppClosing } from "@/ipc/generated/AppClosing";
import type { ExternalChange } from "@/ipc/generated/ExternalChange";
import { LinkResolver } from "@/editor/linkResolver";
import { linkExtensions, linkNodeAt } from "@/editor/links";
import { refreshLinks } from "@/editor/linkState";
import { revealMarkersPlugin } from "@/editor/reveal";
import { syntaxTree } from "@codemirror/language";
import { defaultLinkIpc, type LinkIpcAdapter } from "@/ipc/linkAdapter";
import { defaultEditorIpc, type EditorIpcAdapter } from "@/ipc/editorAdapter";

/** How long after the last keystroke an idle auto-save fires — ADR-0016 §2:
 * "Saves are triggered by 1 s of idle, by blur, by closing the note and by
 * the save command." */
const AUTOSAVE_IDLE_MS = 1000;

/** How long a transient notice (confirmation, "disk matches again") stays up. */
const NOTICE_MS = 3000;

const props = withDefaults(
  defineProps<{
    placeholder?: string;
    /** `editor.live_preview` (`UiSettings`) — `false` selects split preview
     * (FR-EDT-10). Reactive: toggling it at runtime reconfigures the live
     * editor without losing the document, selection or undo history. */
    livePreview?: boolean;
    ipc?: EditorIpcAdapter;
    /** `resolve_links` / `note_anchors` / `switcher_query` / `index://changed`. */
    linkIpc?: LinkIpcAdapter;
    /** The open vault's `vault_id` (`BootInfo` / `vault://opened`; opaque, compared for equality only).
     * `null`/absent until known: events are then not filtered, since nothing can be
     * compared. Captured per buffer at `loadNote`, so an `app://closing` for the OLD
     * vault of a switch still reaches the buffer that belongs to it (architecture 3.1.1 item 6.7). */
    vaultId?: string | null;
    /** `false` = no vault open (after a failed open): every event is dropped, whatever its id. */
    vaultOpen?: boolean;
    /** The app-wide command registry (architecture.md §9.3). The editor adds its commands to
     * it for as long as it is mounted, so the palette lists them; a private one when absent. */
    commands?: CommandRegistry;
  }>(),
  { livePreview: true, vaultId: null, vaultOpen: true, ipc: () => defaultEditorIpc, linkIpc: () => defaultLinkIpc },
);

const emit = defineEmits<{
  "save-outcome": [outcome: SaveOutcome];
  "dirty-changed": [dirty: boolean];
  "toggle-live-preview": [];
  "wikilink-click": [target: string];
  "external-link-click": [url: string];
  /** Follow-link command (FR-EDT-03): the host navigates. `path` is null when unresolved (offer to create). */
  "follow-link": [target: FollowLinkTarget];
  /** *Compare* on an external-change conflict: the host opens a diff of the two texts. */
  "compare-external": [payload: { path: string; mine: string; disk: string }];
  /** A disjoint external edit was merged into unsaved work: the host may show "Updated externally" for 3 s
   * (external-change-notice.md). A clean-buffer reload is silent by spec and emits nothing. */
  "external-merged": [payload: { path: string }];
  /** True while a save cannot proceed and nothing explains it (request_external_diff failed outright):
   * the host shows the persistent "Not saved - file changed on disk" line with Compare (editor.md). */
  "save-blocked": [blocked: boolean];
}>();

const host = ref<HTMLDivElement | null>(null);
let view: EditorView | null = null;
let buffer: EditorBuffer | null = null;
let autosaveTimer: ReturnType<typeof setTimeout> | null = null;

const revealCompartment = new Compartment();
/** Live-preview-only extensions: the inline reveal rule and block widgets. Split
 * mode (FR-EDT-10) drops both together. */
const liveExtensions: Extension[] = [revealMarkersPlugin, blockWidgetsExtension()];
const dirty = ref(false);
const previewSource = ref("");
/** True only while `loadNote`/`setText` replace the whole state — that must
 * not be recorded as a pending edit against the fresh buffer it accompanies,
 * or the note would be "dirty" the instant it opens. */
let suppressChangeTracking = false;

function markDirty(): void {
  const next = buffer?.isDirty ?? false;
  if (next !== dirty.value) {
    dirty.value = next;
    emit("dirty-changed", next);
  }
}

async function performSave(explicit = false): Promise<SaveOutcome | null> {
  if (!buffer) return null;
  if (deleted.value) {
    // The note was deleted on disk (ADR-0016 §4): only the save command
    // recreates it, never an idle or blur trigger.
    return explicit ? recreateDeletedNote() : null;
  }
  const started = buffer;
  const outcome = await started.save(props.ipc, { explicit });
  markDirty();
  if (outcome.kind === "saved") blocked.value = false;
  emit("save-outcome", outcome);
  // ADR-0016 §2: a StaleHash always asks for the diff right away; it does not
  // wait for an event that may already have been dropped.
  if (outcome.kind === "stale" && started === buffer && errorKind(outcome.error) === "stale_hash") {
    void requestFreshDiff(started);
  }
  return outcome;
}

function errorKind(error: unknown): string | null {
  return typeof error === "object" && error !== null ? ((error as { kind?: string }).kind ?? null) : null;
}

// ---------------------------------------------------------------------------
// External-change reconciliation (ADR-0016 §2-§4, FR-VLT-05, NFR-REL-01, C-04)
// ---------------------------------------------------------------------------

interface ExternalConflict {
  /** The edit script, when one exists (null after a StaleHash with no diff). */
  change: Extract<ExternalChange, { kind: "changed" }> | null;
  /** The disk hash the conflict was found at. */
  newHash: string;
}

const conflict = ref<ExternalConflict | null>(null);
const deleted = ref(false);
const notice = ref<string | null>(null);
/** The conflict notice folded into a brass dot (`Esc` on the focused card); the conflict stays pending. */
const minimised = ref(false);
/** The in-place diff (the "Comparing" state of external-change-notice.md). */
const comparing = ref<{ path: string; mine: string; disk: string; resolvable: boolean } | null>(null);
/** A refused save that nothing explains: `request_external_diff` itself failed. */
const blocked = ref(false);
let noticeTimer: ReturnType<typeof setTimeout> | null = null;

watch(blocked, (v) => emit("save-blocked", v));
watch(conflict, (c, was) => {
  if (c && !was) minimised.value = false; // a fresh conflict shows the full notice
  if (!c) {
    minimised.value = false;
    if (comparing.value?.resolvable) comparing.value = null;
  }
});

/** A short status line that clears itself (confirmations, "disk matches again"). */
function flashNotice(text: string): void {
  if (noticeTimer) clearTimeout(noticeTimer);
  notice.value = text;
  noticeTimer = setTimeout(() => {
    noticeTimer = null;
    if (notice.value === text) notice.value = null;
  }, NOTICE_MS);
}

const DISK_MATCHES = "The file on disk matches your last save again; nothing to compare.";

/** The disk equals the base again: the stale block is lifted, and a dirty buffer must resume saving
 * (otherwise it sits unsaved until the next keystroke). */
function resumeAfterDiskMatches(b: EditorBuffer): void {
  b.clearStale();
  blocked.value = false;
  markDirty();
  if (b.isDirty) scheduleAutosave();
  flashNotice(DISK_MATCHES);
}
/** `ExternalChange`s that arrived while a save was in flight (§2: held, not applied). */
let held: Array<Extract<ExternalChange, { kind: "changed" }>> = [];
let draining = false;

/** Dispatches reconciled changes into the live document: marked
 * `addToHistory(false)` so the cursor and undo history map through them and
 * undo never reverts someone else's edit, and never recorded as a local edit. */
function dispatchExternal(changes: ChangeSet): void {
  if (!view || changes.empty) return;
  suppressChangeTracking = true;
  try {
    view.dispatch({ changes, annotations: Transaction.addToHistory.of(false) });
  } finally {
    suppressChangeTracking = false;
  }
}

async function onExternalChange(change: ExternalChange): Promise<void> {
  const b = buffer;
  if (!b || change.path !== b.path) return;
  // "Session scope; discard if it is not the open vault" (contracts/ipc.rs).
  if (!isOurVault(change.vault_id)) return;
  if (change.kind === "deleted") {
    deleted.value = true;
    notice.value = "This note was deleted on disk. Your text is kept; save to recreate it.";
    return;
  }
  if (b.isSaving) {
    held.push(change);
    if (!draining) void drainHeld(b);
    return;
  }
  await handleChange(b, change);
}

async function drainHeld(b: EditorBuffer): Promise<void> {
  draining = true;
  try {
    await b.settled();
    const queue = held;
    held = [];
    if (b !== buffer) return;
    for (const change of queue) await handleChange(b, change);
  } finally {
    draining = false;
  }
}

async function handleChange(b: EditorBuffer, change: Extract<ExternalChange, { kind: "changed" }>): Promise<void> {
  if (b !== buffer) return;
  // Only a change expressed against the current base is applied; anything else
  // is dropped and a fresh diff is requested (§2).
  if (change.base_hash !== b.baseHash) {
    await requestFreshDiff(b);
    return;
  }
  applyChange(b, change);
}

async function requestFreshDiff(b: EditorBuffer): Promise<void> {
  let answer: ExternalChange | null;
  try {
    answer = await props.ipc.requestExternalDiff({ path: b.path, base_hash: b.baseHash });
  } catch (error) {
    if (b !== buffer) return;
    if (errorKind(error) === "stale_hash") {
      // serat-app no longer holds the base bytes: no guess, conflict path (§2).
      conflict.value = { change: null, newHash: (error as { current_hash: string }).current_hash };
    } else if (b.isStale) {
      // The request failed outright after a refused save: nothing explains the refusal, so say so
      // (persistently) rather than sit at "Unsaved" with no indication (editor.md, StaleHash).
      blocked.value = true;
    }
    return;
  }
  if (b !== buffer) return;
  if (answer === null) {
    resumeAfterDiskMatches(b); // disk equals the base: nothing diverged after all
    return;
  }
  if (answer.kind === "deleted") {
    deleted.value = true;
    notice.value = "This note was deleted on disk. Your text is kept; save to recreate it.";
    return;
  }
  // Computed against the base we just sent, so it must match; if it somehow
  // does not, do not loop: treat as an unmergeable conflict.
  if (answer.base_hash !== b.baseHash) {
    conflict.value = { change: null, newHash: answer.new_hash };
    return;
  }
  applyChange(b, answer);
}

function applyChange(b: EditorBuffer, change: Extract<ExternalChange, { kind: "changed" }>): void {
  let external;
  try {
    external = externalChangeSet(b.baseText, change.edits);
  } catch {
    conflict.value = { change: null, newHash: change.new_hash }; // unusable positions: no merge on guessed coordinates
    return;
  }
  const local = b.isDirty ? b.composedPending() : null;
  const plan = planExternal(external, local);
  switch (plan.kind) {
    case "hash-only":
      // edits = [] with a new hash (e.g. CRLF<->LF flip): the base Text already
      // equals disk in the line model; only the hash moves, silently.
      b.adoptExternal(external, change.new_hash, local);
      break;
    case "apply-clean":
      dispatchExternal(plan.external);
      b.adoptExternal(plan.external, change.new_hash, null);
      break;
    case "apply-rebased":
      dispatchExternal(plan.externalOnDoc);
      b.adoptExternal(plan.external, change.new_hash, plan.localRebased);
      emit("external-merged", { path: b.path });
      break;
    case "conflict":
      conflict.value = { change, newHash: change.new_hash };
      notice.value = null;
      return;
  }
  conflict.value = null;
  markDirty();
  if (b.isDirty) scheduleAutosave();
}

/** The disk text behind a conflict: derived from the edit script when there is
 * one, else read (and verified) from disk. */
async function diskTextFor(b: EditorBuffer, c: ExternalConflict): Promise<{ text: string; hash: string } | null> {
  if (c.change) {
    try {
      return { text: externalChangeSet(b.baseText, c.change.edits).apply(b.baseText).toString(), hash: c.newHash };
    } catch {
      /* fall through to a read */
    }
  }
  const file = await props.ipc.readFile({ path: b.path });
  if (file.hash !== c.newHash) {
    // The disk moved again while the notice was up: ask for a fresh diff.
    conflict.value = null;
    await requestFreshDiff(b);
    return null;
  }
  return { text: file.text, hash: file.hash };
}

/** *Keep mine* (§4): re-base the buffer onto the disk version. The edit script
 * from disk to the buffer is computed in Rust by `rebase_buffer`; adopting the
 * disk hash over the old-base ChangeSet would corrupt the file. */
async function keepMine(): Promise<void> {
  const b = buffer;
  const c = conflict.value;
  if (!b || !c || !view) return;
  const disk = await diskTextFor(b, c);
  if (!disk || b !== buffer) return;
  let patches;
  try {
    patches = await props.ipc.rebaseBuffer({ path: b.path, disk_hash: disk.hash, buffer: view.state.doc.toString() });
  } catch (error) {
    if (errorKind(error) === "stale_hash") {
      conflict.value = null;
      await requestFreshDiff(b);
    }
    return;
  }
  b.rebaseOnto(disk.text, disk.hash, patchesToChangeSet(textFromString(disk.text), patches));
  conflict.value = null;
  blocked.value = false;
  markDirty();
  if (b.isDirty) scheduleAutosave();
  flashNotice("Keeping your version \u2014 the external change will be replaced when you save.");
}

/** *Take theirs* (§4): replace the buffer in one history-recorded transaction,
 * so undo brings back what was typed. Trimmed to the changed middle so a cursor
 * outside it stays put. */
async function takeTheirs(): Promise<void> {
  const b = buffer;
  const c = conflict.value;
  if (!b || !c || !view) return;
  const disk = await diskTextFor(b, c);
  if (!disk || b !== buffer || !view) return;
  const replacement = minimalReplacement(view.state.doc.toString(), textFromString(disk.text).toString());
  suppressChangeTracking = true;
  try {
    // History-recorded on purpose, and isolated so one undo reverts exactly this step.
    view.dispatch({ changes: replacement, annotations: isolateHistory.of("full") });
  } finally {
    suppressChangeTracking = false;
  }
  b.rebaseOnto(disk.text, disk.hash, null);
  conflict.value = null;
  blocked.value = false;
  markDirty();
  flashNotice("Took the disk version \u2014 undo brings back what you typed.");
}

/** Asks serat-app for the diff against the current base right now (ADR-0016 section 2, rule 3):
 * the disk may have moved since the notice went up. Returns the conflict to compare against,
 * or null when there is nothing to compare any more (the disk equals the base again, the note
 * was deleted, or the buffer changed). A failed request keeps the conflict we already have. */
async function refreshConflict(b: EditorBuffer, c: ExternalConflict): Promise<ExternalConflict | null> {
  let answer: ExternalChange | null;
  try {
    answer = await props.ipc.requestExternalDiff({ path: b.path, base_hash: b.baseHash });
  } catch (error) {
    if (errorKind(error) === "stale_hash") {
      return { change: null, newHash: (error as { current_hash: string }).current_hash };
    }
    return c;
  }
  if (b !== buffer) return null;
  if (answer === null) {
    conflict.value = null; // disk equals the base: nothing diverged after all
    resumeAfterDiskMatches(b);
    return null;
  }
  if (answer.kind === "deleted") {
    conflict.value = null;
    deleted.value = true;
    notice.value = "This note was deleted on disk. Your text is kept; save to recreate it.";
    return null;
  }
  if (answer.base_hash !== b.baseHash) return { change: null, newHash: answer.new_hash };
  return { change: answer, newHash: answer.new_hash };
}

/** *Compare*: hands both texts to the host; nothing is changed or discarded. */
async function compareExternal(): Promise<void> {
  const b = buffer;
  const c0 = conflict.value;
  if (!b || !c0 || !view) return;
  const c = await refreshConflict(b, c0);
  if (!c || b !== buffer || !view) return;
  conflict.value = c;
  const disk = await diskTextFor(b, c);
  if (!disk) return;
  const mine = view.state.doc.toString();
  emit("compare-external", { path: b.path, mine, disk: disk.text });
  comparing.value = { path: b.path, mine, disk: disk.text, resolvable: true };
}

/** *Compare* from the persistent "Not saved - file changed on disk" line: there is no conflict to
 * resolve, only a refused save, so read the disk and show both (inspection only). */
async function compareWithDisk(): Promise<void> {
  const b = buffer;
  if (!b || !view) return;
  try {
    const file = await props.ipc.readFile({ path: b.path });
    if (b !== buffer || !view) return;
    const mine = view.state.doc.toString();
    emit("compare-external", { path: b.path, mine, disk: file.text });
    comparing.value = { path: b.path, mine, disk: file.text, resolvable: false };
  } catch {
    flashNotice("Could not read the file on disk to compare.");
  }
}

async function resolveFromCompare(action: () => Promise<void>): Promise<void> {
  await action();
  comparing.value = null;
}

/** ADR-0016 §4: a note deleted on disk keeps its buffer; the save command then
 * recreates it through `write_file` with `base_hash: null`. */
async function recreateDeletedNote(): Promise<SaveOutcome | null> {
  const b = buffer;
  if (!b || !view) return null;
  const text = view.state.doc.toString();
  try {
    const written = await props.ipc.writeFile({ path: b.path, text, base_hash: null });
    b.rebaseOnto(text, written.hash, null);
    deleted.value = false;
    notice.value = null;
    markDirty();
    const outcome: SaveOutcome = { kind: "saved", written };
    emit("save-outcome", outcome);
    return outcome;
  } catch (error) {
    const outcome: SaveOutcome = { kind: "rejected", error };
    emit("save-outcome", outcome);
    return outcome;
  }
}

// ---------------------------------------------------------------------------
// Clean close (NFR-REL-01, FR-EDT-01; architecture.md 3.1.1 item 6)
// ---------------------------------------------------------------------------

/** The vault the current buffer was loaded from; `null` when not known then. */
let bufferVaultId: string | null = null;

/** True unless both sides know their vault id and they differ. */
function isOurVault(id: string): boolean {
  if (!props.vaultOpen) return false; // no vault open: nothing is current (task #103)
  const mine = bufferVaultId ?? props.vaultId;
  return mine === null || mine === undefined || mine === id;
}

watch(
  () => props.vaultId,
  (id) => {
    // A buffer opened before the id was known belongs to the vault the store then announces. The
    // store's id is authoritative (BootInfo at launch, `vault://opened` later; the index_status
    // fallback can only fill an empty id at launch and never after an announcement), so this can no
    // longer tag a B buffer with A's id (#91; #80's switch UI builds on it).
    if (id && buffer && bufferVaultId === null) bufferVaultId = id;
  },
);

/** `vault_id`s whose close flush has started: `app://closing` is emitted once per close, but
 * the ack must be sent exactly once whatever the transport does. */
const closingSeen = new Set<string>();

async function onClosing(closing: AppClosing): Promise<void> {
  if (!isOurVault(closing.vault_id)) return; // another vault's close: not ours to ack
  // Never removed: one close, one flush, one ack per vault id, literally (ids are unique per open).
  if (closingSeen.has(closing.vault_id)) return;
  closingSeen.add(closing.vault_id);

  try {
    if (autosaveTimer) {
      clearTimeout(autosaveTimer);
      autosaveTimer = null;
    }
    const b = buffer;
    if (b && !deleted.value) {
      // Awaits an in-flight save, then issues the coalesced remainder. A StaleHash, a refusal or an
      // error leaves the buffer dirty and untouched (C-04: never re-serialised); we ack anyway,
      // because the conflict path needs a user and the close does not wait for one. That buffer's
      // edits are lost at exit: the same residual as a crash during a conflict (architecture 3.1.1
      // item 6), recorded rather than hidden.
      //
      // No deadline race: we never ack while a save is still in flight. An early ack would end the
      // core's wait and refuse an in-transit save_patch; the core's own 2 s bound is the backstop
      // for a save that hangs, and the late ack after it is ignored.
      await b.flush(props.ipc).catch(() => false);
      markDirty();
    }
    // A note deleted on disk is NOT recreated by a close (only the explicit save command does,
    // ADR-0016 section 4): its unsaved text is lost at exit, the same recorded residual.
  } finally {
    try {
      await props.ipc.flushAck({ vault_id: closing.vault_id });
    } catch {
      /* never fails by contract; if the transport does, the core's bound is the backstop */
    }
  }
}

function scheduleAutosave(): void {
  if (autosaveTimer) clearTimeout(autosaveTimer);
  autosaveTimer = setTimeout(() => {
    void performSave();
  }, AUTOSAVE_IDLE_MS);
}

const resolver = new LinkResolver(props.linkIpc);
let unlistenIndexChanged: (() => void) | null = null;
let unlistenExternal: (() => void) | null = null;
let unlistenClosing: (() => void) | null = null;

export interface FollowLinkTarget {
  /** As written, minus brackets and `|display`. */
  target: string;
  /** The `#Heading` / `^id` part, if any. */
  subpath: string | null;
  /** Resolved vault path, or null if unresolved. */
  path: string | null;
  subpathFound: boolean | null;
  isEmbed: boolean;
}

/** Mod-Enter: a wikilink/embed under the cursor is resolved and handed to the
 * host to open; an external URL goes to the system-opener event. Never
 * navigates itself, never edits the document. */
function followLinkUnderCursor(v: EditorView): boolean {
  const pos = v.state.selection.main.head;
  const node = linkNodeAt(v.state, pos);
  if (node) {
    const source = buffer?.path ?? null;
    void (async () => {
      const [answer] = source ? await resolver.resolve(source, [node.link.target]) : [];
      emit("follow-link", {
        target: node.link.target,
        subpath: node.link.subpath,
        path: answer?.path ?? null,
        subpathFound: answer?.subpath_found ?? null,
        isEmbed: node.isEmbed,
      });
    })();
    return true;
  }
  for (let n: ReturnType<ReturnType<typeof syntaxTree>["resolveInner"]> | null = syntaxTree(v.state).resolveInner(pos, -1); n; n = n.parent) {
    if (n.name === "URL") {
      const url = v.state.sliceDoc(n.from, n.to);
      if (/^https?:\/\//i.test(url)) {
        emit("external-link-click", url);
        return true;
      }
    }
  }
  return false;
}

const idsBefore = new Set(props.commands?.list().map((c) => c.id) ?? []);
const registry = createEditorCommandRegistry(
  {
  save: () => {
    void performSave(true); // an explicit command may retry a refused save
    return true;
  },
  toggleLivePreview: () => {
    emit("toggle-live-preview");
    return true;
  },
  followLink: (v) => followLinkUnderCursor(v),
  },
  undefined,
  props.commands,
);
const ownCommandIds = registry.list().map((c) => c.id).filter((id) => !idsBefore.has(id));
const keymapCompartment = new Compartment();
/** Rebindings arrive after mount (settings are read asynchronously): rebuild the one keymap. */
const unsubscribeRegistry = registry.subscribe(() => {
  try {
    view?.dispatch({ effects: keymapCompartment.reconfigure(buildEditorExtensions(registry)) });
  } catch {
    // An override that collides is refused by the registry; keep the keymap in force.
  }
});

function buildExtensions(livePreview: boolean): Extension[] {
  return [
    lineNumbers(),
    // Read-only until `loadNote` installs a buffer: text typed in any earlier
    // window would have no base to be a patch against (C-04, NFR-REL-01).
    EditorState.readOnly.of(!buffer),
    EditorView.editable.of(!!buffer),
    seratMarkdown(),
    revealCompartment.of(livePreview ? liveExtensions : []),
    ...linkExtensions({ ipc: props.linkIpc, resolver, getSourcePath: () => buffer?.path ?? null }),
    keymapCompartment.of(buildEditorExtensions(registry)),
    EditorView.lineWrapping,
    ...editorScrollExtensions(), // NFR-PRF-02: CM owns the scroll; see codemirrorSetup.ts
    EditorView.contentAttributes.of({ spellcheck: "false", "aria-label": "editor" }),
    EditorView.domEventHandlers({
      blur: () => {
        void performSave();
        return false;
      },
    }),
    EditorView.updateListener.of((update) => {
      if (update.docChanged) {
        if (!suppressChangeTracking) {
          const composed = update.transactions.map((tr) => tr.changes).reduce((acc, cs) => acc.compose(cs));
          buffer?.recordChange(composed);
          markDirty();
          scheduleAutosave();
        }
        previewSource.value = update.state.doc.toString();
      }
    }),
    ...(props.placeholder ? [placeholder(props.placeholder)] : []),
  ];
}

onMounted(() => {
  if (!host.value) {
    return;
  }
  const shadowRoot = host.value.attachShadow({ mode: "open" });
  // `index://changed` is invalidation only (§3.4): drop cached resolutions and
  // ask the links plugin to rescan the viewport.
  props.linkIpc
    .onIndexChanged(() => {
      resolver.invalidate();
      view?.dispatch({ effects: refreshLinks.of(null) });
    })
    .then((unlisten) => {
      if (view) unlistenIndexChanged = unlisten;
      else unlisten();
    })
    .catch(() => {
      /* no event channel (tests, early boot): links resolve on the next scan only */
    });
  props.ipc
    .onExternalChange((change) => void onExternalChange(change))
    .then((unlisten) => {
      if (view) unlistenExternal = unlisten;
      else unlisten();
    })
    .catch(() => {
      /* no event channel (tests, early boot): StaleHash still requests the diff */
    });
  props.ipc
    .onClosing((closing) => void onClosing(closing))
    .then((unlisten) => {
      if (view) unlistenClosing = unlisten;
      else unlisten();
    })
    .catch(() => {
      /* no event channel (tests, early boot) */
    });
  view = new EditorView({
    parent: shadowRoot,
    state: EditorState.create({
      doc: "",
      extensions: buildExtensions(props.livePreview),
    }),
  });
});

onBeforeUnmount(() => {
  unsubscribeRegistry();
  if (props.commands) for (const id of ownCommandIds) registry.unregister(id);
  unlistenIndexChanged?.();
  unlistenIndexChanged = null;
  unlistenExternal?.();
  unlistenExternal = null;
  unlistenClosing?.();
  unlistenClosing = null;
  if (autosaveTimer) clearTimeout(autosaveTimer);
  void performSave();
  view?.destroy();
  view = null;
});

watch(
  () => props.livePreview,
  (enabled) => {
    view?.dispatch({ effects: revealCompartment.reconfigure(enabled ? liveExtensions : []) });
  },
);

/** Latest `loadNote` call; a load superseded while it awaited the flush must
 * not install its (now stale) text. */
let loadToken = 0;

/**
 * Replaces the whole editor state: new document, no undo history, selection
 * at 0. `view.setState` (not a dispatch) is what guarantees nothing of the
 * old document can be undone into the new one — a dispatched replacement
 * lands in CodeMirror's history, and Mod-z would then walk the document back
 * to "" or to the previous note's text, which `recordChange` would record as
 * "delete everything" against the loaded base and autosave to disk (C-04,
 * NFR-REL-01). Never route a load through `view.dispatch`.
 */
function replaceState(text: string): void {
  if (!view) return;
  suppressChangeTracking = true;
  try {
    view.setState(EditorState.create({ doc: text, extensions: buildExtensions(props.livePreview) }));
  } finally {
    suppressChangeTracking = false;
  }
  previewSource.value = text;
}

/**
 * Starts editing `path` (content `text`, on-disk hash `baseHash`) — used once
 * per note, when it is opened. Any previous note's edits are flushed to disk
 * first (awaiting an in-flight save too) and the buffer is replaced only once
 * they are safe. Resolves `false`, leaving the previous note untouched, if
 * they could not be saved (stale or refused) or a newer `loadNote` superseded
 * this one: the caller must reconcile (ADR-0016 §4) rather than lose edits.
 */
async function loadNote(text: string, path: string, baseHash: string): Promise<boolean> {
  if (!view) return false;
  const token = ++loadToken;
  if (autosaveTimer) {
    clearTimeout(autosaveTimer);
    autosaveTimer = null;
  }
  if (buffer) {
    const old = buffer;
    const clean = await old.flush(props.ipc);
    markDirty();
    if (token !== loadToken || !view) return false;
    if (!clean) return false;
    void props.ipc.noteClosed({ path: old.path, base_hash: old.baseHash }).catch(() => undefined);
  }
  buffer = new EditorBuffer(path, text, baseHash);
  bufferVaultId = props.vaultId ?? null;
  held = [];
  conflict.value = null;
  deleted.value = false;
  notice.value = null;
  comparing.value = null;
  blocked.value = false;
  replaceState(text);
  markDirty();
  // Tells serat-app this pane holds the note at this base, so it emits
  // `note://external-change` for it (ADR-0016 §3).
  void props.ipc.noteOpened({ path, base_hash: baseHash }).catch(() => undefined);
  return true;
}

/**
 * Drops the buffer of a vault that is no longer open, WITHOUT flushing or telling the backend
 * (#80's rule, #151): the switch's `app://closing` handshake has already flushed it, and the old
 * path means nothing in the new vault, so a save here could only land in the wrong vault.
 * The editor returns to its read-only placeholder. No-op without a buffer.
 */
function closeNote(): void {
  if (!view || !buffer) return;
  ++loadToken; // supersede any in-flight loadNote
  if (autosaveTimer) {
    clearTimeout(autosaveTimer);
    autosaveTimer = null;
  }
  buffer = null;
  bufferVaultId = null;
  held = [];
  conflict.value = null;
  deleted.value = false;
  notice.value = null;
  comparing.value = null;
  blocked.value = false;
  replaceState("");
  markDirty();
}

/** Back-compat with the P0 shell's placeholder-content path (no note open
 * yet, or a read failure message) — does not touch the buffer, so it must
 * never be used for real note content; it is a no-op once a buffer exists, and the
 * placeholder it shows is read-only. Like
 * `loadNote` it clears undo history. */
function setText(text: string): void {
  // With a buffer open this would desynchronise the document from its base
  // (and silently discard edits): refuse. Real content goes through `loadNote`.
  if (buffer) return;
  replaceState(text);
}

function getText(): string {
  return view?.state.doc.toString() ?? "";
}

function focus(): void {
  view?.focus();
}

/**
 * Inserts `text` at the cursor (file-tree "insert link"). An ordinary edit:
 * it enters the undo stack and the buffer like typing. False when no note is
 * open (the editor is read-only), so the caller can say why it did nothing.
 */
function insertAtCursor(text: string): boolean {
  if (!view || !buffer) return false;
  view.dispatch({ ...view.state.replaceSelection(text), scrollIntoView: true, userEvent: "input" });
  view.focus();
  return true;
}

/** Puts the cursor at `offset` (a document offset) and scrolls it into view (jump-to-block). */
function revealOffset(offset: number): void {
  if (!view) return;
  const at = Math.max(0, Math.min(offset, view.state.doc.length));
  view.dispatch({ selection: { anchor: at }, effects: EditorView.scrollIntoView(at, { y: "center" }) });
  view.focus();
}

async function triggerSave(): Promise<SaveOutcome | null> {
  return performSave(true);
}

const isDirty = computed(() => dirty.value);

defineExpose({
  setText,
  getText,
  focus,
  insertAtCursor,
  revealOffset,
  loadNote,
  closeNote,
  triggerSave,
  compareWithDisk,
  isDirty,
  conflict,
  /** The live `EditorView`, for running registry commands from the palette. */
  getView: () => view,
  undo: () => view && undo(view),
  redo: () => view && redo(view),
});
</script>

<template>
  <div class="serat-editor-surface" data-testid="serat-editor">
    <div ref="host" class="cm-editor-host"></div>
    <div
      v-if="conflict && !minimised"
      class="serat-external-notice"
      role="status"
      tabindex="0"
      data-testid="external-conflict"
      @keydown.esc.stop.prevent="minimised = true"
    >
      <span>This note changed on disk and your unsaved edits overlap the change. Nothing has been discarded.</span>
      <button type="button" data-testid="keep-mine" @click="keepMine">Keep mine</button>
      <button type="button" data-testid="take-theirs" @click="takeTheirs">Take theirs</button>
      <button type="button" data-testid="compare" @click="compareExternal">Compare</button>
    </div>
    <button
      v-else-if="conflict"
      type="button"
      class="serat-external-dot"
      title="External change pending \u2014 click to review"
      aria-label="External change pending \u2014 click to review"
      data-testid="external-dot"
      @click="minimised = false"
    ></button>
    <div v-else-if="notice" class="serat-external-notice" role="status" data-testid="external-notice">{{ notice }}</div>
    <ExternalCompare
      v-if="comparing"
      :path="comparing.path"
      :mine="comparing.mine"
      :disk="comparing.disk"
      :resolvable="comparing.resolvable && !!conflict"
      @close="comparing = null"
      @keep-mine="resolveFromCompare(keepMine)"
      @take-theirs="resolveFromCompare(takeTheirs)"
    />
    <Preview
      v-if="!livePreview"
      class="serat-editor-split-preview"
      :source="previewSource"
      @wikilink-click="(t) => emit('wikilink-click', t)"
      @external-link-click="(u) => emit('external-link-click', u)"
    />
  </div>
</template>

<style scoped>
.serat-external-notice {
  position: absolute;
  top: 0;
  right: 0;
  z-index: 2;
  display: flex;
  gap: 0.5em;
  align-items: center;
  padding: 4px 10px;
  border: 1px solid var(--serat-border-hairline, #ddd);
  background: var(--serat-surface-raised, #fff);
  font-size: 0.9em;
}
.serat-external-dot {
  position: absolute;
  top: 6px;
  right: 6px;
  z-index: 2;
  width: 12px;
  height: 12px;
  padding: 0;
  border: 0;
  border-radius: 50%;
  background: var(--serat-accent-secondary, #b08d2f);
  cursor: pointer;
}
.serat-editor-surface {
  position: relative;
  display: flex;
  flex: 1 1 auto;
  min-height: 0;
  width: 100%;
}
.cm-editor-host {
  flex: 1 1 50%;
  min-width: 0;
}
.serat-editor-split-preview {
  flex: 1 1 50%;
  min-width: 0;
  overflow: auto;
  border-left: 1px solid var(--serat-border-hairline, #ddd);
}
</style>
