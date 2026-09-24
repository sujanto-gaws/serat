// Unit-level checks on §9.3's pure functions — no browser interaction, no
// subject, nothing blocked on #5's implementation (#21). Kept green today
// deliberately: a harness directory with nothing passing is
// indistinguishable from one nobody has looked at yet.
import { test, expect } from "@playwright/test";
import {
  percentPaintedFirstFrame,
  p95FramesToPaint,
  countFrameStartsInRange,
  isLongFrame,
  medianRefreshInterval,
  type FrameSample,
} from "./measure";

function frames(...framesToPaint: number[]): FrameSample[] {
  return framesToPaint.map((f) => ({ framesToPaint: f, handlerMs: 0, renderMs: 0 }));
}

test.describe("percentPaintedFirstFrame", () => {
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

  test("fk before any recorded frame start is zero, not negative or undefined", () => {
    expect(countFrameStartsInRange([100, 200], 10, 50)).toBe(0);
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
    // Frame starts 0, 10, 20, 30, 40 -> deltas [10,10,10,10], even count.
    // Use an odd-delta-count series instead: 0, 10, 25, 30 -> deltas
    // [10, 15, 5] -> sorted [5, 10, 15] -> median 10.
    expect(medianRefreshInterval([0, 10, 25, 30])).toBe(10);
  });

  test("even count of deltas: the mean of the two middle values", () => {
    // 0, 10, 20, 40 -> deltas [10, 10, 20] -- that's odd (3), use four
    // deltas instead: 0, 10, 20, 30, 50 -> deltas [10,10,10,20] (even,
    // 4) -> sorted same -> median (10+10)/2 = 10.
    expect(medianRefreshInterval([0, 10, 20, 30, 50])).toBe(10);
  });

  test("refuses fewer than two frame starts", () => {
    expect(() => medianRefreshInterval([0])).toThrow();
  });
});
