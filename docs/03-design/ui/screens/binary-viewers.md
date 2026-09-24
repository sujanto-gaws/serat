# Screen: Binary viewers — PDF (page navigation) and image (zoom)

Serves: FR-BIN-11, FR-BIN-12 · US-19
Design basis: ADR-0017 (`serat-asset://` scheme, range requests, per-response CSP), architecture.md §7.4 (P1-06 branch)

Both viewers open in the same pane an editor tab would occupy — a PDF or image is a tab like any note, not a separate window — and both read bytes exclusively through the `serat-asset://` scheme (ADR-0017), never through `read_file`. If the asset has a sidecar (ADR-0005/§7.4), its metadata shows in a collapsible header above the viewer surface rather than a separate pane, keeping "one entry, one thing" consistent with the tree's fold (`file-tree.md`).

## PDF viewer (FR-BIN-11)

```
┌ tab: contract.pdf ● metadata ──────────────────────────────┐
│ ◂  Page [ 4 ] of 18   ▸     100% ▾   Fit width ▾   🔍       │  toolbar
├───────────────────────────────────────────────────────────┤
│                                                               │
│                     (rendered page, bg-surface-sunken)       │
│                                                               │
├───────────────────────────────────────────────────────────┘
```

- Toolbar: previous/next page buttons; a **mono, editable page field** — typing a number and pressing `Enter` jumps directly (the literal acceptance criterion: "when a page number is entered, then the viewer navigates to that page"); page count in `text-tertiary`; zoom percentage and fit-width/fit-page as a segmented control (not a slider — page-fit modes are discrete states, a slider implies continuous values that aren't meaningful here).
- Rendered via pdf.js (bundled, no network, `isEvalSupported: false` per ADR-0017), loading pages incrementally through single-range requests against `serat-asset://` — the toolbar's page field is responsive even before the whole document has downloaded, because only the requested page's bytes are fetched.
- Text selection is native pdf.js text-layer selection; copy works normally; no custom selection UI is designed here beyond what pdf.js already provides, since FR-BIN-11's acceptance criterion is page navigation, and text selection is stated as a capability, not a bespoke interaction to design.
- A hit that arrives from the search panel with a page locator (P2, PDF-content search — out of scope here per the task boundary) would land through the same page-jump mechanism; this spec notes the seam exists without designing that P2 flow.

### States
| State | Presentation |
|---|---|
| Loading first page | A single skeleton page outline (`bg-surface-sunken` rectangle at the page's aspect ratio once known from the PDF header, else a fixed placeholder) — no spinner-in-a-circle, consistent with the rest of the system's restraint around loading chrome. |
| Page loaded | As drawn above. |
| Jump to out-of-range page number | Field reverts to the current page with a brief `accent-error` outline flash (120ms) rather than a dialog — a wrong page number is corrected in place, not treated as an error worth interrupting for. |
| File replaced on disk mid-view (ADR-0017's `ETag`/`If-Range` restart) | The viewer silently restarts the load for the affected range; if the visible page's bytes were affected, a small `text-tertiary` note appears at the top of the page: "This file changed on disk — showing the latest version," non-blocking, auto-dismissing after 4s. |
| Extraction/search unavailable for this PDF (P2 concern, not P1) | Not shown in P1 — no "not searchable yet" badge is specified here since content extraction is out of scope for this task; noted so it isn't accidentally designed twice when P2 picks it up. |
| Range requests unsupported by the platform webview (ADR-0017's named WebKitGTK/WKWebView risk) | The viewer has no visibly different UI for this — the fallback (IPC bytes for PDFs, per the ADR's contingency) is a transport detail behind the same `serat-asset://`-shaped interface; the only user-visible difference would be a slower initial load on affected platforms, which this spec does not attempt to hide or announce, since it is not yet known whether the fallback is needed (see Design questions below). |

### Keyboard map
| Action | Binding |
|---|---|
| Next / previous page | `→` / `←` (also `PageDown`/`PageUp`), when focus is in the viewer and not in the page field |
| Jump to page field | `⌘` `G` |
| Confirm typed page | `↵` |
| Zoom in / out | `⌘` `+` / `⌘` `-` |
| Fit width / fit page | `⌘` `0` toggles between the two fit modes |
| Find in this PDF (text-layer only, P1) | `⌘` `F` opens a small in-pane find bar (native-style, not the vault search panel) that highlights matches in the currently-loaded page's text layer — scoped to loaded pages only in P1, since full-document PDF text search is P2's FR-BIN-05/06 |

## Image viewer (FR-BIN-12)

```
┌ tab: diagram.png ─────────────────────────────────────────┐
│  100% ▾   Fit ▾   ⤢ Actual size            🔍 −  ▬▬●▬  +   │  toolbar
├───────────────────────────────────────────────────────────┤
│               ░░░ checkerboard behind transparency ░░░      │
│                        (image, centred)                     │
└───────────────────────────────────────────────────────────┘
```

- Toolbar: zoom percentage (mono, editable — type a number, `Enter` applies), fit/actual-size buttons, and a zoom slider for continuous adjustment (unlike the PDF's discrete fit modes, a slider is appropriate here because zoom on an image genuinely is continuous).
- Pan by drag when zoomed beyond fit; scroll-wheel + modifier (`⌘`+scroll, or pinch on trackpad) zooms toward the cursor position, not the image centre — a small but real usability point ("scales without leaving the app" is the letter of FR-BIN-12; zooming toward the cursor is what makes that pleasant rather than merely present).
- Transparent images show the checkerboard backdrop (components.md §4) — the only textured background in the system, because it's information.

### States
| State | Presentation |
|---|---|
| Loading | Same skeleton-rectangle convention as the PDF viewer, sized to the image's declared dimensions where known. |
| Loaded, fit-to-pane | Default on open. |
| Zoomed | Pan cursor (grab/grabbing) replaces the default cursor over the image. |
| Unsupported/corrupt image format | A plain message in place of the canvas: "This image couldn't be displayed" with the file's MIME type shown in `mono`/`micro` beneath it, plus a "Reveal in file tree" action — never a broken-image glyph, which looks like a bug rather than a stated fact. |
| File replaced on disk mid-view | Same non-blocking notice pattern as the PDF viewer. |

### Keyboard map
| Action | Binding |
|---|---|
| Zoom in / out | `⌘` `+` / `⌘` `-` |
| Fit / actual size | `⌘` `0` toggles |
| Reset pan to centre | `⌘` `⇧` `0` |

## Responsive rules

- Both viewers' toolbars collapse secondary controls (fit-mode label, page-count text) behind their icons below `breakpoint-compact`, keeping only the page/zoom field and the primary navigation arrows visible — the same discipline as the rest of the shell rather than a viewer-specific rule.
- Viewer panes never introduce their own horizontal scrollbar at the pane level; pan/zoom is the interaction, not page-level scroll, to avoid two competing scroll mechanisms.

## Accessibility

- Page/zoom fields are real `<input type="number">`-equivalent controls with labels ("Page", "Zoom percent"), reachable by `Tab`, not canvas-drawn text.
- pdf.js's text layer provides selectable, screen-reader-navigable text for PDFs with an extracted text layer already (most digitally-produced PDFs); scanned PDFs with no text layer are announced as "Image-only PDF, page N of M" rather than silently offering nothing.
- Images require alt text to be meaningfully accessible; P1 has no OCR/description mechanism, so the accessible name falls back to the filename — stated here rather than left implicit, since it is a known, honest limitation rather than an oversight.

## Design question for the architect

ADR-0017 names an open consultation with tauri-app-dev on whether WebKitGTK/WKWebView honour 206 range responses from a custom scheme, with an IPC-bytes fallback "for PDFs only" as the contingency. This spec treats that as a transport-level seam invisible to the UI (see the state row above), which only holds if the fallback truly preserves incremental page loading behaviour (i.e., it still serves one page's bytes rather than the whole file up front) — if the IPC fallback instead has to fetch the full PDF before the first page can render, that changes the loading state design here (a real progress state would be needed instead of the current single-skeleton-page treatment) and I'd want to know which of those two shapes to design for once ADR-0017 settles.
