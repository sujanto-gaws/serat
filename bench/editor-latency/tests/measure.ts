// The measurement primitive for NFR-PRF-02, implemented against
// architecture.md §9.3 ("NFR-PRF-02: what is measured — frames, not
// milliseconds"), landed by the architect on task/p1-05-editor @ bdd4fd5,
// task #5. This supersedes every earlier version of this file, which used
// a plain wall-clock "keydown to paint" ms figure — §9.3 records why that
// was withdrawn: a bare `contenteditable` measures 17.1ms p95 under that
// definition, 16.2ms of which is simply the wait for the next vsync-aligned
// frame after a trusted keydown landing at an arbitrary phase of the
// refresh cycle, present for every editor including one that does nothing.
// 16ms is one frame at 60Hz, so §9.3 reads the requirement as a frame
// budget instead.
//
// **Binding definition (§9.3):** at least 95% of keystrokes are painted in
// the first display frame after the keystroke, in every scenario, in a
// 5,000-line note. Equivalently, the p95 of `frames_to_paint` is 1.
//
// - **Frame starts.** A continuous `requestAnimationFrame` loop records
//   each frame's start time `f_i`, running for the whole page lifetime
//   rather than per keystroke. Before real sampling, the median of 120
//   consecutive frame-start deltas is recorded as the refresh interval `R`.
// - **The keystroke's frame.** For a keystroke's own event `timeStamp` `t`
//   (captured on a capture-phase `keydown` listener), `p` is the end of
//   processing — stamped inside a bubble-phase `input` listener, which
//   fires after the target editor's own synchronous handling of that
//   input (CodeMirror's view update, or, for the control arm, the
//   browser's native contenteditable mutation). `p`'s counterpart
//   "processingStart" in the diagnostic below is approximated as `t`
//   itself: on a direct, synchronous dispatch the two differ by a
//   negligible queuing delay, and this harness has no lower-level signal
//   than the event's own `timeStamp` to use instead.
// - **`frames_to_paint`** is the number of frame starts in `(t, f_k]` where
//   `f_k` is the first frame whose start is `>= p`, counting `f_k`. It is
//   **1** when no frame boundary passed between `t` and `p`. If frame `k`
//   itself runs long (`f_{k+1} - f_k > 1.5R`), the paint missed its vsync
//   and `frames_to_paint` gains one — §9.3's long-frame adjustment.
// - **Diagnostic (b), reported and never gated:** the editor's own work —
//   handler time (`p - processingStart`) plus frame `k`'s own rendering,
//   from `f_k` to a post-paint timestamp taken by a `MessageChannel`
//   message posted inside frame `k`'s `rAF` callback. Excludes the vsync
//   wait by construction; the number to read when the frame metric fails.
//
// Keystrokes are driven through the browser's real input pipeline —
// `page.keyboard.press`, CDP `Input.dispatchKeyEvent` — never synthesised
// DOM events, at a fixed cadence between keystrokes (§9.3: 100ms).
import type { ElementHandle, Locator, Page } from "@playwright/test";

/** One keystroke's raw measurement, per §9.3. */
export interface FrameSample {
  /** Frame starts in `(t, f_k]`, counting `f_k`, plus §9.3's long-frame
   * adjustment. 1 = painted on time; the requirement's own unit. */
  framesToPaint: number;
  /** ms, `p - processingStart` (processingStart approximated as `t`,
   * see file header). Diagnostic (b)'s handler component. */
  handlerMs: number;
  /** ms, frame `k`'s own render time: `f_k` to the post-paint macrotask
   * fired from inside frame `k`'s `rAF` callback. Diagnostic (b)'s
   * render component. */
  renderMs: number;
}

// ---------------------------------------------------------------------
// Pure functions — no page, no browser, unit-tested directly in
// measure.unit.spec.ts against synthetic frame timelines.
// ---------------------------------------------------------------------

/** §9.3: a keystroke is "on time" iff it painted in the very first frame
 * after it — `frames_to_paint === 1`. */
export function paintedFirstFrame(sample: FrameSample): boolean {
  return sample.framesToPaint === 1;
}

/** Fraction (0..1) of samples painted on time. §9.3 states the acceptance
 * criterion directly as a percentage ("at least 95%/99% of keystrokes"),
 * so this is what both the subject (>=95%) and control arm (>=99%)
 * assertions read against — a p95-of-frames formulation is equivalent in
 * principle but this is what the requirement's own wording says. */
