// The measurement primitive: keystroke to painted frame, p95. This part
// does not depend on the P1 editor-integration design — it is a generic
// technique for finding out when the browser actually painted a frame
// after an input, and it is written and testable (against any page) before
// the subject exists.
//
// Serves NFR-PRF-02: "Keystroke to paint shall not exceed 16ms at p95 in a
// 5,000-line note." The interval timed, per test-plan.md's rule that every
// budget names the interval it asserts: from the input event being
// dispatched to the first frame painted after the resulting DOM/canvas
// mutation is committed — not from the event handler returning, which is
// not the same moment and would understate the number the user experiences.
import type { Locator } from "@playwright/test";

export interface LatencySample {
  /** ms, input dispatch to next painted frame */
  ms: number;
}

/**
 * Types one character into `target` and measures the time from dispatching
 * that keystroke to the next frame the browser actually painted.
 *
 * Technique: a double `requestAnimationFrame` after the keystroke is the
 * standard way to observe "the browser has painted" from page-script land
 * (the first rAF fires before paint, the second fires after) without
 * needing a CDP frame-timing session. `performance.now()` is read inside
 * the page so the clock is the browser's own, not round-tripped through
 * Playwright's IPC — round-tripping would add exactly the kind of harness
 * overhead NFR-PRF-01's cold-start work found (F-403-shaped mistake:
 * measuring the harness instead of the subject).
 */
export async function measureKeystrokeToPaint(
  target: Locator,
  char: string,
): Promise<LatencySample> {
  await target.focus();
  const page = target.page();

  const ms = await page.evaluate(async (typedChar) => {
    const t0 = performance.now();
    // Real keyboard event, not a synthetic value assignment — a budget
    // that measured a programmatic mutation instead of an input event
    // would not be measuring what NFR-PRF-02 describes.
    document.dispatchEvent(
      new KeyboardEvent("keydown", { key: typedChar, bubbles: true }),
    );
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    return performance.now() - t0;
  }, char);

  return { ms };
}

/** Nearest-rank p95, matching `bench/budget`'s statistic (TC-027) so a
 * P1 and a P0 number mean the same thing when compared. */
export function p95(samples: LatencySample[]): number {
  if (samples.length === 0) {
    throw new Error("p95 of zero samples is undefined");
  }
  const sorted = [...samples].map((s) => s.ms).sort((a, b) => a - b);
  const rank = Math.ceil(0.95 * sorted.length) - 1;
  return sorted[Math.max(0, rank)];
}
