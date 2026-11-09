<!--
  The "Comparing" state of external-change-notice.md: a read-only, non-modal diff in
  place of the editor pane's content. Old (disk) on the left, the current buffer on the
  right, scrolling independently, the differing line range highlighted in both. The
  notice's actions stay in this header (only when there is a conflict to resolve); Esc
  returns to the editor with the notice still present. Inspection only: nothing here
  changes the buffer or the file (C-04, NFR-REL-01). Text is rendered by interpolation,
  never as HTML.
-->
<script setup lang="ts">
import { computed, onMounted, ref } from "vue";

const props = defineProps<{
  path: string;
  mine: string;
  disk: string;
  /** True when an overlap is pending: the header then offers Keep mine / Take theirs. */
  resolvable: boolean;
}>();

const emit = defineEmits<{
  close: [];
  "keep-mine": [];
  "take-theirs": [];
}>();

const root = ref<HTMLElement | null>(null);
onMounted(() => root.value?.focus());

/** Common-prefix/suffix line trim: the middle is the differing range (inclusive start, exclusive end). */
function differingRange(a: string[], b: string[]): { a: [number, number]; b: [number, number] } {
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start += 1;
  let ea = a.length;
  let eb = b.length;
  while (ea > start && eb > start && a[ea - 1] === b[eb - 1]) {
    ea -= 1;
    eb -= 1;
  }
  return { a: [start, ea], b: [start, eb] };
}

const diskLines = computed(() => props.disk.split(/\r\n|\n|\r/));
const mineLines = computed(() => props.mine.split(/\r\n|\n|\r/));
const range = computed(() => differingRange(diskLines.value, mineLines.value));
const identical = computed(() => props.disk === props.mine);
const hot = (side: "a" | "b", i: number): boolean => {
  const [from, to] = range.value[side];
  return i >= from && i < to;
};
</script>

<template>
  <div ref="root" class="serat-compare" tabindex="0" data-testid="external-compare" @keydown.esc.stop.prevent="emit('close')">
    <header class="serat-compare-header">
      <strong>{{ path }}</strong>
      <span v-if="identical">The disk version and your buffer are identical.</span>
      <template v-if="resolvable">
        <button type="button" data-testid="compare-keep-mine" @click="emit('keep-mine')">Keep mine</button>
        <button type="button" data-testid="compare-take-theirs" @click="emit('take-theirs')">Take theirs</button>
      </template>
      <button type="button" data-testid="compare-close" @click="emit('close')">Back to editor (Esc)</button>
    </header>
    <div class="serat-compare-columns">
      <section class="serat-compare-col" aria-label="On disk" data-testid="compare-disk">
        <h3>On disk</h3>
        <pre><span v-for="(line, i) in diskLines" :key="i" class="ln" :class="{ hot: hot('a', i) }">{{ line === "" ? " " : line }}</span></pre>
      </section>
      <section class="serat-compare-col" aria-label="Your buffer" data-testid="compare-mine">
        <h3>Your buffer</h3>
        <pre><span v-for="(line, i) in mineLines" :key="i" class="ln" :class="{ hot: hot('b', i) }">{{ line === "" ? " " : line }}</span></pre>
      </section>
    </div>
  </div>
</template>

<style scoped>
.serat-compare {
  position: absolute;
  inset: 0;
  z-index: 3;
  display: flex;
  flex-direction: column;
  background: var(--serat-surface, #fff);
  outline: none;
}
.serat-compare-header {
  display: flex;
  gap: 0.5em;
  align-items: center;
  padding: 4px 10px;
  border-bottom: 1px solid var(--serat-border-hairline, #ddd);
}
.serat-compare-columns {
  display: flex;
  flex: 1 1 auto;
  min-height: 0;
}
.serat-compare-col {
  flex: 1 1 50%;
  min-width: 0;
  overflow: auto;
  border-right: 1px solid var(--serat-border-hairline, #ddd);
}
.serat-compare-col h3 {
  margin: 0;
  padding: 2px 10px;
  font-size: 0.8em;
}
.serat-compare-col pre {
  margin: 0;
  padding: 0 10px;
  font-family: var(--serat-font-mono, monospace);
  white-space: pre-wrap;
}
.ln {
  display: block;
}
.ln.hot {
  background: var(--serat-accent-secondary-bg, #f3e3b8);
}
</style>
