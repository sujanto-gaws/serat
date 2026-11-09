// Tasks #22/#23, editor half of #63: the clean close (NFR-REL-01, FR-EDT-01;
// architecture.md 3.1.1 item 6; contracts/ipc.rs `AppClosing`, `FlushAckArgs`).
// On `app://closing` the editor submits its pending patch set with `save_patch`
// and then calls `flush_ack`, exactly once, in every branch: clean, dirty,
// refused (StaleHash, never re-serialised: C-04), failed, too slow, wrong vault.

import { EditorView } from "@codemirror/view";
import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import CodeMirrorEditor from "@/editor/CodeMirrorEditor.vue";
import type { EditorIpcAdapter } from "@/ipc/editorAdapter";
import type { AppClosing } from "@/ipc/generated/AppClosing";
import type { ExternalChange } from "@/ipc/generated/ExternalChange";
import type { FileWritten } from "@/ipc/generated/FileWritten";
import type { LinkIpcAdapter } from "@/ipc/linkAdapter";

if (!Range.prototype.getClientRects) {
  Range.prototype.getClientRects = () =>
    ({ length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] }) as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 0, 0);
}

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const written = (hash: string): FileWritten => ({ path: "a.md", hash, mtime_ms: 0, bytes: 0 });
const flush = () => vi.advanceTimersByTimeAsync(0);
const closing = (vault_id = "vA", deadline_ms = 2000): AppClosing => ({ vault_id, deadline_ms });

