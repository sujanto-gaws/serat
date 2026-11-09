// FR-BIN-11, ADR-0017 §9: the range transport against a fake over-cap file, and the exact
// arguments handed to pdf.js (it ignores `length`/`isEvalSupported` in 6.x, so a cast hid that).
import { beforeEach, describe, expect, it, vi } from "vitest";

import { SPAN, makeAssetTransport } from "./assetRangeTransport";

class FakeBase {
  chunks: Array<{ begin: number; len: number }> = [];
  constructor(
    public length: number,
    public initialData: Uint8Array | null,
  ) {}
  onDataRange(begin: number, chunk: Uint8Array | null): void {
    this.chunks.push({ begin, len: chunk?.byteLength ?? 0 });
  }
  requestDataRange(_b: number, _e: number): void {}
}

const HOOKS = { onChanged: () => {}, onFailed: () => {} };
const FILE = 3 * SPAN + 123; // over the cap
function deps(over: Partial<Parameters<typeof makeAssetTransport>[2]> = {}) {
  const reads: Array<[number, number]> = [];
  const d = {
    reads,
    changed: vi.fn(),
    readRange: vi.fn(async (start: number, len: number) => {
      reads.push([start, len]);
      return new ArrayBuffer(Math.min(len, FILE - start));
    }),
    statBytes: vi.fn(async () => FILE),
    onChanged: vi.fn(),
    onFailed: vi.fn(),
    ...over,
  };
  return d;
}
const make = (d: ReturnType<typeof deps>) =>
  makeAssetTransport(FakeBase as never, FILE, d) as unknown as FakeBase & { requestDataRange(b: number, e: number): void };
const settle = () => new Promise((r) => setTimeout(r, 0));

describe("makeAssetTransport", () => {
  it("carries the length from asset_info and never starts with data", () => {
    const t = make(deps());
    expect(t.length).toBe(FILE);
    expect(t.initialData).toBeNull();
  });

  it("splits a large request into spans of at most 1,000 KiB and clamps to the file length", async () => {
    const d = deps();
    const t = make(d);
    t.requestDataRange(0, FILE + 5000);
    await settle();
    expect(d.reads).toEqual([
      [0, SPAN],
      [SPAN, SPAN],
      [2 * SPAN, SPAN],
      [3 * SPAN, 123],
    ]);
    expect(t.chunks.map((c) => c.begin)).toEqual([0, SPAN, 2 * SPAN, 3 * SPAN]);
    expect(Math.max(...d.reads.map(([, len]) => len))).toBeLessThanOrEqual(1000 * 1024);
    expect(d.onChanged).not.toHaveBeenCalled();
  });

  it("serves only the requested page range, not the whole file", async () => {
    const d = deps();
    make(d).requestDataRange(SPAN + 10, SPAN + 110);
    await settle();
    expect(d.reads).toEqual([[SPAN + 10, 100]]);
  });

  it("detects a size change before reading, serves nothing, and reports once", async () => {
    const d = deps({ statBytes: vi.fn(async () => FILE + 1) });
    const t = make(d);
    t.requestDataRange(0, 10);
    t.requestDataRange(10, 20);
    await settle();
    expect(d.reads).toEqual([]);
    expect(t.chunks).toEqual([]);
    expect(d.onChanged).toHaveBeenCalledTimes(1);
  });

  it("detects a file that shrank mid-request by a short span, and stops serving mixed versions", async () => {
    const d = deps();
    (d.readRange as ReturnType<typeof vi.fn>).mockImplementation(async (start: number, len: number) => {
      d.reads.push([start, len]);
      return new ArrayBuffer(start === 0 ? len : 7); // second span comes back short
    });
    const t = make(d);
    t.requestDataRange(0, 2 * SPAN);
    await settle();
    expect(t.chunks).toHaveLength(1);
    expect(d.onChanged).toHaveBeenCalledTimes(1);
    t.requestDataRange(0, 10); // after a change nothing more is served
    await settle();
    expect(d.reads).toHaveLength(2);
  });

  it("a file that is gone (not_found) is evidence of change: restart lands on the missing state", async () => {
    const d = deps({ readRange: vi.fn(async () => Promise.reject({ kind: "not_found" })) });
    make(d).requestDataRange(0, 10);
    await settle();
    expect(d.onChanged).toHaveBeenCalledTimes(1);
    expect(d.onFailed).not.toHaveBeenCalled();
  });

  it("a read error with the size unchanged is a failure, never a restart (no EIO/offline loop)", async () => {
    const d = deps({ readRange: vi.fn(async () => Promise.reject({ kind: "io", message: "EIO" })) });
    const t = make(d);
    t.requestDataRange(0, 10);
    t.requestDataRange(0, 10);
    await settle();
    expect(d.onChanged).not.toHaveBeenCalled();
    expect(d.onFailed).toHaveBeenCalledTimes(1);
  });

  it("asset_changed on a span (even at the same size) is one onChanged and no further spans", async () => {
    const d = deps();
    delete (d as { statBytes?: unknown }).statBytes; // the etag path has no per-request stat
    (d.readRange as ReturnType<typeof vi.fn>).mockImplementation(async (start: number, len: number) => {
      d.reads.push([start, len]);
      if (start >= SPAN) throw { kind: "asset_changed", path: "d/x.pdf", validator: "E2" };
      return new ArrayBuffer(len);
    });
    const t = make(d);
    t.requestDataRange(0, 3 * SPAN);
    await settle();
    expect(d.reads).toEqual([
      [0, SPAN],
      [SPAN, SPAN],
    ]);
    expect(t.chunks).toHaveLength(1);
    expect(d.onChanged).toHaveBeenCalledTimes(1);
    expect(d.onFailed).not.toHaveBeenCalled();
    t.requestDataRange(0, 10);
    await settle();
    expect(d.reads).toHaveLength(2);
  });

  it("without statBytes there is no per-request stat; with it (no etag) the old size check still works", async () => {
    const withStat = deps({ statBytes: vi.fn(async () => FILE + 1) });
    make(withStat).requestDataRange(0, 10);
    await settle();
    expect(withStat.onChanged).toHaveBeenCalledTimes(1);
    expect(withStat.reads).toEqual([]);

    const noStat = deps();
    delete (noStat as { statBytes?: unknown }).statBytes;
    make(noStat).requestDataRange(0, 10);
    await settle();
    expect(noStat.reads).toEqual([[0, 10]]);
    expect(noStat.onChanged).not.toHaveBeenCalled();
  });
});

