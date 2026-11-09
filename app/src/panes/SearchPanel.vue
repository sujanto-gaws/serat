<!--
  Search panel (search-panel.md; FR-NAV-02 P1 half, US-05; architecture.md §4.5).
  - Runs on Enter and after 250 ms of idle typing; nothing is sent while typing.
  - An unbalanced quote or a second `[key:value]` is refused here, before any
    IPC call, with a message; the characters are never swallowed.
  - A not-`Ready` index replaces the results entirely ("Building the index"),
    and the pending query re-runs when the index changes.
  - Snippets are split on the match delimiters into text nodes; never innerHTML.
-->
<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { defaultPaneIpc, type PaneIpcAdapter } from "@/ipc/paneAdapter";
import type { BlockLocation } from "@/ipc/generated/BlockLocation";
import type { SearchHit } from "@/ipc/generated/SearchHit";
import { CASE_SENSITIVE_HINT, checkQuery, SECOND_FILTER_MESSAGE, UNBALANCED_QUOTE_MESSAGE } from "@/panes/searchQuery";
import { folderOf, plainSnippet, splitSnippet, titleOf } from "@/panes/text";

const PAGE = 50;

const props = withDefaults(
  defineProps<{
    ipc?: PaneIpcAdapter;
    /** Idle time before a search runs without Enter (§4.5: 250 ms). */
    settleMs?: number;
    /** A wait longer than this shows skeleton rows. */
    skeletonAfterMs?: number;
  }>(),
  { ipc: () => defaultPaneIpc, settleMs: 250, skeletonAfterMs: 150 },
);

const emit = defineEmits<{
  open: [payload: { path: string; block: BlockLocation | null; newTab: boolean }];
  close: [];
}>();

type View =
  | { kind: "idle" }
  | { kind: "waiting" }
  | { kind: "results"; query: string; filter: string | null }
  | { kind: "not-ready" }
  | { kind: "error" };

const input = ref("");
const view = ref<View>({ kind: "idle" });
const hits = ref<SearchHit[]>([]);
const hasMore = ref(false);
const showSkeleton = ref(false);
const recent = ref<string[]>([]);
const focusedIndex = ref(0);
/** The query waiting for the index to become `Ready`. */
let pendingQuery: string | null = null;
let settleTimer: ReturnType<typeof setTimeout> | null = null;
let skeletonTimer: ReturnType<typeof setTimeout> | null = null;
let seq = 0;
let unlisten: (() => void) | null = null;

const check = computed(() => checkQuery(input.value));
const filterChip = computed(() => (check.value.kind === "ok" ? check.value.filter : null));
const refusal = computed(() => {
  const c = check.value;
  if (c.kind === "unbalanced_quote") return UNBALANCED_QUOTE_MESSAGE;
  if (c.kind === "second_filter") return SECOND_FILTER_MESSAGE(c.first);
  return null;
});

function clearTimers(): void {
  if (settleTimer) clearTimeout(settleTimer);
  if (skeletonTimer) clearTimeout(skeletonTimer);
  settleTimer = null;
  skeletonTimer = null;
}

/** Typing only schedules; the call is made 250 ms after the last keystroke. */
watch(input, () => {
  if (settleTimer) clearTimeout(settleTimer);
  // Setting the input from code that already ran this query (a recent search)
  // must not schedule it a second time.
  if (input.value.trim() === lastQuery) return;
  settleTimer = setTimeout(() => void run(false), props.settleMs);
});
let lastQuery: string | null = null;

