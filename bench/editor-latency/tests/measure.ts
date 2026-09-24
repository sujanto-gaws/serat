// The measurement primitive for NFR-PRF-02, implemented against
// architecture.md §9.3 ("NFR-PRF-02: what is measured — frames, not
// milliseconds"), landed by the architect on task/p1-05-editor @ bdd4fd5,
// task #5, and corrected by code-reviewer's second pass (2026-09-24) —
// see the two fixes below, both binding.
//
// **Binding definition (§9.3, being reworded by the architect to match
// fix 1):** at least 95% of keystrokes are painted in the first display
// frame after the keystroke, in every scenario, in a 5,000-line note.
//
// **Fix 1 — f_k is when the frame's callback RUNS, not its scheduled
// timestamp.** The previous version picked f_k as the first frame whose
// `rAF` *timestamp* (`ts`) was `>= p`. Code review's probe found 18/18
// "late" samples in a run had `p` land *after* a frame's `ts` but *before*
// that frame's callback had actually executed (`ts < p < run`) — meaning
// the edit was still synchronously visible when that frame's callback did
// run, so the edit *was* captured in that frame, but the old rule skipped
// it to the next one and miscounted a missed frame that never happened.
// **f_k is now the first frame whose callback RUNS after `p`** — tracked
// as `{ ts, run: performance.now() }` per frame — and `frames_to_paint` is
// the count of frame timestamps in `(t, ts_k]`, **clamped to >= 1** (the
// clamp is a floor against exactly this kind of boundary case, not a
// normal outcome).
//
// **Fix 2 — one implementation measures, not two.** The in-page frame walk
// used to reimplement the counting and long-frame logic inline inside the
// `page.evaluate` closure, as a second, hand-copied version of
// `countFrameStartsInRange`/`isLongFrame`/`medianRefreshInterval` below.
// Code review mutated the in-page copy alone (`const framesToPaint = 1`)
// and every unit test here stayed green, because the tests exercise the
// Node-side originals, never the in-page copies — textbook "the guard was
// tested and its caller was not" (test-plan.md). **The in-page walk now
// calls these exact functions**, injected into the page verbatim via
// `Function.prototype.toString()` in `installLatencyCapture` (see
// `PURE_FUNCTION_SOURCE` below) rather than being retyped by hand a second
// time. There is exactly one implementation of each; the page runs it by
// reference, not by copy.
//
// Serves NFR-PRF-02. The interval timed, per test-plan.md's rule that
// every budget names it: from the trusted keydown's own `timeStamp` (`t`)
// to the post-render macrotask of the frame in which the edit first
// became visible (`f_k`, redefined above). Diagnostic (b), reported and
// never gated: the editor's own work — handler time (`p - processingStart`,
// `processingStart` approximated as `t`, see below) plus frame `k`'s own
// render time.
import type { ElementHandle, Locator, Page } from "@playwright/test";

/** One keystroke's raw measurement, per §9.3. */
export interface FrameSample {
  /** Frame starts in `(t, ts_k]`, counting `ts_k`, clamped to >= 1, plus
   * §9.3's long-frame adjustment. 1 = painted on time. */
  framesToPaint: number;
  /** ms, `p - processingStart` (processingStart approximated as `t`). */
  handlerMs: number;
  /** ms, frame `k`'s own render time: `ts_k` to the post-paint macrotask
   * fired from inside frame `k`'s `rAF` callback. */
  renderMs: number;
}

// ---------------------------------------------------------------------
// Pure functions — no page, no browser. Unit-tested directly in
// measure.unit.spec.ts AND injected verbatim into the page (see
// PURE_FUNCTION_SOURCE) so the code under test and the code being run
// in-page are the same function, not a hand-copied lookalike (fix 2).
// Written as plain `function` declarations, with no destructuring of
// imported types inside the body, specifically so `.toString()` produces
// self-contained source with nothing left to resolve.
// ---------------------------------------------------------------------

/** §9.3: a keystroke is "on time" iff it painted in the very first frame
 * after it — `frames_to_paint === 1`, not `<= 1`: a sample cannot
 * legitimately be *less* than 1 once the counting rule clamps to a floor
 * of 1, so `<= 1` and `=== 1` agree on every real sample and only diverge
 * on a corrupted one (`framesToPaint: 0`), which is exactly the case a
 * `<= 1` mutant would wrongly accept as on-time. */
