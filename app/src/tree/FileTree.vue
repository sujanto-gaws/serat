<!--
  The file tree pane (file-tree.md; FR-VLT-01, FR-NAV-03, FR-BIN-03; ADR-0015).

  - Messages from `open_tree` are folded into a plain (non-reactive) `TreeModel`
    in arrival order, synchronously, one call per message: nothing is batched
    away or dropped. Vue sees only a version counter.
  - Virtualised: the DOM holds the rows in the viewport plus a small overscan,
    whatever the vault size.
  - `mark_tree`: `first_screen` after the root listing has rendered and a
    double requestAnimationFrame has passed; `complete` the same way after
    `Complete` has been applied. Both are taken from a `flush: 'post'` watcher,
    i.e. after the commit that rendered those rows (ADR-0015 §3).
  - Layout: an outer flex column (toolbar, a persistent live region, the
    scrolling `role="tree"`). Nothing but rows lives inside the tree or its scroll
    container, so row offsets are exactly `index * rowHeight`.
  - Keyboard: roving focus on the `role="tree"` container, announced through
    `aria-activedescendant`; one meaning per key. `Mod-F` (tree-local) focuses the
    filter box; the filter bolds matches and hides the rest.
  - Task #43: New note / New folder (toolbar, ⌘N, ⌘⇧N via the host) with an inline naming
    prompt, and the context menu (right-click, Shift-F10, Menu key). Creation goes through the
    host's `createEntry` (write_file / create_folder); the tree shows the result from the stream,
    never optimistically. NOT built, because contracts/ipc.rs has no command for them (rename and
    move are P3, FR-VLT-09, as they need link repair): rename (F2 is emitted and the host says so),
    move, trash, drag-to-move. Their menu entries are Disabled with a tooltip, not hidden.
  - Not built (no tabs in P1): `newTab` is carried on `open` and ignored by the host.
-->
<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { defaultPaneIpc, type PaneIpcAdapter } from "@/ipc/paneAdapter";
import { folderName, noteFileName, takenNames, uniqueName, type CreateKind, type CreateRequest } from "@/tree/createEntry";
import TreeContextMenu, { type MenuItem } from "@/tree/TreeContextMenu.vue";
import { chordGlyphs, chordSpoken } from "@/editor/commands";
import { detectPlatform } from "@/editor/codemirrorSetup";
import { insertLinkText, isNameUnique, navigate, TREE_STOPPED_TEXT, visibleWindow } from "@/tree/treeLogic";
import { childPath, displayName, matchRange, TreeModel, type TreeRow } from "@/tree/treeModel";

const props = withDefaults(
  defineProps<{
    ipc?: PaneIpcAdapter;
    /** Row height in px (`layout.row-height-tree`). */
    rowHeight?: number;
    /** Used when the container has no measurable height yet. */
    fallbackViewportHeight?: number;
    /** Vault-relative path of the note open in the editor. */
    openPath?: string | null;
    /** Whether that note has unsaved changes (brass dot). */
    openDirty?: boolean;
    /**
     * The open vault's id (from `index_status`). `tree://status` events for any
     * other vault are discarded. Until the id is known the check is a no-op and
     * every status is accepted (the event is only emitted for the open vault).
     */
    vaultId?: string | null;
    /** `false` = no vault open (after a failed open): drop every event, whatever its id. Distinct from a
     * `null` `vaultId`, which means "not known yet, do not filter". */
    vaultOpen?: boolean;
    /**
     * Creates a note or folder and resolves to `null` on success or to the core's refusal as a sentence.
     * Absent: the toolbar buttons stay disabled with a tooltip (disabled, not hidden). That fallback exists
     * for tests and embedding; no production host omits it (App.vue always passes one).
     */
    createEntry?: (request: CreateRequest) => Promise<string | null>;
    /** macOS conventions (⌘ glyphs, Ctrl-click = context menu). Defaults to the detected platform. */
    mac?: boolean;
    /** Menu entry id -> CodeMirror-notation binding shown as its accelerator (the registry's effective
     * bindings, so the menu, the palette and the chips agree). `null` shows none. */
    bindings?: Record<string, string | null>;
  }>(),
  { ipc: () => defaultPaneIpc, rowHeight: 26, fallbackViewportHeight: 600, openPath: null, openDirty: false, vaultId: null, vaultOpen: true },
);