async function run(more: boolean): Promise<void> {
  if (settleTimer) {
    clearTimeout(settleTimer);
    settleTimer = null;
  }
  const c = check.value;
  if (c.kind === "empty") {
    seq += 1;
    view.value = { kind: "idle" };
    hits.value = [];
    hasMore.value = false;
    return;
  }
  if (c.kind !== "ok") return; // refused before sending; the message is already showing
  const query = input.value.trim();
  if (!more) lastQuery = query;
  const mine = ++seq;
  const offset = more ? hits.value.length : 0;
  if (!more) {
    skeletonTimer = setTimeout(() => {
      if (mine === seq) showSkeleton.value = true;
    }, props.skeletonAfterMs);
    view.value = { kind: "waiting" };
  }
  let outcome;
  try {
    outcome = await props.ipc.search({ query, offset, limit: PAGE });
  } catch {
    if (mine === seq) view.value = { kind: "error" };
    return;
  } finally {
    if (skeletonTimer) clearTimeout(skeletonTimer);
    skeletonTimer = null;
    showSkeleton.value = false;
  }
  if (mine !== seq) return; // a newer query won
  if (outcome.kind === "not_ready") {
    // No partial results are ever shown; the query re-runs when the index is Ready.
    pendingQuery = query;
    hits.value = [];
    hasMore.value = false;
    view.value = { kind: "not-ready" };
    return;
  }
  pendingQuery = null;
  const value = outcome.value;
  if (value.kind === "hits") {
    hits.value = more ? [...hits.value, ...value.items] : value.items;
    hasMore.value = value.items.length === PAGE;
    view.value = { kind: "results", query, filter: c.filter };
    if (!more) {
      focusedIndex.value = 0;
      recent.value = [query, ...recent.value.filter((q) => q !== query)].slice(0, 5);
    }
  } else if (value.kind === "too_many_property_terms") {
    hits.value = [];
    view.value = { kind: "results", query, filter: c.filter };
    refusalFromBackend.value = true;
  } else {
    // nothing_to_search / unbalanced_quote answered anyway: an ordinary empty result.
    hits.value = [];
    hasMore.value = false;
    view.value = { kind: "results", query, filter: c.filter };
  }
}
const refusalFromBackend = ref(false);
watch(input, () => {
  refusalFromBackend.value = false;
});

onMounted(() => {
  props.ipc
    .onIndexChanged(() => {
      // The index changed: a query that was waiting for Ready runs now.
      if (pendingQuery !== null) void run(false);
    })
    .then((off) => {
      unlisten = off;
    })
    .catch(() => undefined);
});
onBeforeUnmount(() => {
  clearTimers();
  seq += 1;
  unlisten?.();
});

function removeFilter(): void {
  const chip = filterChip.value;
  if (chip) input.value = input.value.replace(chip, "").replace(/\s+/g, " ").trim();
}

function openHit(hit: SearchHit, newTab: boolean): void {
  emit("open", { path: hit.path, block: hit.block, newTab });
}

function onResultsKeydown(event: KeyboardEvent): void {
  if (hits.value.length === 0) return;
  if (event.key === "ArrowDown") focusedIndex.value = Math.min(hits.value.length - 1, focusedIndex.value + 1);
  else if (event.key === "ArrowUp") focusedIndex.value = Math.max(0, focusedIndex.value - 1);
  else if (event.key === "Enter") openHit(hits.value[focusedIndex.value], event.ctrlKey || event.metaKey);
  else return;
  event.preventDefault();
}

function onInputKeydown(event: KeyboardEvent): void {
  if (event.key === "Enter") {
    event.preventDefault();
    void run(false);
  } else if (event.key === "Escape") {
    emit("close");
  } else if (event.key === "Backspace" && filterChip.value && input.value.trimEnd().endsWith(filterChip.value)) {
    event.preventDefault();
    removeFilter();
  }
}

function useRecent(query: string): void {
  input.value = query;
  void run(false);
}

const noResultsMessage = computed(() => {
  const v = view.value;
  if (v.kind !== "results") return "";
  return v.filter
    ? `No matches for “${v.query}” — try removing the ${v.filter} filter`
    : `No matches for “${v.query}”`;
});

defineExpose({ input, run });
</script>

