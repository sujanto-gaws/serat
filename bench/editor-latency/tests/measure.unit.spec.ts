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

    test("checkKeyInvariants: a complete key (keydown + input within 8 ms) is ok and reports its spread", () => {
      const e = [{ name: "keydown", startTime: 100, duration: 24 }, { name: "input", startTime: 101, duration: 27 }];
      const r = ns.checkKeyInvariants(e, 100);
      expect(r.ok).toBe(true);
      expect(r.spreadMs).toBeCloseTo(4, 9);
      expect(ns.checkKeyInvariants([], 100)).toEqual({ ok: true, reason: "", spreadMs: 0 });
    });
    test("checkKeyInvariants (a): an input entry without the keydown entry is rejected", () => {
      const r = ns.checkKeyInvariants([{ name: "input", startTime: 101, duration: 40 }], 100);
      expect(r.ok).toBe(false);
      expect(r.reason).toMatch(/keydown entry is missing/);
    });
    test("checkKeyInvariants (b): entry ends more than 8 ms apart are rejected; exactly 8 is accepted", () => {
      const wide = [{ name: "keydown", startTime: 100, duration: 20 }, { name: "input", startTime: 101, duration: 40 }];
      const r = ns.checkKeyInvariants(wide, 100);
      expect(r.ok).toBe(false);
      expect(r.reason).toMatch(/apart/);
      const edge = [{ name: "keydown", startTime: 100, duration: 20 }, { name: "input", startTime: 100, duration: 28 }];
      expect(ns.checkKeyInvariants(edge, 100).ok).toBe(true);
      // fractional startTime: float error must not reject an exact 8 ms spread
      const frac = [{ name: "keydown", startTime: 1000.4, duration: 16 }, { name: "input", startTime: 1000.4, duration: 24 }];
      expect(ns.checkKeyInvariants(frac, 1000.4).ok).toBe(true);
      const over = [{ name: "keydown", startTime: 1000.4, duration: 16 }, { name: "input", startTime: 1000.4, duration: 24.01 }];
      expect(ns.checkKeyInvariants(over, 1000.4).ok).toBe(false);
    });
    test("scoreAll throws 'run rejected' with the key index on either invariant failure", () => {
      const frames = [0, R, 2 * R, 3 * R, 4 * R, 5 * R, 6 * R];
      const t0 = R + 1, t1 = 3 * R + 1;
      const lost = [{ name: "input", startTime: t1 + 1, duration: 40 }];
      expect(() => ns.scoreAll(frames, lost, [t0, t1], R)).toThrow(/run rejected: key index 1 /);
      const wide = [{ name: "keydown", startTime: t0, duration: 20 }, { name: "input", startTime: t0 + 1, duration: 60 }];
      expect(() => ns.scoreAll(frames, wide, [t0, t1], R)).toThrow(/run rejected: key index 0 .*apart/);
    });
    test("scoreAll records each key's spread and summarise reports the widest", () => {
      const frames = [0, R, 2 * R, 3 * R, 4 * R, 5 * R, 6 * R];
      const t0 = R + 1, t1 = 3 * R + 1;
      const e = [
        { name: "keydown", startTime: t0, duration: 20 }, { name: "input", startTime: t0 + 1, duration: 27 },
        { name: "keydown", startTime: t1, duration: 16 },
      ];
      const sc = ns.scoreAll(frames, e, [t0, t1], R);
      expect(sc[0].spreadMs).toBeCloseTo(8, 9); expect(sc[1].spreadMs).toBe(0);
      expect(ns.summarise(sc, 0.5, R).widestSpreadMs).toBeCloseTo(8, 9);
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

// TC-181 (task #202): the caret placement that threw in Budgets run 442 is a bounded retry, not a one-shot.
import { retryUntilDeadline, judgePlacement } from "./caret";

test.describe("caret placement (NFR-PRF-02 harness, TC-181)", () => {
  test("a placement that misses twice (layout still moving) and then lands is accepted, not thrown", async () => {
    let t = 0;
    const note = await retryUntilDeadline("x", async (n) => ({ ok: n === 2, note: `try ${n}` }), 1000, () => (t += 10));
    expect(note).toBe("try 2 after 3 attempt(s)");
  });
  test("a placement that never lands throws once, after the deadline, naming every attempt and saying it is not a verdict", async () => {
    let t = 0, calls = 0;
    await expect(retryUntilDeadline("middle", async () => { calls++; return { ok: false, note: "off band" }; }, 100, () => (t += 30)))
      .rejects.toThrow(/middle: not reached within 100 ms.*not a latency verdict.*#1: off band.*#2: off band/);
    expect(calls).toBeGreaterThan(1);
    expect(calls).toBeLessThan(10);
  });
  test("one attempt is always made with a zero deadline, and none starts after it; the attempt is told the time left", async () => {
    let calls = 0, t = 0;
    const left: number[] = [];
    await expect(retryUntilDeadline("x", async (_n, rem) => { calls++; left.push(rem); t += 500; return { ok: false, note: "late" }; }, 100, () => t)).rejects.toThrow(/not a latency verdict/);
    expect(calls).toBe(1);
    expect(left).toEqual([100]);
  });
  test("judgePlacement: the 45-55% band is exclusive and a caret on no Paragraph line is a miss", () => {
    expect(judgePlacement(2500, 5000, 0.45, 0.55).ok).toBe(true);
    expect(judgePlacement(2250, 5000, 0.45, 0.55).ok).toBe(false);
    expect(judgePlacement(2750, 5000, 0.45, 0.55).ok).toBe(false);
    expect(judgePlacement(-1, 5000, 0.45, 0.55).ok).toBe(false);
  });
});
