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
import { computed, nextTick, onMounted, onUnmounted, provide, ref } from "vue";

import BacklinksPane from "@/panes/BacklinksPane.vue";
import SearchPanel from "@/panes/SearchPanel.vue";
import { locateBlock } from "@/panes/locate";
import FileTree from "@/tree/FileTree.vue";
import { readFile } from "@/ipc/client";
import type { BlockLocation } from "@/ipc/generated/BlockLocation";

import type { UnlistenFn } from "@tauri-apps/api/event";

import type { SaveOutcome } from "@/editor/buffer";
import CodeMirrorEditor from "@/editor/CodeMirrorEditor.vue";
import { indexStatus, isIpcError, onIndexStatus, onVaultOpened } from "@/ipc/client";
import { hydrateBuffer, schedulePaintMark } from "@/startup";
import { useVaultStore } from "@/stores/vault";
import { CommandRegistry, COMMANDS_KEY } from "@/editor/commands";
import { detectPlatform } from "@/editor/codemirrorSetup";
import { headingDocOffset } from "@/overlays/headingTarget";
import CommandPalette from "@/overlays/CommandPalette.vue";
import QuickSwitcher from "@/overlays/QuickSwitcher.vue";
import { getUiSettings, setLivePreview } from "@/ipc/client";

const store = useVaultStore();
const editorRef = ref<InstanceType<typeof CodeMirrorEditor> | null>(null);
let unlistenIndexStatus: UnlistenFn | null = null;
let unlistenVaultOpened: UnlistenFn | null = null;
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
async function openNote(path: string, block: BlockLocation | null = null): Promise<string | null> {
  const editor = editorRef.value;
  if (!editor) return null;
  try {
    const file = await readFile({ path });
    const loaded = await editor.loadNote(file.text, file.path, file.hash);
    if (!loaded) {
      store.setMark("not opened: the current note has changes that could not be saved yet");
      return null;
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
    return file.text;
  } catch (error) {
    store.setMark(`could not open ${path}: ${JSON.stringify(error)}`);
    return null;
  }
}

/*
 * Quick switcher (⌘O, FR-NAV-01) and command palette (⌘K, FR-NAV-06). The two
 * overlays are mutually exclusive. Every user-invocable action lives in ONE
 * registry (architecture.md §9.3, editor/commands.ts): the editor adds its own
 * commands to it, this file adds the global ones, the palette lists it, and the
 * global chords below are matched against its effective bindings.
 */
const commands = new CommandRegistry();
commands.onOverrideRefused((ids) =>
  store.setMark(`keybinding ignored (already used by another command): ${ids.join(", ")}`),
);
provide(COMMANDS_KEY, commands);
const mac = detectPlatform() === "mac";
const switcherOpen = ref(false);
const paletteOpen = ref(false);
const livePreview = ref(true);

function openSwitcher(): void {
  paletteOpen.value = false;
  switcherOpen.value = true;
}

function openPalette(): void {
  switcherOpen.value = false;
  paletteOpen.value = true;
}

async function onSwitcherOpen(payload: {
  path: string;
  newTab: boolean;
  heading: { text: string; byteStart: number } | null;
}): Promise<void> {
  switcherOpen.value = false; // `newTab` is ignored: there are no tabs in P1
  const text = await openNote(payload.path);
  const editor = editorRef.value;
  if (editor && text !== null && payload.heading && store.openNote === payload.path) {
    editor.revealOffset(headingDocOffset(text, payload.heading.byteStart, payload.heading.text));
  }
  await nextTick();
  editor?.focus();
}

async function onPaletteRun(id: string): Promise<void> {
  paletteOpen.value = false;
  await nextTick(); // focus returns to the opener first; the command may then move it
  const reason = await commands.execute(id, editorRef.value?.getView() ?? null);
  if (reason !== null) store.setMark(reason);
}

/** A toggle made before `get_ui_settings` answers wins over the stored value. */
let toggledByUser = false;
function toggleLivePreview(): void {
  toggledByUser = true;
  livePreview.value = !livePreview.value;
  void setLivePreview({ enabled: livePreview.value }).catch(() => undefined);
}

/** Global, view-less commands (`when` unset), in CodeMirror key notation. */
function registerCommands(): Array<() => void> {
  return [
    commands.register({ id: "switcher.open", name: "Open quick switcher", defaultBinding: "Mod-o", exec: openSwitcher }),
    commands.register({ id: "palette.open", name: "Open command palette", defaultBinding: "Mod-k", exec: openPalette }),
    commands.register({ id: "search.open", name: "Search vault", defaultBinding: "Mod-Shift-f", exec: openSearch }),
    commands.register({
      id: "tree.focus",
      name: "Focus file tree",
      defaultBinding: "Mod-1",
      exec: () => {
        // The flyover only exists below 720 px (file-tree.md); docked, this just focuses.
        if (window.matchMedia?.("(max-width: 719px)").matches) treeFlyover.value = true;
        void nextTick(() => treeRef.value?.focusTree());
      },
    }),
    commands.register({ id: "backlinks.focus", name: "Focus backlinks", defaultBinding: "Mod-3", exec: () => backlinksRef.value?.focus() }),
  ];
}
const disposeCommands = registerCommands();

function onInsertLink(payload: { path: string; text: string }): void {
  if (!editorRef.value?.insertAtCursor(payload.text)) store.setMark("Open a note first to insert a link.");
}

/**
 * Global chords (components.md §7), matched against the registry's effective
 * bindings so a rebinding changes behaviour, not just the palette's chip.
 * ⌘N / ⌘⇧N (new note / folder) are task #43's and stay no-ops until then.
 */
function onGlobalKeydown(event: KeyboardEvent): void {
  if (event.defaultPrevented) return; // the editor's keymap already took it (mac emacs Ctrl-k/o/n)
  const mod = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (!mod || event.altKey) return;
  const command = commands.matchGlobal(event, mac);
  if (command) {
    event.preventDefault();
    void commands.execute(command.id, null);
    return;
  }
  if (event.key.toLowerCase() === "n") event.preventDefault(); // ⌘N / ⌘⇧N: #43
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
  store.adoptBoot(window.__SERAT_BOOT__?.vault_id);
  store.setVaultRoot(window.__SERAT_BOOT__?.vault ?? null);

  // Installed next so the first transition the actor publishes is not lost — mirrors
  // `run`'s ordering in crates/serat-app/src/lib.rs. This IPC round trip runs
  // concurrently with the rAF pair above, not before it: nothing here can delay a
  // mark that was already scheduled on the line above.
  window.addEventListener("keydown", onGlobalKeydown);

  // `editor.live_preview` and the user's `[keybindings]` (FR-PLT-05, NFR-USE-01). Until the
  // answer arrives the defaults hold (live preview on, default bindings), so nothing here
  // sits on the first-paint path. Rebinding UI + `set_keybinding`: task #93.
  getUiSettings()
    .then((settings) => {
      if (!settings) return; // the bench stub answers null
      if (!toggledByUser) livePreview.value = settings.live_preview;
      const refused = commands.setOverrides(settings.keybindings ?? []);
      if (refused.length > 0) {
        store.setMark(`keybinding ignored (already used by another command): ${refused.join(", ")}`);
      }
    })
    .catch(() => undefined);


  // The vault id's sources (architecture 3.4): BootInfo at launch (read synchronously above),
  // `vault://opened` for every later open; index_status is a restricted fallback only.
  void onVaultOpened((opened) => store.vaultOpened(opened))
    .then((unlisten) => (unlistenVaultOpened = unlisten))
    .catch(() => undefined);

  unlistenIndexStatus = await onIndexStatus((status) => {
    store.adoptFromIndexStatus(status.vault_id);
    if (!store.statusBelongsHere(status.vault_id)) return; // another vault's, or "" between two vaults
    store.setIndexState(status.state);
    // The count of names the vault refused to list is one status fact (not a
    // per-name error). It is read when the status carries it.
    const refused = (status as { refused_names?: number }).refused_names;
    if (typeof refused === "number") store.setRefusedNames(refused);
  });


  // Late subscribers catch up by polling; the contract has both channels for this
  // reason (architecture §3.4).
  indexStatus().then((status) => {
    store.adoptFromIndexStatus(status.vault_id);
    if (!store.statusBelongsHere(status.vault_id)) return;
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
  for (const dispose of disposeCommands.splice(0)) dispose();
  unlistenIndexStatus?.();
  unlistenVaultOpened?.();
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
        :live-preview="livePreview"
        :commands="commands"
        placeholder="No note open."
        @toggle-live-preview="toggleLivePreview"
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

  <QuickSwitcher v-if="switcherOpen" @open="onSwitcherOpen" @close="switcherOpen = false" />
  <CommandPalette v-if="paletteOpen" :registry="commands" @run="onPaletteRun" @close="paletteOpen = false" />

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
