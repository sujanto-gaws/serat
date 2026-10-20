<!--
  Backlinks pane (backlinks-pane.md; FR-NAV-04, US-03; architecture.md §4.5 R2).
  One entry per distinct (source note, source block), grouped by source note
  for display only. Excerpts are plain text nodes (never innerHTML). The
  read is made when the open note changes and when `index://changed`
  invalidates it; never per keystroke. A not-`Ready` index is stated, not
  papered over.
-->
<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";

import { defaultPaneIpc, type PaneIpcAdapter } from "@/ipc/paneAdapter";
import type { Backlink } from "@/ipc/generated/Backlink";
import type { BlockLocation } from "@/ipc/generated/BlockLocation";
import type { IndexState } from "@/ipc/generated/IndexState";
import { splitSnippet, titleOf } from "@/panes/text";

const props = withDefaults(
  defineProps<{
    /** Vault-relative path of the note whose backlinks to show, or null. */
    path: string | null;
    ipc?: PaneIpcAdapter;
    /** Shown instead of the list when the open note was deleted. */
    missing?: boolean;
    /** Skeleton rows are held at least this long so a fast answer does not flash. */
    minLoadingMs?: number;
  }>(),
  { ipc: () => defaultPaneIpc, missing: false, minLoadingMs: 100 },
);

const emit = defineEmits<{
  open: [payload: { path: string; block: BlockLocation | null; newTab: boolean }];
}>();

type PaneState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ready"; groups: Array<{ source: string; title: string; entries: Backlink[] }>; count: number }
  | { kind: "empty" }
  | { kind: "not-ready"; state: IndexState }
  | { kind: "error" };

const state = ref<PaneState>({ kind: "idle" });
const focusedIndex = ref(0);
let token = 0;
let unlisten: (() => void) | null = null;

function group(items: Backlink[]): Array<{ source: string; title: string; entries: Backlink[] }> {
  const order: string[] = [];
  const bySource = new Map<string, { source: string; title: string; entries: Backlink[] }>();
  for (const item of items) {
    let g = bySource.get(item.source_path);
    if (!g) {
      g = { source: item.source_path, title: titleOf(item.source_path, item.source_title), entries: [] };
      bySource.set(item.source_path, g);
      order.push(item.source_path);
    }
    g.entries.push(item);
  }
  return order.map((k) => bySource.get(k) as { source: string; title: string; entries: Backlink[] });
}

async function load(): Promise<void> {
  const mine = ++token;
  const path = props.path;
  if (!path || props.missing) {
    state.value = { kind: "idle" };
    return;
  }
  state.value = { kind: "loading" };
  const floor = new Promise<void>((resolve) => setTimeout(resolve, props.minLoadingMs));
  let result;
  try {
    [result] = await Promise.all([props.ipc.backlinks({ path }), floor]);
  } catch {
    if (mine === token) state.value = { kind: "error" };
    return;
  }
  if (mine !== token) return; // a newer load superseded this one
  if (result.kind === "not_ready") {
    state.value = { kind: "not-ready", state: result.state };
  } else if (result.value.length === 0) {
    state.value = { kind: "empty" };
  } else {
    state.value = { kind: "ready", groups: group(result.value), count: result.value.length };
  }
  focusedIndex.value = 0;
}

watch(() => [props.path, props.missing], () => void load());

onMounted(() => {
  void load();
  props.ipc
    .onIndexChanged(() => void load())
    .then((off) => {
      unlisten = off;
    })
    .catch(() => undefined);
});
onBeforeUnmount(() => {
  token += 1;
  unlisten?.();
});

function flatEntries(): Backlink[] {
  return state.value.kind === "ready" ? state.value.groups.flatMap((g) => g.entries) : [];
}

function openEntry(entry: Backlink, newTab: boolean): void {
  emit("open", { path: entry.source_path, block: entry.block, newTab });
}

function onKeydown(event: KeyboardEvent): void {
  const entries = flatEntries();
  if (entries.length === 0) return;
  if (event.key === "ArrowDown") {
    focusedIndex.value = Math.min(entries.length - 1, focusedIndex.value + 1);
  } else if (event.key === "ArrowUp") {
    focusedIndex.value = Math.max(0, focusedIndex.value - 1);
  } else if (event.key === "Enter") {
    openEntry(entries[focusedIndex.value], event.ctrlKey || event.metaKey);
  } else {
    return;
  }
  event.preventDefault();
}

function indexOfEntry(entry: Backlink): number {
  return flatEntries().indexOf(entry);
}

const total = (): number => (state.value.kind === "ready" ? state.value.count : 0);
</script>

<template>
  <section class="serat-backlinks" aria-label="Backlinks" data-testid="backlinks" tabindex="0" @keydown="onKeydown">
    <h3>Backlinks<span v-if="state.kind === 'ready'"> ({{ total() }})</span></h3>
    <p v-if="missing" class="serat-backlinks-note">This note no longer exists</p>
    <div v-else-if="state.kind === 'loading'" class="serat-backlinks-skeleton" data-testid="backlinks-loading" aria-hidden="true">
      <div class="serat-skeleton-row"></div>
      <div class="serat-skeleton-row"></div>
    </div>
    <p v-else-if="state.kind === 'empty'" class="serat-backlinks-note">Nothing links here yet.</p>
    <p v-else-if="state.kind === 'not-ready'" class="serat-backlinks-note" data-testid="backlinks-not-ready">
      Building the index — backlinks available shortly.
    </p>
    <p v-else-if="state.kind === 'error'" class="serat-backlinks-note">Backlinks could not be read.</p>
    <div v-else-if="state.kind === 'ready'" role="list">
      <template v-for="g in state.groups" :key="g.source">
        <h4 class="serat-backlinks-source">{{ g.title }}</h4>
        <div
          v-for="(entry, i) in g.entries"
          :key="g.source + ':' + (entry.block ? entry.block.block_id : 'fm') + ':' + i"
          role="listitem"
          class="serat-backlink"
          :class="{ 'serat-backlink-focused': indexOfEntry(entry) === focusedIndex }"
          :aria-label="g.title + ': ' + (entry.block ? entry.excerpt : 'frontmatter')"
          @click="openEntry(entry, false)"
        >
          <span v-if="!entry.block" class="serat-backlink-frontmatter">frontmatter</span>
          <span v-else class="serat-backlink-excerpt"
            ><template v-for="(seg, k) in splitSnippet(entry.excerpt)" :key="k">{{ seg.text }}</template></span
          >
        </div>
      </template>
    </div>
  </section>
</template>

<style scoped>
.serat-backlinks {
  outline: none;
  padding: 0.5rem;
}
.serat-backlink {
  cursor: pointer;
  padding: 4px 6px;
  font-family: Georgia, serif;
  font-size: 13px;
}
.serat-backlink-focused {
  background: rgba(36, 69, 126, 0.12);
}
.serat-backlink-frontmatter {
  font-family: ui-monospace, monospace;
  font-size: 11px;
  color: var(--serat-text-tertiary, #8a8f94);
}
.serat-backlinks-source {
  margin: 0.5rem 0 0;
  font-size: 12px;
  color: var(--serat-text-tertiary, #8a8f94);
}
.serat-skeleton-row {
  height: 32px;
  margin: 4px 0;
  opacity: 0.4;
  background: currentColor;
}
.serat-backlinks-note {
  color: var(--serat-text-secondary, #6b7075);
}
</style>
