<!--
  Quick switcher (quick-switcher.md; FR-NAV-01, NFR-PRF-06, US-04; ADR-0015;
  architecture.md §9.2).
  - Every keystroke sends `switcher_query` with the next `seq` immediately: no
    debounce, no work on the input path beyond one call. Only the newest
    answer paints; `discarded` or older answers are ignored silently
    (createSwitcherQuerier).
  - The core ranks, bolds (highlight spans) and decides the corpus; this only
    renders. History is recorded by the core when the note opens in the editor
    (`note_opened`), so this component does not report anything.
  - Combobox input + listbox/option; `aria-activedescendant` tracks the
    selection and DOM focus stays on the input.
-->
<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";

import type { SwitcherItem } from "@/ipc/generated/SwitcherItem";
import type { SwitcherQueryArgs } from "@/ipc/generated/SwitcherQueryArgs";
import { defaultLinkIpc } from "@/ipc/linkAdapter";
import { createSwitcherQuerier, type SwitcherQueryFn } from "@/ipc/switcherQuery";
import { splitBySpans } from "@/overlays/fuzzy";
import { folderOf, titleOf } from "@/panes/text";

const props = withDefaults(
  defineProps<{
    query?: SwitcherQueryFn;
  }>(),
  { query: (args: SwitcherQueryArgs) => defaultLinkIpc.switcherQuery(args) },
);

const emit = defineEmits<{
  open: [payload: { path: string; newTab: boolean; heading: { text: string; byteStart: number } | null }];
  create: [payload: { name: string }];
  close: [];
}>();

/** Empty query shows the most frecent notes; the contract caps a page at 50 and the spec shows 20. */
const RECENT_LIMIT = 20;
const QUERY_LIMIT = 50;

const querier = createSwitcherQuerier((args) => props.query(args));
const text = ref("");
const items = ref<SwitcherItem[]>([]);
/** The query the visible `items` answer; null until the first answer arrives. */
const shown = ref<string | null>(null);
const selected = ref(0);
const input = ref<HTMLInputElement | null>(null);
const card = ref<HTMLElement | null>(null);
let origin: HTMLElement | null = null;
let accepted = false;
let alive = true;

const createName = computed(() => {
  const q = text.value.trim();
  return shown.value !== null && shown.value === text.value && items.value.length === 0 && q !== "" && !q.includes("#")
    ? q
    : null;
});
/** Rows in the listbox: results, or the single create row on no match. */
const rowCount = computed(() => (createName.value !== null ? 1 : items.value.length));

async function ask(query: string): Promise<void> {
  let results;
  try {
    results = await querier.run(query, query === "" ? RECENT_LIMIT : QUERY_LIMIT);
  } catch {
    return; // transport failure: keep what is shown; never throw into typing
  }
  if (!alive || results === null) return; // superseded or discarded: ignore, do not paint
  items.value = results.items;
  shown.value = query;
  selected.value = 0;
}

function onInput(event: Event): void {
  text.value = (event.target as HTMLInputElement).value;
  void ask(text.value);
}

function main(item: SwitcherItem): { pieces: Array<{ text: string; match: boolean }>; lead: string } {
  const title = titleOf(item.path, item.title);
  switch (item.matched) {
    case "heading":
      return { lead: `${title} › `, pieces: splitBySpans(item.candidate, item.highlight) };
    case "alias":
      return { lead: "", pieces: [{ text: title, match: false }] };
    default:
      return { lead: "", pieces: splitBySpans(item.candidate, item.highlight) };
  }
}

function meta(item: SwitcherItem): string {
  if (item.matched === "heading") return "";
  if (item.matched === "alias") return "";
  return folderOf(item.path);
}

function ariaName(item: SwitcherItem): string {
  const title = titleOf(item.path, item.title);
  switch (item.matched) {
    case "heading":
      return `${title} › ${item.candidate}`;
    case "alias":
      return `${title}, alias ${item.candidate}`;
    default: {
      const folder = folderOf(item.path);
      return folder ? `${item.candidate}, in ${folder}` : item.candidate;
    }
  }
}

function accept(index: number, newTab: boolean): void {
  if (createName.value !== null) {
    accepted = true;
    emit("create", { name: createName.value });
    return;
  }
  const item = items.value[index];
  if (!item) return;
  accepted = true;
  emit("open", {
    path: item.path,
    newTab,
    heading: item.matched === "heading" && item.heading ? { text: item.heading.text, byteStart: item.heading.byte_start } : null,
  });
}

