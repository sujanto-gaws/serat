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
// needing to reconcile clocks. Implemented on the Rust side too
// (`crates/serat-app/src/startup.rs::validate_paint_evidence`, task #112):
// `first_contentful_paint_ms` is `Option<f64>` there, so a genuinely absent entry
// (no Paint Timing support) is a typed, recorded rejection rather than a raw
// deserialisation failure that produces no trace and no explanation.
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
 * Resolves once the browser has actually recorded a `first-contentful-paint` entry,
 * or resolves `null` if this platform cannot ever produce one.
 *
 * **Not a synchronous read inside the double rAF — measured, and the measurement is
 * why this exists.** The first version of this file called
 * {@link readFirstContentfulPaintMs} synchronously from inside the second rAF
 * callback, on the reasoning that raf2 fires after the compositor has the frame, so
 * the paint entry should already be recorded by then. A real WebView2 run showed
 * that reasoning wrong: `performance.getEntriesByType("paint")` was still empty at
 * that point even though the frame had genuinely painted (confirmed separately, over
 * the DevTools Protocol, that the entry existed moments later with `#app` already
 * holding real content) — the entry's *availability to script* lags its *own
 * `startTime`*, which is not the same thing. `PerformanceObserver` with
 * `buffered: true` is the correct tool for exactly this gap: it either has the entry
 * already (a paint that beat this call) or delivers it the moment the browser
 * records one, rather than this file guessing how many frames the delay takes.
 *
 * Resolves `null`, rather than hanging, when `PerformanceObserver` or its `"paint"`
 * entry type is unavailable — the platform has nothing to ever deliver, so waiting
 * for it would wait forever (F-377: a missing capability is a refusal, not a stall).
 */
function whenFirstContentfulPaint(): Promise<number | null> {
  return new Promise((resolve) => {
    const existing = readFirstContentfulPaintMs();
    if (existing !== null) {
      resolve(existing);
      return;
    }
    if (
      typeof PerformanceObserver === "undefined" ||
      !(PerformanceObserver.supportedEntryTypes ?? []).includes("paint")
    ) {
      resolve(null);
      return;
    }
    const observer = new PerformanceObserver((list) => {
      const entry = list
        .getEntriesByType("paint")
        .find((candidate) => candidate.name === "first-contentful-paint");
      if (entry) {
        observer.disconnect();
        resolve(entry.startTime);
      }
    });
    observer.observe({ type: "paint", buffered: true });
  });
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

/** The two `requestAnimationFrame` callbacks' own timestamps — see `PaintEvidence.raf1Ms`. */
function whenDoubleRaf(): Promise<{ raf1Ms: number; raf2Ms: number }> {
  return new Promise((resolve) => {
    requestAnimationFrame((raf1Ms) => {
      requestAnimationFrame((raf2Ms) => {
        resolve({ raf1Ms, raf2Ms });
      });
    });
  });
}

/**
 * Schedules the paint mark and returns a promise that resolves once
 * `mark_first_paint` has round-tripped.
 *
 * Call this once, as early as possible after the app mounts — scheduling it before
 * the buffer read is awaited means a slow disk cannot delay it; the two are
 * independent, matching architecture §3.4's second table row. **Nothing else may
 * call `markFirstPaint`**: it is the only source of `t0_to_window_visible_ms`, and a
 * second caller would just be a second way to get the ordering wrong.
 *
 * Waits for **both** the double rAF and the `first-contentful-paint` entry — see
 * {@link whenFirstContentfulPaint} for why the entry needs an actual wait rather than
 * a synchronous read at the second rAF callback. Both start immediately and run
 * concurrently; the call fires once whichever is slower resolves.
 */
export function schedulePaintMark(store: VaultStore): Promise<number | null> {
  // Evidence that cannot change between attempts is read once: the paint entry is
  // buffered and does not move, so a retry reuses it rather than re-observing.
  const firstContentfulPaint = whenFirstContentfulPaint();
  const attempt = async (n: number): Promise<number | null> => {
    const [{ raf1Ms, raf2Ms }, firstContentfulPaintMs] = await Promise.all([
      whenDoubleRaf(),
      firstContentfulPaint,
    ]);
    try {
      const ms = await markFirstPaint({
        firstContentfulPaintMs,
        raf1Ms,
        raf2Ms,
        navDomContentLoadedMs: readNavDomContentLoadedMs(),
      });
      store.setMark(
        n === 1 ? `painted ${ms.toFixed(1)} ms` : `painted ${ms.toFixed(1)} ms (retried ${n - 1})`,
      );
      return ms;
    } catch (err: unknown) {
      // Never silently swallowed: an unhandled rejection here would look identical
      // to a shell that painted nothing, and it is the guard actually firing
      // rather than only a transport failure (task #105/#112).
      if (isFrameGateRefusal(err) && n < PAINT_MARK_MAX_ATTEMPTS) {
        // Task #186: hosted WebView2 sometimes delivers both rAF callbacks with one
        // timestamp. That is a refusal of this pair, not of the page; ask for a new
        // pair. The guard is unchanged: each attempt is a command call and the Rust
        // side prints each refusal under the trace env.
        store.setMark(`paint attempt ${n} refused (frame gate), retrying: ${JSON.stringify(err)}`);
        return attempt(n + 1);
      }
      store.setMark(
        `paint mark failed after ${n} attempt${n === 1 ? "" : "s"}: ${JSON.stringify(err)}`,
      );
      return null;
    }
  };
  return attempt(1);
}

/** Attempts (first included) before the page gives up on the paint mark — task #186. */
export const PAINT_MARK_MAX_ATTEMPTS = 5;

/**
 * True for `PaintEvidenceRejected` whose reason is the frame gate (`frame_gate_failed`
 * in `crates/serat-app/src/startup.rs`: the second rAF timestamp did not follow the
 * first). The wire shape is `{ reason: string }` only, so the reason text is the
 * discriminator; every other reason (missing paint entry, implausible value) and every
 * transport error is not retried, because a new frame pair cannot fix them.
 */
function isFrameGateRefusal(err: unknown): boolean {
  if (typeof err !== "object" || err === null || !("reason" in err)) return false;
  const reason = (err as { reason: unknown }).reason;
  return typeof reason === "string" && reason.includes("did not follow the first");
}

/** What `App.vue`'s CodeMirrorEditor exposes — see `defineExpose` there. */
export interface EditorHandle {
  setText(text: string): void;
  /** Installs the note and its buffer in one step (no gap in which edits could
   * be mistaken for the on-disk text). */
  loadNote(text: string, path: string, baseHash: string): Promise<boolean>;
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
    store.setOpenBuffer(file.path, file.hash);
    await editor.loadNote(file.text, file.path, file.hash);
  } catch (err) {
    // A refused read is an explanation, not a modal (§9.1).
    editor.setText(`${open} could not be read: ${JSON.stringify(err)}`);
  }

  await paintMark;
  const ms = await markVaultUsable(store.openNote);
  store.appendMark(`usable ${ms.toFixed(1)} ms`);
}
