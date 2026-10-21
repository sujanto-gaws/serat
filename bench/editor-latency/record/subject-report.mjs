// Per-key report over SERAT_DUMP_DIR files written by tests/nfr-prf-02.spec.ts (subject tests).
//   node subject-report.mjs dump1.json dump2.json ...
// For each file: summary shares, stale anchors (phase >= 1R) with key index, keys whose
// keydown->presentation (ET end - t) exceeds 100 ms with key index, whether the stalls sit at the
// start of the run (first 10 / first 30 keys), and the 8 worst keys by ET end - D.
import fs from "node:fs";
for (const f of process.argv.slice(2)) {
  const d = JSON.parse(fs.readFileSync(f));
  const R = d.R;
  const keys = d.scores.map((s, i) => ({
    i,
    phase: (s.t - s.tsJ) / R,
    lat: s.etEnd === null ? null : s.etEnd - s.t,
    over: s.etEnd === null ? null : s.etEnd - s.deadline,
    late: s.late,
  }));
  const stale = keys.filter((k) => k.phase >= 1);
  const slow = keys.filter((k) => k.lat !== null && k.lat > 100);
  const lateKeys = keys.filter((k) => k.late);
  const inFirst = (n, arr) => arr.filter((k) => k.i < n).length;
  const m = d.summary;
  console.log(`\n== ${d.build} [${d.where}] ${d.cadence}  n=${keys.length} R=${R.toFixed(2)}`);
  console.log(`   on time @-4/0/+4 ms: ${(100 * m.onTimeAtMinus4).toFixed(1)}/${(100 * m.onTimeAtZero).toFixed(1)}/${(100 * m.onTimeAtPlus4).toFixed(1)}%  by absence ${m.scoredOnTimeByAbsence}  verdict ${m.verdict}`);
  console.log(`   late at 0 ms: ${lateKeys.length}; at indices ${lateKeys.map((k) => k.i).join(",") || "-"}`);
  console.log(`   stale anchors (${stale.length}): ${stale.map((k) => `#${k.i}(phase ${k.phase.toFixed(2)})`).join(" ") || "-"}`);
  console.log(`   keys with ET end - t > 100 ms (${slow.length}): ${slow.map((k) => `#${k.i}=${k.lat.toFixed(0)}ms`).join(" ") || "-"}`);
  console.log(`   clustering: of ${lateKeys.length} late keys ${inFirst(10, lateKeys)} are in the first 10 keys and ${inFirst(30, lateKeys)} in the first 30; of ${slow.length} slow keys ${inFirst(10, slow)} / ${inFirst(30, slow)}`);
  const worst = [...keys].filter((k) => k.over !== null).sort((a, b) => b.over - a.over).slice(0, 8);
  console.log(`   worst by ET end - D: ${worst.map((k) => `#${k.i}:${k.over.toFixed(0)}ms@phase${k.phase.toFixed(2)}`).join(" ")}`);
}
