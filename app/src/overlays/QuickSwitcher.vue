<!--
  Quick switcher (quick-switcher.md; FR-NAV-01, NFR-PRF-06, US-04; ADR-0015;
  architecture.md §9.2).
  - Every keystroke sends `switcher_query` with the next `seq` immediately: no
    debounce, no work on the input path beyond one call (IME composition is
    the one exception: it sends on compositionend). Only the newest answer
    paints; `discarded` or older answers are ignored silently
    (createSwitcherQuerier).
  - The core ranks, bolds (highlight spans) and decides the corpus; this only
    renders. History is recorded by the core when the note opens in the editor
    (`note_opened`), so this component does not report anything.
  - Combobox input + listbox/option; `aria-activedescendant` tracks the
    selection and DOM focus stays on the input.
  - On no match, a `Create note "xyz"` row (quick-switcher.md) runs `createNote` (write_file,
    FR-VLT-11) and shows the core's refusal inline; it is only offered when the host passes one.
-->
<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";

import type { SwitcherItem } from "@/ipc/generated/SwitcherItem";
import type { SwitcherQueryArgs } from "@/ipc/generated/SwitcherQueryArgs";
import { defaultLinkIpc } from "@/ipc/linkAdapter";
import { createSwitcherQuerier, type SwitcherQueryFn } from "@/ipc/switcherQuery";
import { splitBySpans, type Piece } from "@/overlays/fuzzy";
import { folderOf, titleOf } from "@/panes/text";

const props = withDefaults(
  defineProps<{
    query?: SwitcherQueryFn;
    /** Creates the note; resolves to `null` or the refusal sentence. Absent: no create row. */
    createNote?: (name: string) => Promise<string | null>;
  }>(),
  { query: (args: SwitcherQueryArgs) => defaultLinkIpc.switcherQuery(args) },
);

const emit = defineEmits<{
  open: [payload: { path: string; newTab: boolean; heading: { text: string; byteStart: number } | null }];
  close: [];
}>();

/** Empty query: the core answers with the most frecent notes (most recent by mtime without history). */
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
let restoreFocus = false;
let alive = true;

interface Row {
  item: SwitcherItem;
  key: string;
  glyph: string;
  lead: string;
  pieces: Piece[];
  aliasPieces: Piece[] | null;
  meta: string;
  label: string;
}

function toRow(item: SwitcherItem): Row {
  const title = titleOf(item.path, item.title);
  const folder = folderOf(item.path);
  const base = { item, key: `${item.path}#${item.matched}#${item.candidate}` };
  switch (item.matched) {
    case "heading":
      return { ...base, glyph: "#", lead: `${title} › `, pieces: splitBySpans(item.candidate, item.highlight), aliasPieces: null, meta: "", label: `${title} › ${item.candidate}` };
    case "alias":
      return {
        ...base,
        glyph: "≡",
        lead: "",
        pieces: [{ text: title, match: false }],
        aliasPieces: splitBySpans(item.candidate, item.highlight),
        meta: "",
        label: `${title}, alias ${item.candidate}`,
      };
    default:
      return {
        ...base,
        glyph: "≡",
        lead: "",
        pieces: splitBySpans(item.candidate, item.highlight),
        aliasPieces: null,
        meta: folder,
        label: folder ? `${item.candidate}, in ${folder}` : item.candidate,
      };
  }
}

/** Computed once per answer, not once per render of each row. */
const rows = computed(() => items.value.map(toRow));
const createError = ref<string | null>(null);
let creating = false;
const noMatch = computed(() => shown.value !== null && shown.value === text.value && items.value.length === 0 && text.value.trim() !== "");

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

let composing = false;
function onInput(event: Event): void {
  text.value = (event.target as HTMLInputElement).value;
  createError.value = null;
  if (composing || (event as InputEvent).isComposing) return; // IME: wait for compositionend
  void ask(text.value);
}
function onCompositionStart(): void {
  composing = true;
}
function onCompositionEnd(event: Event): void {
  composing = false;
  text.value = (event.target as HTMLInputElement).value;
  void ask(text.value);
}

/** `Create note "xyz"`: offered only on a settled no-match, when the host can create. */
const createRow = computed(() => (props.createNote && noMatch.value ? text.value.trim() : null));

async function create(): Promise<void> {
  const name = createRow.value;
  if (!name || !props.createNote || creating) return;
  creating = true;
  createError.value = null;
  try {
    const refusal = await props.createNote(name);
    if (!alive) return;
    if (refusal === null) emit("close");
    else createError.value = refusal;
  } finally {
    creating = false;
  }
}

