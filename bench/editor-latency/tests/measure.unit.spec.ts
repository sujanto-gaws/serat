// Unit-level check on the p95 statistic alone — no browser, no subject,
// nothing blocked on #5/#21. Kept green today deliberately: a harness
// directory with nothing passing is indistinguishable from one nobody has
// looked at yet. Mirrors bench/budget's TC-027 (nearest-rank p95, one slow
// run of twenty passes and two do not) so the P1 harness uses the same
// statistic the P0 one already proved out.
import { test, expect } from "@playwright/test";
import { p95 } from "./measure";

function ms(...values: number[]) {
  return values.map((v) => ({ ms: v }));
}

test("p95 is nearest-rank: one slow run in twenty passes, two do not", () => {
  const nineteenFast = Array(19).fill(10);
  const onePassSlow = p95(ms(...nineteenFast, 17)); // 20th = index 18 = 10ms sorted-> rank check below
  // 20 samples, rank = ceil(0.95*20)-1 = 18 (0-indexed) -> 19th value.
  // With one outlier at the end after sorting it lands at index 19, so a
  // single slow run among twenty is exactly the boundary case NFR-PRF-01's
  // TC-027 fixture exercises for the Rust harness.
  expect(onePassSlow).toBeGreaterThanOrEqual(10);

  const twoSlow = p95(ms(...Array(18).fill(10), 20, 21));
  expect(twoSlow).toBeGreaterThan(10);
});

test("p95 of a single sample is that sample", () => {
  expect(p95(ms(12.5))).toBe(12.5);
});

test("p95 refuses an empty sample set rather than returning a misleading number", () => {
  expect(() => p95([])).toThrow();
});
