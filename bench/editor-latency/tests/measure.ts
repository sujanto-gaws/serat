// The measurement primitive for NFR-PRF-02 (task #11), rebuilt on RULE E0, the
// rule selected empirically from recordings (bench/editor-latency/record/,
// data/summary.txt) after rules A, B and C failed against recorded frames.
//
// **E0, in one paragraph.** For each trusted keystroke:
//   t    = the keydown event's `timeStamp`;
//   ts_j = the last rAF frame timestamp at or before t;
//   D    = ts_j + 2R           (R = median rAF delta over 120 frames);
//   ETend = max over the keystroke's keydown / keypress / beforeinput / input
//           Event Timing entries whose startTime is in [t, t + 4 ms] (a TIME WINDOW, keyup
//           excluded; NOT grouped by interactionId, which is 0 on input events in Chromium; observed on 141 and 153),
//           of (startTime + duration), i.e. the moment the
//           browser reports the frame after the event as PRESENTED
//           (`PerformanceObserver({type:"event", durationThreshold:16})`);
//   the keystroke is LATE iff ETend > D. An absent entry means the paint took
//   under 16 ms, which is on time.
//   frames_to_paint = 1 + max(0, ceil((ETend - D) / R)); on time iff it is 1.
// Binding: >= 95% of keystrokes on time (§9.3). `run`, `p` (end of input
// handling) and a post-paint MessageChannel are NOT in the scoring path: they
// mis-score at phase >= 0.88R (rule C) or are blind to work done after the
// recorder's own rAF callback, which is where CodeMirror's measure/DOM update
// runs (rule D: a 36 ms rAF stall scored 0% late). Event Timing is the only
// signal that includes presentation. It is Chromium/WebView2-only and rounds
// `duration` to 8 ms; the recorded separation is ETend - D <= -5.3 ms for a
// bare contenteditable and >= +7.9 ms for a blocked one, so threshold 0 sits
// in the gap.
//
// **One implementation measures, not two.** Every scoring function lives in
// `SeratLatencyFns` below as a plain, self-contained `function` declaration.
// The page runs them from that SAME source text (`namespaceSource()` ->
// `new Function`), and the unit tests exercise both the direct import and the
// `new Function` round trip, so the code under test is the code that runs in
// the page. The mutation test substitutes a function's source through the same
// seam (`namespaceSource({ isLate: ... })`) and expects the negative arms to go
// red.
//
// **No blanket try/catch.** A missing frame or a keystroke that never reached
// the editor throws and fails the run; nothing is swallowed into a score.
//
// **One document per capture install.** `installCapture` registers listeners on
// `document`; it refuses a second install on the same document instead of
// silently skipping (the `setContent` double-install hang found in round 2).
import type { Locator, Page } from "@playwright/test";

/** One Event Timing entry, reduced to what scoring needs. */
export interface EtEntry {
  name: string;
  startTime: number;
  duration: number;
}

/** One keystroke's verdict. */
export interface KeystrokeScore {
  t: number;
  tsJ: number;
  deadline: number;
  /** `null`: no Event Timing entry >= 16 ms, i.e. presented in time. */
  etEnd: number | null;
  late: boolean;
  framesToPaint: number;
  /** Spread (max - min) of this key's Event Timing entry ends, ms; 0 with fewer than two entries. */
  spreadMs?: number;
}

export interface RunSummary {
  n: number;
  onTimeAtMinus4: number;
  onTimeAtZero: number;
  onTimeAtPlus4: number;
  scoredOnTimeByAbsence: number;
  staleAnchors: number;
  /** Smallest `D - t - 16` over the keys scored on time by absence: the headroom the
   * absence argument guarantees (an absent entry means presented < 16 ms after `t`).
   * `null` when no key was scored that way. Positive for every key whose anchor is not stale (t - ts_j < R, R >= 16); a stale anchor can make it smaller or negative. */
  worstAbsentMarginMs: number | null;
  /** Widest spread of one key's Event Timing entry ends over the run (invariant b: <= 8 ms). */
  widestSpreadMs: number;
  verdict: "PASS" | "FAIL" | "INDETERMINATE";
}

