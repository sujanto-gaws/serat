// The byte source for pdf.js (FR-BIN-11; ADR-0017 §9 and §Fallback). pdf.js 6.x takes a
// document's length from the first response unless a `PDFDataRangeTransport` supplies it, and
// the scheme answers an over-cap no-Range request with only the first 1,000 KiB span, so the
// scheme URL cannot be handed to `getDocument` directly. Instead the length comes from
// `asset_info.bytes` and every span is a `read_asset_range` call, uniform on all platforms
// (steps 2-7 of the scheme apply on the Rust side, spans are capped at 1,000 KiB).
//
// Change detection: `read_asset_range` returns bytes only (no ETag, no mtime; contract gap),
// so a replaced file is noticed by (a) a fresh `asset_info` size before each request and (b) a
// span that comes back shorter than the range asked for. A same-size in-place rewrite is not
// detectable with this contract. On detection the transport stops serving and calls
// `onChanged`; the viewer restarts the load (never mixing versions) and shows the 4 s notice.

import type { PDFDataRangeTransport } from "pdfjs-dist";

/** ADR-0017 §9: the span cap. */
export const SPAN = 1000 * 1024;

export interface RangeDeps {
  /** `read_asset_range` for this path. */
  readRange(start: number, len: number): Promise<ArrayBuffer>;
  /** `asset_info(path).bytes`, a stat. */
  statBytes(): Promise<number>;
  /** Evidence the file no longer matches what was opened: size differs, a short span, or gone. */
  onChanged(): void;
  /** A read failed but nothing says the file changed (EIO, offline placeholder): do NOT restart. */
  onFailed(): void;
}

type TransportClass = new (length: number, initialData: Uint8Array | null) => PDFDataRangeTransport;

/** A transport of `length` bytes. `Base` is pdf.js's `PDFDataRangeTransport` (injected so the
 * class can be tested against a fake without loading pdf.js). */
export function makeAssetTransport(Base: TransportClass, length: number, deps: RangeDeps): PDFDataRangeTransport {
  class AssetTransport extends Base {
    private stale = false;

    private markChanged(): void {
      if (this.stale) return;
      this.stale = true;
      deps.onChanged();
    }

    private markFailed(): void {
      if (this.stale) return;
      this.stale = true;
      deps.onFailed();
    }

    override requestDataRange(begin: number, end: number): void {
      void this.serve(begin, Math.min(end, length));
    }

    private async serve(begin: number, end: number): Promise<void> {
      if (this.stale) return;
      try {
        if ((await deps.statBytes()) !== length) return this.markChanged();
        for (let pos = begin; pos < end && !this.stale; pos += SPAN) {
          const want = Math.min(SPAN, end - pos);
          const got = await deps.readRange(pos, want);
          if (this.stale) return;
          if (got.byteLength !== want) return this.markChanged();
          this.onDataRange(pos, new Uint8Array(got));
        }
      } catch (error) {
        // Only "the file is gone" is evidence of change (the restart then lands on the missing
        // state). Any other failure with the size unchanged ends in the error state: restarting
        // would loop on a persistent read error and leak a document per pass.
        if ((error as { kind?: string } | null)?.kind === "not_found") this.markChanged();
        else this.markFailed();
      }
    }
  }
  return new AssetTransport(length, null);
}
