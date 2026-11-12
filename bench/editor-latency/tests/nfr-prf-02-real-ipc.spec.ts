// NFR-PRF-02 budget job, the real-IPC arm (task #61, rule E0, see measure.ts).
// Serves NFR-PRF-02, FR-EDT-01, FR-VLT-03, US-35, C-06, architecture.md 9.3 "the stub answers with latencies
// recorded from the real binary".
//
// Playwright project "real-ipc" (depends on "controls"). The same keystrokes, the same caret place and the same
// scoring as the subject, in two arms on the same run:
//
//   stub   the IPC client answers instantly (as in the subject spec); `save_patch` returns a plausible FileWritten
//   real   every IPC command is forwarded over loopback to a REAL serat-app `AppState` with a real vault open
//          (crates/serat-app/examples/editor_ipc_bridge.rs): `save_patch` patches the note's byte range and submits
//          it to the index writer, `note_opened` registers the buffer, `read_file` reads.
//
// Typing is in bursts (25 keys, then a pause that puts the debounced save, 1 s idle, at the start of the next
// burst) so that the save round trip, and the index work it starts, land INSIDE the measured keys. After each run
// the harness asserts the real saves landed: the note on disk is the fixture plus exactly the typed characters.
//
// WHAT THIS ARM DOES NOT INCLUDE: Tauri's own transport between the webview and the Rust process. The bridge is
// a separate process on loopback; WebView2's message channel and the custom-protocol serialisation are not in
// the figure. Rust work is off the renderer's main thread, as in the application. The bridge also prints
// per-command p50/p95/max measured inside it, which is the table the plan asks the stub to replay.
//
// Needs the bridge binary: `cargo build --example editor_ipc_bridge -p serat-app` (path in SERAT_IPC_BRIDGE, or
// $CARGO_TARGET_DIR/debug/examples/editor_ipc_bridge). Without it the real arm fails loudly with that command.
// REPORT ONLY, like the scenarios.
import { test, expect } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { installCapture, runKeystrokes, percentOnTime, p95FramesToPaint, latencyDiagnostic, type Cadence } from "./measure";
import { SUBJECT_MIN_ON_TIME, SUBJECT_KEYS } from "./arms";
import { openNoteForLatencyTest, buildFixtureNote } from "./subject";
import { goToLine, placeOnLine, findLine } from "./caret";
import { ENGINE, RULE_STATEMENT, fmt, pct } from "./report";

const SUBJECT_LABEL = process.env.SERAT_SUBJECT_LABEL ?? "unlabelled build";
const TEXT = buildFixtureNote();
const LINES = TEXT.split("\n");
const NOTE = "bench.md";
/** 25 keys, then 950 ms more: the 1 s autosave fires about 50 ms before the next burst's first key. */
const BURST = { every: 25, ms: 950 };

test.beforeAll(async ({ browser }) => { console.log(RULE_STATEMENT(browser.version())); });

function bridgePath(): string {
  const target = process.env.CARGO_TARGET_DIR ?? path.resolve(path.dirname(new URL(import.meta.url).pathname), "../../../target");
  return process.env.SERAT_IPC_BRIDGE ?? path.join(target, "debug", "examples", "editor_ipc_bridge");
}

interface Bridge { url: string; vault: string; stop: () => Promise<void> }

async function startBridge(): Promise<Bridge> {
  const exe = bridgePath();
  if (!fs.existsSync(exe)) {
    throw new Error(`no IPC bridge at ${exe}. Build it: cargo build --example editor_ipc_bridge -p serat-app (or set SERAT_IPC_BRIDGE).`);
  }
  const vault = fs.mkdtempSync(path.join(os.tmpdir(), "serat-prf02-vault-"));
  fs.writeFileSync(path.join(vault, NOTE), TEXT);
  const child: ChildProcess = spawn(exe, [vault], { stdio: ["pipe", "pipe", "inherit"] });
  const port = await new Promise<number>((resolve, reject) => {
    let buf = "";
    const timer = setTimeout(() => reject(new Error("the IPC bridge did not print READY within 90 s")), 90_000);
    child.stdout!.on("data", (d) => {
      buf += String(d);
      const m = /READY (\d+)/.exec(buf);
      if (m) { clearTimeout(timer); resolve(Number(m[1])); }
    });
    child.once("exit", (code) => reject(new Error(`the IPC bridge exited early (${code})`)));
  });
  return {
    url: `http://127.0.0.1:${port}`,
    vault,
    stop: async () => {
      child.stdin!.end();
      await new Promise<void>((r) => { child.once("exit", () => r()); setTimeout(r, 3000); });
      fs.rmSync(vault, { recursive: true, force: true });
    },
  };
}

