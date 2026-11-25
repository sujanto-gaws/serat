// The seam: the only module that knows how to reach the app under test.
//
// The subject is the PRODUCTION Vue/CodeMirror build with the IPC client stubbed
// (architecture.md §9.3): `npm run build:bench` in app/ writes app/dist-bench/ and
// installs `window.__seratBenchOpenNote(text)` (bench builds only; a release build
// contains neither the branch nor the global). This module serves that directory over
// loopback HTTP (a Tauri webview has no origin Playwright can navigate to), stubs the
// Tauri IPC internals before any script runs, waits for the hook, loads the 5,000-line
// fixture and returns the `.cm-content` locator (Playwright CSS pierces the editor's
// open shadow root).
//
// Fails loudly, with the build command, when dist-bench is missing or the hook never
// appears: a harness that reported "skipped" could be mistaken for a green run.
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { Locator, Page } from "@playwright/test";

export const FIXTURE_NOTE_LINE_COUNT = 5000;

/** Where the bench build lives. Override with SERAT_BENCH_DIST (e.g. a scratch checkout). */
export function benchDistDir(): string {
  return path.resolve(process.env.SERAT_BENCH_DIST ?? path.join(path.dirname(fileURLToPath(import.meta.url)), "../../../app/dist-bench"));
}

/** Deterministic (no randomness, no clock) 5,000-line note in §9.3's shape: headings
 * every ~40 lines, dense wikilinks, GFM tables, task lists, footnotes, fenced code,
 * bold/italic/inline code. Exactly FIXTURE_NOTE_LINE_COUNT lines, LF-terminated. */
export function buildFixtureNote(lines: number = FIXTURE_NOTE_LINE_COUNT): string {
  const out: string[] = [];
  let n = 0;
  while (out.length < lines) {
    n++;
    const h = Math.floor(n / 40);
    if (n % 40 === 1) {
      out.push(`## Section ${h}`, "");
    } else if (n % 40 === 5) {
      out.push("| key | value | link |", "| --- | --- | --- |");
      for (let r = 0; r < 4; r++) out.push(`| row ${r} | ${n * 7 + r} | [[Note ${(n + r) % 97}]] |`);
      out.push("");
    } else if (n % 40 === 12) {
      out.push("```rust", `fn f_${n}(x: u32) -> u32 { x.wrapping_mul(${n}) }`, "let y = f(1);", "```", "");
    } else if (n % 40 === 20) {
      out.push(`- [ ] task ${n} about [[Project ${n % 31}]]`, `- [x] done ${n}`, `- [ ] later [[Note ${n % 53}|alias ${n}]]`, "");
    } else if (n % 40 === 30) {
      out.push(`A claim with a footnote[^${n}] and **bold ${n}** and *italic* and \`code ${n}\`.`, "", `[^${n}]: the footnote text for ${n}.`, "");
    } else {
      out.push(
        `Paragraph ${n} links [[Note ${n % 211}]], [[Folder/Deep ${n % 67}#Heading]] and [[Person ${n % 19}|them]]; ` +
          `it carries owner:: person-${n % 13} and a tag #t${n % 17}.`,
        "",
      );
    }
  }
  return out.slice(0, lines).join("\n") + "\n";
}

/** The fixture's three on-disk shapes (ADR-0016: the 20k vault carries CRLF and BOM notes). The editor holds
 * the same 5,000 lines in all three (CodeMirror splits on \r\n, and a BOM is one character of line 0), so
 * these exercise the load path and the line model, not a different typing path. */
export type FixtureVariant = "lf" | "crlf" | "bom+crlf";

export function fixtureVariant(note: string, variant: FixtureVariant): string {
  if (variant === "lf") return note;
  const crlf = note.replace(/\n/g, "\r\n");
  return variant === "crlf" ? crlf : "\uFEFF" + crlf;
}

function serve(dir: string): Promise<{ url: string; close: () => Promise<void> }> {
  const types: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" };
  const server = http.createServer((req, res) => {
    const rel = decodeURIComponent((req.url ?? "/").split("?")[0]);
    const file = path.join(dir, rel === "/" ? "index.html" : rel);
    if (!file.startsWith(dir) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) {
      res.writeHead(404).end();
      return;
    }
    res.writeHead(200, { "content-type": types[path.extname(file)] ?? "application/octet-stream" });
    fs.createReadStream(file).pipe(res);
  });
  return new Promise((resolve) =>
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address() as { port: number };
      resolve({ url: `http://127.0.0.1:${port}/`, close: () => new Promise((r) => server.close(() => r())) });
    }),
  );
}

