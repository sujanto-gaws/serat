// Unit checks on rule E0's pure functions (NFR-PRF-02, §9.3). No browser, no
// subject. Every behavioural test runs TWICE: against the direct import and
// against the `new Function` round trip of `namespaceSource()`, which is the
// code the page runs. A function that is not self-contained, or that the
// round trip changes, fails the second run.
import { test, expect } from "@playwright/test";
import * as direct from "./measure";
import {
  loadNamespace,
  namespaceSource,
  percentLate,
  percentOnTime,
  p95FramesToPaint,
  mulberry32,
  type KeystrokeScore,
} from "./measure";

const R = 16.7;
const impls: [string, direct.SeratLatencyNamespace][] = [
  ["direct import", direct.SeratLatencyFns],
  ["new Function round trip", loadNamespace()],
];

for (const [label, ns] of impls) {
  test.describe(`E0 pure functions (${label})`, () => {
    test("medianRefreshInterval: odd count takes the middle delta", () => {
      expect(ns.medianRefreshInterval([0, 10, 25, 30])).toBe(10);
    });
    test("medianRefreshInterval: even count averages the two middle deltas (they differ)", () => {
      expect(ns.medianRefreshInterval([0, 1, 3, 6, 10])).toBe(2.5);
    });
    test("medianRefreshInterval refuses fewer than two frames", () => {
      expect(() => ns.medianRefreshInterval([0])).toThrow();
    });

    test("lastFrameAtOrBefore: equality counts (<=), later frames do not", () => {
      expect(ns.lastFrameAtOrBefore([10, 26.7, 43.4], 26.7)).toBe(26.7);
      expect(ns.lastFrameAtOrBefore([10, 26.7, 43.4], 30)).toBe(26.7);
    });
    test("lastFrameAtOrBefore throws when t precedes every frame", () => {
      expect(() => ns.lastFrameAtOrBefore([10, 26.7], 5)).toThrow();
    });

    test("deadline = ts_j + 2R", () => {
      expect(ns.deadline(100, R)).toBeCloseTo(133.4, 9);
    });

    test("keystrokeEventEnd: max of startTime+duration over the key's own events", () => {
      const e = [
        { name: "keydown", startTime: 100, duration: 16 },
        { name: "input", startTime: 101.2, duration: 24 },
      ];
      expect(ns.keystrokeEventEnd(e, 100)).toBeCloseTo(125.2, 9);
    });
    test("keystrokeEventEnd: keyup is excluded", () => {
      expect(ns.keystrokeEventEnd([{ name: "keyup", startTime: 100.5, duration: 80 }], 100)).toBeNull();
    });
    test("keystrokeEventEnd: entries from other keystrokes are excluded (window [t-0.01, t+4])", () => {
      const e = [
        { name: "keydown", startTime: 99.9, duration: 500 }, // before the window
        { name: "keydown", startTime: 104.5, duration: 500 }, // after the window
        { name: "keydown", startTime: 100, duration: 16 }, // in
        { name: "input", startTime: 104, duration: 8 }, // in, at the edge
      ];
      expect(ns.keystrokeEventEnd(e, 100)).toBe(116);
    });
    test("keystrokeEventEnd: no entries -> null", () => {
      expect(ns.keystrokeEventEnd([], 100)).toBeNull();
    });

    test("isLate: strictly greater than D; equality is on time", () => {
      expect(ns.isLate(133.4, 133.4)).toBe(false);
      expect(ns.isLate(133.5, 133.4)).toBe(true);
    });
    test("isLate: null (no entry, < 16 ms) is on time", () => {
      expect(ns.isLate(null, 0)).toBe(false);
    });

    test("framesToPaint: 1 on time; 2 up to one R late; 3 up to two", () => {
      const D = 100;
      expect(ns.framesToPaint(null, D, R)).toBe(1);
      expect(ns.framesToPaint(100, D, R)).toBe(1);
      expect(ns.framesToPaint(100.1, D, R)).toBe(2);
      expect(ns.framesToPaint(100 + R - 0.01, D, R)).toBe(2);
      expect(ns.framesToPaint(100 + R + 0.01, D, R)).toBe(3);
    });

    test("scoreKeystroke: recorded-shape cases from the empirical step", () => {
      // frames every R; keystroke at phase 0.5 of frame j. D = ts_j + 2R.
      const frames = [0, R, 2 * R, 3 * R, 4 * R];
      const t = 2 * R + R / 2;
      const D = 2 * R + 2 * R; // 4R
      // positive-shaped: 16 ms entry, ETend = t + 16 = 3.4R-ish, before D
      const on = ns.scoreKeystroke(frames, [{ name: "keydown", startTime: t, duration: 16 }], t, R);
      expect(on.deadline).toBeCloseTo(D, 9);
      expect(on.late).toBe(false);
      // negative-shaped: 40 ms entry, ETend = t + 40 > D
      const late = ns.scoreKeystroke(frames, [{ name: "keydown", startTime: t, duration: 40 }], t, R);
      expect(late.late).toBe(true);
      expect(late.framesToPaint).toBeGreaterThan(1);
      // no entry at all -> on time
      expect(ns.scoreKeystroke(frames, [], t, R).late).toBe(false);
    });

    test("summarise: PASS / FAIL / INDETERMINATE against the bar, and the reporting counts", () => {
      const mk = (margin: number | null, t = 10, tsJ = 0): KeystrokeScore => ({ t, tsJ, deadline: 100, etEnd: margin === null ? null : 100 + margin, late: false, framesToPaint: 1 });
      // 20 keys: 19 clearly on time (margin -10, 5 of them with no entry), 1 at +2 (late at 0, on time at +4, late at -4)
      const clear = [...Array(14).fill(0).map(() => mk(-10)), ...Array(5).fill(0).map(() => mk(null))];
      const s20 = [...clear, mk(2)];
      const m = ns.summarise(s20, 0.95, R);
      expect(m.n).toBe(20);
      expect(m.onTimeAtMinus4).toBe(0.95); expect(m.onTimeAtZero).toBe(0.95); expect(m.onTimeAtPlus4).toBe(1);
      expect(m.scoredOnTimeByAbsence).toBe(5);
      expect(m.verdict).toBe("PASS");
      // two keys inside the rounding band: 18/20 at -4, 18/20 at 0, 20/20 at +4 -> bar 0.95 is between
      const s2 = [...Array(18).fill(0).map(() => mk(-10)), mk(2), mk(3)];
      expect(ns.summarise(s2, 0.95, R).verdict).toBe("INDETERMINATE");
      // two keys clearly late: fails even at +4
      const s3 = [...Array(18).fill(0).map(() => mk(-10)), mk(30), mk(30)];
      expect(ns.summarise(s3, 0.95, R).verdict).toBe("FAIL");
      // stale anchor: t - tsJ >= R
      expect(ns.summarise([mk(-10, 40, 0), mk(-10, 5, 0)], 0.5, R).staleAnchors).toBe(1);
    });

    test("summarise: worst absent-key margin is D - t - 16 over keys scored by absence", () => {
      const mk = (t: number): KeystrokeScore => ({ t, tsJ: 0, deadline: 33.4, etEnd: null, late: false, framesToPaint: 1 });
      const m = ns.summarise([mk(1), mk(16.5), mk(5)], 0.5, R);
      expect(m.scoredOnTimeByAbsence).toBe(3);
      expect(m.worstAbsentMarginMs).toBeCloseTo(33.4 - 16.5 - 16, 9);
      const none = ns.summarise([{ ...mk(1), etEnd: 20 }], 0.5, R);
      expect(none.worstAbsentMarginMs).toBeNull();
    });

    test("R below 16 ms is rejected as an invalid run; 16 and above are accepted", () => {
      expect(() => ns.assertRefreshIntervalValid(8.33)).toThrow(/below 16 ms/);
      expect(() => ns.assertRefreshIntervalValid(15.99)).toThrow();
      expect(() => ns.assertRefreshIntervalValid(Number.NaN)).toThrow();
      expect(() => ns.assertRefreshIntervalValid(16)).not.toThrow();
      expect(() => ns.assertRefreshIntervalValid(16.7)).not.toThrow();
    });

    test("scoreAll maps keystrokes one to one", () => {
      const frames = [0, R, 2 * R, 3 * R, 4 * R, 5 * R, 6 * R];
      const s = ns.scoreAll(frames, [], [R + 1, 3 * R + 1], R);
      expect(s.map((x) => x.tsJ)).toEqual([R, 3 * R]);
    });
  });
}

