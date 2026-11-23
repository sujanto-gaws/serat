// Task #104 (FR-EDT-01, FR-EDT-02, FR-EDT-10): the hybrid live preview is styled. Highlight tags reach
// the DOM as `serat-tok-*` classes, revealed and code lines take `serat-mono-source`, and the mode class
// follows `livePreview`. These are structural checks (class names, not computed pixels: jsdom has no
// layout); the values behind the classes are pinned by tokens.test.ts and the theme below.

import { mount } from "@vue/test-utils";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { forceParsing, syntaxTree } from "@codemirror/language";
import { highlightingFor } from "@codemirror/language";
import { describe, expect, it, vi } from "vitest";

import CodeMirrorEditor from "@/editor/CodeMirrorEditor.vue";
import { seratMarkdown } from "@/editor/markdownLanguage";
import { frontmatterEndLine, revealMarkersPlugin, scanReveal, selectedLineNumbers } from "@/editor/reveal";
import { seratTags } from "@/editor/mdExtensions";
import { seratHighlightStyle, typographyExtensions } from "@/editor/typography";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { EditorIpcAdapter } from "@/ipc/editorAdapter";
import type { LinkIpcAdapter } from "@/ipc/linkAdapter";
import { tags } from "@lezer/highlight";

if (!Range.prototype.getClientRects) {
  Range.prototype.getClientRects = () =>
    ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] }) as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 0, 0);
}

const ipc = {
  noteOpened: vi.fn().mockResolvedValue(undefined),
  noteClosed: vi.fn().mockResolvedValue(undefined),
  savePatch: vi.fn(),
  requestExternalDiff: vi.fn().mockResolvedValue(null),
  rebaseBuffer: vi.fn().mockResolvedValue([]),
  readFile: vi.fn(),
  writeFile: vi.fn(),
  onExternalChange: vi.fn().mockResolvedValue(() => undefined),
  onClosing: vi.fn().mockResolvedValue(() => undefined),
  flushAck: vi.fn().mockResolvedValue(undefined),
} as unknown as EditorIpcAdapter;
const linkIpc = {
  resolveLinks: vi.fn().mockResolvedValue([]),
  noteAnchors: vi.fn().mockResolvedValue([]),
  switcherQuery: vi.fn().mockResolvedValue([]),
  onIndexChanged: vi.fn().mockResolvedValue(() => undefined),
} as unknown as LinkIpcAdapter;

const DOC = readFileSync(join(__dirname, "__fixtures__/live-preview-all.md"), "utf8");

function mountWith(props: { livePreview: boolean }) {
  const wrapper = mount(CodeMirrorEditor, { props: { ipc, linkIpc, ...props }, attachTo: document.body });
  const host = wrapper.find(".cm-editor-host").element as HTMLElement;
  const content = host.shadowRoot!.querySelector(".cm-content") as HTMLElement;
  const view = EditorView.findFromDOM(content)!;
  (wrapper.vm as unknown as { setText(t: string): void }).setText(DOC);
  return { wrapper, view, content, editor: view.dom };
}

const lineOf = (view: EditorView, needle: string): HTMLElement => {
  const at = view.state.doc.toString().indexOf(needle);
  const node = view.domAtPos(at).node;
  return (node instanceof Element ? node : node.parentElement!).closest(".cm-line") as HTMLElement;
};

const stateAt = (anchor: number) =>
  EditorState.create({ doc: DOC, selection: { anchor }, extensions: [seratMarkdown()] });
function scanAt(anchor: number) {
  const st = stateAt(anchor);
  const scan = scanReveal(st, [{ from: 0, to: st.doc.length }], selectedLineNumbers(st.doc, [{ from: anchor, to: anchor }]));
  const classesOf = (needle: string) => scan.lines.get(st.doc.lineAt(DOC.indexOf(needle)).from) ?? "";
  return { st, scan, classesOf };
}

