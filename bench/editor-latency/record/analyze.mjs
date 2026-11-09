// OFFLINE rule evaluation over raw recordings made by record.mjs. Task #11.
//   node analyze.mjs [--keys] file1.json file2.json ...
// Per recording prints, for each candidate rule, the % of keystrokes judged LATE.
// "score" for positive = % on time (bar >= 99); negative = % late (bar >= 95).
//
// Notation (all times on the page's performance.now() clock):
//   t        keydown event.timeStamp          p   `input` handler return seen by a
//                                                 bubble listener on document
//   ts_j     last recorded rAF timestamp <= t   D   ts_j + 2R  (deadline: the first
//                                                 display frame after the keystroke
//                                                 is j+1 and it must PRESENT by the
//                                                 vsync that ends it, ts_{j+2})
//   k        first recorded rAF callback with run >= p
//   q_k      MessageChannel message time posted from frame k's rAF callback
//   ET end   Event Timing entry startTime + duration (presentation of the next
//            frame after the event; duration is rounded to 8 ms; only >= 16 reported,
//            so an absent entry means "presented within 16 ms": on time)
import fs from "node:fs";

const args = process.argv.slice(2).filter((a) => !a.startsWith("--"));
const showKeys = process.argv.includes("--keys");

export function keyFeatures(rec, R) {
  const { frames, keys, loaf, et } = rec;
  const out = [];
  for (const kk of keys) {
    if (!kk.p) continue;
    let j = -1;
    for (let i = 0; i < frames.length; i++) { if (frames[i].ts <= kk.t) j = i; else break; }
    let k = frames.findIndex((f, i) => i > j - 1 && f.run >= kk.p);
    if (j < 0 || k < 0 || !frames[k].q) continue;
    const D = frames[j].ts + 2 * R;
    // events belonging to this keystroke (keydown, keypress, beforeinput, input)
    const evs = et.filter((e) => e.name !== "keyup" && e.startTime >= kk.t - 0.01 && e.startTime <= kk.t + 4);
    const etEnd = evs.length ? Math.max(...evs.map((e) => e.startTime + e.duration)) : -Infinity;
    // LoAF covering frame k's callback
    const lf = loaf.find((l) => l.startTime - 1 <= frames[k].run && frames[k].run <= l.startTime + l.duration + 1);
    out.push({ t: kk.t, p: kk.p, pStart: kk.pStart, j, k, D, fj: frames[j], fk: frames[k], etEnd, hasEt: evs.length > 0, lf,
      nFramesInRange: frames.filter((f) => f.ts > kk.t && f.ts <= frames[k].ts).length,
      phase: (kk.t - frames[j].ts) / R, handlerMs: kk.p - kk.t });
  }
  return out;
}

export const RULES = {
  // History (documented failures), reproduced on these recordings:
  "A  count rAF ts in (t,ts_k]>1": (f, R) => Math.max(1, f.nFramesInRange) > 1,
  "B  round((ts_k-ts_j)/R)>1": (f, R) => Math.round((f.fk.ts - f.fj.ts) / R) > 1,
  "C  q_k > D (post-paint MC)": (f, R) => f.fk.q > f.D,
  // Candidates:
  "D  run_k > D": (f, R) => f.fk.run > f.D,
  "D+ run_k>D or LoAF end>D (LoAF render)": (f, R) =>
    f.fk.run > f.D || (f.lf ? f.lf.startTime + f.lf.duration > f.D : false),
  "E0 ET end > D": (f, R) => f.etEnd > f.D,
  "E4 ET end > D+4 (rounding)": (f, R) => f.etEnd > f.D + 4,
  "E8 ET end > D+8": (f, R) => f.etEnd > f.D + 8,
  "F  run_k>D or ET end>D+4": (f, R) => f.fk.run > f.D || f.etEnd > f.D + 4,
  "G  ET dur-based: end-t > 2R+4": (f, R) => f.etEnd - f.t > 2 * R + 4,
  "M  mutant: framesToPaint=1": () => false,
};

function pct(a, n) { return (100 * a / n).toFixed(1).padStart(5); }

const isMain = import.meta.url.endsWith(process.argv[1].split("/").pop());
for (const file of isMain ? args : []) {
  const d = JSON.parse(fs.readFileSync(file));
  const feats = keyFeatures(d.rec, d.R).slice(3); // drop first 3 keys (focus warm-up)
  const n = feats.length;
  const name = file.split("/").pop();
  const hdr = `${name}  arm=${d.arm}${d.jitter ? "+jitter" : ""} n=${n} R=${d.R.toFixed(2)} ${d.engine}`;
  console.log("\n" + hdr);
  const ph = feats.map((f) => f.phase);
  console.log(`  phase(t in frame, /R): min ${Math.min(...ph).toFixed(2)} med ${ph.sort((a, b) => a - b)[n >> 1].toFixed(2)} max ${Math.max(...ph).toFixed(2)};  frac phase>=0.88: ${pct(ph.filter((x) => x >= 0.88).length, n)}%;  k=j+1: ${pct(feats.filter((f) => f.k === f.j + 1).length, n)}%`);
  for (const [rule, fn] of Object.entries(RULES)) {
    const late = feats.filter((f) => fn(f, d.R)).length;
    const score = d.arm === "positive" ? `${pct(n - late, n)}% on time` : `${pct(late, n)}% late`;
    console.log(`  ${rule.padEnd(42)} late ${String(late).padStart(3)}/${n}  ${score}`);
  }
  if (showKeys) for (const f of feats) console.log(JSON.stringify({ ph: +f.phase.toFixed(2), hm: +f.handlerMs.toFixed(1), kj: f.k - f.j, runD: +(f.fk.run - f.D).toFixed(1), qD: +(f.fk.q - f.D).toFixed(1), etD: f.hasEt ? +(f.etEnd - f.D).toFixed(1) : null }));
}
