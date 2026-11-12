<!--
  Refused-names count and panel (file-tree.md "Names not shown"; components.md §1, §7;
  FR-VLT-01, NFR-SEC-06, NFR-USE-01, US-01). A status-strip text button, absent at None/0,
  inside an always-mounted polite live region (1 s trailing debounce), toggling an anchored,
  non-modal dialog that lists each refused path (mono) with the backend's reason. Paths and
  reasons are untrusted: rendered as text with control/bidi characters shown as escapes.
-->
<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";

import { refusedNames } from "@/ipc/client";
import type { RefusedName } from "@/ipc/generated/RefusedName";
import { ANNOUNCE_DEBOUNCE_MS, escapeUntrusted, REFETCH_DEBOUNCE_MS, REFUSED_NAMES_CAP, refusedCountText } from "@/refused/refusedText";

const props = withDefaults(
  defineProps<{
    /** `IndexStatus.refused_names`; `null` = absent (not counting yet, or no vault open). */
    count: number | null;
    /** The `refused_names` command; injectable for tests. */
    load?: () => Promise<RefusedName[]>;
  }>(),
  { load: () => refusedNames() },
);

const emit = defineEmits<{
  /** Focus has nowhere of its own to return to (the button is gone): the host picks the tree row or the editor. */
  "focus-fallback": [];
}>();

const open = ref(false);
const list = ref<RefusedName[] | null>(null);
const failed = ref(false);
const root = ref<HTMLElement | null>(null);
const button = ref<HTMLButtonElement | null>(null);
const dialog = ref<HTMLElement | null>(null);

// The button text follows the count at once. The screen-reader announcement lives in its own visually
// hidden status node and is coalesced by a 1 s trailing debounce (a walk raising the count name by name
// announces once); the first value is announced as it stands.
const announced = ref<number | null>(props.count);
let announceTimer: ReturnType<typeof setTimeout> | null = null;
watch(
  () => props.count,
  (next) => {
    if (announceTimer) clearTimeout(announceTimer);
    announceTimer = setTimeout(() => {
      announceTimer = null;
      announced.value = next;
    }, ANNOUNCE_DEBOUNCE_MS);
  },
);

const text = computed(() => refusedCountText(props.count));
const atCap = computed(() => (props.count ?? 0) >= REFUSED_NAMES_CAP);
const announcement = computed(() => {
  const n = announced.value;
  return n !== null && n >= REFUSED_NAMES_CAP ? `${REFUSED_NAMES_CAP} or more names not shown` : (refusedCountText(n) ?? "");
});

const sentence = computed(() => {
  const n = list.value?.length ?? 0;
  if (n >= REFUSED_NAMES_CAP) {
    return `${REFUSED_NAMES_CAP} or more files in your vault folder can't be listed on this computer. This list shows ${REFUSED_NAMES_CAP} of them. Serat leaves them as they are.`;
  }
  return n === 1
    ? "1 file in your vault folder can't be listed on this computer. Serat leaves it as it is."
    : `${n} files in your vault folder can't be listed on this computer. Serat leaves them as they are.`;
});

let fetchSeq = 0;
async function fetchList(): Promise<void> {
  const seq = ++fetchSeq;
  try {
    const next = await props.load();
    if (seq !== fetchSeq) return;
    list.value = next; // replaced in one step: the old list stays on screen until now
    failed.value = false;
  } catch {
    if (seq !== fetchSeq) return;
    failed.value = true; // no retry loop: the Retry button asks again
  }
}

async function openPanel(): Promise<void> {
  if (open.value) return;
  list.value = null;
  failed.value = false;
  opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  open.value = true;
  void fetchList();
  await nextTick();
  dialog.value?.focus();
}

let refetchTimer: ReturnType<typeof setTimeout> | null = null;
/** The element focused when the panel opened: the strip button, or the palette's opener (which the palette
 * has already refocused by the time its command runs). */
let opener: HTMLElement | null = null;