// ---- loadPdf with pdfjs-dist mocked: the exact getDocument arguments and the worker.
const pdfjs = vi.hoisted(() => {
  class Transport {
    constructor(
      public length: number,
      public initialData: Uint8Array | null,
    ) {}
    onDataRange(): void {}
    requestDataRange(): void {}
  }
  return {
    GlobalWorkerOptions: {} as { workerPort?: unknown; workerSrc?: string },
    PDFDataRangeTransport: Transport,
    getDocument: vi.fn(() => ({ promise: Promise.resolve({ numPages: 3 }), destroy: vi.fn(async () => {}) })),
    TextLayer: class {},
  };
});
vi.mock("pdfjs-dist", () => pdfjs);
const ipc = vi.hoisted(() => ({ readAssetRange: vi.fn(), assetInfo: vi.fn() }));
vi.mock("@/ipc/client", () => ipc);

describe("loadPdf (pdf.js arguments)", () => {
  beforeEach(() => {
    pdfjs.getDocument.mockClear();
    pdfjs.GlobalWorkerOptions.workerPort = undefined;
  });

  it("passes a range transport of asset_info's length and NO url; no unsupported options; a real module Worker", async () => {
    const created: Array<{ url: unknown; options: unknown }> = [];
    vi.stubGlobal(
      "Worker",
      class {
        constructor(url: unknown, options: unknown) {
          created.push({ url, options });
        }
      },
    );
    const { loadPdf, RANGE_CHUNK_SIZE } = await import("./pdfEngine");
    const handle = await loadPdf("d/x.pdf", FILE, HOOKS);
    expect(handle.numPages).toBe(3);
    expect(pdfjs.getDocument).toHaveBeenCalledTimes(1);
    const args = (pdfjs.getDocument.mock.calls[0] as unknown[])[0] as Record<string, unknown>;
    expect(Object.keys(args).sort()).toEqual(["disableAutoFetch", "disableStream", "range", "rangeChunkSize", "useWasm"]);
    expect(args.url).toBeUndefined();
    expect(args.useWasm).toBe(false);
    expect(args.disableAutoFetch).toBe(true);
    expect(args.rangeChunkSize).toBe(RANGE_CHUNK_SIZE);
    expect((args.range as { length: number; initialData: unknown }).length).toBe(FILE);
    expect((args.range as { initialData: unknown }).initialData).toBeNull();
    // The worker is created by us, as a module worker, not left to pdf.js's blob: wrapper.
    expect(pdfjs.GlobalWorkerOptions.workerPort).toBeDefined();
    expect(created).toHaveLength(1);
    expect(created[0].options).toEqual({ type: "module" });
    vi.unstubAllGlobals();
  });

  it("refuses to run with the main-thread fake worker when no Worker exists", async () => {
    vi.stubGlobal("Worker", undefined);
    const { loadPdf } = await import("./pdfEngine");
    await expect(loadPdf("d/x.pdf", 10, HOOKS)).rejects.toThrow(/fake worker/);
    expect(pdfjs.getDocument).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it("a new load waits for the previous document's destroy to finish (shared worker port)", async () => {
    vi.stubGlobal("Worker", class {});
    const { loadPdf } = await import("./pdfEngine");
    let finishDestroy!: () => void;
    const slow = new Promise<void>((r) => (finishDestroy = r));
    pdfjs.getDocument.mockReturnValueOnce({ promise: Promise.resolve({ numPages: 1 }), destroy: vi.fn(() => slow) });
    const first = await loadPdf("d/a.pdf", 10, HOOKS);
    first.destroy();
    pdfjs.getDocument.mockClear();
    const second = loadPdf("d/b.pdf", 10, HOOKS);
    await settle();
    expect(pdfjs.getDocument).not.toHaveBeenCalled();
    finishDestroy();
    await second;
    expect(pdfjs.getDocument).toHaveBeenCalledTimes(1);
    vi.unstubAllGlobals();
  });

  it("aborting while getDocument is pending destroys that task and rejects; nothing leaks", async () => {
    vi.stubGlobal("Worker", class {});
    const { loadPdf } = await import("./pdfEngine");
    const destroy = vi.fn(async () => {});
    pdfjs.getDocument.mockReturnValueOnce({ promise: new Promise(() => {}), destroy });
    const controller = new AbortController();
    const pending = loadPdf("d/a.pdf", 10, HOOKS, controller.signal);
    pending.catch(() => {});
    await settle();
    controller.abort();
    expect(destroy).toHaveBeenCalledTimes(1);
    vi.unstubAllGlobals();
  });

  it("an abort that lands during the pdf.js import never reaches getDocument", async () => {
    vi.stubGlobal("Worker", class {});
    const { loadPdf } = await import("./pdfEngine");
    pdfjs.getDocument.mockClear();
    let reads = 0;
    // `aborted` is false for the first check (before the import) and true afterwards.
    const signal = { get aborted() { return reads++ >= 1; }, addEventListener() {}, removeEventListener() {} } as unknown as AbortSignal;
    await expect(loadPdf("d/a.pdf", 10, HOOKS, signal)).rejects.toMatchObject({ name: "AbortError" });
    expect(pdfjs.getDocument).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it("sends if_match = the etag on every span and does not stat; without an etag it omits if_match and stats", async () => {
    vi.stubGlobal("Worker", class {});
    const { loadPdf } = await import("./pdfEngine");
    ipc.readAssetRange.mockImplementation(async (a: { start: number; len: number }) => new ArrayBuffer(Math.min(a.len, FILE - a.start)));
    ipc.assetInfo.mockResolvedValue({ bytes: FILE });
    const transportOf = () => {
      const calls = pdfjs.getDocument.mock.calls as unknown as Array<[{ range: { requestDataRange(b: number, e: number): void } }]>;
      return calls[calls.length - 1][0].range;
    };

    ipc.readAssetRange.mockClear();
    await loadPdf("d/x.pdf", FILE, HOOKS, undefined, "E1");
    transportOf().requestDataRange(0, 2 * SPAN + 5);
    await settle();
    expect(ipc.readAssetRange.mock.calls.map((c) => c[0])).toEqual([
      { path: "d/x.pdf", start: 0, len: SPAN, if_match: "E1" },
      { path: "d/x.pdf", start: SPAN, len: SPAN, if_match: "E1" },
      { path: "d/x.pdf", start: 2 * SPAN, len: 5, if_match: "E1" },
    ]);
    expect(ipc.assetInfo).not.toHaveBeenCalled();

    ipc.readAssetRange.mockClear();
    await loadPdf("d/x.pdf", FILE, HOOKS);
    transportOf().requestDataRange(0, 10);
    await settle();
    expect(ipc.readAssetRange.mock.calls[0][0]).toEqual({ path: "d/x.pdf", start: 0, len: 10 });
    expect("if_match" in (ipc.readAssetRange.mock.calls[0][0] as object)).toBe(false);
    expect(ipc.assetInfo).toHaveBeenCalledWith({ path: "d/x.pdf" });
    vi.unstubAllGlobals();
  });
});
