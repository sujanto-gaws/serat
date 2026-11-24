// #170/#171 (NFR-PRF-02, US-01, ADR-0019): the split-preview pane is contained, and the per-keystroke
// path never stringifies the document unless the preview pane is the thing asking for it.

import { mount } from "@vue/test-utils";
import { EditorView } from "@codemirror/view";
import { Text } from "@codemirror/state";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

import CodeMirrorEditor from "@/editor/CodeMirrorEditor.vue";
import Preview from "@/editor/Preview.vue";
import type { EditorIpcAdapter } from "@/ipc/editorAdapter";

if (!Range.prototype.getClientRects) {
  Range.prototype.getClientRects = () =>
    ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] }) as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 0, 0);
}

function mockIpc(): EditorIpcAdapter {
  return {
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
  };
}

async function mountLoaded(livePreview: boolean) {
  const wrapper = mount(CodeMirrorEditor, { props: { ipc: mockIpc(), livePreview }, attachTo: document.body });
  const host = wrapper.find(".cm-editor-host").element as HTMLElement;
  const view = EditorView.findFromDOM(host.shadowRoot!.querySelector(".cm-content") as HTMLElement)!;
  const api = wrapper.vm as unknown as { loadNote(t: string, p: string, h: string): Promise<boolean> };
  await api.loadNote("# Title\n\nbody", "a.md", "h0");
  return { wrapper, view };
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("#171 no per-keystroke stringify", () => {
  it("typing with live preview on (no preview pane) never calls Text.toString", async () => {
    const { wrapper, view } = await mountLoaded(true);
    const spy = vi.spyOn(Text.prototype, "toString");
    for (let i = 0; i < 5; i += 1) view.dispatch({ changes: { from: view.state.doc.length, insert: "x" } });
    expect(spy).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("typing in split mode does not stringify on the keystroke either; the debounced render does, once", async () => {
    const { wrapper, view } = await mountLoaded(false);
    vi.useFakeTimers();
    const spy = vi.spyOn(Text.prototype, "toString");
    for (let i = 0; i < 5; i += 1) view.dispatch({ changes: { from: view.state.doc.length, insert: "x" } });
    expect(spy).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(400);
    expect(spy).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("the split preview renders the current text after the debounce", async () => {
    const { wrapper, view } = await mountLoaded(false);
    vi.useFakeTimers();
    view.dispatch({ changes: { from: view.state.doc.length, insert: "\n\nTYPED-MARKER" } });
    await vi.advanceTimersByTimeAsync(400);
    vi.useRealTimers();
    await vi.waitFor(() => {
      expect(wrapper.get('[data-testid="serat-preview"]').text()).toContain("TYPED-MARKER");
    });
    wrapper.unmount();
  });

  it("Preview given a Text snapshot renders the same output as given the string", async () => {
    const text = "# Hi\n\nsome *body*";
    const a = mount(Preview, { props: { source: text, debounceMs: 0 } });
    const b = mount(Preview, { props: { source: Text.of(text.split("\n")), debounceMs: 0 } });
    await vi.waitFor(() => expect(a.html()).toContain("<h1"));
    await vi.waitFor(() => expect(b.html()).toBe(a.html()));
    a.unmount();
    b.unmount();
  });
});

describe("#170 split-preview containment", () => {
  const vue = readFileSync(join(__dirname, "CodeMirrorEditor.vue"), "utf8");
  const rule = (src: string): string => /\.serat-editor-split-preview\s*\{([^}]*)\}/.exec(src)?.[1] ?? "";

  it("the pane rule carries contain: strict", () => {
    expect(rule(vue)).toMatch(/contain:\s*strict\s*;/);
  });

  it("the pane keeps an explicit flex basis (size containment needs a definite size) and its own overflow", () => {
    expect(rule(vue)).toMatch(/flex:\s*1 1 50%/);
    expect(rule(vue)).toMatch(/overflow:\s*auto/);
  });

  it("mutant: dropping the contain declaration is detected", () => {
    expect(rule(vue.replace(/contain:\s*strict\s*;/, ""))).not.toMatch(/contain:\s*strict/);
  });
});