function harness(overrides: Partial<EditorIpcAdapter> = {}, props: { vaultId?: string | null } = {}) {
  let fire: (c: AppClosing) => void = () => undefined;
  const unlisten = vi.fn();
  const order: string[] = [];
  const ipc: EditorIpcAdapter = {
    noteOpened: vi.fn().mockResolvedValue(undefined),
    noteClosed: vi.fn().mockResolvedValue(undefined),
    savePatch: vi.fn(async () => {
      order.push("save");
      return written("h1");
    }),
    requestExternalDiff: vi.fn().mockResolvedValue(null),
    rebaseBuffer: vi.fn().mockResolvedValue([]),
    readFile: vi.fn(),
    writeFile: vi.fn(),
    onExternalChange: vi.fn(async (_h: (c: ExternalChange) => void) => () => undefined),
    onClosing: vi.fn(async (handler: (c: AppClosing) => void) => {
      fire = handler;
      return unlisten;
    }),
    flushAck: vi.fn(async () => {
      order.push("ack");
    }),
    ...overrides,
  };
  const linkIpc: LinkIpcAdapter = {
    resolveLinks: vi.fn().mockResolvedValue([]),
    noteAnchors: vi.fn(),
    switcherQuery: vi.fn(),
    onIndexChanged: vi.fn().mockResolvedValue(() => undefined),
  };
  const wrapper = mount(CodeMirrorEditor, { props: { ipc, linkIpc, ...props }, attachTo: document.body });
  const host = wrapper.find(".cm-editor-host").element as HTMLElement;
  const view = EditorView.findFromDOM(host.shadowRoot!.querySelector(".cm-content") as HTMLElement)!;
  const api = wrapper.vm as unknown as {
    loadNote(t: string, p: string, h: string): Promise<boolean>;
    getText(): string;
    isDirty: boolean;
  };
  return { ipc, wrapper, view, api, order, unlisten, fire: (c: AppClosing) => fire(c) };
}

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("app://closing: flush, then flush_ack exactly once", () => {
  it("subscribes at mount and unsubscribes at unmount", async () => {
    const { ipc, wrapper, unlisten } = harness();
    await flush();
    expect(ipc.onClosing).toHaveBeenCalledTimes(1);
    wrapper.unmount();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("dirty: submits the pending patch set with save_patch, THEN acks, echoing the vault_id", async () => {
    const { ipc, api, view, order, fire, wrapper } = harness();
    await api.loadNote("one\ntwo", "a.md", "h0");
    view.dispatch({ changes: { from: 7, insert: "!" } });
    fire(closing("vA"));
    await flush();
    expect(ipc.savePatch).toHaveBeenCalledTimes(1);
    expect(ipc.savePatch).toHaveBeenCalledWith({
      path: "a.md",
      base_hash: "h0",
      patches: [{ from: { line: 1, col: 3 }, to: { line: 1, col: 3 }, insert: "!" }],
    });
    expect(ipc.flushAck).toHaveBeenCalledTimes(1);
    expect(ipc.flushAck).toHaveBeenCalledWith({ vault_id: "vA" });
    expect(order).toEqual(["save", "ack"]);
    expect(api.isDirty).toBe(false);
    wrapper.unmount();
  });

  it("does not wait for the 1 s idle debounce, and the cancelled debounce does not save a second time", async () => {
    const { ipc, api, view, fire, wrapper } = harness();
    await api.loadNote("one", "a.md", "h0");
    view.dispatch({ changes: { from: 3, insert: "!" } }); // arms the idle timer
    fire(closing());
    await flush();
    expect(ipc.savePatch).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(5000);
    expect(ipc.savePatch).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("nothing pending: acks at once without saving", async () => {
    const { ipc, api, fire, wrapper } = harness();
    await api.loadNote("one", "a.md", "h0");
    fire(closing());
    await flush();
    expect(ipc.savePatch).not.toHaveBeenCalled();
    expect(ipc.flushAck).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("no note open: acks at once", async () => {
    const { ipc, fire, wrapper } = harness();
    fire(closing());
    await flush();
    expect(ipc.savePatch).not.toHaveBeenCalled();
    expect(ipc.flushAck).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("a duplicate event while the flush is running does not save or ack twice", async () => {
    const gate = deferred<FileWritten>();
    const { ipc, api, view, fire, wrapper } = harness({ savePatch: vi.fn().mockReturnValue(gate.promise) });
    await api.loadNote("one", "a.md", "h0");
    view.dispatch({ changes: { from: 3, insert: "!" } });
    fire(closing());
    fire(closing());
    await flush();
    expect(ipc.flushAck).not.toHaveBeenCalled(); // not before the save resolves
    gate.resolve(written("h1"));
    await flush();
    expect(ipc.savePatch).toHaveBeenCalledTimes(1);
    expect(ipc.flushAck).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("a save already in flight is awaited first, then the coalesced remainder is issued, then the ack", async () => {
    const first = deferred<FileWritten>();
    const savePatch = vi
      .fn()
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce(written("h2"));
    const { ipc, api, view, order, fire, wrapper } = harness({ savePatch });
    await api.loadNote("one", "a.md", "h0");
    view.dispatch({ changes: { from: 3, insert: "!" } });
    await vi.advanceTimersByTimeAsync(1100); // idle save starts and hangs
    view.dispatch({ changes: { from: 4, insert: "?" } }); // typed meanwhile
    fire(closing());
    await flush();
    expect(ipc.flushAck).not.toHaveBeenCalled();
    first.resolve(written("h1"));
    await flush();
    expect(savePatch).toHaveBeenCalledTimes(2);
    expect(savePatch.mock.calls[1][0].base_hash).toBe("h1"); // the base the first save produced
    expect(ipc.flushAck).toHaveBeenCalledTimes(1);
    expect(api.isDirty).toBe(false);
    expect(order.at(-1)).toBe("ack");
    wrapper.unmount();
  });

  it("StaleHash: acks anyway, keeps the buffer dirty and intact, and never re-serialises or recreates the note (C-04)", async () => {
    const savePatch = vi.fn().mockRejectedValue({ kind: "stale_hash", path: "a.md", current_hash: "hX" });
    const { ipc, api, view, fire, wrapper } = harness({ savePatch });
    await api.loadNote("one", "a.md", "h0");
    view.dispatch({ changes: { from: 3, insert: "!" } });
    fire(closing());
    await flush();
    expect(savePatch).toHaveBeenCalledTimes(1);
    expect(ipc.flushAck).toHaveBeenCalledTimes(1);
    expect(ipc.writeFile).not.toHaveBeenCalled();
    expect(api.getText()).toBe("one!");
    expect(api.isDirty).toBe(true);
    wrapper.unmount();
  });

  it("a refused save (line endings would fuse) and a failed save both still ack, once each, with the buffer intact", async () => {
    for (const error of [{ kind: "line_endings_would_fuse" }, { kind: "io", message: "disk full" }, new Error("transport")]) {
      const savePatch = vi.fn().mockRejectedValue(error);
      const { ipc, api, view, fire, wrapper } = harness({ savePatch });
      await api.loadNote("one", "a.md", "h0");
      view.dispatch({ changes: { from: 3, insert: "!" } });
      fire(closing());
      await flush();
      expect(ipc.flushAck).toHaveBeenCalledTimes(1);
      expect(ipc.writeFile).not.toHaveBeenCalled();
      expect(api.getText()).toBe("one!");
      expect(api.isDirty).toBe(true);
      wrapper.unmount();
    }
  });

  it("a save that outlasts the deadline: the ack is still sent before the core gives up, and only once", async () => {
    const gate = deferred<FileWritten>();
    const { ipc, api, view, fire, wrapper } = harness({ savePatch: vi.fn().mockReturnValue(gate.promise) });
    await api.loadNote("one", "a.md", "h0");
    view.dispatch({ changes: { from: 3, insert: "!" } });
    fire(closing("vA", 2000));
    await vi.advanceTimersByTimeAsync(1800);
    expect(ipc.flushAck).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(200);
    expect(ipc.flushAck).toHaveBeenCalledTimes(1);
    gate.resolve(written("h1")); // late: must not produce a second ack
    await vi.advanceTimersByTimeAsync(5000);
    expect(ipc.flushAck).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("a flush_ack that rejects is swallowed (never fails by contract; the core's bound is the backstop)", async () => {
    const { ipc, api, fire, wrapper } = harness({ flushAck: vi.fn().mockRejectedValue(new Error("gone")) });
    await api.loadNote("one", "a.md", "h0");
    fire(closing());
    await flush();
    expect(ipc.flushAck).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("a note deleted on disk is not recreated by a close: ack only", async () => {
    let send: (c: ExternalChange) => void = () => undefined;
    const { ipc, api, view, fire, wrapper } = harness({
      onExternalChange: vi.fn(async (h: (c: ExternalChange) => void) => {
        send = h;
        return () => undefined;
      }),
    });
    await api.loadNote("keep", "a.md", "h0");
    send({ kind: "deleted", vault_id: "vA", path: "a.md" });
    await flush();
    view.dispatch({ changes: { from: 4, insert: "!" } });
    fire(closing());
    await flush();
    expect(ipc.savePatch).not.toHaveBeenCalled();
    expect(ipc.writeFile).not.toHaveBeenCalled();
    expect(ipc.flushAck).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });
});

describe("app://closing: vault ids are checked", () => {
  it("an event for another vault is ignored: no save, no ack", async () => {
    const { ipc, api, view, fire, wrapper } = harness({}, { vaultId: "vA" });
    await api.loadNote("one", "a.md", "h0");
    view.dispatch({ changes: { from: 3, insert: "!" } });
    fire(closing("vB"));
    await flush();
    expect(ipc.savePatch).not.toHaveBeenCalled();
    expect(ipc.flushAck).not.toHaveBeenCalled();
    fire(closing("vA")); // ours still works afterwards
    await flush();
    expect(ipc.flushAck).toHaveBeenCalledWith({ vault_id: "vA" });
    wrapper.unmount();
  });

  it("a vault switch: the OLD vault's event reaches the buffer that belongs to it, an ack for the new vault is never sent", async () => {
    const { ipc, api, view, fire, wrapper } = harness({}, { vaultId: "vA" });
    await api.loadNote("one", "a.md", "h0");
    view.dispatch({ changes: { from: 3, insert: "!" } });
    await wrapper.setProps({ vaultId: "vB" }); // the view has already moved on to B
    fire(closing("vB"));
    await flush();
    expect(ipc.flushAck).not.toHaveBeenCalled();
    fire(closing("vA"));
    await flush();
    expect(ipc.savePatch).toHaveBeenCalledTimes(1);
    expect(ipc.flushAck).toHaveBeenCalledTimes(1);
    expect(ipc.flushAck).toHaveBeenCalledWith({ vault_id: "vA" });
    wrapper.unmount();
  });

  it("a buffer opened before the vault id was known adopts the first id learned", async () => {
    const { ipc, api, fire, wrapper } = harness();
    await api.loadNote("one", "a.md", "h0");
    await wrapper.setProps({ vaultId: "vA" });
    fire(closing("vB"));
    await flush();
    expect(ipc.flushAck).not.toHaveBeenCalled();
    fire(closing("vA"));
    await flush();
    expect(ipc.flushAck).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });
});
