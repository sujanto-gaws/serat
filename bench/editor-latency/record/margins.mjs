// Distribution of (Event Timing end - D) over ENTRY-BEARING keystrokes only, per recording;
// D = ts_j + 2R. Keystrokes with no reported event entry (duration < 16 ms, scored on time by
// absence) are COUNTED separately and are not in the quantiles (an earlier version put them in
// as a -99 placeholder, which made the positive p10/p50 meaningless).
import fs from "node:fs";
import { keyFeatures } from "./analyze.mjs";
for (const f of process.argv.slice(2)) {
  const d = JSON.parse(fs.readFileSync(f));
  const ft = keyFeatures(d.rec, d.R).slice(3);
  const v = ft.filter((x) => x.hasEt).map((x) => x.etEnd - x.D).sort((a, b) => a - b);
  const q = (p) => (v.length ? v[Math.min(v.length - 1, Math.floor(p * v.length))].toFixed(1) : "-").padStart(6);
  console.log(
    f.split("/").pop().padEnd(12),
    `keys ${String(ft.length).padStart(3)}  noEntry(on time by absence) ${String(ft.length - v.length).padStart(3)}  withEntry ${String(v.length).padStart(3)}`,
    ` ETend-D over entry-bearing keys: min${q(0)} p10${q(0.1)} p50${q(0.5)} p90${q(0.9)} p99${q(0.99)} max${q(1)}`,
  );
}
