<!--
  The read-only rendered pane (architecture.md §9.3, FR-EDT-10): "the same
  markdown-it-rendered, sanitised output as embeds use ... Preview re-renders
  150ms after the last keystroke, scheduled in idle time ... a 5,000-line
  note never renders in one frame."

  Mounts sanitised, already-built DOM (`preview.ts` + `sanitize.ts`) via
  `appendChild`, never `v-html` — components.md §4's rule restated in
  `sanitize.ts`'s own header: nothing here re-parses untrusted markup a
  second time.

  External-link and wikilink clicks are never handled here directly: every
  anchor click is intercepted and `preventDefault()`-ed unconditionally
  (§9.3: "Internal wikilinks never navigate the webview... nothing ever
  opens one automatically"), and re-emitted as a typed event so the
  component that knows how to open a link (system browser via the opener
  capability, or the wikilink router) can — this file has no opinion on
  either.
-->
<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from "vue";

import { chunkMarkdownSource, renderMarkdownToFragment } from "@/editor/preview";

const props = defineProps<{
  source: string;
  /** ms after the last change before a re-render starts (§9.3: 150ms). */
  debounceMs?: number;
}>();

const emit = defineEmits<{
  "external-link-click": [url: string];
  "wikilink-click": [target: string];
}>();

const container = ref<HTMLDivElement | null>(null);
let debounceTimer: ReturnType<typeof setTimeout> | null = null;
let renderToken = 0;

const idleSchedule: (cb: () => void) => void =
  typeof requestIdleCallback === "function" ? (cb) => requestIdleCallback(cb) : (cb) => setTimeout(cb, 0);

async function renderNow(source: string): Promise<void> {
  const token = ++renderToken;
  const host = container.value;
  if (!host) return;

  const chunks = chunkMarkdownSource(source);
  const rendered = await Promise.all(chunks.map((chunk) => renderMarkdownToFragment(chunk)));
  if (token !== renderToken || !container.value) {
    return; // superseded by a later render, or unmounted mid-flight
  }
  host.innerHTML = "";
  let index = 0;
  const appendNext = (): void => {
    if (token !== renderToken || !container.value) return;
    if (index >= rendered.length) return;
    container.value.appendChild(rendered[index]);
    index += 1;
    if (index < rendered.length) {
      idleSchedule(appendNext);
    }
  };
  appendNext();
}

function scheduleRender(source: string): void {
  if (debounceTimer) clearTimeout(debounceTimer);
  debounceTimer = setTimeout(() => {
    void renderNow(source);
  }, props.debounceMs ?? 150);
}

watch(
  () => props.source,
  (source) => scheduleRender(source),
  { immediate: true },
);

onBeforeUnmount(() => {
  if (debounceTimer) clearTimeout(debounceTimer);
  renderToken += 1; // invalidate any in-flight render
});

function onContainerClick(event: MouseEvent): void {
  const target = event.target as HTMLElement | null;
  const anchor = target?.closest("a");
  if (!anchor) return;
  // Never let the webview navigate on a rendered link, of any kind
  // (§9.3) — every anchor click is prevented unconditionally, then routed
  // by data attribute. `auxclick` (middle/other buttons) is intercepted
  // too: it would otherwise open the href in a new webview window. Only a
  // primary click activates a link.
  event.preventDefault();
  if (event.type === "auxclick" || event.button !== 0) return;
  const wikilinkTarget = anchor.getAttribute("data-wikilink");
  if (wikilinkTarget !== null) {
    emit("wikilink-click", wikilinkTarget);
    return;
  }
  if (anchor.getAttribute("data-external") === "true") {
    const href = anchor.getAttribute("href");
    if (href) emit("external-link-click", href);
  }
}

defineExpose({ renderNow: () => renderNow(props.source) });
</script>

<template>
  <div ref="container" class="serat-preview" data-testid="serat-preview" @click="onContainerClick" @auxclick="onContainerClick"></div>
</template>
