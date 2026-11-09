<!--
  PDF viewer (FR-BIN-11; binary-viewers.md "PDF viewer"; ADR-0017). One page at a time, drawn
  by pdf.js (pdfEngine.ts) over `read_asset_range` spans (assetRangeTransport.ts). Toolbar:
  previous/next, a mono page field (Enter jumps), page count, fit-width / fit-page, zoom in/out.
  A file replaced on disk emits `changed`; the host restarts the load and shows the 4 s notice.
  Deferred (follow-ups): annotation links (none built, so none can open), pdf.js find bar.
-->
<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { loadPdf, type PdfDocumentHandle, type PdfLoader, type PdfPageSize } from "./pdfEngine";

const props = defineProps<{ path: string; length: number; name: string; loader?: PdfLoader; startPage?: number }>();
/** The file on disk no longer matches what was opened; carries the page being read. */
const emit = defineEmits<{ (e: "changed", page: number): void }>();

type Fit = "width" | "page" | "custom";

const state = ref<"loading" | "ready" | "error">("loading");
const doc = ref<PdfDocumentHandle | null>(null);
const page = ref(1);
const pageField = ref("1");
const fieldFlash = ref(false);
const fit = ref<Fit>("width");
const zoom = ref(1); // custom scale
const size = ref<PdfPageSize | null>(null);
const stage = ref<HTMLElement | null>(null);
const canvas = ref<HTMLCanvasElement | null>(null);
const textLayer = ref<HTMLElement | null>(null);
const pageField$ = ref<HTMLInputElement | null>(null);
let renderToken = 0;
let alive = true;

const numPages = computed(() => doc.value?.numPages ?? 0);

function effectiveScale(): number {
  if (fit.value === "custom" || !size.value || !stage.value) return zoom.value;
  const w = Math.max(1, stage.value.clientWidth - 32);
  const h = Math.max(1, stage.value.clientHeight - 32);
  const byWidth = w / size.value.width;
  return fit.value === "width" ? byWidth : Math.min(byWidth, h / size.value.height);
}
const percent = computed(() => Math.round((fit.value === "custom" ? zoom.value : effectiveScale()) * 100));

async function draw(): Promise<void> {
  const d = doc.value;
  if (!d || !canvas.value) return;
  const token = ++renderToken;
  try {
    size.value = await d.pageSize(page.value);
    if (token !== renderToken || !alive) return;
    await d.render(page.value, canvas.value, effectiveScale(), textLayer.value);
    if (token === renderToken && alive) state.value = "ready";
  } catch {
    if (token === renderToken && alive) state.value = "error";
  }
}

async function open(): Promise<void> {
  state.value = "loading";
  doc.value?.destroy();
  doc.value = null;
  try {
    const handle = await (props.loader ?? loadPdf)(props.path, props.length, () => emit("changed", page.value));
    if (!alive) {
      handle.destroy();
      return;
    }
    doc.value = handle;
    page.value = Math.min(Math.max(1, props.startPage ?? 1), handle.numPages);
    pageField.value = String(page.value);
    await nextTick();
    await draw();
  } catch {
    if (alive) state.value = "error";
  }
}

function goTo(n: number): void {
  if (!Number.isInteger(n) || n < 1 || n > numPages.value) {
    // Out of range: revert in place with a 120 ms flash, no dialog (spec state table).
    pageField.value = String(page.value);
    fieldFlash.value = true;
    setTimeout(() => (fieldFlash.value = false), 120);
    return;
  }
  page.value = n;
  pageField.value = String(n);
  void draw();
}

function commitField(): void {
  goTo(Number(pageField.value.trim() === "" ? NaN : pageField.value));
}

function zoomBy(factor: number): void {
  zoom.value = Math.min(8, Math.max(0.1, effectiveScale() * factor));
  fit.value = "custom";
  void draw();
}

function setFit(mode: Fit): void {
  fit.value = mode;
  void draw();
}

function onKeydown(event: KeyboardEvent): void {
  const mod = event.metaKey || event.ctrlKey;
  const inField = event.target instanceof HTMLInputElement;
  if (mod && event.key.toLowerCase() === "g") {
    event.preventDefault();
    pageField$.value?.focus();
    pageField$.value?.select();
  } else if (mod && (event.key === "+" || event.key === "=")) {
    event.preventDefault();
    zoomBy(1.25);
  } else if (mod && event.key === "-") {
    event.preventDefault();
    zoomBy(0.8);
  } else if (mod && event.key === "0") {
    event.preventDefault();
    setFit(fit.value === "width" ? "page" : "width");
  } else if (!mod && !inField) {
    if (event.key === "ArrowRight" || event.key === "PageDown") goTo(page.value + 1);
    else if (event.key === "ArrowLeft" || event.key === "PageUp") goTo(page.value - 1);
    else return;
    event.preventDefault();
  }
}

