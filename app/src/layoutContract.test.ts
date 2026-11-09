// NFR-PRF-02 / FR-EDT-01 / US-35: layout contract that keeps the editor's height bounded.
// jsdom does no layout, so this asserts the rules that guarantee it, in both halves: the CSS that
// bounds the column, and the CodeMirror extensions that make the editor fill it and own the scroll.
// Regression: body was a grid whose auto row sized #app to its content, so a 5,000-line note became
// a ~110,000 px page, CodeMirror had no scroller, and keystroke paints stalled for seconds.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import { EDITOR_SCROLL_MARGINS, editorScrollExtensions } from "@/editor/codemirrorSetup";

const css = readFileSync(resolve(__dirname, "style.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const vueSource = readFileSync(resolve(__dirname, "editor/CodeMirrorEditor.vue"), "utf8");

function declarations(selector: string): string {
  const out: string[] = [];
  for (const m of css.matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    if (m[1].split(",").some((s) => s.trim() === selector)) out.push(m[2]);
  }
  return out.join(";");
}

describe("editor height-bounding contract: CSS half (NFR-PRF-02)", () => {
  it("body is not a grid, so #app cannot be sized to its content", () => {
    expect(declarations("body")).not.toMatch(/display\s*:\s*grid/);
  });
  it("#app's middle row is minmax(0, 1fr)", () => {
    expect(declarations("#app")).toMatch(/grid-template-rows\s*:\s*auto\s+minmax\(0,\s*1fr\)\s+auto/);
  });
  it("the editor column may shrink and scrolls (never silently clips)", () => {
    const d = declarations(".editor-column");
    expect(d).toMatch(/min-height\s*:\s*0/);
    expect(d).toMatch(/overflow\s*:\s*auto/);
    expect(d).not.toMatch(/overflow\s*:\s*hidden/);
  });
});

describe("editor height-bounding contract: CodeMirror half (NFR-PRF-02)", () => {
  let view: EditorView | undefined;
  afterEach(() => view?.destroy());

  it("the component installs editorScrollExtensions()", () => {
    expect(vueSource).toContain("...editorScrollExtensions()");
  });

  it("carries the scroll margins 24 / 48", () => {
    view = new EditorView({ state: EditorState.create({ doc: "x", extensions: editorScrollExtensions() }), parent: document.body });
    expect(EDITOR_SCROLL_MARGINS).toEqual({ top: 24, bottom: 48 });
    expect(view.state.facet(EditorView.scrollMargins).map((f) => f(view as EditorView))).toEqual([{ top: 24, bottom: 48 }]);
  });

  it("fills its column (height 100%)", () => {
    view = new EditorView({ state: EditorState.create({ doc: "x", extensions: editorScrollExtensions() }), parent: document.body });
    expect(getComputedStyle(view.dom).height).toBe("100%");
  });

  it("includes the scrollPastEnd plugin (a view without it has fewer extensions)", () => {
    expect(editorScrollExtensions()).toHaveLength(3);
  });
});
