// The front-end half of the NFR-PRF-01 measurement, carried over from the
// placeholder shell's boot.js (crates/serat-app/webview/boot.js, task S1) with its
// ordering unchanged. See crates/serat-app/src/startup.rs for the Rust side and why
// each ordering choice here is load-bearing rather than incidental.
//
//   mark_first_paint   from inside a double requestAnimationFrame. A single rAF
//                       callback runs *before* the frame is painted; the second one
//                       runs after it has been handed to the compositor, which is
//                       the earliest moment a browser can honestly say something was
//                       drawn. Taking this at script start, or in Rust at window
//                       creation, would pass 400 ms over a blank rectangle.
//
//   mark_vault_usable   once the last-open buffer is in the editor. The editor
//                       accepts keystrokes before this: it is mounted synchronously
//                       by App.vue's CodeMirrorEditor, independent of this module.
//
// Nothing here is a product feature (charter: "no product features" for P0).

import { markFirstPaint, markVaultUsable, readFile } from "@/ipc/client";
import type { useVaultStore } from "@/stores/vault";

type VaultStore = ReturnType<typeof useVaultStore>;

/**
 * Schedules the paint mark from inside a double `requestAnimationFrame` and returns
 * a promise that resolves once `mark_first_paint` has round-tripped.
 *
 * Call this once, as early as possible after the app mounts — scheduling it before
 * the buffer read is awaited means a slow disk cannot delay it; the two are
 * independent, matching architecture §3.4's second table row. **Nothing else may
 * call `markFirstPaint`**: it is the only source of `t0_to_window_visible_ms`, and a
 * second caller would just be a second way to get the ordering wrong.
 */
export function schedulePaintMark(store: VaultStore): Promise<number | null> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        markFirstPaint().then(
          (ms) => {
            store.setMark(`painted ${ms.toFixed(1)} ms`);
            resolve(ms);
          },
          (err: unknown) => {
            // Never silently swallowed: an unhandled rejection here would look
            // identical to a shell that painted nothing.
            store.setMark(`paint mark failed: ${JSON.stringify(err)}`);
            resolve(null);
          },
        );
      });
    });
  });
}

/** What `App.vue`'s CodeMirrorEditor exposes — see `defineExpose` there. */
export interface EditorHandle {
  setText(text: string): void;
}

/**
 * Reads `window.__SERAT_BOOT__.open` (if any), loads it into the editor, and marks
 * the vault usable — waiting for `paintMark` first, so `mark_vault_usable` can never
 * be reported before `mark_first_paint` even if the file read wins the race. Both
 * are IPC round trips and the read can finish first; the very first run of the
 * placeholder shell exited with the usable mark taken and the paint mark still in
 * flight, and produced no trace at all (see `crates/serat-app/src/startup.rs`'s
 * module note). The Rust side enforces the ordering too
 * (`startup::Marks::vault_usable`); this keeps the reported numbers honest rather
 * than merely ordered.
 */
export async function hydrateBuffer(
  store: VaultStore,
  editor: EditorHandle,
  paintMark: Promise<number | null>,
): Promise<void> {
  const open = window.__SERAT_BOOT__?.open ?? null;

  if (!open) {
    // Nothing to restore. Still usable: the editor takes keystrokes and a new note
    // can be written. `note: null` records that the number was taken over an empty
    // buffer, so it is legible as one rather than looking like a dropped read.
    await paintMark;
    const ms = await markVaultUsable(null);
    store.appendMark(`usable ${ms.toFixed(1)} ms`);
    return;
  }

  try {
    const file = await readFile({ path: open });
    editor.setText(file.text);
    store.setOpenBuffer(file.path, file.hash);
  } catch (err) {
    // A refused read is an explanation, not a modal (§9.1).
    editor.setText(`${open} could not be read: ${JSON.stringify(err)}`);
  }

  await paintMark;
  const ms = await markVaultUsable(store.openNote);
  store.appendMark(`usable ${ms.toFixed(1)} ms`);
}
