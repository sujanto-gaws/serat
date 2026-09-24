<!--
  Task #21 (FR-EDT-01/02/03/10, NFR-PRF-02, NFR-PRF-01; ADR-0006, ADR-0016):
  the hybrid live-preview editor. Builds on task #98's P0 shell (see the
  history in this file's earlier revisions for the shadow-root/CSP
  reasoning, which is unchanged and restated briefly below) by adding:

    - GFM + custom inline Markdown (`markdownLanguage.ts`, `mdExtensions.ts`);
    - the reveal rule (`reveal.ts`, FR-EDT-02) — hides syntax markers except
      on lines the selection intersects, recomputed synchronously on every
      doc/selection/viewport change so it lands within one frame;
    - the single command registry and its one `keymap.of` call
      (`codemirrorSetup.ts`);
    - a per-note `EditorBuffer` (ADR-0016 §2) that turns local edits into
      patch-set saves through a mockable `EditorIpcAdapter`, debounced 1s
      idle / on blur / on unmount, with at most one save in flight;
    - split-preview mode (FR-EDT-10): `livePreview: false` turns off the
      reveal rule (source stays fully raw, `mono-source` throughout — no
      hide/reveal in this mode, which is the point of the fallback) and
      mounts a read-only `Preview.vue` beside the source.

  Still mounted synchronously in `onMounted`, with no `await` before the
  `EditorView` is constructed, and still into a shadow root — CodeMirror's
  runtime CSS (`style-mod`) needs `adoptedStyleSheets`, which only a shadow
  root gets, to satisfy `tauri.conf.json`'s CSP with no `'unsafe-inline'`.
  See `src/startup.ts` and `app/README.md` for why synchronous-mount is
  load-bearing, and task #98/#102's history for the CSP finding.
-->
<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { redo, undo } from "@codemirror/commands";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import { EditorView, lineNumbers, placeholder } from "@codemirror/view";

import { buildEditorExtensions, createEditorCommandRegistry } from "@/editor/codemirrorSetup";
import { EditorBuffer, type SaveOutcome } from "@/editor/buffer";
import { seratMarkdown } from "@/editor/markdownLanguage";
import Preview from "@/editor/Preview.vue";
import { revealMarkersPlugin } from "@/editor/reveal";
import { defaultEditorIpc, type EditorIpcAdapter } from "@/ipc/editorAdapter";

/** How long after the last keystroke an idle auto-save fires — ADR-0016 §2:
 * "Saves are triggered by 1 s of idle, by blur, by closing the note and by
 * the save command." */
const AUTOSAVE_IDLE_MS = 1000;

const props = withDefaults(
  defineProps<{
    placeholder?: string;
    /** `editor.live_preview` (`UiSettings`) — `false` selects split preview
     * (FR-EDT-10). Reactive: toggling it at runtime reconfigures the live
     * editor without losing the document, selection or undo history. */
    livePreview?: boolean;
    ipc?: EditorIpcAdapter;
  }>(),
  { livePreview: true, ipc: () => defaultEditorIpc },
);

const emit = defineEmits<{
  "save-outcome": [outcome: SaveOutcome];
  "dirty-changed": [dirty: boolean];
  "toggle-live-preview": [];
  "wikilink-click": [target: string];
  "external-link-click": [url: string];
}>();

const host = ref<HTMLDivElement | null>(null);
let view: EditorView | null = null;
let buffer: EditorBuffer | null = null;
let autosaveTimer: ReturnType<typeof setTimeout> | null = null;

const revealCompartment = new Compartment();
const dirty = ref(false);
const previewSource = ref("");
/** True only for the one dispatch `loadNote`/`setText` issues to replace
 * the whole document — that transaction must not be recorded as a pending
 * edit against the fresh buffer it accompanies, or the note would be
 * "dirty" the instant it opens. */
let suppressChangeTracking = false;

function markDirty(): void {
  const next = buffer?.isDirty ?? false;
  if (next !== dirty.value) {
    dirty.value = next;
    emit("dirty-changed", next);
  }
}

async function performSave(): Promise<SaveOutcome | null> {
  if (!buffer) return null;
  const outcome = await buffer.save(props.ipc);
  markDirty();
  emit("save-outcome", outcome);
  return outcome;
}

function scheduleAutosave(): void {
  if (autosaveTimer) clearTimeout(autosaveTimer);
  autosaveTimer = setTimeout(() => {
    void performSave();
  }, AUTOSAVE_IDLE_MS);
}

