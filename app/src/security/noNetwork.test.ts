// @vitest-environment node
//
// NFR-SEC-01 (no telemetry; US-34), NFR-SEC-02 / T-02 (no unconfigured network), T-09 (a note
// fetching a remote resource): the webview side of "the app makes no network request the user did
// not directly cause". Method and limits: docs/05-testing/no-network-evidence.md.
//
// This proves the ABSENCE OF CODE PATHS in what we ship (no URL a script could fetch, no network
// API in our own source, pdf.js configured so it has nothing to fetch). It does not observe a
// running webview; that is the reference-machine protocol's job (docs/05-testing/p1-gate-protocol.md).
import { mkdtempSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import { build } from "vite";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

const APP = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const SHIPPED = join(APP, "..", "crates", "serat-app", "webview");

/** Hosts that are XML / schema namespace identifiers: strings compared, never requested. */
const NAMESPACE_HOSTS = ["www.w3.org", "www.xfa.org", "ns.adobe.com"];

/** The ADR-0017 scheme origin in its http form (Windows/Android); `serat-asset:` has no `//`. */
const SCHEME_ORIGINS = ["http://serat-asset.localhost"];

interface Inert {
  reason: string;
  /** Occurrences in a fresh bench build. An exact count: a new occurrence fails. */
  bench: number;
  /** Occurrences in the committed crates/serat-app/webview build (stale; see the evidence doc). */
  shipped: number;
}

/**
 * Every other URL-shaped string the current bundles contain, with the reason it is inert and how
 * many times it occurs. A NEW string, a new occurrence of one of these, or an entry that no longer
 * occurs, fails the test until someone reads it and edits this table on purpose.
 */
const INERT: Record<string, Inert> = {
  "https://foo.bar": { bench: 2, shipped: 0, reason: "pdf.js: base for `new URL(relative, base)` when parsing a link; never requested" },
  "http://example.com": { bench: 1, shipped: 0, reason: "pdf.js: base for isValidFetchUrl(); never requested" },
  "https://vuejs.org/error-reference/#runtime-$": { bench: 0, shipped: 1, reason: "Vue: text appended to a console warning; never requested" },
  "https://cli.vuejs.org/guide/webpack.html#modifying-options-of-a-loader": { bench: 1, shipped: 0, reason: "Vue: documentation link inside a warning message; never requested" },
  "https://github.com/vitejs/vite-plugin-vue/tree/main/packages/plugin-vue#example-for-passing-options-to-vuecompiler-sfc": {
    bench: 1,
    shipped: 0,
    reason: "Vue (warning text): documentation link; never requested",
  },
  "https://link.vuejs.org/feature-flags.": { bench: 1, shipped: 0, reason: "Vue: documentation link inside a warning message; never requested" },
  "https://pinia.vuejs.org/core-concepts/outside-component-usage.html": { bench: 1, shipped: 0, reason: "Pinia: documentation link inside a warning message; never requested" },
  "http://": { bench: 1, shipped: 1, reason: "prefix test `startsWith('http://')` (link classification); never requested" },
  "https://": { bench: 1, shipped: 1, reason: "prefix test `startsWith('https://')` (link classification); never requested" },
  "http://$": {
    bench: 4,
    shipped: 2,
    reason:
      "template prefix `http://${url}` for a bare `www.` link: markdown-it linkify (2: the shipped build) and pdf.js (2 more in the bench build: the pdf chunk and the worker); rendered as an <a>, never requested",
  },
};

const URL_RE = /(?:https?|wss?):\/\/[A-Za-z0-9._~:/?#@!$&*+=%-]*/g;

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const full = join(dir, name);
    return statSync(full).isDirectory() ? walk(full) : [full];
  });
}

