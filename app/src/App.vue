<!--
  Task #98 (P0/impl): Vue 3 + CodeMirror 6, replacing the placeholder shell
  (crates/serat-app/webview/, task S1). P0 scope only — a plain text box, one
  buffer, no live preview (ADR-0006), no wikilink UI, no lens switcher, no file
  tree (`list_notes` is P1). See app/README.md.
-->
<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";

import type { UnlistenFn } from "@tauri-apps/api/event";

import CodeMirrorEditor from "@/editor/CodeMirrorEditor.vue";
import { indexStatus, isIpcError, onIndexStatus, writeFile } from "@/ipc/client";
import { hydrateBuffer, schedulePaintMark } from "@/startup";
import { useVaultStore } from "@/stores/vault";

const store = useVaultStore();
const editorRef = ref<InstanceType<typeof CodeMirrorEditor> | null>(null);
let unlistenIndexStatus: UnlistenFn | null = null;

onMounted(async () => {
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
    await hydrateBuffer(store, editorRef.value, paintMark);
  }
});

onUnmounted(() => {
  unlistenIndexStatus?.();
});

/**
 * Ctrl/Cmd-S writes the buffer back, optimistically locked on the hash it was read
 * at. A stale hash resolves in place: no modal on the write path (§9.1). This is
 * P0's whole-file write (`write_file`); the byte-range write-back of ADR-0008
 * arrives with views in P2 and is a different command.
 */
async function onSave(): Promise<void> {
  if (!store.openNote || !editorRef.value) {
    return;
  }
  try {
    const written = await writeFile({
      path: store.openNote,
      text: editorRef.value.getText(),
      base_hash: store.baseHash,
    });
    store.setBaseHash(written.hash);
    store.setMark(`saved ${written.bytes} bytes`);
  } catch (err) {
    if (isIpcError(err) && err.kind === "stale_hash") {
      store.setMark("changed on disk since it was read — reopen to merge");
    } else {
      store.setMark(`not saved: ${JSON.stringify(err)}`);
    }
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
      placeholder="P0 shell — a plain text box. Live preview is P1 (ADR-0006)."
      @save="onSave"
    />
  </main>

  <footer class="statusbar">
    <span class="index">{{ store.indexStatusText }}</span>
    <span class="marks">{{ store.marksText }}</span>
  </footer>
</template>
