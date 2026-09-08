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
//
// Task #108 (F-377, F-388, ADR-0014): the double rAF above is a promise this file
// makes, not one the Rust side could ever check — `crates/serat-app/src/lib.rs`'s
// marks test only greps `boot.js` for the substring `requestAnimationFrame`, and
// code-reviewer's mutation at F-377 showed exactly what that misses: hoist the
// `invoke` to script start, leave a no-op `requestAnimationFrame(() => {})` behind
// to satisfy the grep, and 86 tests still pass. `schedulePaintMark` below sends
// `PaintEvidence` alongside the call.
//
// Task #112 (F-396, ADR-0014): F-388's original reason for the conjunction was
// wrong — it assumed `index.html` still carried static skeleton markup that FCP
// could fire on before mount, and #98 replaced that guarantee with an empty `#app`
// div. But the conjunction survives on a better reason, and code-reviewer's F-396
// states it precisely: **`first-contentful-paint` alone is sound only as long as
// `app/index.html` stays contentless — a property nothing asserts, and one that
// lives in a different file (and a different crate's worth of trust) from the
// guard that depends on it.** A splash screen or loading skeleton, an entirely
// ordinary addition, would silently degrade the FCP-alone guard to F-377's
// mutation B with nothing going red. `raf1Ms`/`raf2Ms` are what make the guard's
// soundness independent of what `index.html` happens to contain today: both
// timestamps and `firstContentfulPaintMs` are on this page's own `performance.now()`
// clock, so `crates/serat-app` can validate `raf2Ms >= firstContentfulPaintMs`
// (the second frame did not precede the paint it is supposed to confirm) without
// needing to reconcile clocks. Proposed to tauri-app-dev-2 for the Rust half;
// `first_contentful_paint_ms` also needs to become `Option<f64>` there so a
// genuinely absent entry (no Paint Timing support) is a typed rejection rather
// than a raw deserialisation failure that produces no trace and no explanation.
// See `PaintEvidence` in `src/ipc/client.ts` for the field-by-field reasoning.

import { markFirstPaint, markVaultUsable, readFile } from "@/ipc/client";
import type { useVaultStore } from "@/stores/vault";

type VaultStore = ReturnType<typeof useVaultStore>;

/**
 * `PerformancePaintTiming`'s `first-contentful-paint` entry, if the webview recorded
 * one — see `PaintEvidence.firstContentfulPaintMs` for why this, and not
 * `navDomContentLoadedMs` below, is paint evidence.
 */
function readFirstContentfulPaintMs(): number | null {
  const entry = performance
    .getEntriesByType("paint")
    .find((candidate) => candidate.name === "first-contentful-paint");
  return entry ? entry.startTime : null;
}

/**
 * `PerformanceNavigationTiming.domContentLoadedEventEnd`, if the webview recorded a
 * navigation entry — see `PaintEvidence.navDomContentLoadedMs`. Its own function
 * only so `schedulePaintMark` reads as "gather evidence, then call", rather than
 * burying the fallible reads among timestamps that cannot fail.
 */
function readNavDomContentLoadedMs(): number | null {
  const [entry] = performance.getEntriesByType("navigation") as PerformanceNavigationTiming[];
  return entry ? entry.domContentLoadedEventEnd : null;
}

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
    requestAnimationFrame((raf1) => {
      requestAnimationFrame((raf2) => {
        markFirstPaint({
          firstContentfulPaintMs: readFirstContentfulPaintMs(),
          raf1Ms: raf1,
          raf2Ms: raf2,
          navDomContentLoadedMs: readNavDomContentLoadedMs(),
        }).then(
          (ms) => {
            store.setMark(`painted ${ms.toFixed(1)} ms`);
            resolve(ms);
          },
          (err: unknown) => {
            // Never silently swallowed: an unhandled rejection here would look
            // identical to a shell that painted nothing. It is also, now, the guard
            // actually firing rather than only a transport failure — a rejection here
            // can mean the evidence looked implausible (task #105), and the message
            // says which.
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
