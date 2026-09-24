// A CONTROL ARM, required on every run (architecture.md §9.3): the same
// harness, run against the simplest possible input surface — a bare
// `<div contenteditable>` with no editor logic at all — through the same
// scenario driver. Self-contained: no dependency on #5/#21 or
// `subject.ts`, so it runs today and every time this harness runs, unlike
// the real NFR-PRF-02 subject.
//
// §9.3: "A bare `contenteditable` goes through the same harness, scenarios
// and cadence. It must show >= 99% of keystrokes on time, or the run is
// rejected: a harness or machine that cannot show a trivial editor keeping
// up cannot judge ours." 99%, not 95% — the control arm is held to a
// tighter bar than the real subject precisely because it has no excuse.
import type { Page } from "@playwright/test";
import { runScenario, percentPaintedFirstFrame, type FrameSample } from "./measure";

export const CONTROL_ARM_HTML =
  '<!doctype html><html><body>' +
  '<div id="control-target" contenteditable="true" ' +
  'style="width:600px;height:200px;font:16px sans-serif;outline:none;"></div>' +
  "</body></html>";

/** §9.3's control-arm bar: at least 99% of keystrokes on time. */
export const CONTROL_ARM_MIN_ON_TIME = 0.99;

export interface ControlArmResult {
  samples: FrameSample[];
  percentOnTime: number;
}

/**
 * Loads the bare-contenteditable fixture into `page` (replacing whatever
 * was there) and runs `sampleCount` keystrokes against it with the exact
 * same scenario driver the real subject uses.
 */
export async function measureControlArm(
  page: Page,
  sampleCount: number,
): Promise<ControlArmResult> {
  await page.setContent(CONTROL_ARM_HTML);
  const target = page.locator("#control-target");
  const { samples } = await runScenario(target, sampleCount);
  return { samples, percentOnTime: percentPaintedFirstFrame(samples) };
}