export function paintedFirstFrame(sample: FrameSample): boolean {
  return sample.framesToPaint === 1;
}

/** Fraction (0..1) of samples painted on time. */
export function percentPaintedFirstFrame(samples: FrameSample[]): number {
  if (samples.length === 0) {
    throw new Error("percentPaintedFirstFrame of zero samples is undefined");
  }
  return samples.filter(paintedFirstFrame).length / samples.length;
}

/** Nearest-rank p95 of `frames_to_paint`. */
export function p95FramesToPaint(samples: FrameSample[]): number {
  if (samples.length === 0) {
    throw new Error("p95 of zero samples is undefined");
  }
  const sorted = samples.map((s) => s.framesToPaint).sort((a, b) => a - b);
  const rank = Math.ceil(0.95 * sorted.length) - 1;
  return sorted[Math.max(0, rank)];
}

/** One recorded frame: its scheduled `requestAnimationFrame` timestamp
 * (`ts`) and the wall-clock time its callback actually ran (`run`) —
 * fix 1's distinction. */
export interface FrameRecord {
  ts: number;
  run: number;
}

/** Fix 1: `f_k` is the first frame whose callback **runs** after `p`, not
 * the first whose scheduled `ts` is `>= p` — code review's probe found
 * `ts < p < run` cases where the edit was still captured in that frame
 * (its callback had not run yet), which the old `ts >= p` rule wrongly
 * skipped past. Returns that frame's `ts` (what `countFrameStartsInRange`
 * needs as `fk`). Injected into the page — see `countFrameStartsInRange`'s
 * doc comment.
 *
 * @throws if no recorded frame's callback has run after `p` yet — the
 * caller's signal to keep waiting for more frames, not an error.
 */
export function selectFk(frames: FrameRecord[], p: number): number {
  const match = frames.find((f) => f.run > p);
  if (!match) {
    throw new Error("no frame's callback has run after p yet");
  }
  return match.ts;
}

/** §9.3's `frames_to_paint` counting rule: the count of frame-start
 * timestamps strictly after `t` and up to and including `fk` (`ts_k`),
 * **clamped to a floor of 1** — fix 1's boundary case (t < ts_k always in
 * an honest run, so this is a floor against clock/ordering edge cases,
 * not a normal path). Injected into the page (see
 * `PURE_FUNCTION_SOURCE`) — the in-page walk calls this exact function. */
export function countFrameStartsInRange(
  frameStarts: number[],
  t: number,
  fk: number,
): number {
  const count = frameStarts.filter((f) => f > t && f <= fk).length;
  return Math.max(1, count);
}

/** §9.3's long-frame adjustment: frame `k` itself overran into the next
 * vsync if the gap to the following frame start exceeds 1.5x the
 * measured refresh interval. Injected into the page — see
 * `countFrameStartsInRange`'s doc comment. */
export function isLongFrame(
  fk: number,
  fNext: number,
  refreshIntervalMs: number,
): boolean {
  return fNext - fk > 1.5 * refreshIntervalMs;
}

/** §9.3's refresh-interval `R`: the median of consecutive frame-start
 * deltas over (at least) 120 consecutive frames. Injected into the page
 * — the in-page warmup calls this exact function, not a second hand-copied
 * median. */
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

/** The exact source of the three in-page pure functions, joined for
 * injection via `page.addInitScript`. `Function.prototype.toString()` on
 * a plain function declaration returns its own source text (types already
 * stripped by esbuild/tsc before this runs), so this is not a
 * transcription — it is the same bytes the functions above compile to,
 * read back. Declared as top-level `function` statements in the injected
 * script, so in a non-module `<script>` context they attach directly to
 * `window` and are callable in-page by name with no namespace object. */
function pureFunctionSource(): string {
  return [selectFk, countFrameStartsInRange, isLongFrame, medianRefreshInterval]
    .map((fn) => fn.toString())
    .join("\n");
}

// ---------------------------------------------------------------------
// Browser-integration side.
// ---------------------------------------------------------------------

