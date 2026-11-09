// Task #23: external-change reconciliation (ADR-0016 §2-§4; FR-VLT-05,
// NFR-REL-01, C-04). Pure planning first, then the component's behaviour with
// a mocked IPC: apply / rebase / conflict / hold / stale / delete.

import { ChangeSet, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import CodeMirrorEditor from "@/editor/CodeMirrorEditor.vue";
import { externalChangeSet, minimalReplacement, overlaps, planExternal } from "@/editor/reconcile";
import { textFromString } from "@/editor/patches";
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

describe("planExternal (pure)", () => {
  const base = textFromString("one\ntwo\nthree\nfour");
  const ext = (...edits: Patch[]) => externalChangeSet(base, edits);
  const localAt = (from: number, to: number, insert: string) =>
    EditorState.create({ doc: base }).update({ changes: { from, to, insert } }).changes;

  it("an empty edit script is hash-only", () => {
    expect(planExternal(ext(), null).kind).toBe("hash-only");
    expect(planExternal(ext(), localAt(0, 0, "x")).kind).toBe("hash-only");
  });

  it("no unsaved changes: apply clean", () => {
    expect(planExternal(ext(patch(0, 0, 0, 3, "ONE")), null).kind).toBe("apply-clean");
    expect(planExternal(ext(patch(0, 0, 0, 3, "ONE")), ChangeSet.empty(base.length)).kind).toBe("apply-clean");
  });

  it("disjoint ranges rebase both ways", () => {
    const plan = planExternal(ext(patch(0, 0, 0, 3, "ONE")), localAt(base.length, base.length, "!"));
    expect(plan.kind).toBe("apply-rebased");
    if (plan.kind !== "apply-rebased") return;
    // Live doc = base + local; external applies on it; both orders reach the same text.
    const live = localAt(base.length, base.length, "!").apply(base);
    const viaDoc = plan.externalOnDoc.apply(live).toString();
    const viaBase = plan.localRebased.apply(plan.external.apply(base)).toString();
    expect(viaDoc).toBe("ONE\ntwo\nthree\nfour!");
    expect(viaBase).toBe(viaDoc);
  });

  it("ranges that touch count as overlapping, as do identical insertion points", () => {
    expect(overlaps(localAt(3, 3, "x"), ext(patch(0, 3, 0, 3, "y")))).toBe(true); // same point
    expect(overlaps(localAt(0, 3, "x"), ext(patch(1, 0, 1, 3, "y")))).toBe(false); // line 0 vs line 1: gap of the separator
    expect(overlaps(localAt(0, 4, "x"), ext(patch(1, 0, 1, 3, "y")))).toBe(true); // touches at offset 4
    expect(planExternal(ext(patch(1, 0, 1, 3, "TWO")), localAt(2, 6, "x")).kind).toBe("conflict");
  });

  it("minimalReplacement trims to the changed middle and never splits a surrogate pair", () => {
    expect(minimalReplacement("abcXdef", "abcYYdef")).toEqual({ from: 3, to: 4, insert: "YY" });
    expect(minimalReplacement("same", "same")).toEqual({ from: 4, to: 4, insert: "" });
    const r = minimalReplacement("a\u{1F600}b", "a\u{1F601}b");
    expect(r.from).toBe(1);
    expect(r.to).toBe(3);
  });
});

// ---------------------------------------------------------------------------

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((res) => (resolve = res));
  return { promise, resolve };
}

function harness(overrides: Partial<EditorIpcAdapter> = {}, extraProps: { vaultId?: string | null } = {}) {
  let emit: (change: ExternalChange) => void = () => undefined;
  const ipc: EditorIpcAdapter = {
    noteOpened: vi.fn().mockResolvedValue(undefined),
    noteClosed: vi.fn().mockResolvedValue(undefined),
    savePatch: vi.fn(),
    requestExternalDiff: vi.fn().mockResolvedValue(null),
    rebaseBuffer: vi.fn().mockResolvedValue([]),
    readFile: vi.fn(),
    writeFile: vi.fn(),
    onClosing: vi.fn().mockResolvedValue(() => undefined),
    flushAck: vi.fn().mockResolvedValue(undefined),
    onExternalChange: vi.fn(async (handler: (c: ExternalChange) => void) => {
      emit = handler;
      return () => undefined;
    }),
    ...overrides,
  };
  const linkIpc: LinkIpcAdapter = {
    resolveLinks: vi.fn().mockResolvedValue([]),
    noteAnchors: vi.fn(),
    switcherQuery: vi.fn(),
    onIndexChanged: vi.fn().mockResolvedValue(() => undefined),
  };
  const wrapper = mount(CodeMirrorEditor, { props: { ipc, linkIpc, ...extraProps }, attachTo: document.body });
  const host = wrapper.find(".cm-editor-host").element as HTMLElement;
  const view = EditorView.findFromDOM(host.shadowRoot!.querySelector(".cm-content") as HTMLElement)!;
  const api = wrapper.vm as unknown as {
    loadNote(t: string, p: string, h: string): Promise<boolean>;
    getText(): string;
    triggerSave(): Promise<unknown>;
    undo(): unknown;
    isDirty: boolean;
  };
  return { ipc, wrapper, view, api, send: (c: ExternalChange) => emit(c) };
}

