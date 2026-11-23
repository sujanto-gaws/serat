// Task #104 (NFR-USE-02 habit, editor.md acceptance 10): WCAG 2.x contrast of the editor's text and non-text
// colours, computed from the token values in tokens.css rather than asserted by eye.

import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const CSS = readFileSync(join(__dirname, "tokens.css"), "utf8");
const defs = new Map([...CSS.matchAll(/^\s*(--serat-[a-z0-9-]+)\s*:\s*([^;]+);/gm)].map((m) => [m[1], m[2].trim()]));
const hex = (name: string): string => {
  const v = defs.get(name)!;
  const ref = /^var\((--serat-[a-z0-9-]+)\)$/.exec(v);
  return ref ? hex(ref[1]) : v;
};
const rgb = (h: string) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
const lum = (h: string) => {
  const [r, g, b] = rgb(h).map((c) => {
    const x = c / 255;
    return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
const ratio = (a: string, b: string) => {
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};
const over = (fg: string, bg: string, alpha: number) =>
  "#" + rgb(fg).map((c, i) => Math.round(c * alpha + rgb(bg)[i] * (1 - alpha)).toString(16).padStart(2, "0")).join("");

describe("editor text contrast (>= 4.5:1)", () => {
  const pairs: Array<[string, string]> = [
    ["--serat-text-primary", "--serat-surface"],
    ["--serat-text-primary", "--serat-surface-selection"],
    ["--serat-text-primary", "--serat-surface-highlight"],
    ["--serat-text-secondary", "--serat-surface"],
    ["--serat-text-secondary", "--serat-surface-sunken"],
    ["--serat-text-marker", "--serat-surface"],
    ["--serat-text-marker", "--serat-surface-sunken"],
    ["--serat-text-link", "--serat-surface"],
    ["--serat-unresolved-link", "--serat-surface"],
  ];
  for (const [fg, bg] of pairs) {
    it(`${fg} on ${bg}`, () => {
      expect(ratio(hex(fg), hex(bg))).toBeGreaterThanOrEqual(4.5);
    });
  }

  it("text-tertiary carries no essential text in the editor stylesheets", () => {
    const dir = resolve(__dirname, "../editor");
    for (const file of ["livePreview.css", "livePreviewSupplement.css"]) {
      expect(readFileSync(join(dir, file), "utf8"), file).not.toMatch(/color:\s*var\(--serat-text-tertiary\)/);
    }
  });
});

describe("non-text contrast (>= 3:1), measured on surface", () => {
  it("the quote rule", () => {
    expect(ratio(hex("--serat-rule-quote"), hex("--serat-surface"))).toBeGreaterThanOrEqual(3);
  });
  it("the link underline at the specified alpha", () => {
    const alpha = parseInt(defs.get("--serat-lp-link-underline-alpha")!, 10) / 100;
    const line = over(hex("--serat-text-link"), hex("--serat-surface"), alpha);
    expect(ratio(line, hex("--serat-surface"))).toBeGreaterThanOrEqual(3);
  });
});
