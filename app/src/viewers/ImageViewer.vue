<!--
  Image viewer (FR-BIN-12; binary-viewers.md "Image viewer"; ADR-0017 §6). Every format,
  SVG included, is a plain <img src="serat-asset://…"> and nothing else: never <object>,
  <iframe>, inline markup, or a fetched-then-injected string. Zoom: fit by default, 1:1,
  percent field, slider, +/-; drag to pan when larger than the pane; modifier+wheel zooms
  toward the cursor. The accessible name is the file name (no alt source in P1).
-->
<script setup lang="ts">
import { computed, nextTick, ref } from "vue";

const props = defineProps<{ url: string; name: string; mime: string; bytes: number }>();
const emit = defineEmits<{ (e: "reveal"): void; (e: "dimensions", d: { w: number; h: number }): void }>();

const status = ref<"loading" | "loaded" | "failed">("loading");
const natural = ref<{ w: number; h: number } | null>(null);
const scroller = ref<HTMLElement | null>(null);
/** "fit" or a scale factor. */
const mode = ref<"fit" | number>("fit");
const fitScale = ref(1);
const percentField = ref("");

const scale = computed(() => (mode.value === "fit" ? fitScale.value : mode.value));
const percent = computed(() => Math.round(scale.value * 100));
const zoomed = computed(() => mode.value !== "fit" && natural.value !== null);

function recomputeFit(): void {
  const el = scroller.value;
  const n = natural.value;
  if (!el || !n) return;
  const w = Math.max(1, el.clientWidth - 32);
  const h = Math.max(1, el.clientHeight - 32);
  fitScale.value = Math.min(1, w / n.w, h / n.h) || 1;
}

function onLoad(event: Event): void {
  const img = event.target as HTMLImageElement;
  natural.value = { w: img.naturalWidth, h: img.naturalHeight };
  status.value = "loaded";
  emit("dimensions", natural.value);
  recomputeFit();
  percentField.value = String(percent.value);
}

function setScale(next: number, anchor?: { x: number; y: number }): void {
  const el = scroller.value;
  const before = scale.value;
  const clamped = Math.min(32, Math.max(0.05, next));
  mode.value = clamped;
  percentField.value = String(Math.round(clamped * 100));
  if (el && anchor) {
    // Keep the point under the cursor fixed.
    const ratio = clamped / before;
    const left = (el.scrollLeft + anchor.x) * ratio - anchor.x;
    const top = (el.scrollTop + anchor.y) * ratio - anchor.y;
    void nextTick(() => {
      el.scrollLeft = left;
      el.scrollTop = top;
    });
  }
}

function fit(): void {
  mode.value = "fit";
  recomputeFit();
  percentField.value = String(percent.value);
  if (scroller.value) {
    scroller.value.scrollLeft = 0;
    scroller.value.scrollTop = 0;
  }
}

function applyField(): void {
  const value = Number(percentField.value);
  if (Number.isFinite(value) && value > 0) setScale(value / 100);
  else percentField.value = String(percent.value);
}

function onKeydown(event: KeyboardEvent): void {
  if (!(event.metaKey || event.ctrlKey) || event.target instanceof HTMLInputElement) return;
  if (event.key === "+" || event.key === "=") setScale(scale.value * 1.25);
  else if (event.key === "-") setScale(scale.value / 1.25);
  else if (event.key === "0" && event.shiftKey) fit(); // reset pan to centre
  else if (event.key === "0" || event.key === ")") mode.value === "fit" ? setScale(1) : fit();
  else return;
  event.preventDefault();
}

function onWheel(event: WheelEvent): void {
  if (!(event.metaKey || event.ctrlKey)) return;
  event.preventDefault();
  const rect = scroller.value?.getBoundingClientRect();
  setScale(scale.value * (event.deltaY < 0 ? 1.1 : 1 / 1.1), rect ? { x: event.clientX - rect.left, y: event.clientY - rect.top } : undefined);
}

