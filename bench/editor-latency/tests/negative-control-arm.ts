// A NEGATIVE CONTROL ARM (code review, 2026-09-24): a contenteditable
// whose own `input` handler busy-waits before returning, guaranteeing the
// main thread is blocked past the next vsync and the edit cannot be
// visible in the first frame after the keystroke. This is the complement
// of `control-arm.ts`'s positive control: where the positive control
// proves the harness can report a good result for a good subject, this
// proves it can report a *bad* result for a subject that is genuinely,
// deliberately bad — a harness that always passes regardless of the
// subject would pass the positive control too, and only a negative
// control catches that.
//
// **Required bar: at least 95% of keystrokes must be late (framesToPaint
// > 1), or the run is rejected** — the mirror image of the positive
// control's 99%-on-time bar.
//
// **The busy-wait duration is 3x the measured refresh interval `R`, not
// the ~1.5R code review suggested — measured empirically, not guessed.**
// A block that happens *before* `p` (inside event processing, which is
// where this fixture's busy-wait lives) is invisible to
// `frames_to_paint`'s base count for a structural reason: the same
// main-thread block that delays processing also blocks this harness's
// own `rAF`-based frame recorder, so no intervening frame starts get
// recorded to count. Only once the block ends does f_k get found, on
// whichever tick is next available — meaning detection depends on the
// *long-frame* rule (`f_{k+1} - f_k > 1.5R`) catching an overrun on the
// frame immediately after f_k, not on the base count. Measured directly
// on this harness: 1.5R gave 0% late, 2R gave 7% late, 3R gave 100% late
// over 100 keystrokes, repeatably. **1.5R is not long enough for this
// technique to reliably detect a blocking handler; 3R is.** That the
// literal ~1.5R suggestion doesn't work is itself the negative control
// doing its job — it would have been a silent gap in what this harness
// can detect if the bar had been left where the intuition first put it.
//
// **One document, not two.** An earlier version measured `R` on a
// throwaway blank document, then called `page.setContent` a second time
// to load the real busy-wait fixture built from that `R`. That hung every
// run: `page.setContent` replaces `document` (which is why a keydown
// listener registered against the old document stops firing) but *not*
// `window` (global state like a flag on `window` survives), so
// `installLatencyCapture`'s own idempotency guard saw its flag already
// set from the first document and skipped re-registering the keydown/input
// listeners on the second — silently, because the guard has no way to
// know its document changed under it. This version loads the fixture
// **once**, measures `R` via the normal warmup on that same document, and
// only then attaches the busy-wait handler to the target element with
// a plain `page.evaluate` call — no second navigation, no orphaned
// listeners.
import type { Page } from "@playwright/test";
import {
  installLatencyCapture,
  getRefreshInterval,
  runScenario,
  percentPaintedFirstFrame,
  type FrameSample,
} from "./measure";

/** At least this fraction of keystrokes must be late for the negative
 * control to be believed. */
export const NEGATIVE_CONTROL_MIN_LATE = 0.95;

export interface NegativeControlResult {
  samples: FrameSample[];
  percentLate: number;
  refreshIntervalMs: number;
}

const FIXTURE_HTML =
  '<!doctype html><html><body>' +
  '<div id="negative-control-target" contenteditable="true" ' +
  'style="width:600px;height:200px;font:16px sans-serif;outline:none;"></div>' +
  "</body></html>";

/**
 * Loads the bare-contenteditable fixture once, installs latency capture
 * (measuring `R` via its normal warmup), attaches an `input` handler that
 * busy-waits `3R` before returning (see the file header for why not the
 * originally-suggested `1.5R`), and runs `sampleCount` keystrokes against
 * it with the same scenario driver the real subject uses.
 */
export async function measureNegativeControlArm(
  page: Page,
  sampleCount: number,
): Promise<NegativeControlResult> {
  await page.setContent(FIXTURE_HTML);
  await installLatencyCapture(page);
  const refreshIntervalMs = await getRefreshInterval(page);
  const busyWaitMs = refreshIntervalMs * 3;

  await page.evaluate(
    ({ busyWaitMs }) => {
      const target = document.getElementById("negative-control-target");
      if (!target) throw new Error("negative-control-target not found");
      target.addEventListener("input", () => {
        const deadline = performance.now() + busyWaitMs;
        // Deliberately blocking the main thread -- the whole point of
        // this fixture.
        while (performance.now() < deadline) {
          /* empty */
        }
      });
    },
    { busyWaitMs },
  );

  const target = page.locator("#negative-control-target");
  const { samples } = await runScenario(target, sampleCount);

  return {
    samples,
    percentLate: 1 - percentPaintedFirstFrame(samples),
    refreshIntervalMs,
  };
}
