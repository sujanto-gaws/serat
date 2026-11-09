<!--
  The file tree pane (file-tree.md; FR-VLT-01, FR-VLT-08, FR-BIN-03; ADR-0015).

  - Messages from `open_tree` are folded into a plain (non-reactive) `TreeModel`
    in arrival order, synchronously, one call per message: nothing is batched
    away or dropped. Vue sees only a version counter.
  - Virtualised: the DOM holds the rows in the viewport plus a small overscan,
    whatever the vault size.
  - `mark_tree`: `first_screen` after the root listing has rendered and a
    double requestAnimationFrame has passed; `complete` the same way after
    `Complete` has been applied. Both are taken from a `flush: 'post'` watcher,
    i.e. after the commit that rendered those rows (ADR-0015 §3).
  - Keyboard: roving focus on the `role="tree"` container; one meaning per key.
-->
<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { defaultPaneIpc, type PaneIpcAdapter } from "@/ipc/paneAdapter";
import { insertLinkText, isNameUnique, navigate, visibleWindow } from "@/tree/treeLogic";
import { TreeModel, type TreeRow } from "@/tree/treeModel";

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
  }>(),
  { ipc: () => defaultPaneIpc, rowHeight: 26, fallbackViewportHeight: 600, openPath: null, openDirty: false },
);

const emit = defineEmits<{
  open: [payload: { path: string; newTab: boolean; keepFocus: boolean }];
  "insert-link": [payload: { path: string; text: string }];
  "context-menu": [payload: { path: string }];
  rename: [payload: { path: string }];
}>();

const model = new TreeModel();
const version = ref(0);
const scrollTop = ref(0);
const viewportHeight = ref(props.fallbackViewportHeight);
const stopped = ref(false);
let unlistenStatus: (() => void) | null = null;
const focusedPath = ref<string | null>(null);
const container = ref<HTMLDivElement | null>(null);
let resizeObserver: ResizeObserver | null = null;
let firstScreenMarked = false;
let completeMarked = false;

const rows = computed<readonly TreeRow[]>(() => {
  void version.value;
  return model.rows();
});

const windowed = computed(() =>
  visibleWindow(scrollTop.value, viewportHeight.value, props.rowHeight, rows.value.length),
);
const visibleRows = computed(() => rows.value.slice(windowed.value.start, windowed.value.end));
const rootEmpty = computed(() => {
  void version.value;
  return model.rootListed && rows.value.length === 0;
});

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
  },
  { flush: "post" },
);

onMounted(() => {
  measure();
  if (typeof ResizeObserver !== "undefined" && container.value) {
    resizeObserver = new ResizeObserver(measure);
    resizeObserver.observe(container.value);
  }
  void props.ipc
    .onTreeStatus?.((status) => {
      if (status.stopped) stopped.value = true;
    })
    .then((un) => {
      unlistenStatus = un;
    })
    .catch(() => undefined);
  void props.ipc
    .openTree((message) => {
      model.apply(message); // strictly in arrival order, never dropped
      bump();
    })
    .catch(() => undefined);
});

onBeforeUnmount(() => {
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

function displayName(row: TreeRow): string {
  return row.kind === "note" ? row.name.replace(/\.md$/i, "") : row.name;
}

function isDot(row: TreeRow): boolean {
  if (row.kind === "note") return props.openDirty && props.openPath === row.path;
  return row.kind === "asset" && row.sidecar;
}

function select(path: string): void {
  focusedPath.value = path;
}

function onRowClick(row: TreeRow): void {
  select(row.path);
  if (row.kind === "dir") {
    model.toggle(row.path);
    bump();
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
      model.expand(action.path);
      bump();
      break;
    case "collapse":
      model.collapse(action.path);
      bump();
      break;
    case "open":
      emit("open", { path: action.path, newTab: action.newTab, keepFocus: action.keepFocus });
      break;
    case "rename":
      emit("rename", { path: action.path });
      break;
    case "context-menu":
      emit("context-menu", { path: action.path });
      break;
    case "insert-link": {
      const row = rows.value.find((r) => r.path === action.path);
      if (row) emit("insert-link", { path: row.path, text: insertLinkText(row, isNameUnique(model, row.name)) });
      break;
    }
  }
}

function focusTree(): void {
  void nextTick(() => container.value?.focus());
}

defineExpose({ model, focusTree, version });
</script>

<template>
  <div
    ref="container"
    class="serat-tree"
    role="tree"
    aria-label="Files"
    tabindex="0"
    data-testid="file-tree"
    @scroll="onScroll"
    @keydown="onKeydown"
  >
    <p v-if="stopped" class="serat-tree-stopped" role="status" data-testid="tree-stopped">
      File tree stopped updating; reopen the vault.
    </p>
    <p v-if="rootEmpty" class="serat-tree-empty">Nothing here yet — press ⌘N to start a note.</p>
    <div v-else class="serat-tree-spacer" :style="{ height: windowed.totalHeight + 'px' }">
      <div class="serat-tree-window" :style="{ transform: `translateY(${windowed.offsetTop}px)` }">
        <div
          v-for="row in visibleRows"
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
          @click="onRowClick(row)"
          @dragstart="onDragStart($event, row)"
          @contextmenu.prevent="emit('context-menu', { path: row.path })"
        >
          <span class="serat-tree-chevron" aria-hidden="true">{{
            row.state === "expanded" ? "▾" : row.state === "collapsed" ? "▸" : ""
          }}</span>
          <span class="serat-tree-name">{{ displayName(row) }}</span>
          <span v-if="isDot(row)" class="serat-tree-dot" aria-hidden="true">●</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.serat-tree {
  height: 100%;
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
.serat-tree-chevron {
  width: 1em;
  flex: none;
}
.serat-tree-state-pending {
  color: var(--serat-text-secondary, #6b7075);
}
.serat-tree-state-pending .serat-tree-name {
  outline: 1px dashed var(--serat-border-hairline, #bbb);
}
.serat-tree-dot {
  color: var(--brass, #c08a47);
  font-size: 0.7em;
}
.serat-tree-stopped {
  padding: 0.5rem 0.75rem;
  color: var(--serat-text-secondary, #6b7075);
  border-bottom: 1px solid var(--serat-border-hairline, #bbb);
}
.serat-tree-empty {
  padding: 0.75rem;
  color: var(--serat-text-secondary, #6b7075);
}
</style>