onMounted(() => void open());
onBeforeUnmount(() => {
  alive = false;
  doc.value?.destroy();
});
watch(() => props.path, () => void open());
defineExpose({ goTo });
</script>

<template>
  <section class="pdf" tabindex="0" aria-label="PDF viewer" data-testid="pdf-viewer" @keydown="onKeydown">
    <div class="toolbar" role="toolbar" aria-label="PDF controls">
      <button type="button" aria-label="Previous page" data-testid="pdf-prev" :disabled="page <= 1" @click="goTo(page - 1)">◂</button>
      <label class="pagefield">
        <span class="sr">Page</span>
        <input
          ref="pageField$"
          v-model="pageField"
          class="mono"
          :class="{ flash: fieldFlash }"
          inputmode="numeric"
          aria-label="Page"
          data-testid="pdf-page"
          @keydown.enter.prevent="commitField"
          @blur="pageField = String(page)"
        />
      </label>
      <span class="count" data-testid="pdf-count">of {{ numPages }}</span>
      <button type="button" aria-label="Next page" data-testid="pdf-next" :disabled="page >= numPages" @click="goTo(page + 1)">▸</button>
      <span class="spacer"></span>
      <button type="button" aria-label="Zoom out" data-testid="pdf-zoom-out" @click="zoomBy(0.8)">−</button>
      <span class="mono pct" data-testid="pdf-zoom">{{ percent }}%</span>
      <button type="button" aria-label="Zoom in" data-testid="pdf-zoom-in" @click="zoomBy(1.25)">+</button>
      <span class="segmented" role="group" aria-label="Fit mode">
        <button type="button" :aria-pressed="fit === 'width'" data-testid="pdf-fit-width" @click="setFit('width')">Fit width</button>
        <button type="button" :aria-pressed="fit === 'page'" data-testid="pdf-fit-page" @click="setFit('page')">Fit page</button>
      </span>
    </div>
    <div ref="stage" class="stage">
      <div v-if="state === 'loading'" class="skeleton" data-testid="pdf-loading" aria-label="Loading"></div>
      <p v-if="state === 'error'" class="message" role="alert" data-testid="pdf-error">
        This PDF couldn't be displayed
      </p>
      <div v-show="state === 'ready'" class="sheet">
        <canvas ref="canvas" :aria-label="`${name}, page ${page} of ${numPages}`" role="img"></canvas>
        <div ref="textLayer" class="textLayer"></div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.pdf { display: flex; flex-direction: column; height: 100%; min-height: 0; outline: none; }
.toolbar { display: flex; align-items: center; gap: 0.4rem; padding: 0.3rem 0.6rem; border-bottom: 1px solid #0002; }
.toolbar button { border: 1px solid #0003; background: transparent; border-radius: 3px; padding: 0.1rem 0.5rem; cursor: pointer; }
.toolbar button[aria-pressed="true"] { background: var(--indigo); color: var(--paper); }
.toolbar button:disabled { opacity: 0.4; cursor: default; }
.mono { font-family: ui-monospace, "SF Mono", Menlo, monospace; }
.pagefield input { width: 3.5rem; text-align: right; border: 1px solid #0003; border-radius: 3px; padding: 0.1rem 0.3rem; }
.pagefield input.flash { outline: 2px solid #a3472f; }
.count, .pct { color: #0009; }
.spacer { flex: 1; }
.segmented { display: inline-flex; gap: 0; }
.sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); }
.stage { flex: 1; min-height: 0; overflow: auto; background: #0000000d; display: flex; justify-content: center; padding: 1rem; }
.sheet { position: relative; margin: 0 auto; box-shadow: 0 0 0 1px #0002; background: #fff; align-self: flex-start; }
.sheet canvas { display: block; }
.textLayer { position: absolute; inset: 0; overflow: clip; line-height: 1; text-size-adjust: none; transform-origin: 0 0; }
/* Ported from pdfjs-dist/web/pdf_viewer.css: glyph size/shear come from per-span custom properties. */
.textLayer :deep(span), .textLayer :deep(br) { color: transparent; position: absolute; white-space: pre; cursor: text; transform-origin: 0 0; user-select: text; }
.textLayer :deep(> :not(.markedContent)), .textLayer :deep(.markedContent span:not(.markedContent)) {
  --font-height: 0; --scale-x: 1; --rotate: 0deg;
  font-size: calc(var(--total-scale-factor) * var(--font-height));
  transform: rotate(var(--rotate)) scaleX(var(--scale-x));
}
.textLayer :deep(.markedContent) { display: contents; }
.skeleton { width: min(40rem, 90%); aspect-ratio: 1 / 1.414; background: #0000000d; align-self: flex-start; }
.message { align-self: center; }
</style>