<template>
  <section class="serat-search" aria-label="Search" data-testid="search-panel">
    <div class="serat-search-bar">
      <input
        v-model="input"
        type="search"
        class="serat-search-input"
        data-testid="search-input"
        placeholder="Search notes"
        aria-label="Search notes"
        @keydown="onInputKeydown"
      />
      <button
        v-if="filterChip"
        type="button"
        class="serat-search-chip"
        data-testid="search-chip"
        :aria-label="`Filter: ${filterChip.slice(1, -1).replace(':', ' is ')}. Press Backspace or activate to remove.`"
        @click="removeFilter"
      >
        {{ filterChip }} ×
      </button>
    </div>
    <p v-if="filterChip" class="serat-search-hint" data-testid="search-hint">{{ CASE_SENSITIVE_HINT }}</p>
    <p v-if="refusal" class="serat-search-refusal" role="status" aria-live="polite" data-testid="search-refusal">
      {{ refusal }}
    </p>
    <p v-else-if="refusalFromBackend" class="serat-search-refusal" role="status" aria-live="polite">
      {{ SECOND_FILTER_MESSAGE(filterChip ?? "[filter]") }}
    </p>

    <div v-if="view.kind === 'idle'" class="serat-search-empty" data-testid="search-idle">
      <template v-if="recent.length === 0">Search note text, or narrow with <code>[key:value]</code>.</template>
      <ul v-else>
        <li v-for="q in recent" :key="q"><button type="button" @click="useRecent(q)">{{ q }}</button></li>
      </ul>
    </div>
    <div v-else-if="view.kind === 'waiting'">
      <div v-if="showSkeleton" class="serat-search-skeleton" data-testid="search-loading" aria-hidden="true">
        <div class="serat-skeleton-row"></div>
        <div class="serat-skeleton-row"></div>
      </div>
    </div>
    <p v-else-if="view.kind === 'not-ready'" class="serat-search-note" data-testid="search-not-ready">
      Building the index — search available shortly
    </p>
    <p v-else-if="view.kind === 'error'" class="serat-search-note">Search could not be run.</p>
    <div v-else-if="view.kind === 'results'">
      <p v-if="hits.length === 0 && !refusalFromBackend" class="serat-search-note" data-testid="search-none">
        {{ noResultsMessage }}
      </p>
      <template v-else>
        <p class="serat-search-count">{{ hits.length }}{{ hasMore ? "+" : "" }} result{{ hits.length === 1 ? "" : "s" }}</p>
        <div role="list" tabindex="0" data-testid="search-results" @keydown="onResultsKeydown">
          <div
            v-for="(hit, i) in hits"
            :key="hit.path + ':' + (hit.block ? hit.block.block_id : i)"
            role="listitem"
            class="serat-hit"
            :class="{ 'serat-hit-focused': i === focusedIndex }"
            :aria-label="titleOf(hit.path, hit.title) + (hit.snippet ? ': ' + plainSnippet(hit.snippet) : '')"
            @click="openHit(hit, false)"
          >
            <div class="serat-hit-head">
              <span class="serat-hit-title">{{ titleOf(hit.path, hit.title) }}</span>
              <span v-if="hit.kind === 'asset'" class="serat-hit-kind">{{ hit.mime ?? "file" }}</span>
              <span class="serat-hit-folder">{{ folderOf(hit.path) }}</span>
            </div>
            <div v-if="hit.snippet" class="serat-hit-snippet">
              <template v-for="(seg, k) in splitSnippet(hit.snippet)" :key="k"
                ><b v-if="seg.match">{{ seg.text }}</b
                ><template v-else>{{ seg.text }}</template></template
              >
            </div>
            <div v-else-if="view.filter" class="serat-hit-field">{{ view.filter.slice(1, -1).replace(':', ': ') }}</div>
          </div>
        </div>
        <button v-if="hasMore" type="button" class="serat-search-more" data-testid="search-more" @click="run(true)">
          Load more
        </button>
      </template>
    </div>
  </section>
</template>

<style scoped>
.serat-search {
  padding: 0.5rem;
}
.serat-search-input {
  width: 100%;
}
.serat-search-chip {
  font-family: ui-monospace, monospace;
  color: var(--brass, #c08a47);
}
.serat-search-hint,
.serat-search-note {
  color: var(--serat-text-secondary, #6b7075);
}
.serat-hit {
  cursor: pointer;
  padding: 4px 6px;
}
.serat-hit-focused {
  background: rgba(36, 69, 126, 0.12);
}
.serat-hit-folder,
.serat-hit-kind {
  margin-left: 0.5em;
  font-size: 11px;
  color: var(--serat-text-tertiary, #8a8f94);
}
.serat-hit-snippet {
  font-family: Georgia, serif;
  font-size: 13px;
}
.serat-hit-snippet b {
  color: var(--indigo, #24457e);
}
.serat-hit-field {
  font-family: ui-monospace, monospace;
  font-size: 11px;
}
.serat-skeleton-row {
  height: 32px;
  margin: 4px 0;
  opacity: 0.4;
  background: currentColor;
}
</style>