const INSTALLED_FLAG = "__seratLatencyCaptureInstalled";
const FRAME_RECORDS_KEY = "__seratFrameRecords";
const REFRESH_INTERVAL_KEY = "__seratRefreshIntervalR";
const WATCH_TARGET_KEY = "__seratWatchTarget";
const WATCH_TEXT_BEFORE_KEY = "__seratWatchTextBefore";
const PENDING_SAMPLE_KEY = "__seratPendingSample";
const PENDING_T_KEY = "__seratPendingT";

const R_WARMUP_FRAMES = 121; // 120 deltas

/**
 * Installs, once per page: the injected pure functions (fix 2); the
 * continuous `requestAnimationFrame` loop that records, per frame, both
 * `ts` (the rAF timestamp argument) and `run` (`performance.now()` at the
 * moment the callback actually executes — fix 1's distinction) and, after
 * `R_WARMUP_FRAMES` frames, the median refresh interval `R` (via the
 * injected `medianRefreshInterval`); and the capture-phase `keydown` /
 * bubble-phase `input` listener pair that correlates a trusted keystroke
 * to its `frames_to_paint`, handler time and render time — only while
 * "armed" for a specific element via `measureKeystrokeToPaint` below.
 *
 * Idempotent — safe to call before every sample **against the same
 * document**. **Not safe across a `page.setContent`/navigation on the
 * same `page`:** the installed flag lives on `window`, which
 * `page.setContent` does not replace, but the DOM listeners it guards are
 * attached to `document`, which it does — so a second call after a
 * `setContent` sees the flag already set and silently skips
 * re-registering listeners that are already orphaned, and every
 * subsequent keystroke hangs waiting for a sample that can never arrive.
 * `negative-control-arm.ts`'s doc comment is where this was found; a
 * caller that needs a fresh document mid-run must build it once and
 * attach anything document-specific afterward, not navigate twice.
 * Resolves once `R` has been measured.
 */