// ---------------------------------------------------------------------
// Pure functions. Self-contained declarations (each may call the others
// by name; nothing else is referenced) so `.toString()` round-trips.
// ---------------------------------------------------------------------

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

/** The last frame timestamp `<= t` (`ts_j`). Throws if there is none. */
export function lastFrameAtOrBefore(frameStarts: number[], t: number): number {
  let found: number | undefined;
  for (let i = 0; i < frameStarts.length; i++) {
    if (frameStarts[i] <= t) found = frameStarts[i];
    else break;
  }
  if (found === undefined) {
    throw new Error("no recorded frame at or before t=" + t);
  }
  return found;
}

/** D = ts_j + 2R: the vsync that ends the first display frame after the keystroke. */
export function deadline(tsJ: number, refreshIntervalMs: number): number {
  return tsJ + 2 * refreshIntervalMs;
}

/** Does this Event Timing entry belong to the keystroke at `t`? One definition, used by scoring and by
 * the invariant check. keyup is a separate event. The window is [t - 0.01, t + 4] ms: the entry's
 * startTime is a coarsened copy of the event timeStamp, so it can sit a hair BEFORE the keydown's
 * `timeStamp`; 0.01 ms (10 us) absorbs that and is far below the 100 ms key spacing. */
export function inKeyWindow(e: EtEntry, t: number): boolean {
  return e.name !== "keyup" && e.startTime >= t - 0.01 && e.startTime <= t + 4;
}

/** Presentation time of the keystroke from its Event Timing entries: the max of
 * `startTime + duration` over keydown, keypress, beforeinput and input entries that
 * start within [t - 0.01, t + 4] ms of the keydown. `null` if there are none.
 * keyup is excluded (a separate event, painted separately). */
export function keystrokeEventEnd(entries: EtEntry[], t: number): number | null {
  let end: number | null = null;
  for (let i = 0; i < entries.length; i++) {
    const e = entries[i];
    if (!inKeyWindow(e, t)) continue;
    const v = e.startTime + e.duration;
    if (end === null || v > end) end = v;
  }
  return end;
}

/** Per-key invariants of 9.3, asserted on every run so a lost entry cannot score a key on time:
 * (a) if ANY keydown/keypress/beforeinput/input entry of this key is present (window [t-0.01, t+4]),
 *     the keydown entry must be present too (keydown starts first, so its duration is the longest;
 *     rounding is monotonic, so it can never be the one below the 16 ms threshold);
 * (b) the ends (startTime + duration) of the key's entries lie within 8 ms of each other (they are
 *     presented in one frame; Event Timing rounds duration to 8 ms).
 * Returns { ok, reason, spreadMs }; `reason` is empty when ok. */
export function checkKeyInvariants(entries: EtEntry[], t: number): { ok: boolean; reason: string; spreadMs: number } {
  let any = false;
  let hasKeydown = false;
  let lo = 0;
  let hi = 0;
  for (let i = 0; i < entries.length; i++) {
    const e = entries[i];
    if (!inKeyWindow(e, t)) continue;
    const v = e.startTime + e.duration;
    if (!any) { lo = v; hi = v; } else { if (v < lo) lo = v; if (v > hi) hi = v; }
    any = true;
    if (e.name === "keydown") hasKeydown = true;
  }
  const spreadMs = any ? hi - lo : 0;
  if (any && !hasKeydown) {
    return { ok: false, reason: "an Event Timing entry for this key is present but its keydown entry is missing (a lost entry would score the key on time)", spreadMs };
  }
  // 1e-6 is a floating-point guard, NOT a tolerance: (1000.4+16+8)-(1000.4+16) is 8.000000000000114. The limit stays 8 ms.
  if (spreadMs > 8 + 1e-6) {
    return { ok: false, reason: "the key's Event Timing entries end " + spreadMs.toFixed(1) + " ms apart (> 8 ms): they are not one presentation", spreadMs };
  }
  return { ok: true, reason: "", spreadMs };
}