let drag: { x: number; y: number; left: number; top: number } | null = null;
function onPointerDown(event: PointerEvent): void {
  if (!zoomed.value || !scroller.value) return;
  drag = { x: event.clientX, y: event.clientY, left: scroller.value.scrollLeft, top: scroller.value.scrollTop };
}
function onPointerMove(event: PointerEvent): void {
  if (!drag || !scroller.value) return;
  scroller.value.scrollLeft = drag.left - (event.clientX - drag.x);
  scroller.value.scrollTop = drag.top - (event.clientY - drag.y);
}
function onPointerUp(): void {
  drag = null;
}

defineExpose({ setScale, fit });
</script>

<template>
  <section class="image" tabindex="0" aria-label="Image viewer" data-testid="image-viewer" @keydown="onKeydown">
    <div class="toolbar" role="toolbar" aria-label="Image controls">
      <label class="zoomfield">
        <span class="sr">Zoom percent</span>
        <input
          v-model="percentField"
          class="mono"
          inputmode="numeric"
          aria-label="Zoom percent"
          data-testid="image-zoom-field"
          @keydown.enter.prevent="applyField"
        />%
      </label>
      <button type="button" data-testid="image-fit" :aria-pressed="mode === 'fit'" @click="fit">Fit</button>
      <button type="button" data-testid="image-actual" @click="setScale(1)">Actual size</button>
      <span class="spacer"></span>
      <button type="button" aria-label="Zoom out" @click="setScale(scale / 1.25)">−</button>
      <input
        type="range"
        min="5"
        max="800"
        :value="percent"
        aria-label="Zoom"
        data-testid="image-zoom-slider"
        @input="setScale(Number(($event.target as HTMLInputElement).value) / 100)"
      />
      <button type="button" aria-label="Zoom in" @click="setScale(scale * 1.25)">+</button>
    </div>
    <div
      ref="scroller"
      class="stage"
      :class="{ zoomed }"
      @wheel="onWheel"
      @pointerdown="onPointerDown"
      @pointermove="onPointerMove"
      @pointerup="onPointerUp"
      @pointerleave="onPointerUp"
    >
      <div v-if="status === 'loading'" class="skeleton" data-testid="image-loading"></div>
      <div v-if="status === 'failed'" class="message" role="alert" data-testid="image-failed">
        <p>This image couldn't be displayed</p>
        <p class="mono micro" data-testid="image-mime">{{ mime }}</p>
        <button type="button" data-testid="image-reveal" @click="emit('reveal')">Reveal in file tree</button>
      </div>
      <img
        v-show="status === 'loaded'"
        :src="url"
        :alt="name"
        draggable="false"
        data-testid="image-el"
        :style="natural ? { width: `${natural.w * scale}px`, height: `${natural.h * scale}px` } : undefined"
        @load="onLoad"
        @error="status = 'failed'"
      />
    </div>
  </section>
</template>

<style scoped>
.image { display: flex; flex-direction: column; height: 100%; min-height: 0; outline: none; }
.toolbar { display: flex; align-items: center; gap: 0.4rem; padding: 0.3rem 0.6rem; border-bottom: 1px solid #0002; }
.toolbar button { border: 1px solid #0003; background: transparent; border-radius: 3px; padding: 0.1rem 0.5rem; cursor: pointer; }
.toolbar button[aria-pressed="true"] { background: var(--indigo); color: var(--paper); }
.mono { font-family: ui-monospace, "SF Mono", Menlo, monospace; }
.micro { font-size: 0.75rem; color: #0009; }
.zoomfield input { width: 3.5rem; text-align: right; border: 1px solid #0003; border-radius: 3px; padding: 0.1rem 0.3rem; }
.spacer { flex: 1; }
.sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); }
.stage {
  flex: 1; min-height: 0; overflow: auto; display: grid; place-items: center; padding: 1rem;
  /* the one textured background in the system: it is information (transparency) */
  background: repeating-conic-gradient(#0000000d 0% 25%, transparent 0% 50%) 50% / 16px 16px;
}
.stage.zoomed { cursor: grab; place-items: start; }
.stage.zoomed:active { cursor: grabbing; }
.stage img { max-width: none; image-rendering: auto; }
.skeleton { width: 12rem; height: 9rem; background: #0000000d; }
.message { text-align: center; background: var(--paper); padding: 1rem 1.5rem; }
</style>
