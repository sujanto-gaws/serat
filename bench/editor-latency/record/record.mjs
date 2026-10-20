// NFR-PRF-02 / task #11 empirical step: RAW RECORDER. No rule is applied here.
// Per frame: {ts, run, end, q}. Per keystroke: {t, pStart, p}. Plus every Long
// Animation Frame entry and every Event Timing entry (durationThreshold 16).
// Rules are applied OFFLINE by analyze.mjs on the same recordings.
//   node record.mjs <arm> <n> <outfile> [jitter]
//   arm: positive | negative | graded ; block = 0 | 2.2R | 1.5R busy-wait in the
//   target's own `input` handler (the existing negative-control technique).
//   jitter=1: cadence 100ms + U(0,R) to sweep keystroke phase (strict arm is 100ms).
import { chromium } from "@playwright/test";
import fs from "node:fs";
const [arm, nStr, out, jitter] = process.argv.slice(2);
const n = +nStr;
const blockR = { positive: 0, negative: 2.2, graded: 1.5 }[arm] ?? 0;
// render arms: the work happens in an rAF callback queued by the input handler (this is
// where CodeMirror 6's requestMeasure/DOM update runs), i.e. AFTER the recorder's own callback in
// the same frame, so frame k's recorded `run` is not delayed but its presentation is.
const rafBlockR = { render2: 2.2, render1: 1.0, render05: 0.5 }[arm] ?? 0;
const paintArm = arm === "paint";
const browser = await chromium.launch({
  executablePath: process.env.PW_CHROMIUM_PATH ?? "/opt/pw-browsers/chromium",
});
const page = await browser.newPage();
await page.setContent(
  '<!doctype html><body><div id="t" contenteditable="true" style="width:600px;height:200px;font:16px sans-serif;outline:none"></div></body>');
await page.evaluate(() => {
  const w = window;
  w.rec = { frames: [], keys: [], loaf: [], et: [] };
  const loop = (ts) => {
    const run = performance.now();
    const f = { ts, run, end: 0, q: 0 };
    w.rec.frames.push(f);
    f.end = performance.now();
    const ch = new MessageChannel();
    ch.port2.onmessage = () => { f.q = performance.now(); };
    ch.port1.postMessage(null);
    requestAnimationFrame(loop);
  };
  requestAnimationFrame(loop);
  const obs = (type, extra, cb) => new PerformanceObserver((l) => l.getEntries().forEach(cb))
    .observe({ type, buffered: true, ...extra });
  obs("long-animation-frame", {}, (e) => w.rec.loaf.push(e.toJSON()));
  obs("event", { durationThreshold: 16 }, (e) => w.rec.et.push(e.toJSON()));
  document.addEventListener("keydown", (e) => { if (e.isTrusted) w.rec.keys.push({ t: e.timeStamp, pStart: 0, p: 0 }); }, true);
  window.addEventListener("input", (e) => { if (e.isTrusted) w.rec.keys[w.rec.keys.length - 1].pStart = performance.now(); }, true);
  document.addEventListener("input", (e) => { if (e.isTrusted) w.rec.keys[w.rec.keys.length - 1].p = performance.now(); }, false);
});
await page.waitForFunction(() => window.rec.frames.length > 130);
const R = await page.evaluate(() => {
  const f = window.rec.frames.slice(0, 121).map((x) => x.ts), d = [];
  for (let i = 1; i < f.length; i++) d.push(f[i] - f[i - 1]);
  d.sort((a, b) => a - b); return (d[59] + d[60]) / 2;
});
if (blockR) await page.evaluate(({ ms }) => {
  document.getElementById("t").addEventListener("input", () => {
    const d = performance.now() + ms; while (performance.now() < d) {}
  });
}, { ms: blockR * R });
if (rafBlockR) await page.evaluate(({ ms }) => {
  document.getElementById("t").addEventListener("input", () => {
    requestAnimationFrame(() => { const d = performance.now() + ms; while (performance.now() < d) {} });
  });
}, { ms: rafBlockR * R });
if (paintArm) await page.evaluate(() => {
  const big = document.createElement("div");
  big.style.cssText = "position:fixed;left:0;top:220px;width:1400px;height:700px;background:linear-gradient(red,blue);filter:blur(30px) drop-shadow(0 0 40px black)";
  document.body.appendChild(big);
  document.getElementById("t").addEventListener("input", () => { big.style.opacity = big.style.opacity === "0.9" ? "1" : "0.9"; });
});
await page.focus("#t");
for (let i = 0; i < n; i++) {
  await page.keyboard.press("x");
  const wait = 100 + (jitter ? Math.random() * R : 0);
  await new Promise((r) => setTimeout(r, wait));
}
await new Promise((r) => setTimeout(r, 500));
const rec = await page.evaluate(() => window.rec);
const ver = browser.version();
await browser.close();
fs.writeFileSync(out, JSON.stringify({ arm, n, jitter: !!jitter, R, blockR, engine: "headless chromium " + ver, rec }));
console.log(arm, n, "R=" + R.toFixed(3), "keys=" + rec.keys.length, "frames=" + rec.frames.length, "loaf=" + rec.loaf.length, "et=" + rec.et.length, ver);
