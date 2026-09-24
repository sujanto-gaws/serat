// Unit-level checks on §9.3's pure functions — no browser interaction, no
// subject, nothing blocked on #5's implementation (#21). Kept green today
// deliberately: a harness directory with nothing passing is
// indistinguishable from one nobody has looked at yet.
//
// **These are the same functions the page runs** (measure.ts's
// `pureFunctionSource`, injected verbatim via `page.addInitScript`) —
// code review's second pass found the previous version of this file
// tested Node-side originals while the page ran a hand-copied
// reimplementation, and mutating the in-page copy alone left every test
// here green. There is no separate in-page copy left to diverge from
// what these tests exercise.
import { test, expect } from "@playwright/test";
import {
  percentPaintedFirstFrame,
  paintedFirstFrame,
  p95FramesToPaint,
  countFrameStartsInRange,
  isLongFrame,
  medianRefreshInterval,
  selectFk,
  type FrameSample,
  type FrameRecord,
} from "./measure";

function frames(...framesToPaint: number[]): FrameSample[] {
  return framesToPaint.map((f) => ({ framesToPaint: f, handlerMs: 0, renderMs: 0 }));
}

test.describe("paintedFirstFrame / percentPaintedFirstFrame", () => {
  test("framesToPaint: 1 is on time", () => {
    expect(paintedFirstFrame({ framesToPaint: 1, handlerMs: 0, renderMs: 0 })).toBe(true);
  });

  test("framesToPaint: 0 is NOT on time -- kills a `<= 1` mutant", () => {
    // A framesToPaint of 0 should never occur in an honest run (the
    // counting rule clamps to a floor of 1), but a corrupted or mutated
    // sample carrying 0 must not be read as "even better than on time".
    // `=== 1` rejects it; a `<= 1` mutant would wrongly accept it.
    expect(paintedFirstFrame({ framesToPaint: 0, handlerMs: 0, renderMs: 0 })).toBe(false);
  });

  test("one missed frame in twenty is exactly the 95% boundary — passes at >=95%", () => {
    // 19 on-time (framesToPaint=1), 1 missed (framesToPaint=2): 19/20 = 0.95.
    const rate = percentPaintedFirstFrame(frames(...Array(19).fill(1), 2));
    expect(rate).toBe(0.95);
    expect(rate).toBeGreaterThanOrEqual(0.95);
  });

  test("two missed frames in twenty fails the 95% bar", () => {
    const rate = percentPaintedFirstFrame(frames(...Array(18).fill(1), 2, 2));
    expect(rate).toBe(0.9);
    expect(rate).toBeLessThan(0.95);
  });

  test("a framesToPaint: 0 sample does not inflate the on-time rate", () => {
    // Companion to the mutant-killing case above, at the aggregate level:
    // if paintedFirstFrame wrongly accepted 0 as on-time, this would read
    // 1.0 instead of 0.5.
    const rate = percentPaintedFirstFrame([
      { framesToPaint: 0, handlerMs: 0, renderMs: 0 },
      { framesToPaint: 1, handlerMs: 0, renderMs: 0 },
    ]);
    expect(rate).toBe(0.5);
  });

  test("refuses an empty sample set rather than returning a misleading rate", () => {
    expect(() => percentPaintedFirstFrame([])).toThrow();
  });
});

test.describe("p95FramesToPaint", () => {
  test("nearest-rank: one slow run in twenty passes", () => {
    // 20 samples, rank = ceil(0.95*20) - 1 = 18 (0-indexed) -> the 19th
    // value once sorted. A single outlier at the very end (index 19) does
    // not move it: the nearest-rank value is still 1.
    expect(p95FramesToPaint(frames(...Array(19).fill(1), 3))).toBe(1);
  });

  test("nearest-rank: two slow runs in twenty do not pass", () => {
    expect(p95FramesToPaint(frames(...Array(18).fill(1), 2, 3))).toBe(2);
  });

  test("at n=10, p95 is the maximum — the case that separates nearest-rank from a floor((n-1)*p) percentile", () => {
    // ceil(0.95*10) - 1 = 9 (0-indexed) -> the max. floor((10-1)*0.95) = 8
    // (0-indexed) -> one below the max. The two formulas coincide at
    // n=20 (both tests above), which is exactly why neither of them
    // alone catches a silent switch to the other method.
    expect(p95FramesToPaint(frames(1, 1, 1, 1, 1, 1, 1, 1, 1, 5))).toBe(5);
  });

  test("refuses an empty sample set", () => {
    expect(() => p95FramesToPaint([])).toThrow();
  });
});

