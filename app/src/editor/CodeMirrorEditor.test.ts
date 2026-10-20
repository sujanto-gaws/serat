// Task #21 review fixes: loading a note must never be undoable (NFR-REL-01,
// C-04), switching notes flushes the old buffer first, and the editor's
// built state carries exactly one keymap (architecture.md §9.3).

import { mount } from "@vue/test-utils";
import { keymap, EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import CodeMirrorEditor from "@/editor/CodeMirrorEditor.vue";
import type { EditorIpcAdapter } from "@/ipc/editorAdapter";
import type { FileWritten } from "@/ipc/generated/FileWritten";

// jsdom has no layout; CodeMirror's measure pass calls these on text ranges.
if (!Range.prototype.getClientRects) {
  Range.prototype.getClientRects = () =>
    ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] }) as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 0, 0);
}

function mockIpc(savePatch = vi.fn()): EditorIpcAdapter {
  return {
    noteOpened: vi.fn().mockResolvedValue(undefined),
    noteClosed: vi.fn().mockResolvedValue(undefined),
    savePatch,
    requestExternalDiff: vi.fn().mockResolvedValue(null),
    rebaseBuffer: vi.fn().mockResolvedValue([]),
  };
}

function mountEditor(ipc: EditorIpcAdapter) {
  const wrapper = mount(CodeMirrorEditor, { props: { ipc }, attachTo: document.body });
  const host = wrapper.find(".cm-editor-host").element as HTMLElement;
  const content = host.shadowRoot!.querySelector(".cm-content") as HTMLElement;
  const view = EditorView.findFromDOM(content)!;
  const api = wrapper.vm as unknown as {
    setText(t: string): void;
    getText(): string;
    loadNote(t: string, p: string, h: string): Promise<boolean>;
    undo(): unknown;
    redo(): unknown;
    isDirty: boolean;
  };
  return { wrapper, view, api };
}

const written = (hash: string): FileWritten => ({ path: "a.md", hash, mtime_ms: 0, bytes: 0 });

describe("CodeMirrorEditor load path", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("App's startup path (setText then loadNote) cannot be undone into an empty document", async () => {
    const savePatch = vi.fn().mockResolvedValue(written("h1"));
    const { wrapper, api } = mountEditor(mockIpc(savePatch));

    api.setText("# hello\n\nbody"); // hydrateBuffer
    await api.loadNote(api.getText(), "a.md", "h0"); // App.vue's follow-up

    for (let i = 0; i < 5; i += 1) api.undo();
    expect(api.getText()).toBe("# hello\n\nbody");
    expect(api.isDirty).toBe(false);

    await vi.advanceTimersByTimeAsync(2500); // well past the 1 s autosave
    expect(savePatch).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("a real edit after load is still undoable, but only back to the loaded text", async () => {
    const savePatch = vi.fn().mockResolvedValue(written("h1"));
    const { wrapper, view, api } = mountEditor(mockIpc(savePatch));
    await api.loadNote("abc", "a.md", "h0");

    view.dispatch({ changes: { from: 3, insert: "d" } });
    expect(api.isDirty).toBe(true);
    api.undo();
    api.undo();
    api.undo();
    expect(api.getText()).toBe("abc");

    await vi.advanceTimersByTimeAsync(1500);
    // The edit and its undo cancel out: nothing to send.
    expect(savePatch).not.toHaveBeenCalled();
    expect(api.isDirty).toBe(false);
    wrapper.unmount();
  });

  it("switching notes flushes the old buffer's unsaved edits first, and the new note is not undoable", async () => {
    const order: string[] = [];
    const savePatch = vi.fn().mockImplementation(async (args: { path: string }) => {
      order.push(`save:${args.path}`);
      return written("h1");
    });
    const { wrapper, view, api } = mountEditor(mockIpc(savePatch));
    await api.loadNote("one", "a.md", "h0");
    view.dispatch({ changes: { from: 3, insert: "!" } });

    const switched = await api.loadNote("two", "b.md", "hb");
    order.push("loaded:b.md");

    expect(switched).toBe(true);
    expect(order).toEqual(["save:a.md", "loaded:b.md"]);
    expect(savePatch.mock.calls[0][0].patches[0].insert).toBe("!");
    expect(api.getText()).toBe("two");
    for (let i = 0; i < 5; i += 1) api.undo();
    expect(api.getText()).toBe("two");
    expect(api.isDirty).toBe(false);

    await vi.advanceTimersByTimeAsync(2500);
    expect(savePatch).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("waits for a save already in flight before replacing the buffer", async () => {
    let release!: (w: FileWritten) => void;
    const savePatch = vi.fn().mockImplementation(
      () =>
        new Promise<FileWritten>((resolve) => {
          release = resolve;
        }),
    );
    const { wrapper, view, api } = mountEditor(mockIpc(savePatch));
    await api.loadNote("one", "a.md", "h0");
    view.dispatch({ changes: { from: 3, insert: "!" } });
    await vi.advanceTimersByTimeAsync(1100); // autosave starts, unresolved

    let done = false;
    const p = api.loadNote("two", "b.md", "hb").then((r) => {
      done = true;
      return r;
    });
    await vi.advanceTimersByTimeAsync(10);
    expect(done).toBe(false);
    expect(api.getText()).toBe("one!");

    release(written("h1"));
    await vi.advanceTimersByTimeAsync(10);
    expect(await p).toBe(true);
    expect(api.getText()).toBe("two");
    wrapper.unmount();
  });

  it("keeps the old note (and resolves false) when its edits cannot be saved", async () => {
    const savePatch = vi.fn().mockRejectedValue({ kind: "stale_hash", path: "a.md", current_hash: "x" });
    const { wrapper, view, api } = mountEditor(mockIpc(savePatch));
    await api.loadNote("one", "a.md", "h0");
    view.dispatch({ changes: { from: 3, insert: "!" } });

    expect(await api.loadNote("two", "b.md", "hb")).toBe(false);
    expect(api.getText()).toBe("one!");
    expect(api.isDirty).toBe(true);
    wrapper.unmount();
  });
});

describe("CodeMirrorEditor keymap", () => {
  it("the built state has exactly one keymap entry, so no extension adds a bypass keymap (§9.3)", () => {
    const { wrapper, view } = mountEditor(mockIpc());
    expect(view.state.facet(keymap)).toHaveLength(1);
    wrapper.unmount();
  });

  it("Enter continues a Markdown list through the registry's command", () => {
    const { wrapper, view } = mountEditor(mockIpc());
    view.dispatch({ changes: { from: 0, insert: "- item" }, selection: { anchor: 6 } });
    const binding = view.state.facet(keymap)[0].find((b) => b.key === "Enter")!;
    expect(binding.run!(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("- item\n- ");
    wrapper.unmount();
  });
});