/** `restore`: put focus back (Esc, x, the toggle); false when focus already went somewhere on its own. */
function closePanel(restore: boolean): void {
  if (!open.value) return;
  open.value = false;
  if (refetchTimer) clearTimeout(refetchTimer);
  refetchTimer = null;
  fetchSeq += 1; // a late answer must not repaint a closed panel
  if (!restore) return;
  void nextTick(() => {
    // A click does not focus a button in every browser, so "nothing focused" means the strip button opened it.
    const target = opener && opener.isConnected && opener !== document.body ? opener : button.value;
    opener = null;
    if (target) target.focus();
    else emit("focus-fallback"); // the opener is gone (the count fell to none): the host picks the tree row or the editor
  });
}

function toggle(): void {
  if (open.value) closePanel(true);
  else void openPanel();
}

watch(
  () => props.count,
  (next) => {
    if (!open.value) return;
    if (next === null) {
      // The vault is gone: what the panel described no longer exists, and the button goes with it.
      closePanel(true);
      return;
    }
    if (refetchTimer) clearTimeout(refetchTimer);
    refetchTimer = setTimeout(() => {
      refetchTimer = null;
      if (open.value) void fetchList();
    }, REFETCH_DEBOUNCE_MS);
  },
);

function onKeydown(event: KeyboardEvent): void {
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    closePanel(true);
  }
}

/** Reasons arrive as lowercase predicates with Markdown backticks: capitalised, with backticked spans as
 * inline mono (no backticks). The text is escaped first and only ever rendered as text. */
function reasonParts(reason: string): Array<{ text: string; mono: boolean }> {
  const safe = escapeUntrusted(reason);
  const cased = safe.charAt(0).toUpperCase() + safe.slice(1);
  return cased
    .split("`")
    .map((text, i) => ({ text, mono: i % 2 === 1 }))
    .filter((part) => part.text !== "");
}

/** Focus moved to a non-overlay element, or into another overlay: close, and leave focus where it went. */
function onFocusout(event: FocusEvent): void {
  const next = event.relatedTarget as Node | null;
  if (open.value && next && !root.value?.contains(next)) closePanel(false);
}

function onPointerDown(event: Event): void {
  if (open.value && !root.value?.contains(event.target as Node)) closePanel(false);
}
document.addEventListener("pointerdown", onPointerDown, true);
onBeforeUnmount(() => {
  document.removeEventListener("pointerdown", onPointerDown, true);
  if (announceTimer) clearTimeout(announceTimer);
  if (refetchTimer) clearTimeout(refetchTimer);
});

defineExpose({ open: openPanel, close: () => closePanel(false), isOpen: () => open.value });
</script>

