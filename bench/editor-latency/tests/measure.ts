// The measurement primitive: keystroke to painted frame, p95. This part
// does not depend on the P1 editor-integration design — it is a generic
// technique for finding out when the browser actually painted a frame
// after an input, and it is written and testable (against any page) before
// the subject exists.
//
// Serves NFR-PRF-02: "Keystroke to paint shall not exceed 16ms at p95 in a
// 5,000-line note." The interval timed, per test-plan.md's rule that every
// budget names the interval it asserts: from the *trusted* keydown event's
// own `timeStamp` (t0) to the first macrotask the event loop runs after
// that frame's rendering — style, layout, paint — step completes (t1), per
// the HTML spec's "update the rendering" steps. A macrotask (here, a
// `MessageChannel` message) posted from *inside* the `requestAnimationFrame`
// callback that follows the keystroke fires only once rendering for that
// frame has finished, so it observes "painted" without waiting for a whole
// extra frame the way a second `requestAnimationFrame` callback would (that
// fires on the *next* frame, after rendering, and was this file's original
// bug: it added close to one full frame — ~16.7ms at 60Hz — to every
// sample, which is the size of the entire NFR-PRF-02 budget, so the budget
// was red by construction regardless of editor performance).
//
// **Known limit, stated rather than hidden:** this technique is accurate to
// the browser's own frame cadence — if the renderer coalesces or skips a
// frame under load, or composites/presents to the screen on a separate
// thread some interval after the main-thread rendering step this measures,
// that gap is not observed. It is not a substitute for CDP frame-presentation
// tracing, which is the third option available if this figure and a real
// user's experience diverge enough to matter; kept as plain page-script
// timing for now because it needs no CDP session and stays comparable to
// `performance.now()` elsewhere in this harness.
import type { Locator, Page } from "@playwright/test";

export interface LatencySample {
  /** ms, trusted keydown `timeStamp` to the post-render macrotask */
  ms: number;
}

const INSTALLED_FLAG = "__seratLatencyCaptureInstalled";
const ENTRIES_KEY = "__seratLatencyEntries";

type PageEntries = { ms: number }[];

/**
 * Installs, once per page, a capture-phase `keydown` listener that times
 * every *trusted* keystroke from its own `event.timeStamp` to the next
 * post-render macrotask (see file header) and appends `{ ms }` to a
 * page-global array this module polls. Idempotent — safe to call before
 * every sample; does nothing on the second and later calls.
 */
export async function installLatencyCapture(page: Page): Promise<void> {
  await page.evaluate(
    ({ installedFlag, entriesKey }) => {
      const w = window as unknown as Record<string, unknown>;
      if (w[installedFlag]) return;
      w[installedFlag] = true;
      w[entriesKey] = [] as PageEntries;
      document.addEventListener(
        "keydown",
        (event) => {
          if (!event.isTrusted) return; // only real, OS-level input counts
          const t0 = event.timeStamp;
          requestAnimationFrame(() => {
            const channel = new MessageChannel();
            channel.port2.onmessage = () => {
              const t1 = performance.now();
              (w[entriesKey] as PageEntries).push({ ms: t1 - t0 });
            };
            channel.port1.postMessage(null);
          });
        },
        { capture: true },
      );
    },
    { installedFlag: INSTALLED_FLAG, entriesKey: ENTRIES_KEY },
  );
}

/**
 * Types one **trusted** character into `target` via `page.keyboard.press`,
 * which drives the browser's real input pipeline through CDP — unlike
 * `document.dispatchEvent(new KeyboardEvent(...))`, which is
 * `isTrusted: false` and which CodeMirror's own input handling ignores
 * entirely. (An earlier version of this file used the synthetic form: it
 * timed real frames, but never actually typed a character, so it was
 * silently measuring idle frames.)
 *
 * After the keystroke, asserts the editor's visible text actually grew by
 * one character. A keystroke that did not reach the editor — swallowed,
 * untrusted, or routed somewhere else — fails the sample loudly instead of
 * quietly contributing a fast, meaningless number to the p95.
 */
export async function measureKeystrokeToPaint(
  target: Locator,
  char: string,
): Promise<LatencySample> {
  const page = target.page();
  await installLatencyCapture(page);
  await target.focus();

  const textBefore = (await target.textContent()) ?? "";
  const before = await page.evaluate(
    (key) => ((window as unknown as Record<string, unknown>)[key] as PageEntries).length,
    ENTRIES_KEY,
  );

  await page.keyboard.press(char);

  const handle = await page.waitForFunction(
    ({ key, expected }) => {
      const entries = (window as unknown as Record<string, unknown>)[key] as
        | PageEntries
        | undefined;
      return entries && entries.length > expected ? entries[entries.length - 1] : null;
    },
    { key: ENTRIES_KEY, expected: before },
  );
  const sample = (await handle.jsonValue()) as LatencySample;

  const textAfter = (await target.textContent()) ?? "";
  if (textAfter.length !== textBefore.length + 1) {
    throw new Error(
      `keystroke ${JSON.stringify(char)} did not reach the editor: text length was ` +
        `${textBefore.length}, expected ${textBefore.length + 1} after typing, got ` +
        `${textAfter.length}. The harness would otherwise have timed an idle frame.`,
    );
  }

  return sample;
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