const changed = (base_hash: string, new_hash: string, edits: Patch[], whole = false): ExternalChange => ({
  kind: "changed",
  vault_id: "v",
  path: "a.md",
  base_hash,
  new_hash,
  edits,
  whole,
});
const written = (hash: string): FileWritten => ({ path: "a.md", hash, mtime_ms: 0, bytes: 0 });
const flush = () => vi.advanceTimersByTimeAsync(0);

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("reconciliation in the editor", () => {
  it("tells serat-app the note is open (so it emits external changes)", async () => {
    const { ipc, api, wrapper } = harness();
    await api.loadNote("hi", "a.md", "h0");
    expect(ipc.noteOpened).toHaveBeenCalledWith({ path: "a.md", base_hash: "h0" });
    wrapper.unmount();
  });

  it("no unsaved changes: applies the edit, keeps the cursor on its line, undo reaches earlier local states but not the external edit", async () => {
    const lines = Array.from({ length: 50 }, (_, i) => `line ${i + 1}`);
    const savePatch = vi.fn().mockResolvedValue(written("h1"));
    const { api, view, send, wrapper } = harness({ savePatch });
    await api.loadNote(lines.join("\n"), "a.md", "h0");

    // A local edit on line 40, saved: history keeps it, the buffer is clean.
    const line40 = view.state.doc.line(40);
    view.dispatch({ changes: { from: line40.to, insert: "!" }, selection: { anchor: line40.to + 1 } });
    await api.triggerSave();
    expect(api.isDirty).toBe(false);
    const cursorLineBefore = view.state.doc.lineAt(view.state.selection.main.head).number;
    expect(cursorLineBefore).toBe(40);

    // Someone inserts two lines at the top.
    send(changed("h1", "h2", [patch(0, 0, 0, 0, "new a\nnew b\n")]));
    await flush();

    expect(view.state.doc.line(1).text).toBe("new a");
    expect(view.state.doc.lineAt(view.state.selection.main.head).number).toBe(42); // moved with its text
    expect(api.isDirty).toBe(false); // an external edit is never "mine"

    // Undo reverts the local "!" and leaves the external edit alone.
    api.undo();
    expect(view.state.doc.line(1).text).toBe("new a");
    expect(view.state.doc.line(42).text).toBe("line 40");
    wrapper.unmount();
  });

  it("disjoint unsaved changes: both are kept, the buffer stays dirty, the next save patches the new base", async () => {
    const savePatch = vi.fn().mockResolvedValue(written("h3"));
    const { api, view, send, wrapper } = harness({ savePatch });
    await api.loadNote("one\ntwo\nthree", "a.md", "h0");
    view.dispatch({ changes: { from: view.state.doc.length, insert: "!" } }); // local, end of file
    send(changed("h0", "h2", [patch(0, 0, 0, 3, "ONE")])); // external, line 0
    await flush();

    expect(api.getText()).toBe("ONE\ntwo\nthree!");
    expect(api.isDirty).toBe(true);
    await vi.advanceTimersByTimeAsync(1100); // autosave
    expect(savePatch).toHaveBeenCalledWith({
      path: "a.md",
      base_hash: "h2",
      patches: [{ from: { line: 2, col: 5 }, to: { line: 2, col: 5 }, insert: "!" }],
    });
    wrapper.unmount();
  });

  it("an empty edit script with a new hash moves the base silently (CRLF flip)", async () => {
    const savePatch = vi.fn().mockResolvedValue(written("h3"));
    const { api, view, send, wrapper } = harness({ savePatch });
    await api.loadNote("one\ntwo", "a.md", "h0");
    send(changed("h0", "h9", []));
    await flush();
    expect(api.getText()).toBe("one\ntwo");
    expect(api.isDirty).toBe(false);
    view.dispatch({ changes: { from: 7, insert: "!" } });
    await vi.advanceTimersByTimeAsync(1100);
    expect(savePatch.mock.calls[0][0].base_hash).toBe("h9");
    wrapper.unmount();
  });

  describe("conflict", () => {
    async function conflicted(overrides: Partial<EditorIpcAdapter> = {}) {
      const h = harness(overrides);
      await h.api.loadNote("one\ntwo\nthree", "a.md", "h0");
      h.view.dispatch({ changes: { from: 0, to: 3, insert: "MINE" } }); // local edit on line 0
      h.send(changed("h0", "h2", [patch(0, 0, 0, 3, "THEIRS")])); // overlapping external edit
      await flush();
      return h;
    }

    it("applies nothing, keeps the buffer, and shows the three-action notice", async () => {
      const { api, wrapper } = await conflicted();
      expect(api.getText()).toBe("MINE\ntwo\nthree");
      expect(api.isDirty).toBe(true);
      expect(wrapper.find('[data-testid="external-conflict"]').exists()).toBe(true);
      expect(wrapper.find('[data-testid="keep-mine"]').exists()).toBe(true);
      expect(wrapper.find('[data-testid="take-theirs"]').exists()).toBe(true);
      expect(wrapper.find('[data-testid="compare"]').exists()).toBe(true);
      wrapper.unmount();
    });

    it("Keep mine re-bases through rebase_buffer; the next save patches disk into the buffer", async () => {
      const rebaseBuffer = vi.fn().mockResolvedValue([patch(0, 0, 0, 6, "MINE")]);
      const savePatch = vi.fn().mockResolvedValue(written("h3"));
      const { api, wrapper } = await conflicted({ rebaseBuffer, savePatch });
      await wrapper.find('[data-testid="keep-mine"]').trigger("click");
      await flush();

      expect(rebaseBuffer).toHaveBeenCalledWith({ path: "a.md", disk_hash: "h2", buffer: "MINE\ntwo\nthree" });
      expect(api.getText()).toBe("MINE\ntwo\nthree"); // the user's text is untouched
      expect(wrapper.find('[data-testid="external-conflict"]').exists()).toBe(false);
      await vi.advanceTimersByTimeAsync(1100);
      expect(savePatch).toHaveBeenCalledWith({
        path: "a.md",
        base_hash: "h2",
        patches: [patch(0, 0, 0, 6, "MINE")],
      });
      wrapper.unmount();
    });

    it("Take theirs replaces the buffer in a history-recorded transaction; undo brings back what was typed", async () => {
      const { api, wrapper } = await conflicted();
      await wrapper.find('[data-testid="take-theirs"]').trigger("click");
      await flush();
      expect(api.getText()).toBe("THEIRS\ntwo\nthree");
      expect(api.isDirty).toBe(false);
      api.undo();
      expect(api.getText()).toBe("MINE\ntwo\nthree"); // nothing discarded silently
      wrapper.unmount();
    });

    it("Compare hands both texts to the host and changes nothing", async () => {
      const { api, wrapper } = await conflicted({
        requestExternalDiff: vi.fn().mockResolvedValue(changed("h0", "h2", [patch(0, 0, 0, 3, "THEIRS")])),
      });
      await wrapper.find('[data-testid="compare"]').trigger("click");
      await flush();
      expect(wrapper.emitted("compare-external")?.[0][0]).toEqual({
        path: "a.md",
        mine: "MINE\ntwo\nthree",
        disk: "THEIRS\ntwo\nthree",
      });
      expect(api.getText()).toBe("MINE\ntwo\nthree");
      wrapper.unmount();
    });
  });

  describe("ordering (ADR-0016 §2)", () => {
    it("a change whose base_hash is not the current base is dropped and a fresh diff is requested and applied", async () => {
      const fresh = changed("h0", "h5", [patch(0, 0, 0, 3, "ONE")]);
      const requestExternalDiff = vi.fn().mockResolvedValue(fresh);
      const { api, ipc, send, wrapper } = harness({ requestExternalDiff });
      await api.loadNote("one\ntwo", "a.md", "h0");
      send(changed("hOLD", "h4", [patch(1, 0, 1, 3, "TWO")])); // not against our base
      await flush();
      expect(ipc.requestExternalDiff).toHaveBeenCalledWith({ path: "a.md", base_hash: "h0" });
      expect(api.getText()).toBe("ONE\ntwo"); // the fresh diff was applied, the stale one was not
      wrapper.unmount();
    });

    it("an event arriving while a save is in flight is held, then applied if its base_hash matches the new base", async () => {
      const gate = deferred<FileWritten>();
      const savePatch = vi.fn().mockReturnValueOnce(gate.promise);
      const { api, view, send, wrapper } = harness({ savePatch });
      await api.loadNote("one\ntwo\nthree", "a.md", "h0");
      view.dispatch({ changes: { from: 13, insert: "!" } });
      void api.triggerSave();
      await flush();

      send(changed("h1", "h2", [patch(0, 0, 0, 3, "ONE")])); // based on what the save will produce
      await flush();
      expect(api.getText()).toBe("one\ntwo\nthree!"); // held, not applied

      gate.resolve(written("h1"));
      await flush();
      await flush();
      expect(api.getText()).toBe("ONE\ntwo\nthree!");
      wrapper.unmount();
    });

    it("a held change whose base no longer matches is dropped and a diff is requested", async () => {
      const gate = deferred<FileWritten>();
      const savePatch = vi.fn().mockReturnValueOnce(gate.promise);
      const requestExternalDiff = vi.fn().mockResolvedValue(null);
      const { api, view, send, ipc, wrapper } = harness({ savePatch, requestExternalDiff });
      await api.loadNote("one", "a.md", "h0");
      view.dispatch({ changes: { from: 3, insert: "!" } });
      void api.triggerSave();
      await flush();
      send(changed("h0", "h2", [patch(0, 0, 0, 3, "ONE")])); // against the OLD base: stale once the save lands
      gate.resolve(written("h1"));
      await flush();
      await flush();
      expect(api.getText()).toBe("one!");
      expect(ipc.requestExternalDiff).toHaveBeenCalledWith({ path: "a.md", base_hash: "h1" });
      wrapper.unmount();
    });

    it("a StaleHash save immediately requests the diff and, if it merges, the buffer reconciles", async () => {
      const stale = { kind: "stale_hash", path: "a.md", current_hash: "h7" };
      const savePatch = vi.fn().mockRejectedValueOnce(stale).mockResolvedValue(written("h8"));
      const requestExternalDiff = vi.fn().mockResolvedValue(changed("h0", "h7", [patch(0, 0, 0, 3, "ONE")]));
      const { api, view, ipc, wrapper } = harness({ savePatch, requestExternalDiff });
      await api.loadNote("one\ntwo", "a.md", "h0");
      view.dispatch({ changes: { from: 7, insert: "!" } });
      await vi.advanceTimersByTimeAsync(1100);
      expect(ipc.requestExternalDiff).toHaveBeenCalledWith({ path: "a.md", base_hash: "h0" });
      expect(api.getText()).toBe("ONE\ntwo!");
      // Not retried blindly by the stale state, but the next trigger saves against the new base.
      await vi.advanceTimersByTimeAsync(1100);
      expect(savePatch).toHaveBeenCalledTimes(2);
      expect(savePatch.mock.calls[1][0].base_hash).toBe("h7");
      wrapper.unmount();
    });

    it("StaleHash with no base bytes held by serat-app is a conflict; Keep mine reads and verifies the disk", async () => {
      const stale = { kind: "stale_hash", path: "a.md", current_hash: "h7" };
      const savePatch = vi.fn().mockRejectedValueOnce(stale);
      const requestExternalDiff = vi.fn().mockRejectedValue(stale);
      const readFile = vi.fn().mockResolvedValue({ path: "a.md", text: "DISK", hash: "h7", mtime_ms: 0, bytes: 4 });
      const rebaseBuffer = vi.fn().mockResolvedValue([patch(0, 0, 0, 4, "mine!")]);
      const { api, view, wrapper } = harness({ savePatch, requestExternalDiff, readFile, rebaseBuffer });
      await api.loadNote("mine", "a.md", "h0");
      view.dispatch({ changes: { from: 4, insert: "!" } });
      await vi.advanceTimersByTimeAsync(1100);
      expect(wrapper.find('[data-testid="external-conflict"]').exists()).toBe(true);
      expect(api.getText()).toBe("mine!");

      await wrapper.find('[data-testid="keep-mine"]').trigger("click");
      await flush();
      expect(readFile).toHaveBeenCalledWith({ path: "a.md" });
      expect(rebaseBuffer).toHaveBeenCalledWith({ path: "a.md", disk_hash: "h7", buffer: "mine!" });
      wrapper.unmount();
    });
  });

  describe("deleted on disk", () => {
    it("keeps the buffer, never recreates on idle, and the save command writes it with base_hash null", async () => {
      const savePatch = vi.fn();
      const writeFile = vi.fn().mockResolvedValue(written("hNew"));
      const { api, view, send, wrapper } = harness({ savePatch, writeFile });
      await api.loadNote("keep me", "a.md", "h0");
      send({ kind: "deleted", vault_id: "v", path: "a.md" });
      await flush();
      expect(wrapper.find('[data-testid="external-notice"]').text()).toContain("deleted on disk");

      view.dispatch({ changes: { from: 7, insert: "!" } });
      await vi.advanceTimersByTimeAsync(2000);
      expect(savePatch).not.toHaveBeenCalled();
      expect(writeFile).not.toHaveBeenCalled();

      await api.triggerSave();
      expect(writeFile).toHaveBeenCalledWith({ path: "a.md", text: "keep me!", base_hash: null });
      expect(wrapper.find('[data-testid="external-notice"]').exists()).toBe(false);
      expect(api.isDirty).toBe(false);
      wrapper.unmount();
    });
  });

  it("ignores changes for another note", async () => {
    const { api, send, wrapper } = harness();
    await api.loadNote("one", "a.md", "h0");
    send({ ...(changed("h0", "h1", [patch(0, 0, 0, 3, "X")]) as object), path: "other.md" } as ExternalChange);
    await flush();
    expect(api.getText()).toBe("one");
    wrapper.unmount();
  });
});

