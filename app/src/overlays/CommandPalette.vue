<!--
  Command palette (command-palette.md; FR-NAV-05, NFR-USE-01). A view onto the
  command registry: fuzzy over command names, effective binding as a chip
  pair, unavailable commands greyed and explained rather than hidden. Not
  built here: the inline rebind editor (command-palette.md "Rebinding").
-->
<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";

import { chordParts, commandRegistry, isMac, type Command, type CommandRegistry } from "@/commands/registry";
import { fuzzyMatch, splitBySpans } from "@/overlays/fuzzy";

const props = withDefaults(defineProps<{ registry?: CommandRegistry }>(), { registry: () => commandRegistry });
const emit = defineEmits<{ run: [id: string]; close: [] }>();

const text = ref("");
const selected = ref(0);
const reason = ref<string | null>(null);
const input = ref<HTMLInputElement | null>(null);
const card = ref<HTMLElement | null>(null);
let origin: HTMLElement | null = null;
const mac = isMac();

interface Row {
  command: Command;
  pieces: Array<{ text: string; match: boolean }>;
  chord: string[];
  reason: string | null;
}

const rows = computed<Row[]>(() => {
  void props.registry.version.value; // re-evaluate when commands are added/removed/rebound
  const q = text.value.trim();
  let matched: Array<{ command: Command; spans: Array<{ start: number; end: number }>; score: number }> = [];
  for (const command of props.registry.list()) {
    if (q === "") matched.push({ command, spans: [], score: 0 });
    else {
      const hit = fuzzyMatch(q, command.name);
      if (hit) matched.push({ command, spans: hit.spans, score: hit.score });
    }
  }
  // Empty query keeps the registry's order (frecency, then name); a query ranks by score.
  if (q !== "") matched = matched.sort((a, b) => b.score - a.score);
  return matched.map(({ command, spans }) => {
    const binding = props.registry.effectiveBinding(command.id);
    return {
      command,
      pieces: splitBySpans(command.name, spans),
      chord: binding ? chordParts(binding, mac) : [],
      reason: command.unavailable?.() ?? null,
    };
  });
});

function ariaName(row: Row): string {
  const chord = row.chord.length ? `, ${row.chord.join(", ")}` : "";
  return `${row.command.name}, Command${chord}${row.reason ? `, unavailable: ${row.reason}` : ""}`;
}

function onInput(event: Event): void {
  text.value = (event.target as HTMLInputElement).value;
  selected.value = 0;
  reason.value = null;
}

function accept(index: number): void {
  const row = rows.value[index];
  if (!row) return;
  if (row.reason !== null) {
    reason.value = row.reason; // say why instead of silently failing; stay open
    return;
  }
  emit("run", row.command.id);
}

function onKeydown(event: KeyboardEvent): void {
  const n = rows.value.length;
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    if (n === 0) return;
    selected.value = (selected.value + (event.key === "ArrowDown" ? 1 : -1) + n) % n;
    reason.value = null;
  } else if (event.key === "Enter") {
    event.preventDefault();
    accept(selected.value);
  } else if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    emit("close");
  }
}

function onFocusout(event: FocusEvent): void {
  const next = event.relatedTarget as Node | null;
  if (next && !card.value?.contains(next)) emit("close");
}

onMounted(() => {
  origin = (document.activeElement as HTMLElement | null) ?? null;
  input.value?.focus();
});

onBeforeUnmount(() => {
  // Focus goes back first; a command that moves focus (e.g. "Focus file tree") runs after this.
  if (origin && origin.isConnected) origin.focus();
});
</script>

<template>
  <div class="overlay-scrim" data-testid="command-palette" @mousedown.self="emit('close')">
    <div ref="card" class="overlay-card" @focusout="onFocusout">
      <input
        ref="input"
        class="overlay-input"
        type="text"
        role="combobox"
        aria-label="Run command"
        aria-autocomplete="list"
        aria-expanded="true"
        aria-controls="cp-list"
        :aria-activedescendant="rows.length > 0 ? `cp-opt-${selected}` : undefined"
        autocomplete="off"
        spellcheck="false"
        :value="text"
        @input="onInput"
        @keydown="onKeydown"
      />
      <ul id="cp-list" class="overlay-list" role="listbox" aria-label="Commands">
        <li
          v-for="(row, i) in rows"
          :id="`cp-opt-${i}`"
          :key="row.command.id"
          class="overlay-row"
          :class="{ unavailable: row.reason !== null }"
          role="option"
          :aria-selected="i === selected"
          :aria-label="ariaName(row)"
          @click="accept(i)"
          @mousemove="selected = i"
        >
          <span class="main" aria-hidden="true"
            ><template v-for="(piece, k) in row.pieces" :key="k"
              ><span v-if="piece.match" class="hit">{{ piece.text }}</span
              ><template v-else>{{ piece.text }}</template></template
            ></span
          >
          <span v-if="row.chord.length" class="meta" aria-hidden="true"
            ><kbd v-for="(part, k) in row.chord" :key="k" class="chip">{{ part }}</kbd></span
          >
        </li>
      </ul>
      <p v-if="rows.length === 0" class="overlay-empty">No command matches “{{ text }}”</p>
      <p v-if="reason !== null" class="overlay-empty" role="status" data-testid="palette-reason">{{ reason }}</p>
      <div class="overlay-footer" aria-hidden="true"><span>↑↓ navigate</span><span>↵ run</span><span>esc</span></div>
    </div>
  </div>
</template>

<style src="./overlay.css"></style>