function onKeydown(event: KeyboardEvent): void {
  const n = rowCount.value;
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    if (n === 0) return;
    selected.value = (selected.value + (event.key === "ArrowDown" ? 1 : -1) + n) % n;
    void nextTick(() => document.getElementById(`qs-opt-${selected.value}`)?.scrollIntoView?.({ block: "nearest" }));
  } else if (event.key === "Enter") {
    event.preventDefault();
    accept(selected.value, event.metaKey || event.ctrlKey);
  } else if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    emit("close");
  }
}

/** Overlays close on focus loss to a non-overlay element (components.md §5). */
function onFocusout(event: FocusEvent): void {
  const next = event.relatedTarget as Node | null;
  if (next && !card.value?.contains(next)) emit("close");
}

onMounted(() => {
  origin = (document.activeElement as HTMLElement | null) ?? null;
  input.value?.focus();
  void ask(""); // empty query: the most frecent notes
});

onBeforeUnmount(() => {
  alive = false;
  // Esc / blur return focus to where the switcher was opened; accepting a note
  // leaves focus to the host (it focuses the editor).
  if (!accepted && origin && origin.isConnected) origin.focus();
});
</script>

<template>
  <div class="overlay-scrim" data-testid="quick-switcher" @mousedown.self="emit('close')">
    <div ref="card" class="overlay-card" @focusout="onFocusout">
      <input
        ref="input"
        class="overlay-input"
        type="text"
        role="combobox"
        aria-label="Switch to note"
        aria-autocomplete="list"
        aria-expanded="true"
        aria-controls="qs-list"
        :aria-activedescendant="rowCount > 0 ? `qs-opt-${selected}` : undefined"
        autocomplete="off"
        spellcheck="false"
        :value="text"
        @input="onInput"
        @keydown="onKeydown"
      />
      <ul id="qs-list" class="overlay-list" role="listbox" aria-label="Notes">
        <template v-if="createName !== null">
          <li
            id="qs-opt-0"
            class="overlay-row"
            role="option"
            aria-selected="true"
            :aria-label="`Create note ${createName}`"
            @click="accept(0, false)"
          >
            <span class="main">Create note “{{ createName }}”</span>
          </li>
        </template>
        <template v-else>
          <li
            v-for="(item, i) in items"
            :id="`qs-opt-${i}`"
            :key="`${item.path}#${item.matched}#${item.candidate}`"
            class="overlay-row"
            role="option"
            :aria-selected="i === selected"
            :aria-label="ariaName(item)"
            @click="accept(i, false)"
            @mousemove="selected = i"
          >
            <span class="glyph" aria-hidden="true">{{ item.matched === "heading" ? "#" : "≡" }}</span>
            <span class="main" aria-hidden="true"
              >{{ main(item).lead
              }}<template v-for="(piece, k) in main(item).pieces" :key="k"
                ><span v-if="piece.match" class="hit">{{ piece.text }}</span
                ><template v-else>{{ piece.text }}</template></template
              ></span
            >
            <span v-if="item.matched === 'alias'" class="meta" aria-hidden="true"
              >Alias:
              <template v-for="(piece, k) in splitBySpans(item.candidate, item.highlight)" :key="k"
                ><span v-if="piece.match" class="hit">{{ piece.text }}</span
                ><template v-else>{{ piece.text }}</template></template
              ></span
            >
            <span v-else-if="meta(item)" class="meta" aria-hidden="true">{{ meta(item) }}</span>
          </li>
        </template>
      </ul>
      <p v-if="shown !== null && shown === text && items.length === 0 && text.trim() !== '' && createName === null" class="overlay-empty">
        No note matches “{{ text }}”
      </p>
      <div class="overlay-footer" aria-hidden="true">
        <span>↑↓ navigate</span><span>↵ open</span><span>⌘↵ new tab</span><span>esc</span>
      </div>
      <button type="button" class="sr-only" tabindex="-1" @click="accept(selected, false)">Open selected note</button>
      <button type="button" class="sr-only" tabindex="-1" @click="accept(selected, true)">Open selected note in new tab</button>
      <button type="button" class="sr-only" tabindex="-1" @click="emit('close')">Close switcher</button>
    </div>
  </div>
</template>

<style src="./overlay.css"></style>