test.describe("countFrameStartsInRange (§9.3's frames_to_paint counting rule)", () => {
  test("t excluded, fk included", () => {
    // t = 10, fk = 40: frames at 10 (excluded, equals t), 20, 30, 40
    // (included), 50 (excluded, after fk).
    expect(countFrameStartsInRange([10, 20, 30, 40, 50], 10, 40)).toBe(3);
  });

  test("no frame boundary passed: exactly one frame start qualifies", () => {
    // t and fk both land inside the same single recorded frame start.
    expect(countFrameStartsInRange([16.7, 33.4, 50.1], 10, 16.7)).toBe(1);
  });

  test("a count of zero is clamped to a floor of 1 (fix 1's boundary case)", () => {
    // fk before every recorded frame start shouldn't happen in an honest
    // run (fk is always drawn from the frames array via selectFk), but
    // the clamp is a floor against exactly this kind of clock/ordering
    // edge case, not a normal outcome.
    expect(countFrameStartsInRange([100, 200], 10, 50)).toBe(1);
  });
});

test.describe("selectFk (fix 1: f_k is when the callback RUNS, not its scheduled ts)", () => {
  function record(ts: number, run: number): FrameRecord {
    return { ts, run };
  }

  test("ts < p < run: the edit was still captured in that frame -- selectFk picks it, not the next one", () => {
    // This is exactly the case code review's probe found: p arrives after
    // this frame's scheduled ts but before its callback has run, so the
    // edit is present when the callback executes. The old `ts >= p` rule
    // would have skipped this frame (ts=100 < p=105) and picked a later
    // one; selectFk correctly picks it because run=110 > p=105.
    const frames = [record(100, 110)];
    expect(selectFk(frames, 105)).toBe(100);
  });

  test("combined with countFrameStartsInRange, the ts<p<run case gives frames_to_paint = 1", () => {
    const t = 50; // keydown timeStamp
    const p = 105; // end of processing
    const frameRecords = [record(100, 110)]; // ts < p < run
    const fk = selectFk(frameRecords, p);
    const framesToPaint = countFrameStartsInRange(
      frameRecords.map((f) => f.ts),
      t,
      fk,
    );
    expect(framesToPaint).toBe(1);
  });

  test("a frame whose callback has not run yet is not selected", () => {
    // run <= p: the callback has not executed yet (or ran before p), so
    // this frame cannot be the one that captured the edit.
    const frames = [record(100, 102)]; // run < p
    expect(() => selectFk(frames, 105)).toThrow();
  });

  test("the first qualifying frame wins, not the last", () => {
    const frames = [record(100, 110), record(116.7, 126.7), record(133.4, 143.4)];
    // p=105: frame 0's run (110) already qualifies.
    expect(selectFk(frames, 105)).toBe(100);
  });

  test("when an earlier frame's callback ran before p, the next qualifying one is picked", () => {
    // Frame 0 (ts=16.7) ran promptly at 20 -- before p=105, so it does
    // not qualify (the edit had not happened yet when that frame
    // committed). Frame 1 (ts=33.4) is where the main thread was busy:
    // its callback did not actually run until 140, well after p, so it
    // is the one selected -- exactly the delayed-callback shape fix 1
    // exists for.
    const frames = [record(16.7, 20), record(33.4, 140)];
    expect(selectFk(frames, 105)).toBe(33.4);
  });

  test("no frame has run after p yet: throws (the caller's signal to keep waiting)", () => {
    const frames = [record(100, 102), record(116.7, 118)];
    expect(() => selectFk(frames, 200)).toThrow();
  });
});

test.describe("isLongFrame (§9.3's long-frame adjustment)", () => {
  const R = 16.667;

  test("exactly 1.5R is not long — the condition is strictly greater than", () => {
    expect(isLongFrame(0, R * 1.5, R)).toBe(false);
  });

  test("just over 1.5R is long", () => {
    expect(isLongFrame(0, R * 1.5 + 0.001, R)).toBe(true);
  });

  test("a normal single-frame gap is not long", () => {
    expect(isLongFrame(0, R, R)).toBe(false);
  });
});

test.describe("medianRefreshInterval", () => {
  test("odd count of deltas: the middle value", () => {
    // Frame starts 0, 10, 25, 30 -> deltas [10, 15, 5] -> sorted
    // [5, 10, 15] -> median 10.
    expect(medianRefreshInterval([0, 10, 25, 30])).toBe(10);
  });

  test("even count of deltas, the two middle values differing: the mean of the two", () => {
    // Frame starts 0, 1, 3, 6, 10 -> deltas [1, 2, 3, 4] (already sorted,
    // even count). The two middle values (2, 3) differ, so a mutant that
    // returns `deltas[mid]` (here deltas[2] = 3) instead of averaging the
    // two middle values ((2+3)/2 = 2.5) is caught -- the previous version
    // of this test used deltas whose two middle values happened to be
    // equal (10, 10), which cannot distinguish the two implementations.
    expect(medianRefreshInterval([0, 1, 3, 6, 10])).toBe(2.5);
  });

  test("refuses fewer than two frame starts", () => {
    expect(() => medianRefreshInterval([0])).toThrow();
  });
});
