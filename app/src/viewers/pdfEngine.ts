// The one place pdf.js is touched (FR-BIN-01, ADR-0017 §pdf.js). Imported lazily so the
// ~1 MB library is not on the startup path (NFR-PRF-01). Bundled, inert and local:
//   * bytes come only from the `serat-asset` URL passed in (CSP `connect-src` allows that
//     origin and `ipc:`, nothing remote);
//   * the worker is a same-origin script emitted by Vite (`script-src 'self'`), not a blob:;
//   * `length` from `asset_info`, `rangeChunkSize` at the 1,000 KiB cap, `disableStream` and
//     `disableAutoFetch`, so only the spans of the pages being shown are requested;
//   * no annotation layer is built and the scripting sandbox bundle is never loaded, so a
//     PDF's JavaScript and links cannot run (links wait for the follow-up; see PdfViewer).

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

export type PdfLoader = (url: string, length: number) => Promise<PdfDocumentHandle>;

/** ADR-0017 §8: the handler's span cap. */
export const RANGE_CHUNK_SIZE = 1000 * 1024;

export const loadPdf: PdfLoader = async (url, length) => {
  const pdfjs = await import("pdfjs-dist");
  if (!pdfjs.GlobalWorkerOptions.workerSrc) {
    pdfjs.GlobalWorkerOptions.workerSrc = new URL("pdfjs-dist/build/pdf.worker.min.mjs", import.meta.url).href;
  }
  const params = {
    url,
    length,
    rangeChunkSize: RANGE_CHUNK_SIZE,
    disableStream: true,
    disableAutoFetch: true,
    isEvalSupported: false,
  };
  const task = pdfjs.getDocument(params as Parameters<typeof pdfjs.getDocument>[0]);
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
        textLayer.style.setProperty("--scale-factor", String(scale));
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
