// The one place pdf.js is touched (FR-BIN-11, ADR-0017 §pdf.js). Imported lazily so the
// ~1 MB library is not on the startup path (NFR-PRF-01). Bundled, inert and local:
//   * bytes come only from `read_asset_range` through `assetRangeTransport.ts`; `getDocument` is
//     given NO url, so pdf.js makes no network request of its own (NFR-SEC-01);
//   * pdf.js 6.x has no `isEvalSupported` option and no eval path (none in pdf.mjs or the
//     worker); the control is the app CSP, which has no 'unsafe-eval' (ADR-0017 amendment,
//     task #94). `useWasm: false` is explicit because the CSP has no 'wasm-unsafe-eval';
//   * the worker is a same-origin module Worker built from a Vite asset URL, so no blob: wrapper
//     (blocked by `script-src 'self'`) can silently demote it to a main-thread fake worker; if
//     a Worker cannot be created the load fails loudly instead;
//   * no annotation layer is built and the scripting sandbox bundle is never loaded, so a PDF's
//     JavaScript and links cannot run;
//   * known gap (ADR-0017 amendment, task #94): cMaps and standard fonts are not shipped.

import { makeAssetTransport } from "./assetRangeTransport";
import { assetInfo, readAssetRange } from "@/ipc/client";

export interface PdfPageSize {
  width: number;
  height: number;
}

export interface PdfDocumentHandle {
  numPages: number;
  /** Page size in CSS px at scale 1 (needed for fit-width / fit-page). */
  pageSize(page: number): Promise<PdfPageSize>;
  /** Draws `page` into `canvas` at `scale`; resolves once painted. Text layer goes in `textLayer`. */
  render(page: number, canvas: HTMLCanvasElement, scale: number, textLayer: HTMLElement | null): Promise<void>;
  destroy(): void;
}

export interface PdfHooks {
  /** Evidence the file no longer matches what was opened (size differs, short span, gone). */
  onChanged(): void;
  /** A read failed with the file unchanged (I/O error, offline placeholder): show the error state. */
  onFailed(): void;
}

/** Opens `path` (a resolved vault path) of `length` bytes. Aborting `signal` destroys the load
 * (a pending `getDocument` is otherwise never released). */
export type PdfLoader = (path: string, length: number, hooks: PdfHooks, signal?: AbortSignal) => Promise<PdfDocumentHandle>;

/** The previous document's destroy: with one shared worker port a new `getDocument` must not start
 * until it finishes ("the worker is being destroyed"). */
let lastDestroy: Promise<unknown> = Promise.resolve();

/** ADR-0017 §9: the handler's span cap, also pdf.js's chunk size. */
export const RANGE_CHUNK_SIZE = 1000 * 1024;

export const loadPdf: PdfLoader = async (path, length, hooks, signal) => {
  await lastDestroy;
  if (signal?.aborted) throw new DOMException("aborted", "AbortError");
  const pdfjs = await import("pdfjs-dist");
  if (!pdfjs.GlobalWorkerOptions.workerPort) {
    if (typeof Worker === "undefined") throw new Error("no Worker: refusing pdf.js's main-thread fake worker");
    pdfjs.GlobalWorkerOptions.workerPort = new Worker(new URL("pdfjs-dist/build/pdf.worker.min.mjs", import.meta.url), {
      type: "module",
    });
  }
  const transport = makeAssetTransport(pdfjs.PDFDataRangeTransport, length, {
    readRange: (start, len) => readAssetRange({ path, start, len }),
    statBytes: async () => (await assetInfo({ path })).bytes,
    onChanged: hooks.onChanged,
    onFailed: hooks.onFailed,
  });
  const task = pdfjs.getDocument({
    range: transport,
    rangeChunkSize: RANGE_CHUNK_SIZE,
    disableStream: true, // no effect with a range transport; kept to match the ADR-0017 amendment
    disableAutoFetch: true,
    useWasm: false,
  });
  const destroyTask = (): void => {
    lastDestroy = Promise.resolve(task.destroy()).catch(() => undefined);
  };
  const onAbort = (): void => destroyTask();
  signal?.addEventListener("abort", onAbort, { once: true });
  let doc: Awaited<typeof task.promise>;
  try {
    doc = await task.promise;
  } catch (error) {
    signal?.removeEventListener("abort", onAbort);
    throw error;
  }
  signal?.removeEventListener("abort", onAbort);
  if (signal?.aborted) {
    destroyTask();
    throw new DOMException("aborted", "AbortError");
  }
  return {
    numPages: doc.numPages,
    async pageSize(n) {
      const viewport = (await doc.getPage(n)).getViewport({ scale: 1 });
      return { width: viewport.width, height: viewport.height };
    },
    async render(n, canvas, scale, textLayer) {
      const page = await doc.getPage(n);
      const viewport = page.getViewport({ scale });
      const ratio = window.devicePixelRatio || 1;
      canvas.width = Math.floor(viewport.width * ratio);
      canvas.height = Math.floor(viewport.height * ratio);
      canvas.style.width = `${Math.floor(viewport.width)}px`;
      canvas.style.height = `${Math.floor(viewport.height)}px`;
      await page.render({
        canvas,
        viewport,
        transform: ratio === 1 ? undefined : [ratio, 0, 0, ratio, 0, 0],
      }).promise;
      if (textLayer) {
        textLayer.replaceChildren();
        textLayer.style.width = canvas.style.width;
        textLayer.style.height = canvas.style.height;
        // pdf_viewer.css derives every glyph's size from these (S6).
        textLayer.style.setProperty("--scale-factor", String(scale));
        textLayer.style.setProperty("--total-scale-factor", String(scale));
        await new pdfjs.TextLayer({
          textContentSource: page.streamTextContent(),
          container: textLayer,
          viewport,
        }).render();
      }
    },
    destroy: destroyTask,
  };
};
