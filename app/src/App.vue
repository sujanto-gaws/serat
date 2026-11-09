<!--
  Task #98 (P0) mounted a plain text box; task #21 (P1: FR-EDT-01/02/03/10,
  NFR-PRF-02, ADR-0006, ADR-0016) puts the hybrid live-preview editor behind
  it. No file tree/lens switcher/wikilink UI yet — those are their own
  tasks (#1-#4, #22) — this file's job is still just "one buffer", but that
  buffer now saves through `save_patch` patch sets (ADR-0016 §2) instead of
  whole-document `write_file`, debounced/blurred/closed internally by
  `CodeMirrorEditor.vue` rather than driven from here.
-->
<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";

import BacklinksPane from "@/panes/BacklinksPane.vue";
import SearchPanel from "@/panes/SearchPanel.vue";
import { locateBlock } from "@/panes/locate";
import FileTree from "@/tree/FileTree.vue";
import { readFile } from "@/ipc/client";
import type { BlockLocation } from "@/ipc/generated/BlockLocation";

import type { UnlistenFn } from "@tauri-apps/api/event";

import type { SaveOutcome } from "@/editor/buffer";
import CodeMirrorEditor from "@/editor/CodeMirrorEditor.vue";
import { indexStatus, isIpcError, onIndexStatus } from "@/ipc/client";
import { hydrateBuffer, schedulePaintMark } from "@/startup";
import { useVaultStore } from "@/stores/vault";

const store = useVaultStore();
const editorRef = ref<InstanceType<typeof CodeMirrorEditor> | null>(null);
let unlistenIndexStatus: UnlistenFn | null = null;
const searchOpen = ref(false);
const editorDirty = ref(false);
/** "Updated externally" in the status strip for 3 s after a disjoint external edit was merged
 * into unsaved work (external-change-notice.md, outcome table). */
const updatedExternally = ref(false);
let updatedTimer: ReturnType<typeof setTimeout> | null = null;
/** The persistent "Not saved - file changed on disk" line (editor.md, StaleHash sequencing). */
const saveBlocked = ref(false);

function onExternalMerged(): void {
  updatedExternally.value = true;
  if (updatedTimer) clearTimeout(updatedTimer);
  updatedTimer = setTimeout(() => {
    updatedTimer = null;
    updatedExternally.value = false;
  }, 3000);
}

const treeRef = ref<InstanceType<typeof FileTree> | null>(null);
const backlinksRef = ref<InstanceType<typeof BacklinksPane> | null>(null);
const searchRef = ref<InstanceType<typeof SearchPanel> | null>(null);
/** Below 720 px the tree is a flyover (file-tree.md, Responsive); this is its open state. */
const treeFlyover = ref(false);
/** A selected folder whose listing has not arrived yet: the context pane says "Listing…". */
const pendingListing = ref<string | null>(null);
/** Set when the tree reports the open note was deleted (backlinks-pane.md: "This note no longer exists"). */
const missingNote = ref<string | null>(null);
const openNoteMissing = computed(() => missingNote.value !== null && missingNote.value === store.openNote);
let focusBeforeSearch: HTMLElement | null = null;

function onTreeRemoved(path: string): void {
  const open = store.openNote;
  if (open !== null && (open === path || open.startsWith(`${path}/`))) missingNote.value = open;
}

/** Delete-then-create saves and git checkouts remove and re-add a path: the note exists again. */
function onTreeAdded(path: string): void {
  const gone = missingNote.value;
  if (gone !== null && (gone === path || gone.startsWith(`${path}/`))) missingNote.value = null;
}

/**
 * The tree's open event. `newTab` is ignored: there are no tabs in P1. On a narrow
 * window the flyover closes after a selection and focus returns to the editor.
 */
async function onTreeOpen(payload: { path: string; newTab: boolean; keepFocus: boolean }): Promise<void> {
  await openNote(payload.path);
  if (treeFlyover.value && !payload.keepFocus) {
    treeFlyover.value = false;
    await nextTick();
    editorRef.value?.focus();
  }
}

/*
 * Rename (F2) and the context menu are task #43 (tree operations). Until it
 * lands the events are received here and do nothing, so the tree's keys have a
 * handler to bind to; #43 replaces these two bodies. New note / new folder (the
 * disabled toolbar buttons, ⌘N, ⌘⇧N) are the same: #43 owns the naming flow
 * and the create commands, so the chords are no-ops until then.
 */
function onTreeRename(_payload: { path: string }): void {}
function onTreeContextMenu(_payload: { path: string }): void {}

function openSearch(): void {
  // Open or focus; never toggles closed (search-panel.md). Esc closes.
  if (!searchOpen.value) focusBeforeSearch = document.activeElement as HTMLElement | null;
  searchOpen.value = true;
  void nextTick(() => searchRef.value?.focus());
}

function closeSearch(): void {
  searchOpen.value = false;
  const back = focusBeforeSearch;
  focusBeforeSearch = null;
  void nextTick(() => (back && back.isConnected ? back.focus() : editorRef.value?.focus()));
}

/**
 * Opens `path` in the editor, optionally at a located block (backlinks and
 * search jump-to-block). The previous note's unsaved edits are flushed first by
 * `loadNote`; if they cannot be saved the note is not replaced.
 */
async function openNote(path: string, block: BlockLocation | null = null): Promise<void> {
  const editor = editorRef.value;
  if (!editor) return;
  try {
    const file = await readFile({ path });
    const loaded = await editor.loadNote(file.text, file.path, file.hash);
    if (!loaded) {
      store.setMark("not opened: the current note has changes that could not be saved yet");
      return;
    }
    store.setOpenBuffer(file.path, file.hash);
    missingNote.value = null;
    if (block) {
      // Exact while the file still has the hash the block was indexed at; by `ord`
      // (and said so) when it has changed since (architecture.md §4.5).
      const position = locateBlock(file.text, file.hash, block);
      editor.revealOffset(position.offset);
      if (position.approximate) store.setMark("approximate location \u2014 this note has changed since it was indexed");
    }
  } catch (error) {
    store.setMark(`could not open ${path}: ${JSON.stringify(error)}`);
  }
}

function onInsertLink(payload: { path: string; text: string }): void {
  if (!editorRef.value?.insertAtCursor(payload.text)) store.setMark("Open a note first to insert a link.");
}

/**
 * Global chords (components.md §7): ⌘⇧F open/focus search, ⌘1 tree, ⌘3 backlinks,
 * ⌘N new note, ⌘⇧N new folder. Regardless of which pane has focus.
 */
function onGlobalKeydown(event: KeyboardEvent): void {
  if (!(event.metaKey || event.ctrlKey) || event.altKey) return;
  const key = event.key.toLowerCase();
  if (event.shiftKey && key === "f") {
    event.preventDefault();
    openSearch();
  } else if (event.shiftKey && key === "n") {
    event.preventDefault(); // ⌘⇧N: new folder is task #43; a no-op until then

  } else if (!event.shiftKey && key === "n") {
    event.preventDefault(); // ⌘N: new note is task #43; a no-op until then

  } else if (!event.shiftKey && key === "1") {
    event.preventDefault();
    // The flyover only exists below 720 px (file-tree.md); docked, ⌘1 just focuses.
    if (window.matchMedia?.("(max-width: 719px)").matches) treeFlyover.value = true;
    void nextTick(() => treeRef.value?.focusTree());
  } else if (!event.shiftKey && key === "3") {
    event.preventDefault();
    backlinksRef.value?.focus();
  }
}

onMounted(async () => {
  // NFR-PRF-02's harness seam (`bench/editor-latency/tests/subject.ts`,
  // `openNoteForLatencyTest`, task #11/#21): that harness drives the
  // production vite build with the IPC client stubbed (architecture.md
  // §9.3's own words), so it has no `note_opened`/`read_file` round trip to
  // hang a deterministic note-open off. This global, present the instant
  // the editor exists (synchronous, before any `await` in this function —
  // same property as the editor's own synchronous mount), lets the harness
  // load its fixture note directly through `loadNote` and reach the running
  // `EditorView` the same way a real note open would leave it: with a base
  // buffer, saves wired to the (stubbed) IPC adapter, and the reveal rule
  // live. `bench-hash` is a syntactically valid but obviously-fake hash —
  // nothing the harness types triggers a save, so it is never checked.
  //
  // Installed ONLY in a `vite build --mode bench` build (`npm run build:bench`):
  // `import.meta.env.MODE` is replaced at build time, so a release build
  // contains neither the branch nor the global. A shipped app must not expose
  // a window-level "replace the open note's buffer" entry point.
  if (import.meta.env.MODE === "bench") {
    window.__seratBenchOpenNote = (text: string) => {
      void editorRef.value?.loadNote(text, "__bench__.md", "bench-hash-not-a-real-blake3");
    };
  }

  // The paint mark, first and synchronous — code-reviewer's SHOULD-FIX on #98's
  // review. This used to run after `await onIndexStatus(...)` below, which is an IPC
  // round trip: the double requestAnimationFrame this schedules could miss the frame
  // that actually painted first and mark a later one, inflating S_shell — ADR-0014's
  // binding Serat term — with time that has nothing to do with painting. Scheduling
  // is synchronous (the rAF pair is registered before this call returns); only
  // *waiting* on the promise it returns happens later, in `hydrateBuffer` below.
  const paintMark = schedulePaintMark(store);

  // Installed next so the first transition the actor publishes is not lost — mirrors
  // `run`'s ordering in crates/serat-app/src/lib.rs. This IPC round trip runs
  // concurrently with the rAF pair above, not before it: nothing here can delay a
  // mark that was already scheduled on the line above.
  window.addEventListener("keydown", onGlobalKeydown);

  unlistenIndexStatus = await onIndexStatus((status) => {
    store.setVaultId(status.vault_id);
    store.setIndexState(status.state);
    // The count of names the vault refused to list is one status fact (not a
    // per-name error). It is read when the status carries it.
    const refused = (status as { refused_names?: number }).refused_names;
    if (typeof refused === "number") store.setRefusedNames(refused);
  });

  store.setVaultRoot(window.__SERAT_BOOT__?.vault ?? null);

  // Late subscribers catch up by polling; the contract has both channels for this
  // reason (architecture §3.4).
  indexStatus().then((status) => {
    store.setVaultId(status.vault_id);
    store.setIndexState(status.state);
  });

  if (editorRef.value) {
    // `hydrateBuffer` reads the note and installs it with `loadNote` in one
    // step, from the bytes that were read (never `getText()`): the editor is
    // read-only until then, so nothing typed can become the base text
    // paired with the on-disk hash (C-04, NFR-REL-01).
    await hydrateBuffer(store, editorRef.value, paintMark);
  }
});

onUnmounted(() => {
  window.removeEventListener("keydown", onGlobalKeydown);
  unlistenIndexStatus?.();
  if (updatedTimer) clearTimeout(updatedTimer);
});

/**
 * ADR-0016 §2: the editor saves patch sets through `save_patch`, debounced
 * 1s idle / on blur / on unmount, and on the save command — all internal to
 * `CodeMirrorEditor.vue`. This only reflects the outcome in the status
 * strip (§9.1: no modal on the write path; a refusal resolves in place).
 */
/** The save was refused outright; say exactly why and that nothing was written. */
function refusalMessage(error: unknown): string {
  if (isIpcError(error) && error.kind === "line_endings_would_fuse") {
    return `the save was refused because the note mixes line-ending styles at line ${error.line + 1}; nothing was written`;
  }
  return `the save was refused; nothing was written: ${JSON.stringify(error)}`;
}

/** Mod-Enter on a link. Opening notes needs the file tree and tabs (tasks #1-#4);
 * until then the host only reports what the link resolved to. */
function onFollowLink(link: { target: string; path: string | null }): void {
  store.setMark(link.path ? `link: ${link.path}` : `no note named \u201C${link.target}\u201D`);
}

function onSaveOutcome(outcome: SaveOutcome): void {
  switch (outcome.kind) {
    case "saved":
      store.setBaseHash(outcome.written.hash);
      store.setMark(`saved ${outcome.written.bytes} bytes`);
      break;
    case "stale":
      if (isIpcError(outcome.error) && outcome.error.kind === "stale_hash") {
        store.setMark("changed on disk since it was read — reopen to merge");
      } else {
        store.setMark(`not saved: ${JSON.stringify(outcome.error)}`);
      }
      break;
    case "refused":
      store.setMark(refusalMessage(outcome.error));
      break;
    case "rejected":
      store.setMark(`not saved: ${JSON.stringify(outcome.error)}`);
      break;
    case "noop":
    case "coalesced":
      break;
  }
}
</script>

<template>
  <header class="titlebar">
    <span class="wordmark">Serat</span>
    <span class="vault">{{ store.vaultRoot ?? "no vault" }}</span>
    <button type="button" class="tree-toggle" aria-label="Toggle file tree" @click="treeFlyover = !treeFlyover">Files</button>
  </header>

  <main class="workspace" :class="{ 'tree-open': treeFlyover }">
    <aside class="pane pane-tree" aria-label="File tree">
      <FileTree
        ref="treeRef"
        :open-path="store.openNote"
        :open-dirty="editorDirty"
        :vault-id="store.vaultId"
        @open="onTreeOpen"
        @insert-link="onInsertLink"
        @rename="onTreeRename"
        @context-menu="onTreeContextMenu"
        @pending-listing="(p) => (pendingListing = p)"
        @removed="onTreeRemoved"
        @added="onTreeAdded"
      />
    </aside>
    <div class="editor-column">
      <CodeMirrorEditor
        ref="editorRef"
        :vault-id="store.vaultId"
        placeholder="No note open."
        @save-outcome="onSaveOutcome"
        @follow-link="onFollowLink"
        @dirty-changed="(d: boolean) => (editorDirty = d)"
        @external-merged="onExternalMerged"
        @save-blocked="(b: boolean) => (saveBlocked = b)"
      />
    </div>
    <aside class="pane pane-context" aria-label="Context">
      <p v-if="pendingListing !== null" class="listing-note" role="status" data-testid="listing-note">Listing…</p>
      <SearchPanel ref="searchRef" v-if="searchOpen" @open="(p) => openNote(p.path, p.block)" @close="closeSearch" />
      <BacklinksPane
        ref="backlinksRef"
        :path="store.openNote"
        :missing="openNoteMissing"
        @open="(p) => openNote(p.path, p.block)"
      />
    </aside>
  </main>

  <footer class="statusbar">
    <span class="index">{{ store.indexStatusText }}</span>
    <span v-if="store.refusedNames" class="refused" data-testid="refused-names"
      >{{ store.refusedNames }} {{ store.refusedNames === 1 ? "name" : "names" }} not shown (invalid on this platform)</span
    >
    <span v-if="updatedExternally" class="updated-externally" data-testid="updated-externally">Updated externally</span>
    <span v-if="saveBlocked" class="save-blocked" data-testid="save-blocked">
      Not saved — file changed on disk
      <button type="button" data-testid="blocked-compare" @click="editorRef?.compareWithDisk()">Compare</button>
    </span>
    <span class="marks">{{ store.marksText }}</span>
  </footer>
</template>