function accept(index: number, newTab: boolean): void {
  const item = items.value[index];
  if (!item) return;
  emit("open", {
    path: item.path,
    newTab,
    heading: item.matched === "heading" && item.heading ? { text: item.heading.text, byteStart: item.heading.byte_start } : null,
  });
}

/** Esc or a click on the scrim: close and hand focus back to where the switcher was opened. */
function dismiss(): void {
  restoreFocus = true;
  emit("close");
}

function onKeydown(event: KeyboardEvent): void {
  const n = rows.value.length;
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    if (n === 0) return;
    selected.value = (selected.value + (event.key === "ArrowDown" ? 1 : -1) + n) % n;
    void nextTick(() => document.getElementById(`qs-opt-${selected.value}`)?.scrollIntoView?.({ block: "nearest" }));
  } else if (event.key === "Enter") {
    if (event.isComposing) return;
    event.preventDefault();
    if (n === 0 && createRow.value) void create();
    else accept(selected.value, event.metaKey || event.ctrlKey);
  } else if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    dismiss();
  }
}

/** Focus moved to something outside the card (e.g. ⌘⇧F): close, and leave focus where it went. */
function onFocusout(event: FocusEvent): void {
  const next = event.relatedTarget as Node | null;
  if (next && !card.value?.contains(next)) emit("close");
}

onMounted(() => {
  origin = (document.activeElement as HTMLElement | null) ?? null;
  input.value?.focus();
  void ask(""); // empty query: the recent list
});

onBeforeUnmount(() => {
  alive = false;
  if (restoreFocus && origin && origin.isConnected) origin.focus();
});
</script>

<template>
  <div class="overlay-scrim" data-testid="quick-switcher" @mousedown.self="dismiss">
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
        :aria-activedescendant="rows.length > 0 ? `qs-opt-${selected}` : createRow ? 'qs-create' : undefined"
        autocomplete="off"
        spellcheck="false"
        :value="text"
        @input="onInput"
        @compositionstart="onCompositionStart"
        @compositionend="onCompositionEnd"
        @keydown="onKeydown"
      />
      <ul id="qs-list" class="overlay-list" role="listbox" aria-label="Notes">
        <li
          v-for="(row, i) in rows"
          :id="`qs-opt-${i}`"
          :key="row.key"
          class="overlay-row"
          role="option"
          :aria-selected="i === selected"
          :aria-label="row.label"
          @click="accept(i, false)"
          @mousemove="selected = i"
        >
          <span class="glyph" aria-hidden="true">{{ row.glyph }}</span>
          <span class="main" aria-hidden="true"
            >{{ row.lead
            }}<template v-for="(piece, k) in row.pieces" :key="k"
              ><span v-if="piece.match" class="hit">{{ piece.text }}</span
              ><template v-else>{{ piece.text }}</template></template
            ></span
          >
          <span v-if="row.aliasPieces" class="meta" aria-hidden="true"
            >Alias:
            <template v-for="(piece, k) in row.aliasPieces" :key="k"
              ><span v-if="piece.match" class="hit">{{ piece.text }}</span
              ><template v-else>{{ piece.text }}</template></template
            ></span
          >
          <span v-else-if="row.meta" class="meta" aria-hidden="true">{{ row.meta }}</span>
        </li>
      </ul>
      <p v-if="noMatch" class="overlay-empty" role="status" aria-live="polite">No note matches “{{ text }}”</p>
      <ul v-if="createRow" class="overlay-list" role="listbox" aria-label="Actions">
        <li id="qs-create" class="overlay-row" role="option" aria-selected="true" data-testid="switcher-create" @click="create">
          <span class="glyph" aria-hidden="true">+</span>
          <span class="main">Create note “{{ createRow }}”</span>
        </li>
      </ul>
      <p v-if="createError" class="overlay-empty" role="alert" data-testid="switcher-create-error">{{ createError }}</p>
      <div class="overlay-footer" aria-hidden="true">
        <span>↑↓ navigate</span><span>↵ open</span><span>⌘↵ new tab</span><span>esc</span>
      </div>
      <button type="button" class="sr-only" @click="accept(selected, false)">Open selected note</button>
      <button type="button" class="sr-only" @click="accept(selected, true)">Open selected note in new tab</button>
      <button type="button" class="sr-only" @click="dismiss">Close switcher</button>
    </div>
  </div>
</template>

<style src="./overlay.css"></style>