<template>
  <span ref="root" class="refused-names" @focusout="onFocusout">
    <span role="status" aria-live="polite" aria-atomic="true" class="sr-only" data-testid="refused-status">{{ announcement }}</span>
    <button
      v-if="text !== null"
      ref="button"
      type="button"
      class="refused-count"
      data-testid="refused-names"
      aria-haspopup="dialog"
      :aria-expanded="open"
      @click="toggle"
    >
      <span class="refused-dot" aria-hidden="true"></span>
      <template v-if="atCap"
        ><span aria-hidden="true">{{ REFUSED_NAMES_CAP }}+</span><span class="sr-only">{{ REFUSED_NAMES_CAP }} or more</span> names not shown</template
      >
      <template v-else>{{ text }}</template>
    </button>

    <div
      v-if="open"
      ref="dialog"
      class="refused-panel"
      role="dialog"
      aria-modal="false"
      aria-labelledby="refused-title"
      aria-describedby="refused-sentence"
      tabindex="-1"
      data-testid="refused-panel"
      @keydown="onKeydown"
    >
      <div class="refused-head">
        <h2 id="refused-title">Names not shown</h2>
        <button type="button" class="refused-close" aria-label="Close" data-testid="refused-close" @click="closePanel(true)">&times;</button>
      </div>

      <template v-if="failed">
        <p id="refused-sentence" class="refused-secondary" data-testid="refused-failed">
          Couldn't load the list. The count in the status strip is still correct.
        </p>
        <button type="button" class="refused-retry" data-testid="refused-retry" @click="fetchList">Retry</button>
      </template>
      <p v-else-if="list === null" id="refused-sentence" class="refused-secondary" aria-busy="true">Loading…</p>
      <p v-else-if="list.length === 0" id="refused-sentence" class="refused-secondary" data-testid="refused-empty">
        Every name in this vault can be shown.
      </p>
      <template v-else>
        <p id="refused-sentence" class="refused-secondary" data-testid="refused-sentence">{{ sentence }}</p>
        <ul role="list" tabindex="0" aria-label="Names not shown" class="refused-list" data-testid="refused-list">
          <li v-for="row in list" :key="row.path" class="refused-row">
            <span class="refused-path" data-testid="refused-path">{{ escapeUntrusted(row.path) }}</span>
            <span class="refused-secondary refused-reason" data-testid="refused-reason"><template v-for="(part, i) in reasonParts(row.reason)" :key="i"
                ><code v-if="part.mono" class="refused-mono">{{ part.text }}</code
                ><template v-else>{{ part.text }}</template></template
              ></span
            >
          </li>
        </ul>
        <p class="refused-secondary refused-foot">
          A name stays listed until Serat next scans its folder, even if you have since renamed or deleted the file.
        </p>
      </template>
    </div>
  </span>
</template>

<style>
.refused-names {
  position: relative;
  display: inline-block;
}
.refused-count {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: 0;
  background: none;
  padding: 0;
  font: inherit;
  color: color-mix(in srgb, var(--ink) 70%, transparent);
  cursor: pointer;
}
.refused-count:hover,
.refused-count:focus-visible {
  color: var(--ink);
  text-decoration: underline;
}
.refused-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--rust, #a8452f);
}
.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
  white-space: nowrap;
}
.refused-panel {
  position: absolute;
  bottom: calc(100% + 6px);
  left: 0;
  z-index: 40;
  width: min(440px, 90vw);
  max-height: 50vh;
  display: flex;
  flex-direction: column;
  padding: 10px 12px;
  background: var(--paper);
  color: var(--ink);
  border: 1px solid color-mix(in srgb, var(--ink) 15%, transparent);
  border-radius: 8px;
  box-shadow: 0 12px 32px color-mix(in srgb, var(--ink) 22%, transparent);
  animation: refused-in 160ms ease-out;
}
.refused-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
}
.refused-head h2 {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
}
.refused-close {
  width: 24px;
  height: 24px;
  border: 0;
  background: none;
  font-size: 18px;
  line-height: 1;
  color: inherit;
  cursor: pointer;
}
.refused-secondary {
  margin: 6px 0;
  color: color-mix(in srgb, var(--ink) 70%, transparent);
}
.refused-list {
  list-style: none;
  margin: 0;
  padding: 0;
  overflow-y: auto;
  overflow-anchor: auto;
  user-select: text;
}
.refused-row {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 6px 0;
  border-top: 1px solid color-mix(in srgb, var(--ink) 15%, transparent);
  content-visibility: auto;
}
.refused-mono,
.refused-path {
  font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  overflow-wrap: anywhere;
}
.refused-reason {
  margin: 0;
}
.refused-retry {
  align-self: flex-start;
}
@keyframes refused-in {
  from {
    opacity: 0;
    transform: translateY(4px);
  }
}
@keyframes refused-fade {
  from {
    opacity: 0;
  }
}
@media (prefers-reduced-motion: reduce) {
  .refused-panel {
    animation-name: refused-fade;
  }
}
@media (max-width: 719px) {
  .refused-names {
    position: static;
  }
  .refused-panel {
    left: 0;
    right: 0;
    bottom: 28px;
    width: auto;
    max-height: 60vh;
    border-radius: 8px 8px 0 0;
  }
}
</style>
