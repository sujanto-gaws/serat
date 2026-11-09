// Shared output for the NFR-PRF-02 specs (controls and subject): the rule statement every run
// prints, and the one-line formatting of a RunSummary. Serves NFR-PRF-02.
import type { RunSummary } from "./measure";

export const pct = (x: number) => (x * 100).toFixed(1) + "%";
/** Engine label with the Chromium version (9.3 Basis: every figure carries the version and the measured R). */
export const ENGINE = (version: string | undefined) => `headless Chromium ${version ?? "version unknown"} [${process.env.SERAT_BROWSER ?? "browser unknown"}] (not gate-eligible)`;

export const RULE_STATEMENT = (version: string | undefined) =>
  `engine: ${ENGINE(version)}; ` +
  "NFR-PRF-02 rule E0: key LATE iff EventTimingEnd > D = ts_j + 2R (R = median rAF delta over 120 frames, measured per run and printed as R=); " +
  "VALIDITY: a run with R < 16 ms is INVALID (throws), because an absent Event Timing entry (duration < 16 ms) is only provably on time when D - t > R >= 16; " +
  "ABSENT KEY: no Event Timing entry >= 16 ms means presented in < 16 ms, scored ON TIME and counted in 'on time by absence', with the worst margin (D - t - 16) reported; " +
  "PER-KEY INVARIANTS: a key with any Event Timing entry must have its keydown entry, and its entry ends must lie within 8 ms, else the run is rejected; " +
  "VERDICT: PASS iff on-time share >= bar at -4, 0 and +4 ms threshold shifts, FAIL iff < bar at all, else INDETERMINATE (not a pass); " +
  "WARM-UP: none (warm-up 0), every key counts.";

export const fmt = (m: RunSummary) =>
  `warm-up 0; on time @-4/0/+4 ms = ${pct(m.onTimeAtMinus4)}/${pct(m.onTimeAtZero)}/${pct(m.onTimeAtPlus4)}; ` +
  `${m.scoredOnTimeByAbsence}/${m.n} on time by absence of an Event Timing entry; ${m.staleAnchors} stale anchors (phase >= 1R); ` +
  `worst absent-key margin ${m.worstAbsentMarginMs === null ? "n/a" : m.worstAbsentMarginMs.toFixed(1) + " ms"}; ` +
  `widest entry-end spread ${m.widestSpreadMs.toFixed(1)} ms (limit 8); verdict ${m.verdict}`;
