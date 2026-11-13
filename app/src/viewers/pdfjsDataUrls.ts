// Where the viewer fetches pdf.js's CMaps and standard fonts from: the bundle's own `cmaps/` and
// `standard_fonts/` (FR-BIN-11, ADR-0017 Amendment 1 item 6). Relative, with the trailing slash pdf.js
// requires, resolved against the document by `fetch`: same origin, never a scheme or `//host` (NFR-SEC-01).
// Kept free of node imports because pdfEngine.ts (browser code) imports it; the build-time copy that
// ships the files lives in `src/build/pdfjsData.ts`.

export const PDFJS_CMAP_URL = "./cmaps/";
export const PDFJS_STANDARD_FONT_URL = "./standard_fonts/";