function isAccounted(url: string): boolean {
  const host = url.replace(/^[a-z]+:\/\//, "").split(/[/:?#]/)[0];
  if (NAMESPACE_HOSTS.includes(host)) return true;
  return SCHEME_ORIGINS.some((origin) => url === origin || url.startsWith(`${origin}/`));
}

/** Unaccounted URLs ("file: url"), and the occurrence count of each INERT string. */
export function scan(dir: string): { unaccounted: string[]; counts: Record<string, number> } {
  const unaccounted = new Set<string>();
  const counts: Record<string, number> = Object.fromEntries(Object.keys(INERT).map((url) => [url, 0]));
  for (const file of walk(dir)) {
    if (!/\.(js|mjs|css|html)$/.test(file)) continue;
    for (const url of readFileSync(file, "utf8").match(URL_RE) ?? []) {
      if (isAccounted(url)) continue;
      if (url in INERT) counts[url] += 1;
      else unaccounted.add(`${relative(dir, file)}: ${url}`);
    }
  }
  return { unaccounted: [...unaccounted].sort(), counts };
}

describe("the shipped webview contains no URL it could request", () => {
  let benchDir = "";
  beforeAll(async () => {
    // The bench build is the full feature set (it adds only a test hook), including the lazily
    // loaded pdf.js chunk and worker, so it is the strictest thing to scan. It goes to a temporary
    // directory: app/dist-bench belongs to the editor-latency harness, and emptyOutDir would wipe
    // a concurrent run's build.
    benchDir = mkdtempSync(join(tmpdir(), "serat-no-network-"));
    await build({ root: APP, mode: "bench", logLevel: "silent", build: { outDir: benchDir, emptyOutDir: true } });
  }, 240_000);
  afterAll(() => {
    if (benchDir) rmSync(benchDir, { recursive: true, force: true });
  });

  it("a fresh bench build has no http(s)/ws(s) URL beyond namespaces, the scheme origin and the inert table", () => {
    expect(walk(benchDir).length).toBeGreaterThan(0);
    const { unaccounted, counts } = scan(benchDir);
    expect(unaccounted).toEqual([]);
    expect(counts).toEqual(Object.fromEntries(Object.entries(INERT).map(([url, e]) => [url, e.bench])));
  });

  it("the committed crates/serat-app/webview build is held to the same rule", () => {
    const { unaccounted, counts } = scan(SHIPPED);
    expect(unaccounted).toEqual([]);
    expect(counts).toEqual(Object.fromEntries(Object.entries(INERT).map(([url, e]) => [url, e.shipped])));
  });

  it("no INERT entry is stale (each occurs somewhere)", () => {
    for (const [url, entry] of Object.entries(INERT)) {
      expect(entry.bench + entry.shipped, url).toBeGreaterThan(0);
    }
  });

  it("the scanner fails on a URL someone adds (it is not vacuous)", () => {
    const hit = ["const x = fetch('https://telemetry.example.org/ping');", "const y = new WebSocket('wss://evil.example/s');"].join("\n");
    const found = hit.match(URL_RE) ?? [];
    expect(found).toEqual(["https://telemetry.example.org/ping", "wss://evil.example/s"]);
    expect(found.every((u) => !isAccounted(u) && !(u in INERT))).toBe(true);
  });
});

// ---- our own source names no network API -----------------------------------------------------
// The bundle scan cannot do this: pdf.js and Vue legitimately contain `fetch`. Our source has none,
// and there is no allow-list: the first use has to come with a decision to add one.
const NETWORK_API = /\bfetch\s*\(|XMLHttpRequest|new\s+WebSocket|\bEventSource\b|sendBeacon|importScripts/;

export function networkApiUses(files: Array<{ name: string; text: string }>): string[] {
  const out: string[] = [];
  for (const { name, text } of files) {
    text.split("\n").forEach((line, index) => {
      const code = line.trim();
      if (code.startsWith("//") || code.startsWith("*") || code.startsWith("/*")) return;
      if (NETWORK_API.test(code)) out.push(`${name}:${index + 1}: ${code}`);
    });
  }
  return out;
}

describe("app/src names no network API", () => {
  it("no .ts or .vue file (tests excluded) uses fetch, XMLHttpRequest, WebSocket, EventSource, sendBeacon or importScripts", () => {
    const files = walk(join(APP, "src"))
      .filter((file) => /\.(ts|vue)$/.test(file) && !/\.test\.ts$/.test(file))
      .map((file) => ({ name: relative(APP, file), text: readFileSync(file, "utf8") }));
    expect(files.length).toBeGreaterThan(20);
    expect(networkApiUses(files)).toEqual([]);
  });

  it("the scan fails on each thing it names (it is not vacuous)", () => {
    for (const code of [
      "const r = await fetch(url);",
      "const x = new XMLHttpRequest();",
      "const s = new WebSocket('ws://x');",
      "const e = new EventSource(u);",
      "navigator.sendBeacon(u, d);",
      "importScripts('x.js');",
    ]) {
      expect(networkApiUses([{ name: "planted.ts", text: code }]), code).toHaveLength(1);
    }
    expect(networkApiUses([{ name: "planted.ts", text: "// fetch(url) is not used" }])).toEqual([]);
  });
});

// ---- pdf.js has nothing to fetch ---------------------------------------------------------------
const pdfjs = vi.hoisted(() => ({
  GlobalWorkerOptions: {} as { workerPort?: unknown },
  PDFDataRangeTransport: class {
    constructor(
      public length: number,
      public initialData: Uint8Array | null,
    ) {}
    onDataRange(): void {}
    requestDataRange(): void {}
  },
  getDocument: vi.fn(() => ({ promise: Promise.resolve({ numPages: 1 }), destroy: vi.fn(async () => {}) })),
}));
vi.mock("pdfjs-dist", () => pdfjs);

/** Relative: no scheme and not protocol-relative. */
const isRelativeUrl = (value: unknown): boolean => typeof value === "string" && !/^([a-z][a-z0-9+.-]*:|\/\/)/i.test(value);

describe("pdf.js is configured with nothing to fetch", () => {
  it("getDocument is given no url/httpHeaders/withCredentials, and any factory URL it is given is relative", async () => {
    vi.stubGlobal("Worker", class {});
    const { loadPdf } = await import("../viewers/pdfEngine");
    await loadPdf("d/x.pdf", 10, { onChanged: () => {}, onFailed: () => {} });
    const args = (pdfjs.getDocument.mock.calls[0] as unknown[])[0] as Record<string, unknown>;
    for (const key of ["url", "httpHeaders", "withCredentials"]) expect(args[key], key).toBeUndefined();
    // ADR-0017 Amendment 1 item 6 may require cMapUrl / standardFontDataUrl, served from the
    // bundle: if present they must be relative (same origin), never a scheme or `//host`.
    for (const key of ["cMapUrl", "standardFontDataUrl", "wasmUrl", "iccUrl"]) {
      if (args[key] !== undefined && args[key] !== null) expect(isRelativeUrl(args[key]), `${key}: ${String(args[key])}`).toBe(true);
    }
    expect(args.useWasm).toBe(false);
    vi.unstubAllGlobals();
  });

  it("the relative-URL rule rejects what it should", () => {
    for (const bad of ["https://cdn.example/cmaps/", "http://x/", "//cdn.example/f/", "data:,x", "blob:x"]) expect(isRelativeUrl(bad), bad).toBe(false);
    for (const good of ["./cmaps/", "cmaps/", "/assets/font/"]) expect(isRelativeUrl(good), good).toBe(true);
  });

  it("and pdf.js's own defaults for those options are null; the version these facts were read from is pinned", () => {
    const dir = join(APP, "node_modules", "pdfjs-dist");
    // The defaults below were read from this version. A different one must be re-read, not assumed.
    expect(JSON.parse(readFileSync(join(dir, "package.json"), "utf8")).version).toBe("6.5.438");
    const source = readFileSync(join(dir, "build", "pdf.mjs"), "utf8");
    expect(source).toMatch(/cMapUrl = null,\s*standardFontDataUrl = null,\s*wasmUrl = null/);
    // iccUrl has no default of its own: getFactoryUrlProp(undefined) is null.
    expect(source).toMatch(/const iccUrl = getFactoryUrlProp\(src\.iccUrl\)/);
    expect(source).toMatch(/function getFactoryUrlProp\(val\) \{\s*if \(typeof val !== "string"\) \{\s*return null;/);
  });
});