// Task #23 audit: base-hash handling, Compare, vault scoping, the quiet merge signal.
describe("external-change audit (FR-VLT-05, NFR-REL-01, C-04)", () => {
  it("clean buffer: reloads silently and the next save's base hash is the new on-disk hash", async () => {
    const savePatch = vi.fn().mockResolvedValue(written("h3"));
    const { api, view, send, wrapper } = harness({ savePatch });
    await api.loadNote("one\ntwo", "a.md", "h0");
    send(changed("h0", "h2", [patch(0, 0, 0, 3, "ONE")]));
    await flush();
    expect(api.getText()).toBe("ONE\ntwo");
    expect(api.isDirty).toBe(false);
    expect(wrapper.find('[data-testid="external-conflict"]').exists()).toBe(false);
    expect(wrapper.find('[data-testid="external-notice"]').exists()).toBe(false);
    expect(wrapper.emitted("external-merged")).toBeUndefined(); // a clean reload is not announced
    view.dispatch({ changes: { from: 7, insert: "!" } });
    await vi.advanceTimersByTimeAsync(1100);
    expect(savePatch.mock.calls[0][0].base_hash).toBe("h2");
    wrapper.unmount();
  });

  it("a FileWritten with mtime_ms 0 (unknown) is never taken as 'unchanged': hashes alone decide, so the next external change still applies", async () => {
    const savePatch = vi.fn().mockResolvedValue({ path: "a.md", hash: "h1", mtime_ms: 0, bytes: 3 });
    const { api, view, send, wrapper } = harness({ savePatch });
    await api.loadNote("one\ntwo", "a.md", "h0");
    view.dispatch({ changes: { from: 7, insert: "!" } });
    await api.triggerSave();
    expect(api.isDirty).toBe(false);
    send(changed("h1", "h2", [patch(0, 0, 0, 3, "ONE")])); // against the write's own hash
    await flush();
    expect(api.getText()).toBe("ONE\ntwo!");
    wrapper.unmount();
  });

  it("dirty and disjoint: merges and tells the host once (the 'Updated externally' strip), no conflict notice", async () => {
    const { api, view, send, wrapper } = harness();
    await api.loadNote("one\ntwo\nthree", "a.md", "h0");
    view.dispatch({ changes: { from: view.state.doc.length, insert: "!" } });
    send(changed("h0", "h2", [patch(0, 0, 0, 3, "ONE")]));
    await flush();
    expect(api.getText()).toBe("ONE\ntwo\nthree!");
    expect(wrapper.emitted("external-merged")).toEqual([[{ path: "a.md" }]]);
    expect(wrapper.find('[data-testid="external-conflict"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("an external change or deletion scoped to another vault is discarded", async () => {
    const { api, send, wrapper } = harness({}, { vaultId: "vA" });
    await api.loadNote("one", "a.md", "h0");
    send({ ...(changed("h0", "h1", [patch(0, 0, 0, 3, "X")]) as object), vault_id: "vB" } as ExternalChange);
    send({ kind: "deleted", vault_id: "vB", path: "a.md" });
    await flush();
    expect(api.getText()).toBe("one");
    expect(wrapper.find('[data-testid="external-notice"]').exists()).toBe(false);
    send({ ...(changed("h0", "h1", [patch(0, 0, 0, 3, "X")]) as object), vault_id: "vA" } as ExternalChange);
    await flush();
    expect(api.getText()).toBe("X");
    wrapper.unmount();
  });

  describe("Compare asks serat-app for the current diff (requestExternalDiff)", () => {
    async function conflicted(overrides: Partial<EditorIpcAdapter>) {
      const h = harness(overrides);
      await h.api.loadNote("one\ntwo\nthree", "a.md", "h0");
      h.view.dispatch({ changes: { from: 0, to: 3, insert: "MINE" } });
      h.send(changed("h0", "h2", [patch(0, 0, 0, 3, "THEIRS")]));
      await flush();
      return h;
    }

    it("calls requestExternalDiff with the current base and compares against the FRESH disk text", async () => {
      const requestExternalDiff = vi.fn().mockResolvedValue(changed("h0", "h5", [patch(0, 0, 0, 3, "NEWER")]));
      const { wrapper, api } = await conflicted({ requestExternalDiff });
      await wrapper.find('[data-testid="compare"]').trigger("click");
      await flush();
      expect(requestExternalDiff).toHaveBeenCalledWith({ path: "a.md", base_hash: "h0" });
      expect(wrapper.emitted("compare-external")?.[0][0]).toEqual({ path: "a.md", mine: "MINE\ntwo\nthree", disk: "NEWER\ntwo\nthree" });
      expect(api.getText()).toBe("MINE\ntwo\nthree");
      wrapper.unmount();
    });

    it("a stale_hash answer (no base bytes held) falls back to reading the file; nothing is applied", async () => {
      const requestExternalDiff = vi.fn().mockRejectedValue({ kind: "stale_hash", path: "a.md", current_hash: "h7" });
      const readFile = vi.fn().mockResolvedValue({ path: "a.md", text: "DISK", hash: "h7", mtime_ms: 0, bytes: 4 });
      const { wrapper, api } = await conflicted({ requestExternalDiff, readFile });
      await wrapper.find('[data-testid="compare"]').trigger("click");
      await flush();
      expect(wrapper.emitted("compare-external")?.[0][0]).toEqual({ path: "a.md", mine: "MINE\ntwo\nthree", disk: "DISK" });
      expect(api.getText()).toBe("MINE\ntwo\nthree");
      wrapper.unmount();
    });

    it("a failed request keeps the conflict we have and still compares", async () => {
      const requestExternalDiff = vi.fn().mockRejectedValue({ kind: "io", message: "x" });
      const { wrapper } = await conflicted({ requestExternalDiff });
      await wrapper.find('[data-testid="compare"]').trigger("click");
      await flush();
      expect(wrapper.emitted("compare-external")?.[0][0]).toMatchObject({ disk: "THEIRS\ntwo\nthree" });
      wrapper.unmount();
    });

    it("if the disk equals the base again there is nothing to compare: the conflict resolves, the buffer is untouched", async () => {
      const requestExternalDiff = vi.fn().mockResolvedValue(null);
      const { wrapper, api } = await conflicted({ requestExternalDiff });
      await wrapper.find('[data-testid="compare"]').trigger("click");
      await flush();
      expect(wrapper.emitted("compare-external")).toBeUndefined();
      expect(wrapper.find('[data-testid="external-conflict"]').exists()).toBe(false);
      expect(api.getText()).toBe("MINE\ntwo\nthree");
      expect(api.isDirty).toBe(true);
      wrapper.unmount();
    });
  });
});
