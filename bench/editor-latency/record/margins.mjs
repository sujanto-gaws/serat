// Distribution of (Event Timing end - D) per recording; D = ts_j + 2R. noET = keystrokes with no
// reported event entry (duration < 16 ms, i.e. presented in time).
import fs from "node:fs";
import { keyFeatures } from "./analyze.mjs";
for (const f of process.argv.slice(2)) {
  const d = JSON.parse(fs.readFileSync(f));
  const ft = keyFeatures(d.rec, d.R).slice(3);
  const v = ft.map((x) => (x.hasEt ? x.etEnd - x.D : -99)).sort((a, b) => a - b);
  const q = (p) => v[Math.min(v.length - 1, Math.floor(p * v.length))].toFixed(1);
  console.log(f.split("/").pop().padEnd(12), "noET", v.filter((x) => x === -99).length, "etEnd-D min", q(0), "p10", q(0.1), "p50", q(0.5), "p90", q(0.9), "p99", q(0.99), "max", q(1));
}
