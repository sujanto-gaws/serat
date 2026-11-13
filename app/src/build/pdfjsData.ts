// Ships pdf.js's CMaps and standard fonts inside the bundle (FR-BIN-11, US-19, NFR-SEC-01; ADR-0017
// Amendment 1 item 6; binary-viewers.md, PDF state row). Without them a PDF that names a predefined CJK
// CMap or one of the 14 standard fonts without embedding it renders blank or wrong. The viewer asks for
// them at `./cmaps/` and `./standard_fonts/`, the same origin as the app: fixed names, no content hash,
// no network (the app CSP's `connect-src` covers 'self').
//
// This is a Vite plugin rather than a dependency: it reads `pdfjs-dist/cmaps` and
// `pdfjs-dist/standard_fonts` from node_modules at build time and emits each file under its own name.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

import type { Plugin } from "vite";

/** `[bundle directory, directory inside pdfjs-dist]`. */
export const PDFJS_DATA_DIRS = [
  ["cmaps", "cmaps"],
  ["standard_fonts", "standard_fonts"],
] as const;

function pdfjsRoot(): string {
  return dirname(createRequire(import.meta.url).resolve("pdfjs-dist/package.json"));
}

/** Every file to ship, as `{ fileName inside the bundle, absolute source path }`. */
export function pdfjsDataFiles(root: string = pdfjsRoot()): Array<{ fileName: string; source: string }> {
  const out: Array<{ fileName: string; source: string }> = [];
  for (const [bundleDir, packageDir] of PDFJS_DATA_DIRS) {
    const dir = join(root, packageDir);
    for (const name of readdirSync(dir).sort()) {
      const source = join(dir, name);
      if (statSync(source).isFile()) out.push({ fileName: `${bundleDir}/${name}`, source });
    }
  }
  return out;
}

export function pdfjsDataPlugin(): Plugin {
  return {
    name: "serat-pdfjs-data",
    // Build: emit each file under its exact name (emitFile with `fileName` is never hashed).
    generateBundle() {
      for (const { fileName, source } of pdfjsDataFiles()) {
        this.emitFile({ type: "asset", fileName, source: readFileSync(source) });
      }
    },
    // Dev server: serve the same paths so `./cmaps/...` resolves there too.
    configureServer(server) {
      const files = new Map(pdfjsDataFiles().map((f) => [`/${f.fileName}`, f.source]));
      server.middlewares.use((req, res, next) => {
        const source = files.get((req.url ?? "").split("?")[0]);
        if (!source) return next();
        res.setHeader("content-type", "application/octet-stream");
        res.end(readFileSync(source));
      });
    },
  };
}
