// Task #104 (NFR-USE-01, FR-EDT-01; editor.md Live-preview typography, acceptance 1): every `--serat-*`
// custom property the app reads is defined in styles/tokens.css, and tokens.css agrees with
// docs/03-design/ui/tokens.json: every colour ramp and role, and every type-scale step, resolves to the same value.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const SRC = resolve(__dirname, "..");
const CSS = readFileSync(join(__dirname, "tokens.css"), "utf8");
const JSON_TEXT = readFileSync(resolve(SRC, "../../docs/03-design/ui/tokens.json"), "utf8");
const TOKENS = JSON.parse(JSON_TEXT);

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return name === "generated" || name === "build" ? [] : sourceFiles(path);
    return /\.(ts|vue|css)$/.test(name) && !/\.test\.ts$/.test(name) ? [path] : [];
  });
}

/** Set per line by a class (`serat-depth-N`, livePreviewSupplement.css), read with a fallback: not a token. */
const PER_LINE = "--serat-depth";

const defs = new Map<string, string>(
  [...CSS.matchAll(/^\s*(--serat-[a-z0-9-]+)\s*:\s*([^;]+);/gm)].map((m) => [m[1], m[2].trim()]),
);

/** A custom property's value with `var()` references followed to the end. */
function resolved(name: string, depth = 0): string {
  const raw = defs.get(name);
  if (raw === undefined || depth > 8) throw new Error(`undefined ${name}`);
  const ref = /^var\((--serat-[a-z0-9-]+)\)$/.exec(raw);
  return ref ? resolved(ref[1], depth + 1) : raw;
}

/** tokens.json `{a.b.c}` references followed to a literal. */
function literal(value: string): string {
  const ref = /^\{(.+)\}$/.exec(value);
  if (!ref) return value;
  const path = ref[1].split(".");
  const walk = (root: any) => path.reduce((o: any, k) => o?.[k], root);
  return literal((walk(TOKENS.color) ?? walk(TOKENS)) as string); // colour refs are relative to `color`
}

const norm = (v: string) => v.trim().toLowerCase();

/** tokens.json role `bg-X` is `--serat-surface-X` (`bg-surface` is `--serat-surface`, `bg-surface-X` is `surface-X`). */
function roleProperty(role: string): string {
  if (role === "bg-surface") return "--serat-surface";
  if (role.startsWith("bg-surface-")) return `--serat-${role.slice(3)}`;
  if (role.startsWith("bg-")) return `--serat-surface-${role.slice(3)}`;
  return `--serat-${role}`;
}

describe("token coverage", () => {
  it("every var(--serat-*) read under src/ has a definition in styles/tokens.css", () => {
    const missing: string[] = [];
    for (const file of sourceFiles(SRC)) {
      if (file === join(__dirname, "tokens.css")) continue;
      const text = readFileSync(file, "utf8");
      for (const m of text.matchAll(/var\((--serat-[a-z0-9-]+)/g)) {
        if (!defs.has(m[1]) && m[1] !== PER_LINE) missing.push(`${file.slice(SRC.length + 1)}: ${m[1]}`);
      }
    }
    expect(missing).toEqual([]);
  });

  it("the P0 shell variables are aliases of the brand tokens, not copies", () => {
    const style = readFileSync(join(SRC, "style.css"), "utf8");
    for (const name of ["indigo", "brass", "ink", "paper"]) {
      expect(style).toMatch(new RegExp(`--${name}:\\s*var\\(--serat-brand-${name}\\)`));
    }
  });
});

describe("tokens.css agrees with tokens.json", () => {
  it("every colour in color.brand and the ramps", () => {
    let checked = 0;
    for (const [group, entries] of Object.entries(TOKENS.color as Record<string, Record<string, string>>)) {
      if (group === "role") continue;
      for (const [key, value] of Object.entries(entries)) {
        if (key.startsWith("_")) continue;
        const prop = group === "brand" ? `--serat-brand-${key}` : `--serat-${group}-${key}`;
        expect(norm(resolved(prop)), prop).toBe(norm(literal(value)));
        checked++;
      }
    }
    expect(checked).toBeGreaterThan(20);
  });

  it("every colour role", () => {
    let checked = 0;
    for (const [role, value] of Object.entries(TOKENS.color.role as Record<string, string>)) {
      if (role.startsWith("_")) continue;
      expect(norm(resolved(roleProperty(role))), role).toBe(norm(literal(value)));
      checked++;
    }
    expect(checked).toBeGreaterThan(25);
  });

  it("every type-scale step: size, line height, tracking and weight", () => {
    for (const [step, spec] of Object.entries(TOKENS.typography.scale as Record<string, any>)) {
      expect(norm(resolved(`--serat-type-${step}-size`)), step).toBe(norm(spec.size));
      expect(norm(resolved(`--serat-type-${step}-line`)), step).toBe(norm(spec.lineHeight));
      expect(norm(resolved(`--serat-type-${step}-tracking`)), step).toBe(norm(spec.tracking));
      expect(Number(resolved(`--serat-type-${step}-weight`)), step).toBe(spec.weight ?? 400);
    }
  });

  it("the type families, measure and live-preview values", () => {
    for (const [k, v] of Object.entries(TOKENS.typography.families as Record<string, string>)) {
      expect(norm(resolved(`--serat-font-${k}`)).replace(/["']/g, ""), k).toBe(norm(v).replace(/["']/g, ""));
    }
    expect(resolved("--serat-measure-editor")).toBe(TOKENS.typography.measure.editor);
    expect(resolved("--serat-lp-link-underline-alpha")).toBe(TOKENS["live-preview"]["link-underline-alpha"]);
  });
});
