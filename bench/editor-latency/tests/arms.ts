// The bare-contenteditable arms, all on the SAME fixture and the SAME scoring
// path as the real subject (measure.ts). Each loads the fixture ONCE, installs
// capture (measuring R on that document), then attaches the arm's own listener
// with a plain page.evaluate: no second navigation, no orphaned listeners.
//
//   positive : no blocking. Required >= 99% on time (§9.3 control arm).
//   negative : 2.2R busy-wait in the target's own `input` handler. Required >= 95% late.
//   render2  : 2.2R busy-wait in a rAF callback QUEUED by the input handler (where
//              CodeMirror's requestMeasure/DOM update runs; after the recorder's callback, so
//              frame timing alone cannot see it). REQUIRED second negative control, >= 95% late.
//   graded   : 1.5R in the input handler. Reported only; the expected band is 85-95% late.
import type { Page } from "@playwright/test";
import { installCapture, namespaceSource, runKeystrokes, type Cadence, type KeystrokeScore, type RunSummary } from "./measure";

export type ArmName = "positive" | "negative" | "render2" | "graded";

export const POSITIVE_MIN_ON_TIME = 0.99;
export const NEGATIVE_MIN_LATE = 0.95;
export const SUBJECT_MIN_ON_TIME = 0.95;
/** Reported, not gated. */
export const GRADED_BAND: [number, number] = [0.85, 0.95];

const FIXTURE_HTML =
  "<!doctype html><html><body>" +
  '<div id="arm-target" contenteditable="true" ' +
  'style="width:600px;height:200px;font:16px sans-serif;outline:none;"></div>' +
  "</body></html>";

export interface ArmResult {
  arm: ArmName;
  scores: KeystrokeScore[];
  summary: RunSummary;
  refreshIntervalMs: number;
}

/** `src` is the namespace source; the mutation test passes a mutated one. */
export async function measureArm(
  page: Page,
  arm: ArmName,
  count: number,
  cadence: Cadence = { kind: "strict" },
  src: string = namespaceSource(),
): Promise<ArmResult> {
  await page.setContent(FIXTURE_HTML);
  const { refreshIntervalMs } = await installCapture(page, src);
  const mult = { positive: 0, negative: 2.2, render2: 2.2, graded: 1.5 }[arm];
  if (mult > 0) {
    await page.evaluate(
      ({ ms, viaRaf }) => {
        const target = document.getElementById("arm-target");
        if (!target) throw new Error("arm-target not found");
        const spin = () => {
          const end = performance.now() + ms;
          while (performance.now() < end) {
            /* deliberately blocking: the point of the arm */
          }
        };
        target.addEventListener("input", () => (viaRaf ? requestAnimationFrame(spin) : spin()));
      },
      { ms: mult * refreshIntervalMs, viaRaf: arm === "render2" },
    );
  }
  const { scores, summary } = await runKeystrokes(page, page.locator("#arm-target"), count, {
    cadence,
    checkTextGrowth: true,
  });
  return { arm, scores, summary, refreshIntervalMs };
}