test.describe("aggregates", () => {
  const sc = (...late: boolean[]): KeystrokeScore[] =>
    late.map((l) => ({ t: 0, tsJ: 0, deadline: 0, etEnd: null, late: l, framesToPaint: l ? 2 : 1 }));
  test("19/20 on time is exactly the 95% boundary", () => {
    expect(percentOnTime(sc(...Array(19).fill(false), true))).toBe(0.95);
  });
  test("percentLate is the complement", () => {
    expect(percentLate(sc(true, false, false, false))).toBe(0.25);
  });
  test("refuses zero scores", () => {
    expect(() => percentOnTime([])).toThrow();
    expect(() => p95FramesToPaint([])).toThrow();
  });
  test("p95 nearest-rank: n=10 gives the max (not floor((n-1)p))", () => {
    const s = [1, 1, 1, 1, 1, 1, 1, 1, 1, 5].map((f) => ({ ...sc(false)[0], framesToPaint: f }));
    expect(p95FramesToPaint(s)).toBe(5);
  });
});

test.describe("the page runs this source (fix 2, kept)", () => {
  test("namespaceSource contains each function's own toString()", () => {
    const src = namespaceSource();
    for (const f of Object.values(direct.SeratLatencyFns)) expect(src).toContain(f.toString());
  });
  test("an override replaces exactly that function, and the namespace uses it", () => {
    const ns = loadNamespace(
      namespaceSource({ isLate: "function isLate(etEnd, deadlineMs) { return false; }" }),
    );
    const frames = [0, R, 2 * R, 3 * R];
    const s = ns.scoreKeystroke(frames, [{ name: "keydown", startTime: 2 * R + 1, duration: 400 }], 2 * R + 1, R);
    expect(s.late).toBe(false); // the mutant forces on time; the unmutated rule says late
    expect(direct.scoreKeystroke(frames, [{ name: "keydown", startTime: 2 * R + 1, duration: 400 }], 2 * R + 1, R).late).toBe(true);
  });
  test("an override for an unknown function is refused", () => {
    expect(() => namespaceSource({ nope: "function nope(){}" } as never)).toThrow();
  });
});

test("mulberry32 is deterministic and in [0,1)", () => {
  const a = mulberry32(7), b = mulberry32(7);
  for (let i = 0; i < 50; i++) {
    const x = a();
    expect(x).toBe(b());
    expect(x).toBeGreaterThanOrEqual(0);
    expect(x).toBeLessThan(1);
  }
});
