// @vitest-environment node
//
// NFR-SEC-01 / NFR-SEC-06 / T-04 / US-07: the webview side of "the app makes no network request the
// user did not directly cause". Method and limits: docs/05-testing/no-network-evidence.md.
//
// This proves the ABSENCE OF CODE PATHS in what we ship (no URL a script could fetch, pdf.js
// configured so it has nothing to fetch). It does not observe a running webview; that is the
// reference-machine protocol's job (docs/05-testing/p1-gate-protocol.md).
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import { build } from "vite";
import { beforeAll, describe, expect, it, vi } from "vitest";

const APP = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const BENCH_DIST = join(APP, "dist-bench");
const SHIPPED = join(APP, "..", "crates", "serat-app", "webview");

/** Hosts that are XML / schema namespace identifiers: strings compared, never requested. */
const NAMESPACE_HOSTS = ["www.w3.org", "www.xfa.org", "ns.adobe.com"];

/** The two ADR-0017 scheme origins (the scheme itself and Windows/Android's http form of it). */
const SCHEME_ORIGINS = ["http://serat-asset.localhost"];

/**
 * Every other URL-shaped string the current bundles contain, each with the reason it is inert.
 * An entry is an exact match on the extracted text; a NEW string fails the test until someone
 * reads it and adds it here on purpose.
 */
const INERT: Record<string, string> = {
  "https://foo.bar": "pdf.js: base for `new URL(relative, base)` when parsing a link; never requested",
  "http://example.com": "pdf.js: base for isValidFetchUrl(); never requested",
  "https://vuejs.org/error-reference/#runtime-$": "Vue: text appended to a console warning; never requested",
  "https://cli.vuejs.org/guide/webpack.html#modifying-options-of-a-loader": "Vue: documentation link inside a warning message; never requested",
  "https://github.com/vitejs/vite-plugin-vue/tree/main/packages/plugin-vue#example-for-passing-options-to-vuecompiler-sfc":
    "Vue (dev-only warning text): documentation link; never requested",
  "https://link.vuejs.org/feature-flags.": "Vue: documentation link inside a warning message; never requested",
  "https://pinia.vuejs.org/core-concepts/outside-component-usage.html": "Pinia: documentation link inside a warning message; never requested",
  "http://": "prefix test `startsWith('http://')` (link classification); never requested",
  "https://": "prefix test `startsWith('https://')` (link classification); never requested",
  "http://$": "markdown-it linkify: template prefix `http://${url}` for a link the user typed; rendered as an <a>, never requested",
  "http://serat-asset.localhost/$": "the ADR-0017 scheme origin in a template literal",
};

const URL_RE = /(?:https?|wss?):\/\/[A-Za-z0-9._~:/?#@!$&*+=%-]*/g;

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const full = join(dir, name);
    return statSync(full).isDirectory() ? walk(full) : [full];
  });
}

/** Every URL-shaped string in `dir`'s JS/CSS/HTML that is not accounted for. */
export function unaccountedUrls(dir: string): string[] {
  const out: string[] = [];
  for (const file of walk(dir)) {
    if (!/\.(js|mjs|css|html)$/.test(file)) continue;
    for (const url of readFileSync(file, "utf8").match(URL_RE) ?? []) {
      const host = url.replace(/^[a-z]+:\/\//, "").split(/[/:?#]/)[0];
      if (NAMESPACE_HOSTS.includes(host)) continue;
      if (SCHEME_ORIGINS.some((origin) => url === origin || url.startsWith(`${origin}/`))) continue;
      if (url in INERT) continue;
      out.push(`${relative(dir, file)}: ${url}`);
    }
  }
  return [...new Set(out)].sort();
}

describe("the shipped webview contains no URL it could request", () => {
  beforeAll(async () => {
    // The bench build is the full feature set (it adds only a test hook), including the lazily
    // loaded pdf.js chunk and worker, so it is the strictest thing to scan.
    await build({ root: APP, mode: "bench", logLevel: "silent" });
  }, 240_000);

  it("app/dist-bench has no http(s)/ws(s) URL beyond namespaces, the scheme origin and the inert list", () => {
    expect(walk(BENCH_DIST).length).toBeGreaterThan(0);
    expect(unaccountedUrls(BENCH_DIST)).toEqual([]);
  });

  it("the committed crates/serat-app/webview build is held to the same rule", () => {
    expect(unaccountedUrls(SHIPPED)).toEqual([]);
  });

  it("the scanner fails on a URL someone adds (it is not vacuous)", () => {
    const hit = [
      "const x = fetch('https://telemetry.example.org/ping');",
      "const y = new WebSocket('wss://evil.example/s');",
    ].join("\n");
    const found = hit.match(URL_RE) ?? [];
    expect(found).toEqual(["https://telemetry.example.org/ping", "wss://evil.example/s"]);
    expect(found.every((u) => !(u in INERT))).toBe(true);
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

describe("pdf.js is configured with nothing to fetch", () => {
  it("getDocument is given no url, cMapUrl, standardFontDataUrl, wasmUrl or iccUrl", async () => {
    vi.stubGlobal("Worker", class {});
    const { loadPdf } = await import("../viewers/pdfEngine");
    await loadPdf("d/x.pdf", 10, { onChanged: () => {}, onFailed: () => {} });
    const args = (pdfjs.getDocument.mock.calls[0] as unknown[])[0] as Record<string, unknown>;
    for (const key of ["url", "cMapUrl", "standardFontDataUrl", "wasmUrl", "iccUrl", "httpHeaders", "withCredentials"]) {
      expect(args[key], key).toBeUndefined();
    }
    expect(args.useWasm).toBe(false);
    vi.unstubAllGlobals();
  });

  it("and pdf.js's own defaults for those options are null (an upgrade that changes them fails here)", () => {
    const source = readFileSync(join(APP, "node_modules", "pdfjs-dist", "build", "pdf.mjs"), "utf8");
    expect(source).toMatch(/cMapUrl = null,\s*standardFontDataUrl = null,\s*wasmUrl = null/);
    // iccUrl has no default of its own: getFactoryUrlProp(undefined) is null.
    expect(source).toMatch(/const iccUrl = getFactoryUrlProp\(src\.iccUrl\)/);
    expect(source).toMatch(/function getFactoryUrlProp\(val\) \{\s*if \(typeof val !== "string"\) \{\s*return null;/);
  });
});
