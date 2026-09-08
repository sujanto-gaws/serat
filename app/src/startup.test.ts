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
 * so the test does not depend on a browser's paint scheduler. Each call gets a
 * distinct, increasing timestamp (10, 20, 30, ...) so `schedulePaintMark`'s tests can
 * tell `raf1_ms` and `raf2_ms` apart rather than both landing on the same stub value
 * — the same property crates/serat-app/src/startup.rs's `mark_first_paint` test
 * asserts on the Rust side ("Nothing here is a product feature" — this is purely a
 * scheduling contract).
 */
function stubDoubleRaf() {
  let calls = 0;
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    calls += 1;
    queueMicrotask(() => cb(calls * 10));
    return calls;
  });
}

/** The shape `markFirstPaint` is called with — matches `PaintEvidence`, kept local so this file does not import a type only to satisfy a cast. */
interface PaintEvidenceForTest {
  firstContentfulPaintMs: number | null;
  raf1Ms: number;
  raf2Ms: number;
  navDomContentLoadedMs: number | null;
}

/**
 * `schedulePaintMark` reads two different `performance.getEntriesByType` calls —
 * `"paint"` for `firstContentfulPaintMs` and `"navigation"` for
 * `navDomContentLoadedMs` — so a single blanket mock would make one test's fixture
 * leak into the other's assertion. This stubs both by type, defaulting to an empty
 * list for whichever the caller does not supply.
 */
function stubPerformanceEntries(byType: Partial<Record<string, PerformanceEntry[]>>) {
  vi.spyOn(performance, "getEntriesByType").mockImplementation(
    (type: string) => byType[type] ?? [],
  );
}

beforeEach(() => {
  setActivePinia(createPinia());
  stubDoubleRaf();
  // Deterministic default: no PerformanceObserver at all, so `whenFirstContentfulPaint`
  // takes its "platform cannot ever produce one" branch unless a test stubs otherwise.
  // Real jsdom's own support for the Paint Timing API is not something this suite
  // should depend on incidentally.
  vi.stubGlobal("PerformanceObserver", undefined);
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

  it("sends the two callbacks' own timestamps as evidence, not a recomputed pair", async () => {
    // task #108 / F-377: the point of PaintEvidence is that raf1Ms and raf2Ms are
    // the browser's own callback arguments. If schedulePaintMark ever starts calling
    // performance.now() a second time instead of using the callback argument, this
    // still passes as long as the two values it sends came from the two distinct
    // scheduled callbacks in order.
    markFirstPaint.mockResolvedValue(1.0);
    const store = useVaultStore();
    await schedulePaintMark(store);

    expect(markFirstPaint).toHaveBeenCalledTimes(1);
    const evidence = markFirstPaint.mock.calls[0][0] as PaintEvidenceForTest;
    expect(evidence.raf1Ms).toBe(10);
    expect(evidence.raf2Ms).toBe(20);
    expect(evidence.raf2Ms).toBeGreaterThan(evidence.raf1Ms);
  });

  it("sends camelCase keys, matching Tauri's default rename of command parameters", async () => {
    // Confirmed against task #105's actual Rust signature (first_contentful_paint_ms),
    // which Tauri only matches when the JS side sends firstContentfulPaintMs — the
    // snake_case form this file shipped for one revision would have silently failed
    // to reach the parameter at all. See PaintEvidence's own doc comment.
    markFirstPaint.mockResolvedValue(1.0);
    const store = useVaultStore();
    await schedulePaintMark(store);

    const evidence = markFirstPaint.mock.calls[0][0] as Record<string, unknown>;
    expect(Object.keys(evidence).sort()).toEqual(
      ["firstContentfulPaintMs", "navDomContentLoadedMs", "raf1Ms", "raf2Ms"].sort(),
    );
  });

  it("reports the first-contentful-paint entry, not the frame gate", async () => {
    // F-388: a frame gate alone is forgeable by a mutation that honestly waits for
    // two real, early frames before this app has mounted anything visible.
    // firstContentfulPaintMs is the evidence that closes that gap, and it is not the
    // same signal as navDomContentLoadedMs (which fires on HTML parse, before any
    // script runs, and cannot see whether real content ever painted).
    stubPerformanceEntries({
      paint: [{ name: "first-contentful-paint", startTime: 17 } as PerformancePaintTiming],
    });
    markFirstPaint.mockResolvedValue(1.0);

    const store = useVaultStore();
    await schedulePaintMark(store);

    const evidence = markFirstPaint.mock.calls[0][0] as PaintEvidenceForTest;
    expect(evidence.firstContentfulPaintMs).toBe(17);
  });

  it("reports null for first-contentful-paint when the platform has no PerformanceObserver", async () => {
    // Set explicitly by this test even though beforeEach already does — the property
    // under test is that "no way to ever get one" resolves null rather than hanging.
    vi.stubGlobal("PerformanceObserver", undefined);
    stubPerformanceEntries({});
    markFirstPaint.mockResolvedValue(1.0);

    const store = useVaultStore();
    await schedulePaintMark(store);

    const evidence = markFirstPaint.mock.calls[0][0] as PaintEvidenceForTest;
    expect(evidence.firstContentfulPaintMs).toBeNull();
  });

  it("waits for the entry rather than reading it too early — the real bug this file shipped once", async () => {
    // A real WebView2 run found that a synchronous performance.getEntriesByType
    // check inside the second rAF callback was too early: the entry was not yet in
    // the buffer even though the frame had genuinely painted. This fakes exactly
    // that shape — nothing available synchronously, PerformanceObserver delivers the
    // entry on its own schedule — and asserts markFirstPaint is not called until it
    // does, with the delivered value.
    stubPerformanceEntries({});
    let deliverEntry!: (entries: PerformanceEntry[]) => void;
    class FakePaintObserver {
      static supportedEntryTypes = ["paint"];
      #callback: PerformanceObserverCallback;
      constructor(callback: PerformanceObserverCallback) {
        this.#callback = callback;
      }
      observe() {
        deliverEntry = (entries) => {
          this.#callback({ getEntries: () => entries, getEntriesByType: () => entries } as never, this as never);
        };
      }
      disconnect() {}
    }
    vi.stubGlobal("PerformanceObserver", FakePaintObserver);
    markFirstPaint.mockResolvedValue(1.0);

    const store = useVaultStore();
    const done = schedulePaintMark(store);

    await Promise.resolve();
    await Promise.resolve();
    expect(markFirstPaint).not.toHaveBeenCalled();

    deliverEntry([{ name: "first-contentful-paint", startTime: 184 } as PerformancePaintTiming]);
    await done;

    const evidence = markFirstPaint.mock.calls[0][0] as PaintEvidenceForTest;
    expect(evidence.firstContentfulPaintMs).toBe(184);
  });

  it("reports the navigation-timing anchor when the webview recorded one", async () => {
    stubPerformanceEntries({
      navigation: [{ domContentLoadedEventEnd: 42 } as PerformanceNavigationTiming],
    });
    markFirstPaint.mockResolvedValue(1.0);

    const store = useVaultStore();
    await schedulePaintMark(store);

    const evidence = markFirstPaint.mock.calls[0][0] as PaintEvidenceForTest;
    expect(evidence.navDomContentLoadedMs).toBe(42);
  });

  it("reports null rather than throwing when there is no navigation entry", async () => {
    stubPerformanceEntries({});
    markFirstPaint.mockResolvedValue(1.0);

    const store = useVaultStore();
    await schedulePaintMark(store);

    const evidence = markFirstPaint.mock.calls[0][0] as PaintEvidenceForTest;
    expect(evidence.navDomContentLoadedMs).toBeNull();
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