describe("highlight style", () => {
  it("styles every tag the markdown parser and the Serat nodes emit", () => {
    const state = EditorState.create({ extensions: [seratMarkdown(), typographyExtensions()] });
    const expected: Array<[unknown, string]> = [
      [tags.strong, "serat-strong"],
      [tags.emphasis, "serat-em"],
      [tags.strikethrough, "serat-strike"],
      [tags.monospace, "serat-code"],
      [tags.link, "serat-md-link"],
      [tags.url, "serat-url"],
      [tags.processingInstruction, "serat-marker"],
      [seratTags.highlight, "serat-highlight"],
      [seratTags.blockRef, "serat-machine"],
      [seratTags.field, "serat-machine"],
      [seratTags.footnoteRef, "serat-footnote"],
      [seratTags.wikiLink, "serat-wikilink"],
      [seratTags.embed, "serat-wikilink"],
    ];
    for (const [tag, cls] of expected) {
      expect(highlightingFor(state, [tag as never]), cls).toContain(cls);
    }
  });

  it("a wikilink takes no markdown-link class (links.ts styles it once the index answers), and machine syntax no link class", () => {
    const state = EditorState.create({ extensions: [seratMarkdown(), typographyExtensions()] });
    expect(highlightingFor(state, [seratTags.wikiLink])).not.toContain("serat-md-link");
    expect(highlightingFor(state, [seratTags.blockRef])).not.toContain("serat-md-link");
    expect(seratHighlightStyle).toBeDefined();
  });
});

describe("scanReveal: the fixture note", () => {
  it("headings 1 to 6, the quote, the fence and the rule carry their classes", () => {
    const { classesOf } = scanAt(DOC.length);
    for (let n = 1; n <= 6; n++) expect(classesOf(`Heading ${["", "one", "two", "three", "four", "five", "six"][n]}`)).toContain(`serat-h${n}`);
    expect(classesOf("A quotation")).toContain("serat-quote");
    expect(classesOf("It runs over")).toContain("serat-quote");
    expect(classesOf("const answer")).toContain("serat-code-line");
    expect(classesOf("```ts")).toContain("serat-code-line");
  });

  it("list items carry serat-li and their nesting depth as a class (a style attribute would break the CSP)", () => {
    const { classesOf } = scanAt(DOC.length);
    expect(classesOf("- first bullet")).toContain("serat-depth-0");
    expect(classesOf("- nested bullet at depth one")).toContain("serat-depth-1");
    expect(classesOf("- nested bullet at depth two")).toContain("serat-depth-2");
    expect(classesOf("- nested bullet at depth three")).toContain("serat-depth-3");
    expect(classesOf("- second bullet")).toContain("serat-depth-0");
    expect(classesOf("1. first numbered")).toMatch(/serat-li serat-depth-0/);
    expect(classesOf("1. nested numbered")).toContain("serat-depth-1");
    expect(classesOf("- first bullet")).toContain("serat-li");
  });

  it("the properties block is raw: frontmatter class on its lines, nothing in it hidden, no heading or rule class", () => {
    const { st, scan, classesOf } = scanAt(DOC.length);
    const end = frontmatterEndLine(st.doc);
    expect(end).toBe(4);
    for (let n = 1; n <= end; n++) expect(scan.lines.get(st.doc.line(n).from)).toBe("serat-frontmatter");
    expect(classesOf("tags: [design")).toBe("serat-frontmatter");
    expect(scan.hidden.every((span) => span.from > st.doc.line(end).to)).toBe(true);
    // the later `---` is a real thematic break
    const rule = st.doc.lineAt(DOC.indexOf("```\n\n---") + 5).number;
    expect(scan.lines.get(st.doc.line(rule).from)).toContain("serat-hr");
  });

  it("a note that opens with a rule and never closes it has no properties block", () => {
    expect(frontmatterEndLine(EditorState.create({ doc: "---\nnot closed\n" + "x\n".repeat(300) }).doc)).toBe(0);
    expect(frontmatterEndLine(EditorState.create({ doc: "text\n---\n" }).doc)).toBe(0);
  });

  it("a bullet off the selected line becomes a glyph widget with its indent hidden; revealed it stays raw", () => {
    const nested = DOC.indexOf("- nested bullet at depth one");
    const off = scanAt(DOC.length).scan.listMarks.find((m) => m.from === nested)!;
    expect(off.widget).toBe(true);
    expect(off.indent).toEqual({ from: nested - 2, to: nested });
    expect(off.to - off.from).toBe(2); // "- ": the space goes with the mark so the item text starts at the column edge
    const on = scanAt(nested + 5).scan.listMarks.find((m) => m.from === nested)!;
    expect(on.widget).toBe(false);
  });

  it("an ordered numeral is never replaced, revealed or not", () => {
    const at = DOC.indexOf("1. first numbered");
    for (const sel of [DOC.length, at + 4]) {
      const mark = scanAt(sel).scan.listMarks.find((m) => m.from === at)!;
      expect(mark.ordered).toBe(true);
      expect(mark.widget).toBe(false);
    }
  });

  it("selecting a line reveals its markers as serat-marker spans (custom syntax) and unhides the standard ones", () => {
    const wiki = DOC.indexOf("[[Existing note]]");
    const { scan, st } = scanAt(wiki);
    expect(scan.revealedMarkers.some((m) => m.from === wiki && m.to === wiki + 2)).toBe(true);
    expect(scan.hidden.some((m) => m.from >= st.doc.lineAt(wiki).from && m.to <= st.doc.lineAt(wiki).to)).toBe(false);
  });
});