const emit = defineEmits<{
  open: [payload: { path: string; newTab: boolean; keepFocus: boolean }];
  "insert-link": [payload: { path: string; text: string }];
  rename: [payload: { path: string }];
  /** A pending (not yet listed) folder was selected: the context pane says "Listing…". `null` clears it. */
  "pending-listing": [path: string | null];
  /** An entry (or a folder, with everything under it) disappeared from the tree; the host may need to react. */
  removed: [path: string];
  /** An entry was created or re-created at this path (an upsert); clears a "removed" the host was holding. */
  added: [path: string];
}>();

const model = new TreeModel();

/** The inline naming prompt of New note / New folder. */
const prompt = ref<{ kind: CreateKind; dir: string; value: string; error: string | null; busy: boolean } | null>(null);
const promptInput = ref<HTMLInputElement | null>(null);
/** The open context menu: which row, where, and whether it was opened by keyboard. */
const menu = ref<{ path: string; x: number; y: number } | null>(null);
const version = ref(0);
const scrollTop = ref(0);
const viewportHeight = ref(props.fallbackViewportHeight);
const focusedPath = ref<string | null>(null);
const filter = ref("");
const stopped = ref(false);
const container = ref<HTMLDivElement | null>(null);
const filterInput = ref<HTMLInputElement | null>(null);
let resizeObserver: ResizeObserver | null = null;
let firstScreenMarked = false;
let completeMarked = false;
let disposed = false;
let unlistenStatus: (() => void) | null = null;
let pendingSelected: string | null = null;

const rows = computed<readonly TreeRow[]>(() => {
  void version.value;
  return model.rows(filter.value);
});
const filtering = computed(() => filter.value.trim() !== "");

const windowed = computed(() =>
  visibleWindow(scrollTop.value, viewportHeight.value, props.rowHeight, rows.value.length),
);
const visibleRows = computed(() => rows.value.slice(windowed.value.start, windowed.value.end));
const rootEmpty = computed(() => {
  void version.value;
  return model.rootListed && (model.listing("")?.length ?? 0) === 0;
});
const noMatches = computed(() => filtering.value && model.rootListed && rows.value.length === 0);

function bump(): void {
  version.value += 1;
}

function afterPaint(callback: () => void): void {
  requestAnimationFrame(() => requestAnimationFrame(callback));
}

// Marks are taken after the commit that rendered the rows (flush: 'post').
watch(
  version,
  () => {
    if (!firstScreenMarked && model.rootListed) {
      firstScreenMarked = true;
      afterPaint(() => {
        void props.ipc.markTree({ mark: "first_screen", entries_in_model: model.fileCount }).catch(() => undefined);
      });
    }
    if (!completeMarked && model.complete) {
      completeMarked = true;
      afterPaint(() => {
        void props.ipc.markTree({ mark: "complete", entries_in_model: model.fileCount }).catch(() => undefined);
      });
    }
    // A selected pending folder stops saying "Listing…" the moment its listing lands.
    if (pendingSelected !== null && model.isListed(pendingSelected)) {
      pendingSelected = null;
      emit("pending-listing", null);
    }
  },
  { flush: "post" },
);

// A new filter changes the row list: start from the top.
watch(filter, () => {
  if (container.value) container.value.scrollTop = 0;
  scrollTop.value = 0;
});

onMounted(() => {
  measure();
  if (typeof ResizeObserver !== "undefined" && container.value) {
    resizeObserver = new ResizeObserver(measure);
    resizeObserver.observe(container.value);
  }
  stopped.value = false; // a new open_tree starts afresh (contracts/ipc.rs `TreeStatus`)
  void props.ipc
    .openTree((message) => {
      model.apply(message); // strictly in arrival order, never dropped
      bump();
      if (message.kind === "remove") emit("removed", childPath(message.dir, message.name));
      else if (message.kind === "upsert") emit("added", childPath(message.dir, message.entry.name));
    })
    .catch(() => undefined);
  // Registered after `open_tree` is on the wire.
  void props.ipc
    .onTreeStatus?.((status) => {
      if (!props.vaultOpen) return; // no vault open: nothing is current
      if (props.vaultId !== null && status.vault_id !== props.vaultId) return; // another vault's session
      if (status.stopped) stopped.value = true;
    })
    .then((un) => {
      if (disposed) un(); // resolved after unmount: do not leak the listener
      else unlistenStatus = un;
    })
    .catch(() => undefined);
});