/** E0's verdict. `etEnd === null` (no entry, duration < 16 ms) is on time. */
export function isLate(etEnd: number | null, deadlineMs: number): boolean {
  if (etEnd === null) return false;
  return etEnd > deadlineMs;
}

/** 1 = painted in the first display frame; 2 = one frame late; ... */
export function framesToPaint(etEnd: number | null, deadlineMs: number, refreshIntervalMs: number): number {
  if (!isLate(etEnd, deadlineMs)) return 1;
  return 1 + Math.max(1, Math.ceil(((etEnd as number) - deadlineMs) / refreshIntervalMs));
}

export function scoreKeystroke(
  frameStarts: number[],
  entries: EtEntry[],
  t: number,
  refreshIntervalMs: number,
): KeystrokeScore {
  const tsJ = lastFrameAtOrBefore(frameStarts, t);
  const d = deadline(tsJ, refreshIntervalMs);
  const etEnd = keystrokeEventEnd(entries, t);
  return {
    t,
    tsJ,
    deadline: d,
    etEnd,
    late: isLate(etEnd, d),
    framesToPaint: framesToPaint(etEnd, d, refreshIntervalMs),
  };
}

export function scoreAll(
  frameStarts: number[],
  entries: EtEntry[],
  keydownTimes: number[],
  refreshIntervalMs: number,
): KeystrokeScore[] {
  const out: KeystrokeScore[] = [];
  for (let i = 0; i < keydownTimes.length; i++) {
    const inv = checkKeyInvariants(entries, keydownTimes[i]);
    if (!inv.ok) throw new Error("run rejected: key index " + i + " (t=" + keydownTimes[i] + "): " + inv.reason);
    const sc = scoreKeystroke(frameStarts, entries, keydownTimes[i], refreshIntervalMs);
    sc.spreadMs = inv.spreadMs;
    out.push(sc);
  }
  return out;
}

/** On-time share when the lateness threshold is moved by `thetaMs` (late iff ETend > D + theta).
 * Goes through `isLate`, so the mutation seam reaches it. */
export function shareOnTime(scores: KeystrokeScore[], thetaMs: number): number {
  let on = 0;
  for (let i = 0; i < scores.length; i++) {
    if (!isLate(scores[i].etEnd, scores[i].deadline + thetaMs)) on++;
  }
  return on / scores.length;
}

/** Everything a run must report (architecture.md 9.3): the on-time share at theta = -4, 0 and +4 ms
 * (the 8 ms rounding of Event Timing `duration` puts +-4 ms on ETend); keys scored on time by ABSENCE
 * of an entry; keys with a stale anchor (t - ts_j >= R, the rAF loop missed a frame, D too early). Verdict
 * against `bar`: PASS iff the strictest share (-4) meets it, FAIL iff even the most lenient (+4) misses it,
 * otherwise INDETERMINATE (the bar falls between the shares), which is not a pass. */
export function summarise(
  scores: KeystrokeScore[],
  bar: number,
  refreshIntervalMs: number,
): RunSummary {
  if (scores.length === 0) throw new Error("summarise of zero scores is undefined");
  const strict = shareOnTime(scores, -4);
  const nominal = shareOnTime(scores, 0);
  const lenient = shareOnTime(scores, 4);
  let absent = 0;
  let stale = 0;
  let worstAbsent: number | null = null;
  let widest = 0;
  for (let i = 0; i < scores.length; i++) {
    if (scores[i].etEnd === null && !scores[i].late) {
      absent++;
      const margin = scores[i].deadline - scores[i].t - 16;
      if (worstAbsent === null || margin < worstAbsent) worstAbsent = margin;
    }
    if (scores[i].t - scores[i].tsJ >= refreshIntervalMs) stale++;
    if ((scores[i].spreadMs || 0) > widest) widest = scores[i].spreadMs as number;
  }
  const verdict = strict >= bar ? "PASS" : lenient < bar ? "FAIL" : "INDETERMINATE";
  return { n: scores.length, onTimeAtMinus4: strict, onTimeAtZero: nominal, onTimeAtPlus4: lenient, scoredOnTimeByAbsence: absent, staleAnchors: stale, worstAbsentMarginMs: worstAbsent, widestSpreadMs: widest, verdict };
}

