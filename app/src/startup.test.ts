import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useVaultStore } from "@/stores/vault";

const markFirstPaint = vi.fn();
const markVaultUsable = vi.fn();
const readFile = vi.fn();

vi.mock("@/ipc/client", () => ({
  markFirstPaint: (...args: unknown[]) => markFirstPaint(...args),
  markVaultUsable: (...args: unknown[]) => markVaultUsable(...args),
  readFile: (...args: unknown[]) => readFile(...args),
}));

// Imported after the mock so startup.ts resolves the mocked module.
const { hydrateBuffer, schedulePaintMark } = await import("./startup");

/**
 * A double `requestAnimationFrame`, faked with microtasks rather than real frames
 * so the test does not depend on a browser's paint scheduler. What matters for this
 * test is only that the mark is not taken synchronously — the same property
 * crates/serat-app/src/startup.rs's `mark_first_paint` test asserts on the Rust
 * side ("Nothing here is a product feature" — this is purely a scheduling contract).
 */
function stubDoubleRaf() {
  let calls = 0;
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    calls += 1;
    queueMicrotask(() => cb(0));
    return calls;
  });
}

beforeEach(() => {
  setActivePinia(createPinia());
  stubDoubleRaf();
  markFirstPaint.mockReset();
  markVaultUsable.mockReset();
  readFile.mockReset();
  window.__SERAT_BOOT__ = undefined;
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("schedulePaintMark", () => {
  it("does not call mark_first_paint synchronously", () => {
    markFirstPaint.mockResolvedValue(12.5);
    const store = useVaultStore();
    schedulePaintMark(store);
    expect(markFirstPaint).not.toHaveBeenCalled();
  });

  it("resolves with the mark and records it on the store", async () => {
    markFirstPaint.mockResolvedValue(12.5);
    const store = useVaultStore();
    const ms = await schedulePaintMark(store);
    expect(ms).toBe(12.5);
    expect(store.marksText).toBe("painted 12.5 ms");
  });

  it("resolves with null and records the failure rather than throwing", async () => {
    markFirstPaint.mockRejectedValue({ kind: "io", message: "boom" });
    const store = useVaultStore();
    const ms = await schedulePaintMark(store);
    expect(ms).toBeNull();
    expect(store.marksText).toContain("paint mark failed");
  });
});

describe("hydrateBuffer", () => {
  it("with nothing to open, waits for the paint mark before calling mark_vault_usable(null)", async () => {
    window.__SERAT_BOOT__ = { vault: "/vault", open: null, exit_after_ready: false };
    markVaultUsable.mockResolvedValue(20);

    // A paint mark that has not resolved yet: the ordering assertion below only
    // means something if hydrateBuffer is actually made to wait for it.
    let resolvePaint!: (ms: number | null) => void;
    const paintMark = new Promise<number | null>((resolve) => {
      resolvePaint = resolve;
    });

    const editor = { setText: vi.fn() };
    const store = useVaultStore();
    const done = hydrateBuffer(store, editor, paintMark);

    await Promise.resolve();
    await Promise.resolve();
    expect(markVaultUsable).not.toHaveBeenCalled();

    resolvePaint(9.0);
    await done;

    expect(markVaultUsable).toHaveBeenCalledWith(null);
    expect(store.marksText).toContain("usable 20.0 ms");
  });

  it("reads the boot note into the editor and reports it as the usable note", async () => {
    window.__SERAT_BOOT__ = { vault: "/vault", open: "a.md", exit_after_ready: false };
    readFile.mockResolvedValue({
      path: "a.md",
      text: "# hello",
      hash: "deadbeef",
      mtime_ms: 0,
      bytes: 7,
    });
    markVaultUsable.mockResolvedValue(31);

    const editor = { setText: vi.fn() };
    const store = useVaultStore();
    await hydrateBuffer(store, editor, Promise.resolve(9.0));

    expect(readFile).toHaveBeenCalledWith({ path: "a.md" });
    expect(editor.setText).toHaveBeenCalledWith("# hello");
    expect(store.openNote).toBe("a.md");
    expect(store.baseHash).toBe("deadbeef");
    expect(markVaultUsable).toHaveBeenCalledWith("a.md");
  });

  it("shows a refused read as text in the buffer rather than a modal (§9.1)", async () => {
    window.__SERAT_BOOT__ = { vault: "/vault", open: "missing.md", exit_after_ready: false };
    readFile.mockRejectedValue({ kind: "not_found", path: "missing.md" });
    markVaultUsable.mockResolvedValue(5);

    const editor = { setText: vi.fn() };
    const store = useVaultStore();
    await hydrateBuffer(store, editor, Promise.resolve(1.0));

    expect(editor.setText).toHaveBeenCalledWith(expect.stringContaining("missing.md"));
    expect(store.openNote).toBeNull();
    // Still reported usable: an unreadable note is not a blocked shell (§9.1).
    expect(markVaultUsable).toHaveBeenCalledWith(null);
  });
});
