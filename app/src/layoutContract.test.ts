// NFR-PRF-02 / FR-EDT-01 / US-05: layout contract that keeps the editor's height bounded.
// jsdom does no layout, so this asserts the CSS rules that guarantee it. Regression: body was a
// grid whose auto row sized #app to its content, so a 5,000-line note became a ~110,000 px page,
// CodeMirror had no scroller, and keystroke paints stalled for seconds.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const css = readFileSync(resolve(__dirname, "style.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

function declarations(selector: string): string {
  const out: string[] = [];
  for (const m of css.matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    if (m[1].split(",").some((s) => s.trim() === selector)) out.push(m[2]);
  }
  return out.join(";");
}

describe("editor height-bounding contract (NFR-PRF-02)", () => {
  it("body is not a grid, so #app cannot be sized to its content", () => {
    expect(declarations("body")).not.toMatch(/display\s*:\s*grid/);
  });
  it("#app is viewport-height and its middle row is minmax(0, 1fr)", () => {
    expect(declarations("#app")).toMatch(/grid-template-rows\s*:\s*auto\s+minmax\(0,\s*1fr\)\s+auto/);
    expect(declarations("html")).toMatch(/height\s*:\s*100%/);
  });
  it("the editor column owns overflow and may shrink", () => {
    const d = declarations(".editor-column");
    expect(d).toMatch(/overflow\s*:\s*hidden/);
    expect(d).toMatch(/min-height\s*:\s*0/);
  });
});