/** Smallest refresh interval at which "no Event Timing entry means presented in time" holds. */
export const MIN_VALID_REFRESH_INTERVAL_MS = 16;

/** A gated run is REJECTED at R < 16 ms (120/144 Hz): the absence argument needs
 * D - t > R >= 16, i.e. an unreported event (< 16 ms) is before D. Not a verdict, an invalid run.
 * @throws when R is below 16 ms. */
export function assertRefreshIntervalValid(refreshIntervalMs: number): void {
  if (!(refreshIntervalMs >= 16)) {
    throw new Error(
      "run rejected: refresh interval R=" + refreshIntervalMs + " ms is below 16 ms (>= 120 Hz). " +
        "An absent Event Timing entry (duration < 16 ms) is scored on time only when R >= 16 ms, so " +
        "this machine/display cannot produce a gated figure",
    );
  }
}

/** The functions the page runs, in dependency order. */
export const SeratLatencyFns = {
  medianRefreshInterval,
  lastFrameAtOrBefore,
  deadline,
  inKeyWindow,
  keystrokeEventEnd,
  checkKeyInvariants,
  isLate,
  framesToPaint,
  scoreKeystroke,
  scoreAll,
  shareOnTime,
  summarise,
  assertRefreshIntervalValid,
} as const;

export type SeratLatencyNamespace = typeof SeratLatencyFns;

/** Source of an IIFE returning the namespace object. `overrides` replaces a
 * function's declaration source (mutation testing); the override must declare a
 * function of the same name. */
export function namespaceSource(overrides: Partial<Record<keyof SeratLatencyNamespace, string>> = {}): string {
  const names = Object.keys(SeratLatencyFns) as (keyof SeratLatencyNamespace)[];
  for (const k of Object.keys(overrides)) {
    if (!names.includes(k as keyof SeratLatencyNamespace)) throw new Error("unknown function to override: " + k);
  }
  const decls = names.map((n) => overrides[n] ?? SeratLatencyFns[n].toString()).join("\n");
  return "(function () {\n" + decls + "\nreturn { " + names.join(", ") + " };\n})()";
}

/** Builds the namespace in this (Node) process from the same source the page gets. */
export function loadNamespace(src: string = namespaceSource()): SeratLatencyNamespace {
  return new Function("return " + src)() as SeratLatencyNamespace;
}

// ---------------------------------------------------------------------
// Aggregates
// ---------------------------------------------------------------------

export function percentOnTime(scores: KeystrokeScore[]): number {
  if (scores.length === 0) throw new Error("percentOnTime of zero scores is undefined");
  return scores.filter((s) => !s.late).length / scores.length;
}

export function percentLate(scores: KeystrokeScore[]): number {
  return 1 - percentOnTime(scores);
}

/** Nearest-rank p95 of frames_to_paint. */
export function p95FramesToPaint(scores: KeystrokeScore[]): number {
  if (scores.length === 0) throw new Error("p95 of zero scores is undefined");
  const sorted = scores.map((s) => s.framesToPaint).sort((a, b) => a - b);
  return sorted[Math.max(0, Math.ceil(0.95 * sorted.length) - 1)];
}

// ---------------------------------------------------------------------
// Browser side
// ---------------------------------------------------------------------

const R_WARMUP_FRAMES = 121; // 120 deltas

