// The measurement primitive for NFR-PRF-02 (task #11), rebuilt on RULE E0, the
// rule selected empirically from recordings (bench/editor-latency/record/,
// data/summary.txt) after rules A, B and C failed against recorded frames.
//
// **E0, in one paragraph.** For each trusted keystroke:
//   t    = the keydown event's `timeStamp`;
//   ts_j = the last rAF frame timestamp at or before t;
//   D    = ts_j + 2R           (R = median rAF delta over 120 frames);
//   ETend = max over the keystroke's keydown / keypress / beforeinput / input
//           Event Timing entries of (startTime + duration), i.e. the moment the
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
}

export interface RunSummary {
  n: number;
  onTimeAtMinus4: number;
  onTimeAtZero: number;
  onTimeAtPlus4: number;
  scoredOnTimeByAbsence: number;
  staleAnchors: number;
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

/** Presentation time of the keystroke from its Event Timing entries: the max of
 * `startTime + duration` over keydown, keypress, beforeinput and input entries that
 * start within [t - 0.01, t + 4] ms of the keydown. `null` if there are none.
 * keyup is excluded (a separate event, painted separately). */
export function keystrokeEventEnd(entries: EtEntry[], t: number): number | null {
  let end: number | null = null;
  for (let i = 0; i < entries.length; i++) {
    const e = entries[i];
    if (e.name === "keyup") continue;
    if (e.startTime < t - 0.01 || e.startTime > t + 4) continue;
    const v = e.startTime + e.duration;
    if (end === null || v > end) end = v;
  }
  return end;
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
  return keydownTimes.map((t) => scoreKeystroke(frameStarts, entries, t, refreshIntervalMs));
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
  for (let i = 0; i < scores.length; i++) {
    if (scores[i].etEnd === null && !scores[i].late) absent++;
    if (scores[i].t - scores[i].tsJ >= refreshIntervalMs) stale++;
  }
  const verdict = strict >= bar ? "PASS" : lenient < bar ? "FAIL" : "INDETERMINATE";
  return { n: scores.length, onTimeAtMinus4: strict, onTimeAtZero: nominal, onTimeAtPlus4: lenient, scoredOnTimeByAbsence: absent, staleAnchors: stale, verdict };
}

/** The functions the page runs, in dependency order. */
export const SeratLatencyFns = {
  medianRefreshInterval,
  lastFrameAtOrBefore,
  deadline,
  keystrokeEventEnd,
  isLate,
  framesToPaint,
  scoreKeystroke,
  scoreAll,
  shareOnTime,
  summarise,
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
  const refreshIntervalMs = await page.evaluate(
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
  );
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
    await page.keyboard.press("x");
    await new Promise((r) => setTimeout(r, interval + next() * R));
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
  if (result.keydowns !== count || result.inputs !== count) {
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
