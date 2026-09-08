<!--
  ADR-0006: CodeMirror 6 with decorations. Task #98's slice of that decision is the
  P0 one — "CodeMirror 6 mounted as a plain text box. No live preview" — so this
  component is deliberately not the hybrid live-preview editor ADR-0006 describes.
  It is the seam that editor will grow behind: everything outside this file talks to
  it through `setText`/`getText`/`focus`, never through CodeMirror's API directly, so
  P1's decorations land inside this component and nowhere else has to change.

  Mounted synchronously in `onMounted`, with no `await` before the `EditorView` is
  constructed — so the editor exists and is focusable in the same script execution
  that produces the page's first paint, not after an IPC round trip. See
  src/startup.ts and app/README.md for why that property is load-bearing rather
  than cosmetic.

  Mounted into a shadow root rather than directly into `host` (code-reviewer-2's
  #102 finding, confirmed live over the DevTools Protocol against a real WebView2
  window: a console error, "Applying inline style violates ... style-src 'self'").
  CodeMirror applies its own runtime CSS through style-mod, which chooses between two
  mechanisms depending on whether its mount root has a `.head` — a `<style>` tag with
  `textContent` set (an inline style, refused outright by `tauri.conf.json`'s CSP,
  which has no `'unsafe-inline'`, nonce or hash for it) when mounted into the
  document, or `adoptedStyleSheets` (constructed programmatically, not "inline style"
  under CSP at all) when mounted into a `ShadowRoot`, which has no `.head`. Refused
  silently: the DOM still gets a `<style>` tag with real content, so nothing throws,
  but none of its rules apply — no gutter layout, no line white-space, no cursor
  styling. A shadow root needs no CSP change, which is why it is the fix taken here
  rather than adding `'unsafe-inline'` to `style-src` (a real weakening) or a nonce
  Tauri does not emit for runtime-created style tags.
-->
<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";

import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, placeholder } from "@codemirror/view";

const props = defineProps<{
  placeholder?: string;
}>();

const emit = defineEmits<{
  save: [];
}>();

const host = ref<HTMLDivElement | null>(null);
let view: EditorView | null = null;

onMounted(() => {
  if (!host.value) {
    return;
  }
  // `mode: "open"` only affects whether *other* script can reach in via
  // `host.shadowRoot` — CSP's style-src is about where a style came from, not
  // whether the shadow tree is inspectable, so this choice is cosmetic here.
  const shadowRoot = host.value.attachShadow({ mode: "open" });
  view = new EditorView({
    parent: shadowRoot,
    state: EditorState.create({
      doc: "",
      extensions: [
        lineNumbers(),
        history(),
        keymap.of([
          // Preempts the browser's own save-page shortcut, matching the placeholder
          // shell's `event.preventDefault()` on the same key.
          {
            key: "Mod-s",
            preventDefault: true,
            run: () => {
              emit("save");
              return true;
            },
          },
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        EditorView.lineWrapping,
        EditorView.contentAttributes.of({ spellcheck: "false", "aria-label": "editor" }),
        ...(props.placeholder ? [placeholder(props.placeholder)] : []),
      ],
    }),
  });
});

onBeforeUnmount(() => {
  view?.destroy();
  view = null;
});

/** Replaces the whole document. Used once, when the read buffer arrives. */
function setText(text: string): void {
  if (!view) {
    return;
  }
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: text },
  });
}

function getText(): string {
  return view?.state.doc.toString() ?? "";
}

function focus(): void {
  view?.focus();
}

defineExpose({ setText, getText, focus });
</script>

<template>
  <div ref="host" class="cm-editor-host"></div>
</template>