async function invoke<T>(bridge: Bridge, command: string, args: unknown): Promise<T> {
  const r = await fetch(`${bridge.url}/invoke/${command}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ args }) });
  const v = (await r.json()) as { __error?: unknown } & T;
  if (v && (v as { __error?: unknown }).__error) throw new Error(`${command}: ${JSON.stringify((v as { __error: unknown }).__error)}`);
  return v;
}

const cadences: [string, Cadence][] = [["strict", { kind: "strict" }], ["swept", { kind: "swept", seed: 1 }]];

for (const arm of ["stub", "real"] as const) {
  test(`NFR-PRF-02 real-IPC [${arm}]: typing mid-note in bursts, saves landing inside the measured keys; >= 95% in the first frame`, async ({ page }) => {
    test.setTimeout(900_000);
    const bridge = arm === "real" ? await startBridge() : null;
    try {
      const chromiumVersion = page.context().browser()?.version();
      const rows: string[] = [];
      const verdicts: string[] = [];
      const rs: number[] = [];
      let worst = 1;
      let opened = false;
      for (const [label, cadence] of cadences) {
        let note: { path: string; hash: string } | undefined;
        if (bridge) {
          // Fresh note on disk for every cadence; the previous page's buffer is closed first.
          if (opened) await invoke(bridge, "note_closed", { path: NOTE, base_hash: "" }).catch(() => undefined);
          fs.writeFileSync(path.join(bridge.vault, NOTE), TEXT);
          await new Promise((r) => setTimeout(r, 1500)); // let the watcher and the index settle on the rewrite
          const read = await invoke<{ hash: string; text: string }>(bridge, "read_file", { path: NOTE });
          expect(read.text.length, "the vault holds the fixture").toBe(TEXT.length);
          note = { path: NOTE, hash: read.hash };
        }
        // One fresh document per cadence (the init scripts re-run on reload); the first cadence reuses the first load.
        const subject = await openNoteForLatencyTest(page, { text: TEXT, ipcBridge: bridge?.url, note });
        opened = true;
        const { refreshIntervalMs } = await installCapture(page);
        rs.push(refreshIntervalMs);
        const at = findLine(LINES, 0.5, (l) => /^Paragraph \d+ links/.test(l));
        const n = /^Paragraph (\d+) /.exec(LINES[at])![1];
        const line = await goToLine(page, subject.editor, LINES, at, `Paragraph ${n} links`);
        await placeOnLine(page, subject.editor, line, `Paragraph ${n} links`, "end", LINES[at].length);
        await new Promise((r) => setTimeout(r, 600));
        const { scores, summary } = await runKeystrokes(page, subject.editor, SUBJECT_KEYS, { cadence, bar: SUBJECT_MIN_ON_TIME, burst: BURST });
        verdicts.push(summary.verdict);
        const on = percentOnTime(scores);
        worst = Math.min(worst, on);
        const d = latencyDiagnostic(scores);
        let ipcNote = "IPC answered instantly by the stub";
        if (bridge) {
          // The real saves must have landed: the note on disk is the fixture plus exactly the typed characters.
          let onDisk = -1;
          for (let i = 0; i < 30 && onDisk !== TEXT.length + SUBJECT_KEYS; i++) {
            await new Promise((r) => setTimeout(r, 200));
            onDisk = (await invoke<{ text: string }>(bridge, "read_file", { path: NOTE })).text.length;
          }
          expect(onDisk, "the real save_patch calls left the note as the fixture plus the typed characters").toBe(TEXT.length + SUBJECT_KEYS);
          const stats = (await (await fetch(`${bridge.url}/stats`)).json()) as Record<string, { count: number; p50_ms: number; p95_ms: number; max_ms: number }>;
          const sp = stats["save_patch"];
          expect(sp?.count ?? 0, "saves went through the bridge during the run").toBeGreaterThanOrEqual(10);
          ipcNote = `REAL serat-app over loopback; bridge-side ms per command: ${Object.entries(stats).map(([c, s]) => `${c} n=${s.count} p50=${s.p50_ms.toFixed(1)} p95=${s.p95_ms.toFixed(1)} max=${s.max_ms.toFixed(1)}`).join(", ")}`;
        }
        rows.push(`${label} (bursts ${BURST.every}+${BURST.ms} ms; ${ipcNote}): ${fmt(summary)}; ${pct(on)} on time, p95 frames=${p95FramesToPaint(scores)}, ${d.withEntry}/${SUBJECT_KEYS} with ET entry (mean ${d.meanMs.toFixed(1)} ms, max ${d.maxMs.toFixed(1)} ms)`);
        const dump = process.env.SERAT_DUMP_DIR;
        if (dump) {
          fs.mkdirSync(dump, { recursive: true });
          fs.writeFileSync(`${dump}/${SUBJECT_LABEL}-realipc-${arm}-${label}.json`, JSON.stringify({ build: SUBJECT_LABEL, chromium: chromiumVersion, arm, cadence: label, R: refreshIntervalMs, summary, scores }));
        }
        await subject.close();
      }
      console.log(`REAL-IPC ${SUBJECT_LABEL} [${arm}] report-only; n=${SUBJECT_KEYS} per cadence, R=${rs.map((r) => r.toFixed(2)).join("/")}, load average ${os.loadavg().map((x) => x.toFixed(1)).join("/")} on ${os.cpus().length} CPUs, ${ENGINE(chromiumVersion)}: ${rows.join(" | ")}`);
      expect(verdicts, "every cadence must be PASS; INDETERMINATE is not a pass").toEqual(["PASS", "PASS"]);
      expect(worst).toBeGreaterThanOrEqual(SUBJECT_MIN_ON_TIME);
    } finally {
      await bridge?.stop();
    }
  });
}