/** Task #221: bounds a page-side wait on the Node side. A page whose requestAnimationFrame has stopped never settles
 * an in-page promise, and `locator.evaluate`'s `{ timeout }` only bounds the locator wait, so the page promise is
 * raced against a Node timer. The rejection says it is a harness stall, not a latency verdict. Wraps setup and
 * settle waits only; no measured interval is inside it. */
export async function withHarnessDeadline<T>(work: Promise<T>, ms: number, what: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${what}: page-side wait exceeded ${ms} ms (rAF stalled?); harness timeout, not a latency verdict`)), ms);
  });
  try { return await Promise.race([work, deadline]); } finally { clearTimeout(timer); }
}

export interface Capture {
  refreshIntervalMs: number;
}

/**
 * Installs, once per document: the namespace (from `src`); a rAF loop recording
 * frame timestamps; an Event Timing observer (durationThreshold 16); and
 * capture-phase keydown / input listeners that record trusted keystrokes.
 * Resolves with R once 120 frame deltas have been seen. Throws on a second
 * install on the same document.
 */
export async function installCapture(page: Page, src: string = namespaceSource()): Promise<Capture> {
  const refreshIntervalMs = await withHarnessDeadline(page.evaluate(
    async ({ nsSrc, warmup }) => {
      const w = window as unknown as Record<string, any>;
      if (w.__seratCapture) throw new Error("installCapture called twice on one document");
      const ns = new Function("return " + nsSrc)();
      const cap = { ns, frames: [] as number[], entries: [] as any[], keydowns: [] as number[], inputs: 0, R: 0 };
      w.__seratCapture = cap;
      const loop = (ts: number) => {
        cap.frames.push(ts);
        requestAnimationFrame(loop);
      };
      requestAnimationFrame(loop);
      const obs = new PerformanceObserver((list) => {
        for (const e of list.getEntries()) {
          cap.entries.push({ name: e.name, startTime: e.startTime, duration: e.duration });
        }
      });
      obs.observe({ type: "event", durationThreshold: 16 } as PerformanceObserverInit);
      w.__seratObs = obs;
      document.addEventListener("keydown", (e) => { if (e.isTrusted) cap.keydowns.push(e.timeStamp); }, true);
      document.addEventListener("input", (e) => { if (e.isTrusted) cap.inputs++; }, true);
      await new Promise<void>((resolve) => {
        const check = () => (cap.frames.length >= warmup ? resolve() : requestAnimationFrame(check));
        requestAnimationFrame(check);
      });
      cap.R = ns.medianRefreshInterval(cap.frames.slice(0, warmup));
      return cap.R;
    },
    { nsSrc: src, warmup: R_WARMUP_FRAMES },
  ), 30_000, "installCapture warmup frames");
  // Validity condition: the same function the page carries, run on the measured R.
  loadNamespace(src).assertRefreshIntervalValid(refreshIntervalMs);
  return { refreshIntervalMs };
}

export type Cadence = { kind: "strict" } | { kind: "swept"; seed: number };

/** Deterministic PRNG (mulberry32) so a swept run is reproducible from its seed. */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export interface RunOptions {
  intervalMs?: number;
  cadence?: Cadence;
  /** Also require the target's textContent to grow by exactly `count` (bare fixtures). */
  checkTextGrowth?: boolean;
  /** The on-time bar the INDETERMINATE rule is judged against (default 0.99). */
  bar?: number;
  /** The key pressed for every keystroke (default "x"). "Enter" and "Backspace" are handled by CodeMirror's own
   * keydown keymap, which prevents the default action, so no native `input` event follows: pass
   * `expectInputs: false` for them (the per-key invariants still demand the keydown entry). */
  key?: string;
  /** Require one trusted `input` event per keystroke (default true: typed characters). */
  expectInputs?: boolean;
  /** After every `every` keystrokes wait `ms` more (typing in bursts): the editor's debounced save then fires between
   * bursts and its reply lands inside the next burst's keys, which are scored like any other. */
  burst?: { every: number; ms: number };
}

/**
 * Types `count` trusted "x" keystrokes (CDP Input.dispatchKeyEvent) into `target`
 * and scores them with the page's own namespace. strict: fixed 100 ms cadence
 * (a multiple of 6R, so keystroke phase is nearly locked); swept: 100 ms +
 * U(0, R) from a seeded PRNG so the keystroke phase covers the whole frame.
 */
export async function runKeystrokes(
  page: Page,
  target: Locator,
  count: number,
  opts: RunOptions = {},
): Promise<{ scores: KeystrokeScore[]; summary: RunSummary; refreshIntervalMs: number; dropped: string[] }> {
  const interval = opts.intervalMs ?? 100;
  const cadence = opts.cadence ?? { kind: "strict" };
  const next = cadence.kind === "swept" ? mulberry32(cadence.seed) : () => 0;
  await target.focus();
  const before = await page.evaluate(() => {
    const c = (window as any).__seratCapture;
    if (!c) throw new Error("installCapture was not called on this document");
    return { keys: c.keydowns.length, inputs: c.inputs };
  });
  const textBefore = opts.checkTextGrowth ? ((await target.textContent()) ?? "").length : 0;
  const R = await page.evaluate(() => (window as any).__seratCapture.R as number);
  for (let i = 0; i < count; i++) {
    await page.keyboard.press(opts.key ?? "x");
    await new Promise((r) => setTimeout(r, interval + next() * R));
    if (opts.burst && (i + 1) % opts.burst.every === 0) await new Promise((r) => setTimeout(r, opts.burst!.ms));
  }
  // Let the last frames present and Event Timing deliver, then flush the observer.
  await new Promise((r) => setTimeout(r, 500));
  const result = await page.evaluate(
    ({ keys0, inputs0, bar }) => {
      const w = window as any;
      const cap = w.__seratCapture;
      for (const e of w.__seratObs.takeRecords()) {
        cap.entries.push({ name: e.name, startTime: e.startTime, duration: e.duration });
      }
      const times = cap.keydowns.slice(keys0);
      const r = {
        keydowns: times.length,
        inputs: cap.inputs - inputs0,
        scores: cap.ns.scoreAll(cap.frames, cap.entries, times, cap.R),
      };
      return { ...r, summary: cap.ns.summarise(r.scores, bar, cap.R) };
    },
    { keys0: before.keys, inputs0: before.inputs, bar: opts.bar ?? 0.99 },
  );
  if (result.keydowns !== count || (opts.expectInputs !== false && result.inputs !== count)) {
    throw new Error(
      `${count} keystrokes were sent but the page saw ${result.keydowns} trusted keydown and ` +
        `${result.inputs} input events: the keystrokes did not all reach the editor, so the run measures nothing`,
    );
  }
  if (opts.checkTextGrowth) {
    const grown = ((await target.textContent()) ?? "").length - textBefore;
    if (grown !== count) throw new Error(`text grew by ${grown}, expected ${count}`);
  }
  // Nothing is dropped: every keydown is scored or the run throws. The list exists so a run can
  // never lose a key without a logged reason; it is empty by construction.
  const dropped: string[] = [];
  if (result.scores.length !== count) throw new Error(`scored ${result.scores.length} of ${count} keystrokes with no logged reason`);
  return { scores: result.scores as KeystrokeScore[], summary: result.summary as RunSummary, refreshIntervalMs: R, dropped };
}

/** Diagnostic (reported, never gates): keydown -> presentation, ms, over keystrokes
 * that have an Event Timing entry (>= 16 ms); the rest are < 16 ms. */
export function latencyDiagnostic(scores: KeystrokeScore[]): { withEntry: number; meanMs: number; maxMs: number } {
  const v = scores.filter((s) => s.etEnd !== null).map((s) => (s.etEnd as number) - s.t);
  return {
    withEntry: v.length,
    meanMs: v.length ? v.reduce((a, b) => a + b, 0) / v.length : 0,
    maxMs: v.length ? Math.max(...v) : 0,
  };
}