const registry = createEditorCommandRegistry({
  save: () => {
    void performSave();
    return true;
  },
  toggleLivePreview: () => {
    emit("toggle-live-preview");
    return true;
  },
  followLink: () => {
    // Task #22's scope (wikilink resolution / autocompletion) also owns
    // "follow link under cursor" — the binding is registered here (it is
    // part of #21's keymap surface, components.md §7) but does nothing yet
    // rather than mis-navigating. Consuming the keystroke (returning
    // `true`) is still correct: there is no sane default CodeMirror action
    // for Mod-Enter to fall back to.
    return true;
  },
});

function buildExtensions(livePreview: boolean): Extension[] {
  return [
    lineNumbers(),
    seratMarkdown(),
    revealCompartment.of(livePreview ? [revealMarkersPlugin] : []),
    ...buildEditorExtensions(registry),
    EditorView.lineWrapping,
    EditorView.contentAttributes.of({ spellcheck: "false", "aria-label": "editor" }),
    EditorView.domEventHandlers({
      blur: () => {
        void performSave();
        return false;
      },
    }),
    EditorView.updateListener.of((update) => {
      if (update.docChanged) {
        if (!suppressChangeTracking) {
          const composed = update.transactions.map((tr) => tr.changes).reduce((acc, cs) => acc.compose(cs));
          buffer?.recordChange(composed);
          markDirty();
          scheduleAutosave();
        }
        previewSource.value = update.state.doc.toString();
      }
    }),
    ...(props.placeholder ? [placeholder(props.placeholder)] : []),
  ];
}

onMounted(() => {
  if (!host.value) {
    return;
  }
  const shadowRoot = host.value.attachShadow({ mode: "open" });
  view = new EditorView({
    parent: shadowRoot,
    state: EditorState.create({
      doc: "",
      extensions: buildExtensions(props.livePreview),
    }),
  });
});

onBeforeUnmount(() => {
  if (autosaveTimer) clearTimeout(autosaveTimer);
  void performSave();
  view?.destroy();
  view = null;
});

watch(
  () => props.livePreview,
  (enabled) => {
    view?.dispatch({ effects: revealCompartment.reconfigure(enabled ? [revealMarkersPlugin] : []) });
  },
);

/**
 * Replaces the whole document and starts a fresh buffer for `path`/`hash` —
 * used once per note, when it is opened (never on every keystroke: this is
 * the P1 analogue of P0's `setText`, now also resetting ADR-0016's base).
 */
function loadNote(text: string, path: string, baseHash: string): void {
  if (!view) return;
  buffer = new EditorBuffer(path, text, baseHash);
  suppressChangeTracking = true;
  try {
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: text },
      // The initial load is not an edit to undo back through.
      selection: { anchor: 0 },
    });
  } finally {
    suppressChangeTracking = false;
  }
  previewSource.value = text;
  dirty.value = false;
}

/** Back-compat with the P0 shell's placeholder-content path (no note open
 * yet, or a read failure message) — does not touch the buffer, so it must
 * never be used for real note content once `loadNote` is available. */
function setText(text: string): void {
  suppressChangeTracking = true;
  try {
    view?.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
  } finally {
    suppressChangeTracking = false;
  }
  previewSource.value = text;
}

function getText(): string {
  return view?.state.doc.toString() ?? "";
}

function focus(): void {
  view?.focus();
}

async function triggerSave(): Promise<SaveOutcome | null> {
  return performSave();
}

const isDirty = computed(() => dirty.value);

defineExpose({
  setText,
  getText,
  focus,
  loadNote,
  triggerSave,
  isDirty,
  undo: () => view && undo(view),
  redo: () => view && redo(view),
});
</script>

<template>
  <div class="serat-editor-surface" data-testid="serat-editor">
    <div ref="host" class="cm-editor-host"></div>
    <Preview
      v-if="!livePreview"
      class="serat-editor-split-preview"
      :source="previewSource"
      @wikilink-click="(t) => emit('wikilink-click', t)"
      @external-link-click="(u) => emit('external-link-click', u)"
    />
  </div>
</template>

<style scoped>
.serat-editor-surface {
  display: flex;
  height: 100%;
  width: 100%;
}
.cm-editor-host {
  flex: 1 1 50%;
  min-width: 0;
}
.serat-editor-split-preview {
  flex: 1 1 50%;
  min-width: 0;
  overflow: auto;
  border-left: 1px solid var(--serat-border-hairline, #ddd);
}
</style>
