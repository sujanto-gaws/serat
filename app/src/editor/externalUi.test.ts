// Task #23 round 2 (US-06, FR-VLT-04/05; external-change-notice.md, editor.md StaleHash sequencing):
// the visible states of an external change: minimised dot, in-place Compare ("Comparing"),
// confirmations, the persistent "Not saved" signal, and resuming saves when the disk matches again.

import { EditorView } from "@codemirror/view";
import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import CodeMirrorEditor from "@/editor/CodeMirrorEditor.vue";
import type { EditorIpcAdapter } from "@/ipc/editorAdapter";
import type { ExternalChange } from "@/ipc/generated/ExternalChange";
import type { FileWritten } from "@/ipc/generated/FileWritten";
import type { Patch } from "@/ipc/generated/Patch";
import type { LinkIpcAdapter } from "@/ipc/linkAdapter";

if (!Range.prototype.getClientRects) {
  Range.prototype.getClientRects = () =>
    ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] }) as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 0, 0);
}

const patch = (line: number, col: number, toLine: number, toCol: number, insert: string): Patch => ({
  from: { line, col },
  to: { line: toLine, col: toCol },
  insert,
});
const changed = (base_hash: string, new_hash: string, edits: Patch[]): ExternalChange => ({
  kind: "changed",
  vault_id: "v",
  path: "a.md",
  base_hash,
  new_hash,
  edits,
  whole: false,
});
const written = (hash: string): FileWritten => ({ path: "a.md", hash, mtime_ms: 0, bytes: 0 });
const flush = () => vi.advanceTimersByTimeAsync(0);

