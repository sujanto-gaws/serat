<!--
  Command palette (command-palette.md; FR-NAV-06, NFR-USE-01). A view onto the
  one command registry (editor/commands.ts): every registered command, fuzzy
  matched by name, its effective binding as a chip pair, unavailable commands
  greyed and explained rather than hidden.
  Not built here: the inline rebind editor, `set_keybinding` and conflict UI
  (command-palette.md "Rebinding") — task #93 (NFR-USE-01).
-->
<script setup lang="ts">
import { computed, inject, onBeforeUnmount, onMounted, ref, shallowRef, watch } from "vue";

import { chordGlyphs, chordSpoken, CommandRegistry, COMMANDS_KEY, type Command } from "@/editor/commands";
import { detectPlatform } from "@/editor/codemirrorSetup";
import { fuzzyMatch, splitBySpans } from "@/overlays/fuzzy";

const props = defineProps<{ registry?: CommandRegistry; mac?: boolean }>();
const emit = defineEmits<{ run: [id: string]; close: [] }>();

const registry = props.registry ?? inject(COMMANDS_KEY, undefined) ?? new CommandRegistry();
const mac = props.mac ?? detectPlatform() === "mac";

const text = ref("");
const selected = ref(0);
const reason = ref<string | null>(null);
const input = ref<HTMLInputElement | null>(null);
const card = ref<HTMLElement | null>(null);
let origin: HTMLElement | null = null;
let restoreFocus = false;

/** Bumped when commands or bindings change, so `rows` recomputes. */
const version = shallowRef(0);
const unsubscribe = registry.subscribe(() => version.value++);

interface Row {
  command: Command;
  pieces: Array<{ text: string; match: boolean }>;
  chord: string[];
  spoken: string[];
  reason: string | null;
}

const rows = computed<Row[]>(() => {
  void version.value;
  const q = text.value.trim();
  let matched: Array<{ command: Command; spans: Array<{ start: number; end: number }>; score: number }> = [];
  for (const command of registry.ranked()) {
    if (q === "") matched.push({ command, spans: [], score: 0 });
    else {
      const hit = fuzzyMatch(q, command.name);
      if (hit) matched.push({ command, spans: hit.spans, score: hit.score });
    }
  }
  // Empty query keeps the registry's order (use, then name); a query ranks by score.
  if (q !== "") matched = matched.sort((a, b) => b.score - a.score);
  return matched.map(({ command, spans }) => {
    const binding = registry.effectiveBinding(command.id);
    return {
      command,
      pieces: splitBySpans(command.name, spans),
      chord: binding ? chordGlyphs(binding, mac) : [],
      spoken: binding ? chordSpoken(binding, mac) : [],
      reason: command.unavailable?.() ?? null,
    };
  });
});

// The registry can shrink while the palette is open (an editor unmounts): keep the selection valid.
watch(rows, (list) => {
  if (selected.value >= list.length) selected.value = Math.max(0, list.length - 1);
});

/** "Toggle live preview, Command, Shift, M": the chord is spoken as words, not glyphs. */
function ariaName(row: Row): string {
  const chord = row.spoken.length ? `, ${row.spoken.join(", ")}` : "";
  return `${row.command.name}${chord}${row.reason ? `, unavailable: ${row.reason}` : ""}`;
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
  // Running closes the palette; focus goes back to the opener first, then the command runs.
  restoreFocus = true;
  emit("run", row.command.id);
}

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
    reason.value = null;
    document.getElementById(`cp-opt-${selected.value}`)?.scrollIntoView?.({ block: "nearest" });
  } else if (event.key === "Enter") {
    event.preventDefault();
    accept(selected.value);
  } else if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    dismiss();
  }
}

/** Focus moved to something outside the card: close, and leave focus where it went. */
function onFocusout(event: FocusEvent): void {
  const next = event.relatedTarget as Node | null;
  if (next && !card.value?.contains(next)) emit("close");
}

onMounted(() => {
  origin = (document.activeElement as HTMLElement | null) ?? null;
  input.value?.focus();
});

onBeforeUnmount(() => {
  unsubscribe();
  if (restoreFocus && origin && origin.isConnected) origin.focus();
});
</script>

<template>
  <div class="overlay-scrim" data-testid="command-palette" @mousedown.self="dismiss">
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
      <p v-if="rows.length === 0" class="overlay-empty" role="status" aria-live="polite">No command matches “{{ text }}”</p>
      <p v-if="reason !== null" class="overlay-empty" role="status" data-testid="palette-reason">{{ reason }}</p>
      <div class="overlay-footer" aria-hidden="true"><span>↑↓ navigate</span><span>↵ run</span><span>esc</span></div>
    </div>
  </div>
</template>

<style src="./overlay.css"></style>
