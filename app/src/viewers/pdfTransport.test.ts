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

  it("an IPC failure (file gone, refused) takes the same restart path", async () => {
    const d = deps({ readRange: vi.fn(async () => Promise.reject({ kind: "not_found" })) });
    make(d).requestDataRange(0, 10);
    await settle();
    expect(d.onChanged).toHaveBeenCalledTimes(1);
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
    getDocument: vi.fn(() => ({ promise: Promise.resolve({ numPages: 3 }), destroy: vi.fn() })),
    TextLayer: class {},
  };
});
vi.mock("pdfjs-dist", () => pdfjs);

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
    const handle = await loadPdf("d/x.pdf", FILE, () => {});
    expect(handle.numPages).toBe(3);
    expect(pdfjs.getDocument).toHaveBeenCalledTimes(1);
    const args = (pdfjs.getDocument.mock.calls[0] as unknown[])[0] as Record<string, unknown>;
    expect(Object.keys(args).sort()).toEqual(["disableAutoFetch", "range", "rangeChunkSize", "useWasm"]);
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
    await expect(loadPdf("d/x.pdf", 10, () => {})).rejects.toThrow(/fake worker/);
    expect(pdfjs.getDocument).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });
});