export function percentPaintedFirstFrame(samples: FrameSample[]): number {
  if (samples.length === 0) {
    throw new Error("percentPaintedFirstFrame of zero samples is undefined");
  }
  return samples.filter(paintedFirstFrame).length / samples.length;
}

/** Nearest-rank p95 of `frames_to_paint`, matching §9.3's own framing
 * ("the p95 of frames_to_paint is 1") and `bench/budget`'s TC-027
 * statistic. Reported alongside the percentage above, not in place of
 * it. */
export function p95FramesToPaint(samples: FrameSample[]): number {
  if (samples.length === 0) {
    throw new Error("p95 of zero samples is undefined");
  }
  const sorted = samples.map((s) => s.framesToPaint).sort((a, b) => a - b);
  const rank = Math.ceil(0.95 * sorted.length) - 1;
  return sorted[Math.max(0, rank)];
}

/** §9.3's `frames_to_paint` counting rule, isolated as a pure function
 * over a plain array of frame-start timestamps: the count of entries
 * strictly after `t` and up to and including `fk`. */
export function countFrameStartsInRange(
  frameStarts: number[],
  t: number,
  fk: number,
): number {
  return frameStarts.filter((f) => f > t && f <= fk).length;
}

/** §9.3's long-frame adjustment: frame `k` itself overran into the next
 * vsync if the gap to the following frame start exceeds 1.5x the
 * measured refresh interval. */
export function isLongFrame(
  fk: number,
  fNext: number,
  refreshIntervalMs: number,
): boolean {
  return fNext - fk > 1.5 * refreshIntervalMs;
}

/** §9.3's refresh-interval `R`: the median of consecutive frame-start
 * deltas over (at least) 120 consecutive frames. */
export function medianRefreshInterval(frameStarts: number[]): number {
  if (frameStarts.length < 2) {
    throw new Error("need at least 2 frame starts to compute deltas");
  }
  const deltas: number[] = [];
  for (let i = 1; i < frameStarts.length; i++) {
    deltas.push(frameStarts[i] - frameStarts[i - 1]);
  }
  deltas.sort((a, b) => a - b);
  const mid = Math.floor(deltas.length / 2);
  return deltas.length % 2 === 0 ? (deltas[mid - 1] + deltas[mid]) / 2 : deltas[mid];
}

// ---------------------------------------------------------------------
// Browser-integration side — installs the continuous rAF loop and the
// keydown/input correlation in-page, so the frame walk costs no Node
// round trip per frame.
// ---------------------------------------------------------------------

const INSTALLED_FLAG = "__seratLatencyCaptureInstalled";
const FRAMES_KEY = "__seratFrameStarts";
const REFRESH_INTERVAL_KEY = "__seratRefreshIntervalR";
const WATCH_TARGET_KEY = "__seratWatchTarget";
const WATCH_TEXT_BEFORE_KEY = "__seratWatchTextBefore";
const PENDING_SAMPLE_KEY = "__seratPendingSample";
const PENDING_T_KEY = "__seratPendingT";

const R_WARMUP_FRAMES = 121; // 120 deltas

/**
 * Installs, once per page: the continuous `requestAnimationFrame` loop
 * that records frame-start timestamps and, after `R_WARMUP_FRAMES`
 * frames, the median refresh interval `R`; and the capture-phase
 * `keydown` / bubble-phase `input` listener pair that correlates a
 * trusted keystroke to its `frames_to_paint`, handler time and render
 * time — only while "armed" for a specific element via
 * `measureKeystrokeToPaint` below.
 *
 * Idempotent — safe to call before every sample. Resolves once `R` has
 * been measured, so callers never race the warmup window.
 */