onBeforeUnmount(() => {
  disposed = true;
  resizeObserver?.disconnect();
  unlistenStatus?.();
});

function measure(): void {
  const height = container.value?.clientHeight ?? 0;
  viewportHeight.value = height > 0 ? height : props.fallbackViewportHeight;
}

function onScroll(): void {
  scrollTop.value = container.value?.scrollTop ?? 0;
}

function rowLabel(row: TreeRow): string {
  if (row.kind === "dir") {
    if (row.state === "pending") return `${row.name}, folder, contents loading`;
    if (row.state === "empty") return `${row.name}, folder, empty`;
    return `${row.name}, folder`;
  }
  if (row.kind === "asset") {
    const ext = row.name.includes(".") ? row.name.slice(row.name.lastIndexOf(".") + 1).toUpperCase() : "file";
    const stem = row.name.includes(".") ? row.name.slice(0, row.name.lastIndexOf(".")) : row.name;
    return `${stem}, ${ext}${row.sidecar ? ", has metadata note" : ""}`;
  }
  return row.name.replace(/\.md$/i, "");
}

const IMAGE_EXT = new Set(["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "svg"]);

/** Format glyph class + text (file-tree.md: PDF, image, generic file; notes and folders their own). */
function glyph(row: TreeRow): { cls: string; text: string } {
  if (row.kind === "dir") return { cls: "folder", text: "▭" };
  if (row.kind === "note") return { cls: "note", text: "≡" };
  const ext = row.name.includes(".") ? row.name.slice(row.name.lastIndexOf(".") + 1).toLowerCase() : "";
  if (ext === "pdf") return { cls: "pdf", text: "P" };
  if (IMAGE_EXT.has(ext)) return { cls: "image", text: "▣" };
  return { cls: "file", text: "◻" };
}

/** The row's name split around the filter match, for bolding. */
function nameParts(row: TreeRow): { before: string; match: string; after: string } {
  const name = displayName(row.name, row.kind);
  const range = filtering.value ? matchRange(name, filter.value) : null;
  if (!range) return { before: name, match: "", after: "" };
  return { before: name.slice(0, range[0]), match: name.slice(range[0], range[1]), after: name.slice(range[1]) };
}

function isDot(row: TreeRow): boolean {
  if (row.kind === "note") return props.openDirty && props.openPath === row.path;
  return row.kind === "asset" && row.sidecar;
}

// Stable DOM ids, one per path, assigned on first use. Bounded by the vault's entry
// count (only rendered rows ever ask), and the pane lives as long as the vault is open.
const rowIds = new Map<string, number>();
function rowId(path: string): string {
  let n = rowIds.get(path);
  if (n === undefined) {
    n = rowIds.size;
    rowIds.set(path, n);
  }
  return `serat-tree-row-${n}`;
}
const activeDescendant = computed(() => {
  const path = focusedPath.value;
  return path !== null && visibleRows.value.some((r) => r.path === path) ? rowId(path) : undefined;
});

function select(path: string): void {
  focusedPath.value = path;
  const row = rows.value.find((r) => r.path === path);
  const pending = row && row.kind === "dir" && row.state === "pending" ? path : null;
  if (pending !== pendingSelected) {
    pendingSelected = pending;
    emit("pending-listing", pending);
  }
}

function toggleDir(path: string, expand: boolean | null): void {
  if (filtering.value) return; // the filtered view shows its own expansion; the user's is untouched
  if (expand === null) model.toggle(path);
  else if (expand) model.expand(path);
  else model.collapse(path);
  bump();
}

/** Accelerators shown when the host passes none: the tree-local chords of file-tree.md. */
// TODO(#39 follow-up: register tree.insertLink in the CommandRegistry, default Mod-Shift-l, active only when the tree has focus; route treeLogic.ts:89 and this menu through commands.effectiveBinding)
// DEFAULT_MENU_BINDINGS goes away once that follow-up lands (the host then always passes effective bindings).
// `rename: "F2"` stays fixed (file-tree.md keyboard map).
const DEFAULT_MENU_BINDINGS: Record<string, string | null> = { "insert-link": "Mod-Shift-l", rename: "F2" };

const isMac = computed(() => props.mac ?? detectPlatform() === "mac");

/** The platform's own conventions (components.md §7): on macOS Ctrl-click is a secondary click. WebKit sends
 * `contextmenu` for it (handled by the row's `@contextmenu`); this also catches an engine that delivers a
 * `click` with `ctrlKey` instead, so it never OPENS the note. Both paths open the same single menu. */
function onRowClick(row: TreeRow, event?: MouseEvent): void {
  if (event && isMac.value && event.ctrlKey) {
    event.preventDefault();
    openMenu(row.path, event.clientX, event.clientY);
    return;
  }
  select(row.path);
  if (row.kind === "dir") {
    toggleDir(row.path, null);
  } else {
    emit("open", { path: row.path, newTab: false, keepFocus: false });
  }
}

function onDragStart(event: DragEvent, row: TreeRow): void {
  if (row.kind === "dir" || !event.dataTransfer) return;
  // The editor accepts a dropped `text/plain` natively and inserts it at the drop point.
  event.dataTransfer.setData("text/plain", insertLinkText(row, isNameUnique(model, row.name)));
  event.dataTransfer.effectAllowed = "copy";
}

function scrollIntoView(path: string): void {
  const index = rows.value.findIndex((r) => r.path === path);
  if (index < 0 || !container.value) return;
  const top = index * props.rowHeight;
  const el = container.value;
  if (top < el.scrollTop) el.scrollTop = top;
  else if (top + props.rowHeight > el.scrollTop + viewportHeight.value) {
    el.scrollTop = top + props.rowHeight - viewportHeight.value;
  }
  scrollTop.value = el.scrollTop;
}

function onKeydown(event: KeyboardEvent): void {
  const action = navigate(rows.value, focusedPath.value, {
    key: event.key,
    ctrlOrMeta: event.ctrlKey || event.metaKey,
    shift: event.shiftKey,
  });
  if (action.kind === "none") return;
  event.preventDefault();
  switch (action.kind) {
    case "focus":
      select(action.path);
      scrollIntoView(action.path);
      break;
    case "expand":
      toggleDir(action.path, true);
      break;
    case "collapse":
      toggleDir(action.path, false);
      break;
    case "open":
      emit("open", { path: action.path, newTab: action.newTab, keepFocus: action.keepFocus });
      break;
    case "rename":
      emit("rename", { path: action.path });
      break;
    case "context-menu": {
      const el = container.value?.querySelector<HTMLElement>(`[data-path="${CSS.escape(action.path)}"]`);
      const box = el?.getBoundingClientRect();
      openMenu(action.path, (box?.left ?? 0) + 24, box?.bottom ?? 0);
      break;
    }
    case "insert-link": {
      const row = rows.value.find((r) => r.path === action.path);
      if (row) emit("insert-link", { path: row.path, text: insertLinkText(row, isNameUnique(model, row.name)) });
      break;
    }
  }
}

/** Mod-F anywhere in the tree pane: the tree's own filter (never the global search chord, Mod-Shift-F). */
function onPaneKeydown(event: KeyboardEvent): void {
  if ((event.ctrlKey || event.metaKey) && !event.shiftKey && (event.key === "f" || event.key === "F")) {
    event.preventDefault();
    focusFilter();
  }
}

function onFilterKeydown(event: KeyboardEvent): void {
  if (event.key === "Escape") {
    event.preventDefault();
    filter.value = "";
    focusTree();
  } else if (event.key === "ArrowDown" || event.key === "Enter") {
    event.preventDefault();
    focusTree();
    if (focusedPath.value === null && rows.value.length > 0) select(rows.value[0].path);
  }
}

function focusTree(): void {
  void nextTick(() => container.value?.focus());
}
function focusFilter(): void {
  void nextTick(() => {
    filterInput.value?.focus();
    filterInput.value?.select();
  });
}

/** Where New note / New folder create: the selected folder, the selected entry's folder, else the vault root. */
function currentDir(): string {
  const path = focusedPath.value;
  const row = path === null ? undefined : rows.value.find((r) => r.path === path);
  if (!row) return "";
  return row.kind === "dir" ? row.path : row.dir;
}

/** Where focus was when the prompt opened (the editor, usually): Esc and a finished folder go back there. */
let promptOrigin: HTMLElement | null = null;

function restoreFocus(): void {
  const back = promptOrigin;
  promptOrigin = null;
  if (back && back.isConnected) back.focus();
  else focusTree();
}

function startCreate(kind: CreateKind, dir: string = currentDir()): void {
  if (!props.createEntry) return;
  if (prompt.value && prompt.value.kind === kind) {
    // The chord again while the prompt is open: back to it, keeping what was typed.
    void nextTick(() => promptInput.value?.focus());
    return;
  }
  if (!prompt.value) {
    const active = document.activeElement;
    promptOrigin = active instanceof HTMLElement && active !== document.body ? active : null;
  }
  const taken = takenNames(model, dir);
  const value = kind === "note" ? uniqueName(taken, "Untitled", ".md").replace(/\.md$/i, "") : uniqueName(taken, "New folder");
  prompt.value = { kind, dir, value, error: null, busy: false };
  void nextTick(() => {
    promptInput.value?.focus();
    promptInput.value?.select();
  });
}

function cancelPrompt(): void {
  prompt.value = null;
  restoreFocus();
}

async function submitPrompt(): Promise<void> {
  const p = prompt.value;
  if (!p || p.busy || !props.createEntry) return;
  const fileName = p.kind === "note" ? noteFileName(p.value) : folderName(p.value);
  if (fileName === null) {
    p.error = "Give it a name.";
    return;
  }
  // Uniqueness is the front end's job and is case-insensitive (contracts/ipc.rs, create_folder); every
  // other rule (characters, reserved names, escapes) is the core's and arrives as its refusal below.
  if (takenNames(model, p.dir).has(fileName.toLowerCase())) {
    p.error = `A ${p.kind} named \u201C${fileName}\u201D already exists here.`;
    return;
  }
  p.error = null;
  p.busy = true;
  const name = p.kind === "note" ? fileName.replace(/\.md$/i, "") : fileName;
  let refusal: string | null;
  try {
    refusal = await props.createEntry({ kind: p.kind, dir: p.dir, name });
  } catch (error) {
    refusal = `Could not create it: ${error instanceof Error ? error.message : String(error)}`;
  }
  if (prompt.value !== p) return; // cancelled meanwhile
  p.busy = false;
  if (refusal !== null) {
    p.error = refusal; // stays open with what the user typed
    return;
  }
  prompt.value = null;
  if (p.kind === "folder") restoreFocus();
  else promptOrigin = null; // a created note moves focus to the editor (the host)
}

const promptWhere = computed(() => (prompt.value && prompt.value.dir !== "" ? prompt.value.dir : "the vault root"));

function openMenu(path: string, x: number, y: number): void {
  select(path);
  menu.value = { path, x, y };
}

/** Esc returns focus to the tree; Tab moves on; a press outside leaves focus where that press put it. */
function closeMenu(reason: "escape" | "tab" | "outside"): void {
  menu.value = null;
  if (reason === "escape") focusTree();
}

/** WebView2 fires a native `contextmenu` on key-up of Menu / Shift-F10; with no row under it, open ours for
 * the focused row. (Row right-clicks are handled by the row, which prevents default, so skip those.)
 * Windows manual check: #77, see `nativeMenu.ts` for the full list. */
function onTreeContextMenu(event: MouseEvent): void {
  if (event.defaultPrevented) return;
  event.preventDefault();
  const path = focusedPath.value;
  if (path === null) return;
  const el = container.value?.querySelector<HTMLElement>(`[data-path="${CSS.escape(path)}"]`);
  const box = el?.getBoundingClientRect();
  openMenu(path, (box?.left ?? event.clientX) + 24, box?.bottom ?? event.clientY);
}

const menuItems = computed<MenuItem[]>(() => {
  const m = menu.value;
  if (!m) return [];
  const row = rows.value.find((r) => r.path === m.path);
  const items: MenuItem[] = [];
  if (row && row.kind !== "dir") {
    const can = props.openPath !== null;
    items.push({ id: "insert-link", label: "Insert link", enabled: can, reason: "Open a note first to insert a link." });
  }
  items.push(
    {
      id: "new-note",
      label: "New note here",
      enabled: !!props.createEntry,
      reason: "Creating notes is not available right now.",
    },
    { id: "new-folder", label: "New folder here", enabled: !!props.createEntry, reason: "Creating folders is not available right now." },
  );
  const accelFor = (id: string): Pick<MenuItem, "accel" | "spoken"> => {
    const binding = (props.bindings ?? DEFAULT_MENU_BINDINGS)[id];
    return binding ? { accel: chordGlyphs(binding, isMac.value), spoken: chordSpoken(binding, isMac.value) } : {};
  };
  const later = "Arrives with link repair (P3, FR-VLT-09): moving or renaming without repairing links would break them.";
  items.push(
    { id: "rename", label: "Rename", enabled: false, reason: later },
    { id: "move", label: "Move to\u2026", enabled: false, reason: later },
    { id: "trash", label: "Move to trash", enabled: false, reason: later },
  );
  return items.map((item) => ({ ...item, ...accelFor(item.id) }));
});

function onMenuSelect(id: string): void {
  const m = menu.value;
  menu.value = null;
  if (!m) return;
  const row = rows.value.find((r) => r.path === m.path);
  if (!row) return;
  const dir = row.kind === "dir" ? row.path : row.dir;
  if (id === "insert-link") {
    emit("insert-link", { path: row.path, text: insertLinkText(row, isNameUnique(model, row.name)) });
    focusTree();
  } else if (id === "new-note") startCreate("note", dir);
  else if (id === "new-folder") startCreate("folder", dir);
}

defineExpose({
  model,
  focusTree,
  focusFilter,
  version,
  filter,
  currentDir,
  newNote: () => startCreate("note"),
  newFolder: () => startCreate("folder"),
});
</script>

<template>
  <div class="serat-tree-pane" data-testid="file-tree-pane" @keydown="onPaneKeydown">
    <div class="serat-tree-toolbar" role="toolbar" aria-label="File tree actions">
      <!-- Disabled, not hidden (file-tree.md), only when the host gave no create command. -->
      <button
        type="button"
        class="serat-tree-tool"
        data-testid="tree-new-note"
        :disabled="!createEntry"
        :title="createEntry ? 'New note (\u2318N)' : 'Creating notes is not available right now'"
        @click="startCreate('note')"
      >
        New note
      </button>
      <button
        type="button"
        class="serat-tree-tool"
        data-testid="tree-new-folder"
        :disabled="!createEntry"
        :title="createEntry ? 'New folder (\u2318\u21E7N)' : 'Creating folders is not available right now'"
        @click="startCreate('folder')"
      >
        New folder
      </button>
      <input
        ref="filterInput"
        v-model="filter"
        type="search"
        class="serat-tree-filter"
        data-testid="tree-filter"
        placeholder="Filter"
        aria-label="Filter files"
        @keydown="onFilterKeydown"
      />
    </div>
    <form
      v-if="prompt"
      class="serat-tree-prompt"
      data-testid="tree-prompt"
      @submit.prevent="submitPrompt"
      @keydown.esc.stop.prevent="cancelPrompt"
    >
      <label for="serat-tree-prompt-input">New {{ prompt.kind }} in {{ promptWhere }}</label>
      <input
        id="serat-tree-prompt-input"
        ref="promptInput"
        v-model="prompt.value"
        type="text"
        autocomplete="off"
        spellcheck="false"
        data-testid="tree-prompt-input"
        :aria-invalid="prompt.error !== null"
        aria-describedby="serat-tree-prompt-error"
        @input="prompt.error = null"
      />
      <p v-if="prompt.error" id="serat-tree-prompt-error" class="serat-tree-prompt-error" role="alert" data-testid="tree-prompt-error">
        {{ prompt.error }}
      </p>
      <span class="serat-tree-prompt-actions">
        <button type="submit" :disabled="prompt.busy" data-testid="tree-prompt-create">Create</button>
        <button type="button" data-testid="tree-prompt-cancel" @click="cancelPrompt">Cancel</button>
      </span>
    </form>
    <TreeContextMenu
      v-if="menu"
      :items="menuItems"
      :x="menu.x"
      :y="menu.y"
      label="File actions"
      @select="onMenuSelect"
      @close="closeMenu"
    />
    <p
      class="serat-tree-live"
      :class="{ 'serat-tree-live-on': stopped }"
      role="status"
      aria-live="polite"
      data-testid="tree-status"
    >{{ stopped ? TREE_STOPPED_TEXT : "" }}</p>
    <div
      ref="container"
      class="serat-tree"
      role="tree"
      aria-label="Files"
      tabindex="0"
      data-testid="file-tree"
      :aria-activedescendant="activeDescendant"
      @scroll="onScroll"
      @keydown="onKeydown"
      @contextmenu="onTreeContextMenu"
    >
      <p v-if="rootEmpty" class="serat-tree-empty">Nothing here yet — press ⌘N to start a note.</p>
      <p v-else-if="noMatches" class="serat-tree-empty" data-testid="tree-no-matches">No files match “{{ filter.trim() }}”.</p>
      <div v-else class="serat-tree-spacer" :style="{ height: windowed.totalHeight + 'px' }">
        <div class="serat-tree-window" :style="{ transform: `translateY(${windowed.offsetTop}px)` }">
          <div
            v-for="row in visibleRows"
            :id="rowId(row.path)"
            :key="row.path"
            class="serat-tree-row"
            :class="[
              `serat-tree-${row.kind}`,
              `serat-tree-state-${row.state}`,
              { 'serat-tree-selected': row.path === focusedPath || row.path === openPath },
            ]"
            :style="{ height: rowHeight + 'px', paddingLeft: row.depth * 16 + 8 + 'px' }"
            role="treeitem"
            :aria-level="row.depth + 1"
            :aria-expanded="row.kind === 'dir' && (row.state === 'collapsed' || row.state === 'expanded') ? row.state === 'expanded' : undefined"
            :aria-selected="row.path === focusedPath"
            :aria-label="rowLabel(row)"
            :title="row.name"
            :draggable="row.kind !== 'dir'"
            :data-path="row.path"
            @click="onRowClick(row, $event)"
            @dragstart="onDragStart($event, row)"
            @contextmenu.prevent="openMenu(row.path, $event.clientX, $event.clientY)"
          >
            <span class="serat-tree-chevron" aria-hidden="true">{{
              row.state === "expanded" ? "▾" : row.state === "collapsed" ? "▸" : ""
            }}</span>
            <span class="serat-tree-glyph" :class="`serat-glyph-${glyph(row).cls}`" aria-hidden="true">{{
              glyph(row).text
            }}</span>
            <span class="serat-tree-name"
              >{{ nameParts(row).before
              }}<b v-if="nameParts(row).match" class="serat-tree-match">{{ nameParts(row).match }}</b
              >{{ nameParts(row).after }}</span
            >
            <span v-if="isDot(row)" class="serat-tree-dot" aria-hidden="true">●</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.serat-tree-pane {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}
.serat-tree-toolbar {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  padding: 4px 6px;
  border-bottom: 1px solid var(--serat-border-hairline, rgba(22, 25, 27, 0.12));
}
.serat-tree-filter {
  flex: 1 1 100%;
  min-width: 0;
}
.serat-tree-prompt {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 6px;
  border-bottom: 1px solid var(--serat-border-hairline, rgba(22, 25, 27, 0.12));
}
.serat-tree-prompt-error {
  margin: 0;
  color: var(--serat-rust, #b7410e);
}
.serat-tree-prompt-actions {
  display: flex;
  gap: 4px;
}
.serat-tree-live {
  margin: 0;
}
.serat-tree-live-on {
  padding: 0.5rem 0.75rem;
  color: var(--serat-text-secondary, #6b7075);
  border-bottom: 1px solid var(--serat-border-hairline, #bbb);
}
.serat-tree {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
  outline: none;
}
.serat-tree-spacer {
  position: relative;
}
.serat-tree-row {
  display: flex;
  align-items: center;
  gap: 4px;
  cursor: pointer;
  white-space: nowrap;
  border-left: 2px solid transparent;
}
.serat-tree-selected {
  border-left-color: var(--indigo, #24457e);
}
.serat-tree-name {
  overflow: hidden;
  text-overflow: ellipsis;
}
.serat-tree-match {
  color: var(--indigo, #24457e);
  font-weight: 700;
}
.serat-tree-chevron {
  width: 1em;
  flex: none;
}
.serat-tree-glyph {
  flex: none;
  width: 1.1em;
  text-align: center;
  color: var(--serat-text-secondary, #6b7075);
}
.serat-tree-state-pending {
  color: var(--serat-text-secondary, #6b7075);
}
.serat-tree-state-pending .serat-tree-glyph {
  outline: 1px dashed var(--serat-border-hairline, #bbb);
}
.serat-tree-state-pending .serat-tree-name {
  outline: none;
}
.serat-tree-dot {
  color: var(--brass, #c08a47);
  font-size: 0.7em;
}
.serat-tree-empty {
  padding: 0.75rem;
  color: var(--serat-text-secondary, #6b7075);
}
</style>
