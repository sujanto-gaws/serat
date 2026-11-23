// Task #162 (FR-EDT-02, NFR-SEC-01, NFR-SEC-07, NFR-USE-01; editor.md Live-preview typography, acceptance 7):
// the three OFL families are bundled, each @font-face tries local() first and then a bundled woff2 reached
// through Vite, nothing points off-origin, and the licence text of each font package is on disk.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const CSS = readFileSync(join(__dirname, "fonts.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const APP = resolve(__dirname, "..", "..");

interface Face {
  family: string;
  src: string;
}
const faces: Face[] = [...CSS.matchAll(/@font-face\s*\{([^}]*)\}/g)].map((m) => ({
  family: /font-family:\s*"([^"]+)"/.exec(m[1])![1],
  src: /src:\s*([^;]+);/.exec(m[1])![1],
}));

const PACKAGES: Record<string, string> = {
  "Source Serif 4": "source-serif-4",
  "Bricolage Grotesque": "bricolage-grotesque",
  "JetBrains Mono": "jetbrains-mono",
};

describe("bundled fonts (NFR-SEC-01)", () => {
  it("declares all three families", () => {
    expect(new Set(faces.map((f) => f.family))).toEqual(new Set(Object.keys(PACKAGES)));
  });

  it.each(faces)("$family: local() first, then one bundled woff2 url", ({ family, src }) => {
    expect(src.trimStart().startsWith("local(")).toBe(true);
    const urls = [...src.matchAll(/url\("([^"]+)"\)\s*format\("woff2"\)/g)].map((m) => m[1]);
    expect(urls).toHaveLength(1);
    expect(src.match(/url\(/g)).toHaveLength(1);
    // A package path (resolved by Vite, hashed and emitted under 'self'), never a scheme or a host.
    expect(urls[0]).toMatch(new RegExp(`^@fontsource-variable/${PACKAGES[family]}/files/[a-z0-9-]+-latin-wght-(normal|italic)\\.woff2$`));
    expect(existsSync(join(APP, "node_modules", urls[0]))).toBe(true);
  });

  it("has no remote source and no @import", () => {
    expect(CSS).not.toMatch(/https?:|\/\/|@import/);
  });

  it.each(Object.values(PACKAGES))("%s ships its OFL licence text", (name) => {
    const dir = join(APP, "node_modules", "@fontsource-variable", name);
    expect(JSON.parse(readFileSync(join(dir, "package.json"), "utf8")).license).toBe("OFL-1.1");
    expect(readFileSync(join(dir, "LICENSE"), "utf8")).toMatch(/SIL OPEN FONT LICENSE/i);
    expect(dirname(join(dir, "LICENSE"))).toBe(dir);
  });

  it("the committed bundle carries the four woff2 files", () => {
    const assets = resolve(APP, "..", "crates", "serat-app", "webview", "assets");
    for (const f of [
      "source-serif-4-latin-wght-normal.woff2",
      "source-serif-4-latin-wght-italic.woff2",
      "bricolage-grotesque-latin-wght-normal.woff2",
      "jetbrains-mono-latin-wght-normal.woff2",
    ]) {
      expect(existsSync(join(assets, f)), f).toBe(true);
    }
  });
});