export async function installLatencyCapture(page: Page): Promise<void> {
  await page.evaluate(
    async ({
      installedFlag,
      framesKey,
      refreshIntervalKey,
      watchTargetKey,
      watchTextBeforeKey,
      pendingSampleKey,
      pendingTKey,
      warmupFrames,
    }) => {
      const w = window as unknown as Record<string, unknown>;
      if (w[installedFlag]) return;
      w[installedFlag] = true;
      w[framesKey] = [] as number[];

      function median(deltas: number[]): number {
        const sorted = [...deltas].sort((a, b) => a - b);
        const mid = Math.floor(sorted.length / 2);
        return sorted.length % 2 === 0 ? (sorted[mid - 1] + sorted[mid]) / 2 : sorted[mid];
      }

      // Continuous frame-start recorder + per-keystroke frame resolution,
      // in one rAF loop so a keystroke's frame walk shares the same
      // timestamps the refresh-interval measurement uses.
      const rafLoop = (ts: number) => {
        const frames = w[framesKey] as number[];
        frames.push(ts);

        const pendingT = w[pendingTKey] as number | undefined;
        const pendingFk = (w as unknown as { __seratPendingFk?: number }).__seratPendingFk;
        if (pendingT !== undefined && pendingFk === undefined) {
          const p = (w as unknown as { __seratPendingP?: number }).__seratPendingP;
          if (p !== undefined && ts >= p) {
            // This frame is f_k.
            (w as unknown as { __seratPendingFk?: number }).__seratPendingFk = ts;
            const framesToPaint = frames.filter((f) => f > pendingT && f <= ts).length;
            (w as unknown as { __seratFramesToPaint?: number }).__seratFramesToPaint = framesToPaint;
            const channel = new MessageChannel();
            channel.port2.onmessage = () => {
              (w as unknown as { __seratPostPaintTs?: number }).__seratPostPaintTs = performance.now();
            };
            channel.port1.postMessage(null);
          }
        } else if (pendingT !== undefined && pendingFk !== undefined) {
          // This frame is f_{k+1}: finalise the long-frame check and the
          // sample, then clear the in-flight state.
          const refreshInterval = (w[refreshIntervalKey] as number | undefined) ?? 16.667;
          let framesToPaint = (w as unknown as { __seratFramesToPaint: number }).__seratFramesToPaint;
          if (ts - pendingFk > 1.5 * refreshInterval) framesToPaint += 1;

          const target = w[watchTargetKey] as Element | undefined;
          const textBefore = w[watchTextBeforeKey] as number | undefined;
          const currentLength = target ? (target.textContent ?? "").length : -1;
          const textGrew = textBefore !== undefined && currentLength === textBefore + 1;

          const p = (w as unknown as { __seratPendingP: number }).__seratPendingP;
          const postPaintTs =
            (w as unknown as { __seratPostPaintTs?: number }).__seratPostPaintTs ?? pendingFk;

          w[pendingSampleKey] = {
            framesToPaint,
            handlerMs: p - pendingT,
            renderMs: postPaintTs - pendingFk,
            textGrew,
          };

          w[pendingTKey] = undefined;
          (w as unknown as { __seratPendingP?: number }).__seratPendingP = undefined;
          (w as unknown as { __seratPendingFk?: number }).__seratPendingFk = undefined;
          (w as unknown as { __seratFramesToPaint?: number }).__seratFramesToPaint = undefined;
          (w as unknown as { __seratPostPaintTs?: number }).__seratPostPaintTs = undefined;
        }

        if (frames.length === warmupFrames && w[refreshIntervalKey] === undefined) {
          const deltas: number[] = [];
          for (let i = 1; i < frames.length; i++) deltas.push(frames[i] - frames[i - 1]);
          w[refreshIntervalKey] = median(deltas);
        }

        requestAnimationFrame(rafLoop);
      };
      requestAnimationFrame(rafLoop);

      document.addEventListener(
        "keydown",
        (event) => {
          if (!event.isTrusted) return;
          // Only arm if a target is currently watched — otherwise this is
          // a keystroke outside a measurement window (shouldn't happen
          // given the fixed cadence, but a stray event must not corrupt
          // in-flight state).
          if (w[watchTargetKey] === undefined) return;
          w[pendingTKey] = event.timeStamp;
        },
        { capture: true },
      );
      document.addEventListener(
        "input",
        (event) => {
          if (!event.isTrusted) return;
          if (w[pendingTKey] === undefined) return;
          if ((w as unknown as { __seratPendingP?: number }).__seratPendingP !== undefined) return; // already stamped for this keystroke
          (w as unknown as { __seratPendingP?: number }).__seratPendingP = performance.now();
        },
        { capture: false },
      );

      // Wait for the refresh-interval warmup to complete before returning,
      // so callers never measure a real keystroke against an unmeasured R.
      await new Promise<void>((resolve) => {
        const check = () => {
          if (w[refreshIntervalKey] !== undefined) resolve();
          else requestAnimationFrame(check);
        };
        requestAnimationFrame(check);
      });
    },
    {
      installedFlag: INSTALLED_FLAG,
      framesKey: FRAMES_KEY,
      refreshIntervalKey: REFRESH_INTERVAL_KEY,
      watchTargetKey: WATCH_TARGET_KEY,
      watchTextBeforeKey: WATCH_TEXT_BEFORE_KEY,
      pendingSampleKey: PENDING_SAMPLE_KEY,
      pendingTKey: PENDING_T_KEY,
      warmupFrames: R_WARMUP_FRAMES,
    },
  );
}