describe("the mounted editor", () => {
  it("live mode: serat-live on the editor root, highlight classes, heading semantics, glyph bullets", () => {
    const { wrapper, editor } = mountWith({ livePreview: true });
    expect(editor.classList.contains("serat-live")).toBe(true);
    expect(editor.classList.contains("serat-source")).toBe(false);
    for (const cls of ["serat-strong", "serat-em", "serat-strike", "serat-highlight", "serat-code", "serat-machine", "serat-md-link"]) {
      expect(editor.querySelector(`.${cls}`), cls).not.toBeNull();
    }
    const h1 = editor.querySelector(".cm-line.serat-h1") as HTMLElement;
    expect(h1.getAttribute("role")).toBe("heading");
    expect(h1.getAttribute("aria-level")).toBe("1");
    wrapper.unmount();
  });

  it("code lines and glyph bullets render (compact note: jsdom has no layout, so the fixture's tail is out of viewport)", () => {
    const { wrapper, view, editor } = mountWith({ livePreview: true });
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "# T\n\n- a\n  - b\n\n1. n\n\n```\ncode\n```\n" }, selection: { anchor: 0 } });
    expect(editor.querySelectorAll(".cm-line.serat-code-line").length).toBe(3);
    expect(editor.querySelectorAll("span.serat-bullet[aria-hidden]").length).toBe(2);
    expect(lineOf(view, "- b").textContent).toBe("\u2022b"); // indent and the mark's space are gone; the glyph column is CSS
    expect(lineOf(view, "1. n").textContent).toBe("1. n"); // the numeral is never hidden
    wrapper.unmount();
  });

  it("moving the caret reveals that line's markers in the same update and re-hides the old line's", () => {
    const { wrapper, view, editor } = mountWith({ livePreview: true });
    const bold = DOC.indexOf("**bold**");
    expect(lineOf(view, "Body text").textContent).not.toContain("https://example.com"); // `(url)` is syntax: hidden off-line
    view.dispatch({ selection: { anchor: bold + 4 } });
    expect(lineOf(view, "Body text").textContent).toContain("**bold**");
    expect(lineOf(view, "Body text").textContent).toContain("(https://example.com/page)");
    expect(editor.querySelector(".cm-line.serat-h1")!.textContent).toBe("Heading one"); // `#` hidden again
    view.dispatch({ selection: { anchor: DOC.indexOf("Heading one") } });
    expect(editor.querySelector(".cm-line.serat-h1")!.textContent).toBe("# Heading one");
    expect(lineOf(view, "Body text").textContent).not.toContain("**");
    wrapper.unmount();
  });

  it("split-source mode: serat-source on the root, no reveal decorations, still highlighted; toggling reconfigures", async () => {
    const { wrapper, editor } = mountWith({ livePreview: false });
    expect(editor.classList.contains("serat-source")).toBe(true);
    expect(editor.classList.contains("serat-live")).toBe(false);
    expect(editor.querySelector(".serat-h1")).toBeNull();
    expect(editor.querySelector(".serat-bullet")).toBeNull();
    expect(editor.querySelector(".serat-strong")).not.toBeNull();
    await wrapper.setProps({ livePreview: true });
    expect(editor.classList.contains("serat-live")).toBe(true);
    expect(editor.classList.contains("serat-source")).toBe(false);
    wrapper.unmount();
  });
});

