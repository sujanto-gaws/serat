// Tests ADR-0016 §2's save orchestration: at most one save in flight per
// note, coalesced triggers, and the base only ever moving on a successful
// save or an explicit rebase.

import { EditorState } from "@codemirror/state";
import { describe, expect, it, vi } from "vitest";

import { EditorBuffer } from "@/editor/buffer";
import type { EditorIpcAdapter } from "@/ipc/editorAdapter";
import type { FileWritten } from "@/ipc/generated/FileWritten";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function mockIpc(overrides: Partial<EditorIpcAdapter> = {}): EditorIpcAdapter {
  return {
    noteOpened: vi.fn().mockResolvedValue(undefined),
    noteClosed: vi.fn().mockResolvedValue(undefined),
    savePatch: vi.fn(),
    requestExternalDiff: vi.fn().mockResolvedValue(null),
    rebaseBuffer: vi.fn().mockResolvedValue([]),
    ...overrides,
  };
}

function changesFor(doc: string, from: number, to: number, insert: string) {
  const state = EditorState.create({ doc });
  return state.update({ changes: { from, to, insert } }).changes;
}

describe("EditorBuffer.save", () => {
  it("is a no-op when nothing is dirty", async () => {
    const buffer = new EditorBuffer("a.md", "hello", "hash0");
    const ipc = mockIpc();
    const outcome = await buffer.save(ipc);
    expect(outcome).toEqual({ kind: "noop" });
    expect(ipc.savePatch).not.toHaveBeenCalled();
  });

  it("sends exactly the pending patches and moves the base on success", async () => {
    const buffer = new EditorBuffer("a.md", "hello", "hash0");
    buffer.recordChange(changesFor("hello", 5, 5, " world"));
    const written: FileWritten = { path: "a.md", hash: "hash1", mtime_ms: 0, bytes: 11 };
    const savePatch = vi.fn().mockResolvedValue(written);
    const ipc = mockIpc({ savePatch });

    const outcome = await buffer.save(ipc);

    expect(savePatch).toHaveBeenCalledWith({
      path: "a.md",
      base_hash: "hash0",
      patches: [{ from: { line: 0, col: 5 }, to: { line: 0, col: 5 }, insert: " world" }],
    });
    expect(outcome).toEqual({ kind: "saved", written });
    expect(buffer.baseHash).toBe("hash1");
    expect(buffer.baseText.toString()).toBe("hello world");
    expect(buffer.isDirty).toBe(false);
  });

  it("never has two saves in flight, and a save during a save coalesces", async () => {
    const buffer = new EditorBuffer("a.md", "abc", "hash0");
    buffer.recordChange(changesFor("abc", 3, 3, "d"));

    const first = deferred<FileWritten>();
    const savePatch = vi.fn().mockReturnValueOnce(first.promise);
    const ipc = mockIpc({ savePatch });

    const p1 = buffer.save(ipc); // starts the real save
    // A second, third trigger while the first is in flight must not start a
    // second network call.
    const p2 = buffer.save(ipc);
    const p3 = buffer.save(ipc);

    expect(savePatch).toHaveBeenCalledTimes(1);
    await expect(p2).resolves.toEqual({ kind: "coalesced" });
    await expect(p3).resolves.toEqual({ kind: "coalesced" });

    // More typing arrives while the first save is still in flight.
    buffer.recordChange(changesFor("abcd", 4, 4, "e"));

    first.resolve({ path: "a.md", hash: "hash1", mtime_ms: 0, bytes: 4 });
    const outcome1 = await p1;
    expect(outcome1).toEqual({
      kind: "saved",
      written: { path: "a.md", hash: "hash1", mtime_ms: 0, bytes: 4 },
    });

    // The coalesced trigger fires exactly one more save, carrying the change
    // made while the first save was in flight, once the first resolves.
    // Give the fire-and-continue microtask a tick.
    await Promise.resolve();
    await Promise.resolve();
    expect(savePatch).toHaveBeenCalledTimes(2);
    expect(savePatch).toHaveBeenLastCalledWith({
      path: "a.md",
      base_hash: "hash1",
      patches: [{ from: { line: 0, col: 4 }, to: { line: 0, col: 4 }, insert: "e" }],
    });
  });

  it("leaves the buffer dirty and the base unmoved on a refused save", async () => {
    const buffer = new EditorBuffer("a.md", "abc", "hash0");
    buffer.recordChange(changesFor("abc", 3, 3, "d"));
    const error = { kind: "stale_hash", path: "a.md", current_hash: "hashX" };
    const savePatch = vi.fn().mockRejectedValue(error);
    const ipc = mockIpc({ savePatch });

    const outcome = await buffer.save(ipc);

    expect(outcome).toEqual({ kind: "stale", error });
    expect(buffer.baseHash).toBe("hash0");
    expect(buffer.isDirty).toBe(true);
  });

  it("only ever composes changes made since the base, across many transactions", async () => {
    const buffer = new EditorBuffer("a.md", "a", "hash0");
    buffer.recordChange(changesFor("a", 1, 1, "b"));
    buffer.recordChange(changesFor("ab", 2, 2, "c"));
    buffer.recordChange(changesFor("abc", 3, 3, "d"));
    expect(buffer.composedPending().apply(buffer.baseText).toString()).toBe("abcd");

    const written: FileWritten = { path: "a.md", hash: "hash1", mtime_ms: 0, bytes: 4 };
    const savePatch = vi.fn().mockResolvedValue(written);
    await buffer.save(mockIpc({ savePatch }));

    expect(savePatch).toHaveBeenCalledTimes(1);
    const call = savePatch.mock.calls[0][0];
    expect(call.patches).toHaveLength(1);
    expect(call.patches[0].insert).toBe("bcd");
  });
});

describe("EditorBuffer.rebaseOnto", () => {
  it("adopts the disk text and hash as the new base and clears pending changes by default", () => {
    const buffer = new EditorBuffer("a.md", "mine", "hash0");
    buffer.recordChange(changesFor("mine", 4, 4, "!"));
    expect(buffer.isDirty).toBe(true);

    buffer.rebaseOnto("theirs", "hashX");

    expect(buffer.baseHash).toBe("hashX");
    expect(buffer.baseText.toString()).toBe("theirs");
    expect(buffer.isDirty).toBe(false);
  });
});
