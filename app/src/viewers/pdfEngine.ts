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

/** Opens `path` (a resolved vault path) of `length` bytes. `onChanged` fires when the file no
 * longer matches what was opened. */
export type PdfLoader = (path: string, length: number, onChanged: () => void) => Promise<PdfDocumentHandle>;

/** ADR-0017 §9: the handler's span cap, also pdf.js's chunk size. */
export const RANGE_CHUNK_SIZE = 1000 * 1024;

export const loadPdf: PdfLoader = async (path, length, onChanged) => {
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
    onChanged,
  });
  const task = pdfjs.getDocument({
    range: transport,
    rangeChunkSize: RANGE_CHUNK_SIZE,
    disableAutoFetch: true,
    useWasm: false,
  });
  const doc = await task.promise;
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
    destroy() {
      void task.destroy();
    },
  };
};