describe("reveal plugin and the parser", () => {
  it("restyles when a background parse advances the tree, with no doc, selection or viewport change", () => {
    const doc = "word ".repeat(40).trimEnd() + "\n\n";
    const big = doc.repeat(6000) + "# Late heading\n";
    const view = new EditorView({
      parent: document.body,
      state: EditorState.create({ doc: big, selection: { anchor: big.length - 3 }, extensions: [seratMarkdown(), revealMarkersPlugin] }),
    });
    view.dispatch({ selection: { anchor: big.length - 3 }, effects: EditorView.scrollIntoView(big.length - 3) });
    const heading = () => view.dom.querySelector(".cm-line.serat-h1");
    if (heading()) return view.destroy(); // jsdom parsed it all in the first slice: nothing to advance
    expect(syntaxTree(view.state).length).toBeLessThan(big.length);
    forceParsing(view, big.length, 10_000);
    view.dispatch({}); // let the plugin see the new tree
    expect(heading()).not.toBeNull();
    view.destroy();
  });
});

describe("markdown corner cases in the reveal scan", () => {
  const scanDoc = (doc: string, anchor = doc.length) => {
    const st = EditorState.create({ doc, selection: { anchor }, extensions: [seratMarkdown()] });
    return { st, scan: scanReveal(st, [{ from: 0, to: st.doc.length }], selectedLineNumbers(st.doc, [{ from: anchor, to: anchor }])) };
  };

  it("a Setext heading gives its first line the heading class and its underline none", () => {
    const { st, scan } = scanDoc("Title\n=====\n\nbody", 15);
    expect(scan.lines.get(st.doc.line(1).from)).toBe("serat-h1");
    expect(scan.lines.has(st.doc.line(2).from)).toBe(false);
  });

  it("frontmatter follows the index's rule: a --- pair is a block, a ... closer is not", () => {
    expect(frontmatterEndLine(EditorState.create({ doc: "---\n---\nbody" }).doc)).toBe(2); // an empty block, as comrak and Obsidian read it
    expect(frontmatterEndLine(EditorState.create({ doc: "---\na: 1\n...\nbody" }).doc)).toBe(0);
    expect(frontmatterEndLine(EditorState.create({ doc: "--- \na: 1\n---\n" }).doc)).toBe(0); // the opener is exactly ---
  });

  it("the (url) and the title of a link, and the url of an image, hide off the selected line", () => {
    const doc = 'a [t](u.md "ti") and ![img](p.png) end';
    const { st, scan } = scanDoc(doc + "\n\nx", doc.length + 2);
    const hiddenText = scan.hidden.map((s) => st.sliceDoc(s.from, s.to)).join("|");
    expect(hiddenText).toContain("u.md");
    expect(hiddenText).toContain(' "ti"'); // with its leading space
    expect(hiddenText).toContain("p.png");
    const shown = scanDoc(doc, 3).scan.hidden;
    expect(shown.length).toBe(0); // revealed: nothing hidden
  });

  it("a bullet followed by a tab is still a glyph widget", () => {
    const { scan } = scanDoc("-\titem\n\nx", 9);
    expect(scan.listMarks[0].to - scan.listMarks[0].from).toBe(2);
  });

  it("a revealed nested ordered item keeps its indent in the layout class instead of replacing it", () => {
    const doc = "1. a\n   1. b\n";
    const on = scanDoc(doc, doc.indexOf("b")).scan.listMarks.find((m) => m.indent)!;
    expect(on.revealed).toBe(true);
    const off = scanDoc(doc + "\nz", doc.length + 1).scan.listMarks.find((m) => m.indent)!;
    expect(off.revealed).toBe(false);
  });
});