/** Tauri v2 IPC internals, stubbed: every command resolves; listeners register and never fire. */
const TAURI_STUB = `
  (function () {
    var cbs = 0;
    // Task #147: since the vault chooser (#151) the shell shows "No vault open" and hides the editor (v-show) unless
    // the launch boot info names a vault. The harness stubs one, so the editor is mounted and visible as before.
    // vault and vault_id are fake in bridge mode too: the bridge's own vault is a temp directory the shell never sees.
    window.__SERAT_BOOT__ = { vault: "/bench/vault", vault_id: "bench-vault-id", open: null, exit_after_ready: false };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
      transformCallback: function () { return ++cbs; },
      unregisterCallback: function () {},
      invoke: function (cmd, payload) {
        if (cmd === "plugin:event|listen") return Promise.resolve(++cbs);
        // The real-IPC arm (task #61): forward every command to a real serat-app AppState over loopback.
        if (window.__SERAT_IPC_BRIDGE__ && cmd.indexOf("plugin:") !== 0) {
          return fetch(window.__SERAT_IPC_BRIDGE__ + "/invoke/" + cmd, {
            method: "POST",
            headers: { "content-type": "application/json", "x-serat-bridge-token": window.__SERAT_IPC_BRIDGE_TOKEN__ || "" },
            body: JSON.stringify(payload || {}),
          })
            .then(function (r) { return r.json(); })
            .then(function (v) { return v && v.__error ? Promise.reject(v.__error) : v; });
        }
        // NOTE (task #61): without a bridge, save_patch answers a fake success, so subject-arm figures from before
        // #61 (where it answered null) are not strictly comparable with figures from after it.
        if (cmd === "save_patch") {
          var a = (payload && payload.args) || {};
          return Promise.resolve({ path: a.path || "", hash: "stub-hash-" + (++cbs), mtime_ms: 0, bytes: 0 });
        }
        if (cmd === "index_status") return Promise.resolve({ state: "idle" });
        // Task #147: both used to be answered with null, which threw in the app (null.toFixed, null.map) on every run.
        if (cmd === "mark_first_paint" || cmd === "mark_vault_usable") return Promise.resolve(1);
        // Stub arm only: with a bridge, resolve_links and note_anchors were forwarded above to the real commands (task #181).
        if (cmd === "resolve_links") return Promise.resolve([]);
        if (cmd === "note_anchors") return Promise.resolve({ kind: "not_ready", state: { kind: "opening" } });
        if (cmd === "get_ui_settings") return Promise.resolve(window.__SERAT_STUB_UI_SETTINGS__ || null);
        return Promise.resolve(null);
      },
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: function () {} };
  })();
`;

export interface Subject {
  editor: Locator;
  close: () => Promise<void>;
}

export interface OpenOptions {
  /** The note to load (default: the LF fixture). */
  text?: string;
  /** What `get_ui_settings` answers; `{ live_preview: false }` selects the split-preview mode (FR-EDT-10). */
  uiSettings?: { live_preview: boolean; keybindings: unknown[] };
  /** Base URL of the editor_ipc_bridge example: every IPC command is answered by a real `serat-app`. */
  ipcBridge?: string;
  /** The per-run token the bridge printed on its READY line; sent as `x-serat-bridge-token`. */
  ipcToken?: string;
  /** With a bridge: the note's vault path and its real content hash (from `read_file`). */
  note?: { path: string; hash: string };
}

/** Serves dist-bench, loads the fixture and returns the editable surface. */
export async function openNoteForLatencyTest(page: Page, options: OpenOptions = {}): Promise<Subject> {
  const dir = benchDistDir();
  if (!fs.existsSync(path.join(dir, "index.html"))) {
    throw new Error(
      `no bench build at ${dir}. Build it: (cd app && npm ci && npm run build:bench), or set SERAT_BENCH_DIST.`,
    );
  }
  const server = await serve(dir);
  const pageErrors: string[] = [];
  page.on("pageerror", (e) => pageErrors.push(String(e)));
  if (options.uiSettings) {
    await page.addInitScript({ content: `window.__SERAT_STUB_UI_SETTINGS__ = ${JSON.stringify(options.uiSettings)};` });
  }
  if (options.ipcBridge) {
    await page.addInitScript({ content: `window.__SERAT_IPC_BRIDGE__ = ${JSON.stringify(options.ipcBridge)}; window.__SERAT_IPC_BRIDGE_TOKEN__ = ${JSON.stringify(options.ipcToken ?? "")};` });
  }
  await page.addInitScript({ content: TAURI_STUB });
  // Starvation repro for the harness (task #202): SERAT_CPU_THROTTLE=6 slows the page 6x. Never set in CI.
  const throttle = Number(process.env.SERAT_CPU_THROTTLE ?? "1");
  if (throttle > 1) await (await page.context().newCDPSession(page)).send("Emulation.setCPUThrottlingRate", { rate: throttle });
  await page.goto(server.url);
  await page.waitForFunction(() => typeof (window as any).__seratBenchOpenNote === "function", null, { timeout: 10_000 }).catch((e) => {
    throw new Error(`window.__seratBenchOpenNote never appeared (is this a --mode bench build?): ${e.message}; page errors: ${pageErrors.join(" | ")}`);
  });
  const text = options.text ?? buildFixtureNote();
  await page.evaluate(({ t, n }) => (window as any).__seratBenchOpenNote(t, n?.path, n?.hash), { t: text, n: options.note });
  const editor = page.locator('[data-testid="serat-editor"] .cm-content');
  await editor.waitFor({ state: "visible", timeout: 10_000 });
  await editor.getByText("Section 0").first().waitFor({ timeout: 10_000 });
  if (options.uiSettings?.live_preview === false) {
    await page.locator(".serat-editor-split-preview").waitFor({ state: "attached", timeout: 10_000 });
  }
  return { editor, close: server.close };
}
