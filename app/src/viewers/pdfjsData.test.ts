// @vitest-environment node
// pdf.js's CMaps and standard fonts ship in the bundle and the viewer asks for them by relative URL
// (FR-BIN-11, US-19, NFR-SEC-01; ADR-0017 Amendment 1 item 6).

import { readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

import { describe, expect, it, vi } from "vitest";

import { pdfjsDataFiles } from "@/build/pdfjsData";
import { PDFJS_CMAP_URL, PDFJS_STANDARD_FONT_URL } from "@/viewers/pdfjsDataUrls";

const root = dirname(createRequire(import.meta.url).resolve("pdfjs-dist/package.json"));

describe("the files shipped into the bundle", () => {
  const files = pdfjsDataFiles();

  it("are every file of pdfjs-dist/cmaps and pdfjs-dist/standard_fonts, under fixed names", () => {
    const names = files.map((f) => f.fileName);
    for (const dir of ["cmaps", "standard_fonts"]) {
      const expected = readdirSync(join(root, dir)).map((n) => `${dir}/${n}`);
      expect(names.filter((n) => n.startsWith(`${dir}/`)).sort()).toEqual(expected.sort());
    }
    for (const name of names) expect(name, name).toMatch(/^(cmaps|standard_fonts)\/[A-Za-z0-9._-]+$/); // no hash, no path tricks
    expect(names).toContain("cmaps/UniJIS-UCS2-H.bcmap");
    expect(names).toContain("cmaps/Adobe-Japan1-UCS2.bcmap");
    expect(names).toContain("standard_fonts/FoxitSerif.pfb");
  });

  it("the URLs the viewer asks for are relative, end in a slash, and name those directories", () => {
    expect(PDFJS_CMAP_URL).toBe("./cmaps/");
    expect(PDFJS_STANDARD_FONT_URL).toBe("./standard_fonts/");
    for (const url of [PDFJS_CMAP_URL, PDFJS_STANDARD_FONT_URL]) expect(url).not.toMatch(/^([a-z][a-z0-9+.-]*:|\/\/)/i);
  });
});

describe("pdfEngine passes the options to pdf.js", () => {
  it("cMapUrl, cMapPacked and standardFontDataUrl are relative; wasmUrl and iccUrl stay unset; useWasm is off", async () => {
    const getDocument = vi.fn(() => ({ promise: Promise.resolve({ numPages: 1 }), destroy: vi.fn(async () => {}) }));
    vi.doMock("pdfjs-dist", () => ({
      GlobalWorkerOptions: { workerPort: null },
      PDFDataRangeTransport: class {},
      getDocument,
    }));
    vi.stubGlobal("Worker", class {});
    const { loadPdf } = await import("@/viewers/pdfEngine");
    await loadPdf("d/x.pdf", 10, { onChanged: () => {}, onFailed: () => {} });
    const args = (getDocument.mock.calls[0] as unknown[])[0] as Record<string, unknown>;
    expect(args.cMapUrl).toBe("./cmaps/");
    expect(args.cMapPacked).toBe(true);
    expect(args.standardFontDataUrl).toBe("./standard_fonts/");
    expect(args.wasmUrl).toBeUndefined();
    expect(args.iccUrl).toBeUndefined();
    expect(args.useWasm).toBe(false);
    vi.unstubAllGlobals();
    vi.doUnmock("pdfjs-dist");
  });
});

/** A one-page PDF whose only text is 日本語 set in a non-embedded CJK font through the predefined CMap
 * UniJIS-UCS2-H: pdf.js can only turn those codes into text with the bundled CMaps. */
function cjkPdf(): Uint8Array {
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    null, // content stream, below
    "<< /Type /Font /Subtype /Type0 /BaseFont /HeiseiMin-W3 /Encoding /UniJIS-UCS2-H /DescendantFonts [6 0 R] >>",
    "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /HeiseiMin-W3 /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 2 >> /FontDescriptor 7 0 R /DW 1000 >>",
    "<< /Type /FontDescriptor /FontName /HeiseiMin-W3 /Flags 6 /FontBBox [-123 -257 1001 910] /ItalicAngle 0 /Ascent 723 /Descent -241 /CapHeight 709 /StemV 69 >>",
  ];
  const stream = "BT /F1 24 Tf 20 100 Td <65E5672C8A9E> Tj ET";
  objects[3] = `<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`;
  let out = "%PDF-1.4\n";
  const offsets: number[] = [];
  objects.forEach((body, i) => {
    offsets.push(out.length);
    out += `${i + 1} 0 obj\n${body}\nendobj\n`;
  });
  const xref = out.length;
  out += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets) out += `${String(offset).padStart(10, "0")} 00000 n \n`;
  out += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return new TextEncoder().encode(out);
}

describe("a PDF with a CJK CMap", () => {
  // The viewer itself needs a Worker and a browser, which vitest does not give it; this runs pdf.js's
  // Node build against the very directories the bundle ships (the same files, the packed form the viewer
  // requests). The full path, webview and all, is the manual check in docs/05-testing/test-plan.md.
  async function textOf(withCMaps: boolean): Promise<string> {
    const pdfjs = await import("pdfjs-dist/legacy/build/pdf.mjs");
    const task = pdfjs.getDocument({
      data: cjkPdf(),
      useWasm: false,
      useSystemFonts: false,
      ...(withCMaps
        ? {
            cMapUrl: `${join(root, "cmaps")}/`,
            cMapPacked: true,
            standardFontDataUrl: `${join(root, "standard_fonts")}/`,
          }
        : {}),
    });
    const doc = await task.promise;
    const content = await (await doc.getPage(1)).getTextContent();
    await task.destroy();
    return content.items.map((item) => ("str" in item ? item.str : "")).join("");
  }

  it("yields its text with the bundled CMaps", async () => {
    const text = await textOf(true);
    expect(text.length).toBeGreaterThan(0);
    expect(text).toContain("日本語");
  });
});