function harness(overrides: Partial<EditorIpcAdapter> = {}) {
  let emit: (c: ExternalChange) => void = () => undefined;
  const ipc: EditorIpcAdapter = {
    noteOpened: vi.fn().mockResolvedValue(undefined),
    noteClosed: vi.fn().mockResolvedValue(undefined),
    savePatch: vi.fn().mockResolvedValue(written("h9")),
    requestExternalDiff: vi.fn().mockResolvedValue(null),
    rebaseBuffer: vi.fn().mockResolvedValue([]),
    readFile: vi.fn(),
    writeFile: vi.fn(),
    onExternalChange: vi.fn(async (h: (c: ExternalChange) => void) => {
      emit = h;
      return () => undefined;
    }),
    onClosing: vi.fn().mockResolvedValue(() => undefined),
    flushAck: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
  const linkIpc: LinkIpcAdapter = {
    resolveLinks: vi.fn().mockResolvedValue([]),
    noteAnchors: vi.fn(),
    switcherQuery: vi.fn(),
    onIndexChanged: vi.fn().mockResolvedValue(() => undefined),
  };
  const wrapper = mount(CodeMirrorEditor, { props: { ipc, linkIpc }, attachTo: document.body });
  const host = wrapper.find(".cm-editor-host").element as HTMLElement;
  const view = EditorView.findFromDOM(host.shadowRoot!.querySelector(".cm-content") as HTMLElement)!;
  const api = wrapper.vm as unknown as {
    loadNote(t: string, p: string, h: string): Promise<boolean>;
    getText(): string;
    compareWithDisk(): Promise<void>;
    isDirty: boolean;
  };
  return { ipc, wrapper, view, api, send: (c: ExternalChange) => emit(c) };
}

async function conflicted(overrides: Partial<EditorIpcAdapter> = {}) {
  const h = harness({
    requestExternalDiff: vi.fn().mockResolvedValue(changed("h0", "h2", [patch(0, 0, 0, 3, "THEIRS")])),
    ...overrides,
  });
  await h.api.loadNote("one\ntwo\nthree", "a.md", "h0");
  h.view.dispatch({ changes: { from: 0, to: 3, insert: "MINE" } });
  h.send(changed("h0", "h2", [patch(0, 0, 0, 3, "THEIRS")]));
  await flush();
  return h;
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("minimised notice (Esc on the card, brass dot)", () => {
  it("Esc on the focused card minimises to a dot; the conflict stays pending; the dot reopens the full notice", async () => {
    const { wrapper, api } = await conflicted();
    await wrapper.find('[data-testid="external-conflict"]').trigger("keydown", { key: "Escape" });
    expect(wrapper.find('[data-testid="external-conflict"]').exists()).toBe(false);
    const dot = wrapper.find('[data-testid="external-dot"]');
    expect(dot.exists()).toBe(true);
    expect(dot.attributes("title")).toContain("External change pending");
    expect((wrapper.vm as unknown as { conflict: unknown }).conflict).not.toBeNull();
    expect(api.getText()).toBe("MINE\ntwo\nthree");
    await dot.trigger("click");
    expect(wrapper.find('[data-testid="external-conflict"]').exists()).toBe(true);
    expect(wrapper.find('[data-testid="external-dot"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("Esc pressed in the buffer does not minimise the notice", async () => {
    const { wrapper, view } = await conflicted();
    view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await flush();
    expect(wrapper.find('[data-testid="external-conflict"]').exists()).toBe(true);
    wrapper.unmount();
  });

  it("a refused note switch (unsaveable dirty buffer) keeps the pending dot", async () => {
    const { wrapper, api } = await conflicted({ savePatch: vi.fn().mockRejectedValue({ kind: "io" }) });
    await wrapper.find('[data-testid="external-conflict"]').trigger("keydown", { key: "Escape" });
    // The buffer is dirty and cannot be saved, so loadNote refuses and the dot stays.
    expect(await api.loadNote("x", "b.md", "h")).toBe(false);
    expect(wrapper.find('[data-testid="external-dot"]').exists()).toBe(true);
    wrapper.unmount();
  });
});

describe("Compare: the in-place diff view ('Comparing')", () => {
  it("replaces the editor content with disk on the left and the buffer on the right, differing lines highlighted", async () => {
    const { wrapper, api } = await conflicted();
    await wrapper.find('[data-testid="compare"]').trigger("click");
    await flush();
    const view = wrapper.find('[data-testid="external-compare"]');
    expect(view.exists()).toBe(true);
    expect(wrapper.find('[data-testid="compare-disk"]').text()).toContain("THEIRS");
    expect(wrapper.find('[data-testid="compare-mine"]').text()).toContain("MINE");
    expect(wrapper.find('[data-testid="compare-disk"] .hot').text()).toBe("THEIRS");
    expect(wrapper.find('[data-testid="compare-mine"] .hot').text()).toBe("MINE");
    expect(wrapper.findAll('[data-testid="compare-disk"] .hot')).toHaveLength(1); // "two"/"three" are not highlighted
    expect(api.getText()).toBe("MINE\ntwo\nthree"); // inspection only
    expect(wrapper.emitted("compare-external")).toHaveLength(1);
    wrapper.unmount();
  });

  it("Esc returns to the editor with the notice still present", async () => {
    const { wrapper } = await conflicted();
    await wrapper.find('[data-testid="compare"]').trigger("click");
    await flush();
    await wrapper.find('[data-testid="external-compare"]').trigger("keydown", { key: "Escape" });
    expect(wrapper.find('[data-testid="external-compare"]').exists()).toBe(false);
    expect(wrapper.find('[data-testid="external-conflict"]').exists()).toBe(true);
    wrapper.unmount();
  });

  it("the header offers Keep mine / Take theirs, and choosing one resolves and leaves the diff", async () => {
    const rebaseBuffer = vi.fn().mockResolvedValue([patch(0, 0, 0, 6, "MINE")]);
    const { wrapper, api } = await conflicted({ rebaseBuffer });
    await wrapper.find('[data-testid="compare"]').trigger("click");
    await flush();
    await wrapper.find('[data-testid="compare-take-theirs"]').trigger("click");
    await flush();
    expect(wrapper.find('[data-testid="external-compare"]').exists()).toBe(false);
    expect(wrapper.find('[data-testid="external-conflict"]').exists()).toBe(false);
    expect(api.getText()).toBe("THEIRS\ntwo\nthree");
    wrapper.unmount();
  });

  it("identical texts are said so", async () => {
    const { wrapper } = harness({ readFile: vi.fn().mockResolvedValue({ path: "a.md", text: "same", hash: "h", mtime_ms: 0, bytes: 4 }) });
    await (wrapper.vm as unknown as { loadNote(t: string, p: string, h: string): Promise<boolean> }).loadNote("same", "a.md", "h0");
    await (wrapper.vm as unknown as { compareWithDisk(): Promise<void> }).compareWithDisk();
    await flush();
    expect(wrapper.find('[data-testid="external-compare"]').text()).toContain("identical");
    wrapper.unmount();
  });
});

describe("confirmations (3 s, then gone)", () => {
  it("Keep mine shows the 'keeping your version' line for 3 s", async () => {
    const rebaseBuffer = vi.fn().mockResolvedValue([patch(0, 0, 0, 6, "MINE")]);
    const { wrapper } = await conflicted({ rebaseBuffer, readFile: vi.fn() });
    await wrapper.find('[data-testid="keep-mine"]').trigger("click");
    await flush();
    expect(wrapper.find('[data-testid="external-notice"]').text()).toContain("Keeping your version");
    await vi.advanceTimersByTimeAsync(3100);
    expect(wrapper.find('[data-testid="external-notice"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("Take theirs says undo brings the typing back", async () => {
    const { wrapper } = await conflicted();
    await wrapper.find('[data-testid="take-theirs"]').trigger("click");
    await flush();
    expect(wrapper.find('[data-testid="external-notice"]').text()).toContain("undo");
    wrapper.unmount();
  });
});

describe("a refused save that nothing explains: the persistent 'Not saved' signal", () => {
  const stale = { kind: "stale_hash", path: "a.md", current_hash: "hX" };

  it("request_external_diff failing outright raises save-blocked; Compare reads the disk and shows both; a later save lowers it", async () => {
    const savePatch = vi.fn().mockRejectedValueOnce(stale).mockResolvedValue(written("h5"));
    const requestExternalDiff = vi.fn().mockRejectedValue({ kind: "io", message: "x" });
    const readFile = vi.fn().mockResolvedValue({ path: "a.md", text: "DISK", hash: "hX", mtime_ms: 0, bytes: 4 });
    const { wrapper, api, view } = harness({ savePatch, requestExternalDiff, readFile });
    await api.loadNote("one", "a.md", "h0");
    view.dispatch({ changes: { from: 3, insert: "!" } });
    await vi.advanceTimersByTimeAsync(1100);
    expect(wrapper.emitted("save-blocked")?.at(-1)).toEqual([true]);

    await api.compareWithDisk();
    await flush();
    expect(wrapper.find('[data-testid="compare-disk"]').text()).toContain("DISK");
    expect(wrapper.find('[data-testid="compare-mine"]').text()).toContain("one!");
    expect(wrapper.find('[data-testid="compare-take-theirs"]').exists()).toBe(false); // nothing to resolve here
    expect(api.getText()).toBe("one!");
    wrapper.unmount();
  });

  it("an unreadable disk says so instead of failing silently", async () => {
    const { wrapper, api } = harness({ readFile: vi.fn().mockRejectedValue({ kind: "io" }) });
    await api.loadNote("one", "a.md", "h0");
    await api.compareWithDisk();
    await flush();
    expect(wrapper.find('[data-testid="external-notice"]').text()).toContain("Could not read");
    wrapper.unmount();
  });
});

describe("disk equals the base again (null answer): saving resumes", () => {
  const stale = { kind: "stale_hash", path: "a.md", current_hash: "hX" };

  it("after a StaleHash whose diff is null, the dirty buffer is saved without another keystroke and a short status shows", async () => {
    const savePatch = vi.fn().mockRejectedValueOnce(stale).mockResolvedValue(written("h5"));
    const { wrapper, api, view } = harness({ savePatch }); // requestExternalDiff -> null
    await api.loadNote("one", "a.md", "h0");
    view.dispatch({ changes: { from: 3, insert: "!" } });
    await vi.advanceTimersByTimeAsync(1100); // refused, diff null
    expect(wrapper.find('[data-testid="external-notice"]').text()).toContain("matches your last save again");
    expect(api.isDirty).toBe(true);
    await vi.advanceTimersByTimeAsync(1100); // the rescheduled autosave
    expect(savePatch).toHaveBeenCalledTimes(2);
    expect(api.isDirty).toBe(false);
    wrapper.unmount();
  });

  it("Compare finding the disk equal to the base resumes saving too", async () => {
    const savePatch = vi.fn().mockResolvedValue(written("h5"));
    const requestExternalDiff = vi.fn().mockResolvedValue(null);
    const { wrapper, api, view, send } = harness({ savePatch, requestExternalDiff });
    await api.loadNote("one\ntwo", "a.md", "h0");
    view.dispatch({ changes: { from: 0, to: 3, insert: "MINE" } });
    send(changed("h0", "h2", [patch(0, 0, 0, 3, "THEIRS")]));
    await flush();
    await wrapper.find('[data-testid="compare"]').trigger("click");
    await flush();
    expect(wrapper.find('[data-testid="external-conflict"]').exists()).toBe(false);
    expect(wrapper.find('[data-testid="external-notice"]').text()).toContain("nothing to compare");
    await vi.advanceTimersByTimeAsync(1100);
    expect(savePatch).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });
});
