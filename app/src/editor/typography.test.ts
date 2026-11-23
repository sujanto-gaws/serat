// Task #104 (FR-EDT-01, FR-EDT-02, FR-EDT-10): the hybrid live preview is styled. Highlight tags reach
// the DOM as `serat-tok-*` classes, revealed and code lines take `serat-mono-source`, and the mode class
// follows `livePreview`. These are structural checks (class names, not computed pixels: jsdom has no
// layout); the values behind the classes are pinned by tokens.test.ts and the theme below.

import { mount } from "@vue/test-utils";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { highlightingFor } from "@codemirror/language";
import { describe, expect, it, vi } from "vitest";

import CodeMirrorEditor from "@/editor/CodeMirrorEditor.vue";
import { seratMarkdown } from "@/editor/markdownLanguage";
import { scanReveal, selectedLineNumbers } from "@/editor/reveal";
import { seratTags } from "@/editor/mdExtensions";
import { seratHighlightStyle, typographyExtensions } from "@/editor/typography";
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

const DOC = ["# Title", "", "plain **bold** and [[Note]]", "", "```", "code", "```", "", "tail ^blk"].join("\n");

function mountWith(props: { livePreview: boolean }) {
  const wrapper = mount(CodeMirrorEditor, { props: { ipc, linkIpc, ...props }, attachTo: document.body });
  const host = wrapper.find(".cm-editor-host").element as HTMLElement;
  const content = host.shadowRoot!.querySelector(".cm-content") as HTMLElement;
  const view = EditorView.findFromDOM(content)!;
  (wrapper.vm as unknown as { setText(t: string): void }).setText(DOC);
  return { wrapper, view, content };
}

const lineOf = (view: EditorView, needle: string): HTMLElement => {
  const at = view.state.doc.toString().indexOf(needle);
  const node = view.domAtPos(at).node;
  return (node instanceof Element ? node : node.parentElement!).closest(".cm-line") as HTMLElement;
};

describe("highlight style", () => {
  it("styles every tag the markdown parser and the Serat nodes emit", () => {
    const state = EditorState.create({ extensions: [seratMarkdown(), typographyExtensions()] });
    for (const tag of [
      tags.heading1,
      tags.heading2,
      tags.heading3,
      tags.strong,
      tags.emphasis,
      tags.strikethrough,
      tags.monospace,
      tags.link,
      tags.processingInstruction,
      seratTags.wikiLink,
      seratTags.embed,
      seratTags.highlight,
      seratTags.blockRef,
      seratTags.field,
      seratTags.footnoteRef,
    ]) {
      expect(highlightingFor(state, [tag]), String(tag)).toMatch(/\S/);
    }
  });

  it("the machine-syntax tags carry no generic link style (blockRef and field are not links)", () => {
    expect(seratHighlightStyle.style([seratTags.blockRef])).toBe(seratHighlightStyle.style([seratTags.field]));
    expect(seratHighlightStyle.style([seratTags.blockRef])).not.toBe(seratHighlightStyle.style([tags.link]));
  });
});

describe("scanReveal", () => {
  const state = (selAt: number) =>
    EditorState.create({ doc: DOC, selection: { anchor: selAt }, extensions: [seratMarkdown()] });
  const scan = (selAt: number) => {
    const st = state(selAt);
    return {
      st,
      scan: scanReveal(st, [{ from: 0, to: st.doc.length }], selectedLineNumbers(st.doc, [{ from: selAt, to: selAt }])),
    };
  };

  it("a selected line with syntax is revealed (mono-source); one without is not", () => {
    const boldAt = DOC.indexOf("bold");
    const { st, scan: s } = scan(boldAt);
    expect([...s.revealedLines]).toEqual([st.doc.lineAt(boldAt).from]);
    const blank = DOC.indexOf("\n\n") + 1; // the empty line after the heading
    expect(scan(blank).scan.revealedLines.size).toBe(0);
  });

  it("code lines are every line of the fence, markers included", () => {
    const { st, scan: s } = scan(0);
    const fenceFrom = DOC.indexOf("```");
    const first = st.doc.lineAt(fenceFrom).number;
    expect([...s.codeLines].sort((a, b) => a - b)).toEqual([first, first + 1, first + 2].map((n) => st.doc.line(n).from));
  });
});

describe("the mounted editor", () => {
  it("live mode: serat-live content, serat-tok classes on rendered text, mono-source only on a revealed line", () => {
    const { wrapper, view, content } = mountWith({ livePreview: true });
    expect(content.classList.contains("serat-live")).toBe(true);
    expect(content.classList.contains("serat-source")).toBe(false);
    expect(view.dom.querySelector(".serat-tok-h1")).not.toBeNull();
    expect(view.dom.querySelector(".serat-tok-strong")).not.toBeNull();
    expect(view.dom.querySelector(".serat-tok-machine")).not.toBeNull();
    // the cursor sits at 0 (the heading line): that line is raw, the paragraph is not
    expect(lineOf(view, "Title").classList.contains("serat-mono-source")).toBe(true);
    expect(lineOf(view, "plain").classList.contains("serat-mono-source")).toBe(false);
    // code lines are mono-source on a filled band whether or not the caret is there
    const code = lineOf(view, "code\n");
    expect(code.classList.contains("serat-mono-source")).toBe(true);
    expect(code.classList.contains("serat-code-line")).toBe(true);
    wrapper.unmount();
  });

  it("moving the caret reveals the new line within the same update and releases the old one", () => {
    const { wrapper, view } = mountWith({ livePreview: true });
    view.dispatch({ selection: { anchor: DOC.indexOf("plain") } });
    expect(lineOf(view, "plain").classList.contains("serat-mono-source")).toBe(true);
    expect(lineOf(view, "Title").classList.contains("serat-mono-source")).toBe(false);
    wrapper.unmount();
  });

  it("split-source mode: serat-source content, no reveal decorations, still highlighted", async () => {
    const { wrapper, view, content } = mountWith({ livePreview: false });
    expect(content.classList.contains("serat-source")).toBe(true);
    expect(content.classList.contains("serat-live")).toBe(false);
    expect(view.dom.querySelectorAll(".serat-mono-source").length).toBe(0);
    expect(view.dom.querySelector(".serat-tok-h1")).not.toBeNull();
    await wrapper.setProps({ livePreview: true });
    expect(content.classList.contains("serat-live")).toBe(true);
    expect(content.classList.contains("serat-source")).toBe(false);
    wrapper.unmount();
  });
});
