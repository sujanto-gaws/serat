// Unit-level check on the p95 statistic alone — no browser, no subject,
// nothing blocked on #5/#21. Kept green today deliberately: a harness
// directory with nothing passing is indistinguishable from one nobody has
// looked at yet. Mirrors bench/budget's TC-027 (nearest-rank p95, one slow
// run of twenty passes and two do not) so the P1 harness uses the same
// statistic the P0 one already proved out.
//
// Both assertions below are exact values, not loose bounds, because a
// >= or > bound is satisfied by the wrong implementation too — dropping
// the `- 1` in `p95`'s rank calculation, or replacing nearest-rank with a
// plain max, both still pass a `toBeGreaterThanOrEqual`/`toBeGreaterThan`
// check on this fixture. Verified by hand against both of those mutants
// (rank without `-1`; `Math.max(...sorted)` in place of the rank lookup):
// both turn `p95(19x10, 17) === 10` red, since a `max`-shaped or
// off-by-one implementation reports the 17/21 outlier instead of the
// nearest-rank value.
import { test, expect } from "@playwright/test";
import { p95 } from "./measure";

function ms(...values: number[]) {
  return values.map((v) => ({ ms: v }));
}

test("p95 is nearest-rank: one slow run in twenty passes", () => {
  // 20 samples, rank = ceil(0.95*20) - 1 = 18 (0-indexed) -> the 19th
  // value once sorted. A single outlier at the very end (index 19) does
  // not move it: the nearest-rank value is still 10ms.
  expect(p95(ms(...Array(19).fill(10), 17))).toBe(10);
});

test("p95 is nearest-rank: two slow runs in twenty do not pass", () => {
  // Two outliers push the 19th sorted value (index 18) up to the smaller
  // of the two, 20ms — this is the case a single-outlier-tolerant
  // implementation (e.g. one that just takes the minimum of the top N%)
  // would still get right, so it is the case that actually separates
  // nearest-rank from a plausible-looking wrong implementation.
  expect(p95(ms(...Array(18).fill(10), 20, 21))).toBe(20);
});

test("p95 of a single sample is that sample", () => {
  expect(p95(ms(12.5))).toBe(12.5);
});

test("p95 refuses an empty sample set rather than returning a misleading number", () => {
  expect(() => p95([])).toThrow();
});
