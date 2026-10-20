<!--
  Task #98 (P0) mounted a plain text box; task #21 (P1: FR-EDT-01/02/03/10,
  NFR-PRF-02, ADR-0006, ADR-0016) puts the hybrid live-preview editor behind
  it. No file tree/lens switcher/wikilink UI yet — those are their own
  tasks (#1-#4, #22) — this file's job is still just "one buffer", but that
  buffer now saves through `save_patch` patch sets (ADR-0016 §2) instead of
  whole-document `write_file`, debounced/blurred/closed internally by
  `CodeMirrorEditor.vue` rather than driven from here.
-->
<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";

import type { UnlistenFn } from "@tauri-apps/api/event";

import type { SaveOutcome } from "@/editor/buffer";
import CodeMirrorEditor from "@/editor/CodeMirrorEditor.vue";
import { indexStatus, isIpcError, onIndexStatus } from "@/ipc/client";
import { hydrateBuffer, schedulePaintMark } from "@/startup";
import { useVaultStore } from "@/stores/vault";

const store = useVaultStore();
const editorRef = ref<InstanceType<typeof CodeMirrorEditor> | null>(null);
let unlistenIndexStatus: UnlistenFn | null = null;

onMounted(async () => {
  // NFR-PRF-02's harness seam (`bench/editor-latency/tests/subject.ts`,
  // `openNoteForLatencyTest`, task #11/#21): that harness drives the
  // production vite build with the IPC client stubbed (architecture.md
  // §9.3's own words), so it has no `note_opened`/`read_file` round trip to
  // hang a deterministic note-open off. This global, present the instant
  // the editor exists (synchronous, before any `await` in this function —
  // same property as the editor's own synchronous mount), lets the harness
  // load its fixture note directly through `loadNote` and reach the running
  // `EditorView` the same way a real note open would leave it: with a base
  // buffer, saves wired to the (stubbed) IPC adapter, and the reveal rule
  // live. `bench-hash` is a syntactically valid but obviously-fake hash —
  // nothing the harness types triggers a save, so it is never checked.
  //
  // Installed ONLY in a `vite build --mode bench` build (`npm run build:bench`):
  // `import.meta.env.MODE` is replaced at build time, so a release build
  // contains neither the branch nor the global. A shipped app must not expose
  // a window-level "replace the open note's buffer" entry point.
  if (import.meta.env.MODE === "bench") {
    window.__seratBenchOpenNote = (text: string) => {
      void editorRef.value?.loadNote(text, "__bench__.md", "bench-hash-not-a-real-blake3");
    };
  }

  // The paint mark, first and synchronous — code-reviewer's SHOULD-FIX on #98's
  // review. This used to run after `await onIndexStatus(...)` below, which is an IPC
  // round trip: the double requestAnimationFrame this schedules could miss the frame
  // that actually painted first and mark a later one, inflating S_shell — ADR-0014's
  // binding Serat term — with time that has nothing to do with painting. Scheduling
  // is synchronous (the rAF pair is registered before this call returns); only
  // *waiting* on the promise it returns happens later, in `hydrateBuffer` below.
  const paintMark = schedulePaintMark(store);

  // Installed next so the first transition the actor publishes is not lost — mirrors
  // `run`'s ordering in crates/serat-app/src/lib.rs. This IPC round trip runs
  // concurrently with the rAF pair above, not before it: nothing here can delay a
  // mark that was already scheduled on the line above.
  unlistenIndexStatus = await onIndexStatus((status) => store.setIndexState(status.state));

  store.setVaultRoot(window.__SERAT_BOOT__?.vault ?? null);

  // Late subscribers catch up by polling; the contract has both channels for this
  // reason (architecture §3.4).
  indexStatus().then((status) => store.setIndexState(status.state));

  if (editorRef.value) {
    // `hydrateBuffer` reads the note and installs it with `loadNote` in one
    // step, from the bytes that were read (never `getText()`): the editor is
    // read-only until then, so nothing typed can become the base text
    // paired with the on-disk hash (C-04, NFR-REL-01).
    await hydrateBuffer(store, editorRef.value, paintMark);
  }
});

onUnmounted(() => {
  unlistenIndexStatus?.();
});

/**
 * ADR-0016 §2: the editor saves patch sets through `save_patch`, debounced
 * 1s idle / on blur / on unmount, and on the save command — all internal to
 * `CodeMirrorEditor.vue`. This only reflects the outcome in the status
 * strip (§9.1: no modal on the write path; a refusal resolves in place).
 */
/** The save was refused outright; say exactly why and that nothing was written. */
function refusalMessage(error: unknown): string {
  if (isIpcError(error) && error.kind === "line_endings_would_fuse") {
    return `the save was refused because the note mixes line-ending styles at line ${error.line + 1}; nothing was written`;
  }
  return `the save was refused; nothing was written: ${JSON.stringify(error)}`;
}

/** Mod-Enter on a link. Opening notes needs the file tree and tabs (tasks #1-#4);
 * until then the host only reports what the link resolved to. */
function onFollowLink(link: { target: string; path: string | null }): void {
  store.setMark(link.path ? `link: ${link.path}` : `no note named \u201C${link.target}\u201D`);
}

function onSaveOutcome(outcome: SaveOutcome): void {
  switch (outcome.kind) {
    case "saved":
      store.setBaseHash(outcome.written.hash);
      store.setMark(`saved ${outcome.written.bytes} bytes`);
      break;
    case "stale":
      if (isIpcError(outcome.error) && outcome.error.kind === "stale_hash") {
        store.setMark("changed on disk since it was read — reopen to merge");
      } else {
        store.setMark(`not saved: ${JSON.stringify(outcome.error)}`);
      }
      break;
    case "refused":
      store.setMark(refusalMessage(outcome.error));
      break;
    case "rejected":
      store.setMark(`not saved: ${JSON.stringify(outcome.error)}`);
      break;
    case "noop":
    case "coalesced":
      break;
  }
}
</script>

<template>
  <header class="titlebar">
    <span class="wordmark">Serat</span>
    <span class="vault">{{ store.vaultRoot ?? "no vault" }}</span>
  </header>

  <main class="workspace">
    <CodeMirrorEditor
      ref="editorRef"
      placeholder="No note open — the file tree (task #1-#4) arrives separately."
      @save-outcome="onSaveOutcome"
      @follow-link="onFollowLink"
    />
  </main>

  <footer class="statusbar">
    <span class="index">{{ store.indexStatusText }}</span>
    <span class="marks">{{ store.marksText }}</span>
  </footer>
</template>