/** The measured refresh interval `R` (ms), available after
 * `installLatencyCapture` resolves. */
export async function getRefreshInterval(page: Page): Promise<number> {
  return page.evaluate(
    (key) => (window as unknown as Record<string, number>)[key],
    REFRESH_INTERVAL_KEY,
  );
}

/**
 * Types one **trusted** character into `target` via `page.keyboard.press`
 * (CDP `Input.dispatchKeyEvent`) and returns its `FrameSample` per §9.3.
 * Throws loudly, naming what did not arrive, if no sample appears within
 * a generous timeout or if the text never grew by one character.
 */
export async function measureKeystrokeToPaint(
  target: Locator,
  char: string,
): Promise<FrameSample> {
  const page = target.page();
  await installLatencyCapture(page);
  await target.focus();

  const textBeforeLength = ((await target.textContent()) ?? "").length;
  const elementHandle = (await target.elementHandle()) as ElementHandle<Element> | null;
  if (!elementHandle) {
    throw new Error("measureKeystrokeToPaint: target locator resolved to no element handle");
  }

  await page.evaluate(
    ({ el, before, watchTargetKey, watchTextBeforeKey, pendingSampleKey }) => {
      const w = window as unknown as Record<string, unknown>;
      w[watchTargetKey] = el;
      w[watchTextBeforeKey] = before;
      w[pendingSampleKey] = null;
    },
    {
      el: elementHandle,
      before: textBeforeLength,
      watchTargetKey: WATCH_TARGET_KEY,
      watchTextBeforeKey: WATCH_TEXT_BEFORE_KEY,
      pendingSampleKey: PENDING_SAMPLE_KEY,
    },
  );

  await page.keyboard.press(char);

  let handle;
  try {
    handle = await page.waitForFunction(
      (pendingSampleKey) => (window as unknown as Record<string, unknown>)[pendingSampleKey] ?? null,
      PENDING_SAMPLE_KEY,
      { polling: 5, timeout: 2000 },
    );
  } catch {
    await elementHandle.dispose();
    throw new Error(
      `no sample arrived within 2000ms of pressing ${JSON.stringify(char)} — either the ` +
        `keystroke never reached the page as a trusted event, or its frame never resolved`,
    );
  }
  const sample = (await handle.jsonValue()) as FrameSample & { textGrew: boolean };
  await elementHandle.dispose();

  // Clear the watch so a stray event between samples cannot arm a phantom
  // measurement.
  await page.evaluate(
    ({ watchTargetKey, watchTextBeforeKey }) => {
      const w = window as unknown as Record<string, unknown>;
      w[watchTargetKey] = undefined;
      w[watchTextBeforeKey] = undefined;
    },
    { watchTargetKey: WATCH_TARGET_KEY, watchTextBeforeKey: WATCH_TEXT_BEFORE_KEY },
  );

  if (!sample.textGrew) {
    throw new Error(
      `keystroke ${JSON.stringify(char)} did not reach the editor: text length was ` +
        `${textBeforeLength}, never observed at ${textBeforeLength + 1}. The harness would ` +
        `otherwise have timed an idle frame.`,
    );
  }

  return { framesToPaint: sample.framesToPaint, handlerMs: sample.handlerMs, renderMs: sample.renderMs };
}

/**
 * Runs `count` keystrokes against `target`, each `intervalMs` apart
 * (§9.3: fixed 100ms cadence, "recorded" — the actual achieved cadence is
 * returned alongside the samples so a run that could not keep pace says
 * so rather than silently sampling faster or slower than specified).
 */
export async function runScenario(
  target: Locator,
  count: number,
  intervalMs = 100,
): Promise<{ samples: FrameSample[]; actualCadenceMs: number[] }> {
  const samples: FrameSample[] = [];
  const actualCadenceMs: number[] = [];
  let lastStart: number | null = null;
  for (let i = 0; i < count; i++) {
    const now = Date.now();
    if (lastStart !== null) {
      actualCadenceMs.push(now - lastStart);
      const wait = intervalMs - (now - lastStart);
      if (wait > 0) await new Promise((resolve) => setTimeout(resolve, wait));
    }
    lastStart = Date.now();
    samples.push(await measureKeystrokeToPaint(target, "x"));
  }
  return { samples, actualCadenceMs };
}
