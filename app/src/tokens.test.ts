// Task #104 (NFR-USE-01, FR-EDT-01): every `--serat-*` custom property the app reads is defined in
// src/tokens.css, and every colour defined there is one docs/03-design/ui/tokens.json names. Before this,
// 18 variables were referenced under src/ and none was defined, so every `var()` silently took its
// hard-coded fallback and the design tokens styled nothing.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const SRC = resolve(__dirname);
const TOKENS_CSS = readFileSync(join(SRC, "tokens.css"), "utf8");
const TOKENS_JSON = readFileSync(resolve(SRC, "../../docs/03-design/ui/tokens.json"), "utf8");

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return name === "generated" || name === "build" ? [] : sourceFiles(path);
    return /\.(ts|vue|css)$/.test(name) && !/\.test\.ts$/.test(name) ? [path] : [];
  });
}

const defined = new Set([...TOKENS_CSS.matchAll(/^\s*(--serat-[a-z0-9-]+)\s*:/gm)].map((m) => m[1]));

describe("token coverage", () => {
  it("defines tokens at all", () => {
    expect(defined.size).toBeGreaterThan(40);
  });

  it("every var(--serat-*) read under src/ has a definition in tokens.css", () => {
    const missing: string[] = [];
    for (const file of sourceFiles(SRC)) {
      if (file === join(SRC, "tokens.css")) continue;
      const text = readFileSync(file, "utf8");
      // `var(--serat-x` and the `var(--serat-${size}...)` template form in typography.ts are both checked:
      // the template is expanded over the sizes the scale defines.
      for (const m of text.matchAll(/var\((--serat-[a-z0-9-]+)/g)) {
        if (!m[1].endsWith("-") && !defined.has(m[1])) missing.push(`${file.slice(SRC.length + 1)}: ${m[1]}`);
      }
      for (const m of text.matchAll(/var\(--serat-(size|line)-\$\{size\}/g)) {
        for (const size of ["heading-sm", "heading-md", "heading-lg"]) {
          if (!defined.has(`--serat-${m[1]}-${size}`)) missing.push(`${file.slice(SRC.length + 1)}: --serat-${m[1]}-${size}`);
        }
      }
    }
    expect(missing).toEqual([]);
  });

  it("every colour in tokens.css is a hex tokens.json names", () => {
    const named = new Set([...TOKENS_JSON.matchAll(/#[0-9a-fA-F]{6}\b/g)].map((m) => m[0].toLowerCase()));
    const used = [...TOKENS_CSS.matchAll(/:\s*(#[0-9a-fA-F]{6})\s*;/g)].map((m) => m[1].toLowerCase());
    expect(used.length).toBeGreaterThan(20);
    expect(used.filter((hex) => !named.has(hex))).toEqual([]);
  });

  it("the type scale in tokens.css matches typography.scale in tokens.json", () => {
    const scale = JSON.parse(TOKENS_JSON).typography.scale as Record<string, { size: string; lineHeight: string }>;
    const css = (name: string) => TOKENS_CSS.match(new RegExp(`${name}\\s*:\\s*([^;]+);`))?.[1].trim();
    for (const step of ["mono-source", "body-serif", "heading-sm", "heading-md", "heading-lg", "small", "micro"]) {
      expect(css(`--serat-size-${step}`), step).toBe(scale[step].size);
      expect(css(`--serat-line-${step}`), step).toBe(scale[step].lineHeight);
    }
  });
});
