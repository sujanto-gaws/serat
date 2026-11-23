// Also ships pdfjs-dist's LICENSE (Apache-2.0) as assets/LICENSE_pdfjs.
// Ships the SIL OFL 1.1 licence text of each bundled font package beside the fonts (OFL-1.1 section 2: the
// licence travels with every redistributed copy). Emits node_modules/@fontsource-variable/<pkg>/LICENSE as
// assets/LICENSE_<pkg>, the way pdfjsData.ts ships LICENSE_FOXIT / LICENSE_LIBERATION (task #162, NFR-SEC-07).

import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

import type { Plugin } from "vite";

export const FONT_PACKAGES = ["source-serif-4", "bricolage-grotesque", "jetbrains-mono"] as const;

export function fontLicenseFiles(): Array<{ fileName: string; source: string }> {
  const require = createRequire(import.meta.url);
  const fonts = FONT_PACKAGES.map((name) => ({
    fileName: `assets/LICENSE_${name}`,
    source: join(dirname(require.resolve(`@fontsource-variable/${name}/package.json`)), "LICENSE"),
  }));
  // pdf.js's Apache-2.0 text (section 4(a): recipients get a copy of the licence), beside its worker (task #149).
  const pdfjs = {
    fileName: "assets/LICENSE_pdfjs",
    source: join(dirname(require.resolve("pdfjs-dist/package.json")), "LICENSE"),
  };
  return [...fonts, pdfjs];
}

export function fontLicensesPlugin(): Plugin {
  return {
    name: "serat-font-licenses",
    generateBundle() {
      for (const { fileName, source } of fontLicenseFiles()) {
        this.emitFile({ type: "asset", fileName, source: readFileSync(source) });
      }
    },
  };
}