export async function installLatencyCapture(page: Page): Promise<void> {
  // `addInitScript` only takes effect on the *next* navigation -- and
  // `control-arm.ts` (and the real subject, once wired) calls
  // `page.setContent`/navigates *before* this function ever runs, so
  // relying on it alone left `window.selectFk` etc. undefined for the
  // document already loaded, which silently threw inside the first rAF
  // callback and hung every caller forever (the warmup promise never
  // resolves once the loop that would set it has crashed). Registered
  // anyway for any future navigation, but the actual, immediate
  // definition is the indirect `eval` below: it runs in global scope (a
  // direct `eval(src)` here would run in this arrow function's local
  // scope instead), so the injected top-level function declarations
  // attach to `window` synchronously, before the rAF loop below is ever
  // scheduled.
  await page.addInitScript({ content: pureFunctionSource() });
  await page.evaluate((src) => {
    (0, eval)(src);
  }, pureFunctionSource());

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
      type FrameRecordJs = { ts: number; run: number };
      const w = window as unknown as Record<string, unknown> & {
        selectFk: (frames: FrameRecordJs[], p: number) => number;
        countFrameStartsInRange: (frameStarts: number[], t: number, fk: number) => number;
        isLongFrame: (fk: number, fNext: number, refreshIntervalMs: number) => boolean;
        medianRefreshInterval: (frameStarts: number[]) => number;
      };
      if (w[installedFlag]) return;
      w[installedFlag] = true;
      w[framesKey] = [] as FrameRecordJs[];

      // Continuous frame recorder ({ts, run} pairs, fix 1) + per-keystroke
      // frame resolution, in one rAF loop so a keystroke's frame walk
      // shares the same records the refresh-interval measurement uses.
      const rafLoop = (ts: number) => {
        const run = performance.now(); // fix 1: when this callback actually runs
        const frames = w[framesKey] as FrameRecordJs[];
        frames.push({ ts, run });

        const pendingT = w[pendingTKey] as number | undefined;
        const pendingFkIdx = (w as unknown as { __seratPendingFkIdx?: number }).__seratPendingFkIdx;

        if (pendingT !== undefined && pendingFkIdx === undefined) {
          const p = (w as unknown as { __seratPendingP?: number }).__seratPendingP;
          if (p !== undefined) {
            // Fix 1: f_k is the first frame whose callback RUNS after p
            // (selectFk), not the first whose scheduled ts is >= p.
            // Throws (caught here) until a qualifying frame has actually
            // run -- that is the normal "keep waiting" path, not an error.
            let tsK: number | undefined;
            try {
              tsK = w.selectFk(frames, p);
            } catch {
              tsK = undefined;
            }
            if (tsK !== undefined) {
              (w as unknown as { __seratPendingFkIdx?: number }).__seratPendingFkIdx = frames.length - 1;
              (w as unknown as { __seratFkTs?: number }).__seratFkTs = tsK;
              const framesToPaint = w.countFrameStartsInRange(
                frames.map((f) => f.ts),
                pendingT,
                tsK,
              );
              (w as unknown as { __seratFramesToPaint?: number }).__seratFramesToPaint = framesToPaint;
              const channel = new MessageChannel();
              channel.port2.onmessage = () => {
                (w as unknown as { __seratPostPaintTs?: number }).__seratPostPaintTs = performance.now();
              };
              channel.port1.postMessage(null);
            }
          }
        } else if (pendingT !== undefined && pendingFkIdx !== undefined) {
          // This frame is the one immediately after f_k: finalise the
          // long-frame check and the sample.
          const refreshInterval = w[refreshIntervalKey] as number | undefined;
          if (refreshInterval === undefined) {
            throw new Error(
              "the long-frame check ran before the refresh interval R was measured -- " +
                "installLatencyCapture must finish its warmup before any keystroke is armed",
            );
          }
          const fk = (w as unknown as { __seratFkTs: number }).__seratFkTs;
          let framesToPaint = (w as unknown as { __seratFramesToPaint: number }).__seratFramesToPaint;
          if (w.isLongFrame(fk, ts, refreshInterval)) framesToPaint += 1;

          const target = w[watchTargetKey] as Element | undefined;
          const textBefore = w[watchTextBeforeKey] as number | undefined;
          const currentLength = target ? (target.textContent ?? "").length : -1;
          const textGrew = textBefore !== undefined && currentLength === textBefore + 1;

          const p = (w as unknown as { __seratPendingP: number }).__seratPendingP;
          const postPaintTs =
            (w as unknown as { __seratPostPaintTs?: number }).__seratPostPaintTs ?? fk;

          w[pendingSampleKey] = {
            framesToPaint,
            handlerMs: p - pendingT,
            renderMs: postPaintTs - fk,
            textGrew,
          };

          w[pendingTKey] = undefined;
          (w as unknown as { __seratPendingP?: number }).__seratPendingP = undefined;
          (w as unknown as { __seratPendingFkIdx?: number }).__seratPendingFkIdx = undefined;
          (w as unknown as { __seratFkTs?: number }).__seratFkTs = undefined;
          (w as unknown as { __seratFramesToPaint?: number }).__seratFramesToPaint = undefined;
          (w as unknown as { __seratPostPaintTs?: number }).__seratPostPaintTs = undefined;
        }

        if (frames.length === warmupFrames && w[refreshIntervalKey] === undefined) {
          w[refreshIntervalKey] = w.medianRefreshInterval(frames.map((f) => f.ts));
        }

        requestAnimationFrame(rafLoop);
      };
      requestAnimationFrame(rafLoop);

      document.addEventListener(
        "keydown",
        (event) => {
          if (!event.isTrusted) return;
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
          if ((w as unknown as { __seratPendingP?: number }).__seratPendingP !== undefined) return;
          (w as unknown as { __seratPendingP?: number }).__seratPendingP = performance.now();
        },
        { capture: false },
      );

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
      framesKey: FRAME_RECORDS_KEY,
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
 *
 * **NIT, recorded rather than fixed:** the `input`-listener timing this
 * relies on for `p` (end of processing) has only been verified against a
 * bare contenteditable (`control-arm.ts`), where the browser's own native
 * edit handling is synchronous with `input`. CodeMirror 6 applies DOM
 * changes through a `MutationObserver`-driven update cycle that may not
 * be complete by the time `input` fires; this needs re-verifying against
 * a real CM6 instance once #21 lands, and `p` may need to move to a
 * different signal (e.g. a CM6 `updateListener`) at that point.
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
 * (§9.3: fixed 100ms cadence).
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
