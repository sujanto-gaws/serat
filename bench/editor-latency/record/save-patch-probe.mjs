// The bridge-side `save_patch` probe (task #172; NFR-PRF-03 / NFR-PRF-02 evidence, ADR-0019).
//
//   node save-patch-probe.mjs <editor_ipc_bridge binary> [saves=60] [gap_ms=1000]
//
// Spawns the bridge on a fresh 5,000-line note inside the temp directory, opens the note, then
// sends `saves` one-character `save_patch` patches mid-note, one per `gap_ms` (the editor's 1 s
// autosave cadence), each on the hash the previous one returned. Prints the client-side wall
// time and the bridge's own `/stats`: the `save_patch` line and, when the bridge was built with
// phase timing (any debug build, or `--release --features bench-bridge`), one line per phase in
// call order. Report-only: it asserts nothing but that every save succeeded.
//
// This is the method of #147's 60-save probe without strace, so the two are comparable.
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const exe = process.argv[2];
const saves = Number(process.argv[3] ?? 60);
const gap = Number(process.argv[4] ?? 1000);
if (!exe) throw new Error("usage: node save-patch-probe.mjs <editor_ipc_bridge> [saves] [gap_ms]");

const vault = fs.mkdtempSync(path.join(os.tmpdir(), "serat-save-probe-"));
const lines = [];
for (let i = 0; i < 5000; i++) {
  lines.push(
    i % 7 === 0
      ? `# Section ${i}`
      : `Paragraph ${i} links [[Note ${i % 50}]], [[Folder/Deep ${i % 9}]] and **bold ${i}** text to give the line a plausible length.`,
  );
}
fs.writeFileSync(path.join(vault, "bench.md"), lines.join("\n") + "\n");

const child = spawn(exe, [vault], { stdio: ["pipe", "pipe", "inherit"] });
const ready = await new Promise((resolve) => {
  let buffer = "";
  child.stdout.on("data", (chunk) => {
    buffer += chunk;
    const m = /READY (\d+) ([0-9a-f]+)/.exec(buffer);
    if (m) resolve({ port: Number(m[1]), token: m[2] });
  });
});
const url = `http://127.0.0.1:${ready.port}`;
const headers = { "content-type": "application/json", "x-serat-bridge-token": ready.token };
async function invoke(command, args) {
  const t0 = performance.now();
  const r = await fetch(`${url}/invoke/${command}`, { method: "POST", headers, body: JSON.stringify({ args }) });
  return [await r.json(), performance.now() - t0];
}

await new Promise((r) => setTimeout(r, 4000)); // let the first index pass settle, as the harness does
const [read] = await invoke("read_file", { path: "bench.md" });
let hash = read.hash;
await invoke("note_opened", { path: "bench.md", base_hash: hash });
const wall = [];
for (let i = 0; i < saves; i++) {
  const at = { line: 2500, col: 10 + i };
  const [v, t] = await invoke("save_patch", {
    path: "bench.md",
    base_hash: hash,
    patches: [{ from: at, to: at, insert: "x" }],
  });
  if (v.__error) throw new Error(`save ${i} refused: ${JSON.stringify(v)}`);
  hash = v.hash;
  wall.push(t);
  await new Promise((r) => setTimeout(r, gap));
}
wall.sort((a, b) => a - b);
const q = (p) => wall[Math.min(wall.length - 1, Math.ceil(p * wall.length) - 1)].toFixed(1);
console.log(`client wall (loopback HTTP) n=${wall.length} p50=${q(0.5)} p95=${q(0.95)} max=${wall.at(-1).toFixed(1)} ms`);
const stats = await fetch(`${url}/stats`, { headers }).then((r) => r.json());
const f = (s) => `p50=${s.p50_ms.toFixed(2)} p95=${s.p95_ms.toFixed(2)} max=${s.max_ms.toFixed(2)}`;
console.log(`save_patch (bridge) n=${stats.save_patch.count} ${f(stats.save_patch)} ms`);
let sumP50 = 0;
for (const [name, s] of Object.entries(stats).sort()) {
  if (!name.startsWith("save_patch.phase.")) continue;
  sumP50 += s.p50_ms;
  console.log(`  ${name.slice("save_patch.phase.".length).padEnd(32)} ${f(s)}`);
}
console.log(`  (sum of phase p50s ${sumP50.toFixed(2)} ms)`);
child.stdin.end();
await new Promise((r) => child.once("exit", r));
fs.rmSync(vault, { recursive: true, force: true });
