// Task #26: viewer states, URL containment, routing helpers (FR-BIN-11, FR-BIN-12, FR-BIN-02, NFR-SEC-06;
// ADR-0017; binary-viewers.md).
import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { assetUrl, isIgnoredAssetPath, isSeratAssetUrl } from "@/editor/assetUrl";
import type { AssetInfo } from "@/ipc/generated/AssetInfo";

const assetInfo = vi.hoisted(() => vi.fn());
const writeFile = vi.hoisted(() => vi.fn());
vi.mock("@/ipc/client", async (orig) => ({
  ...(await orig<typeof import("@/ipc/client")>()),
  assetInfo,
  writeFile,
}));

import ImageViewer from "./ImageViewer.vue";
import PdfViewer from "./PdfViewer.vue";
import ViewerHost from "./ViewerHost.vue";
import { isViewerPath, openKind, sidecarPathFor } from "./assetKind";
import type { PdfDocumentHandle } from "./pdfEngine";

const info = (over: Partial<AssetInfo> = {}): AssetInfo => ({
  path: "p/x.png",
  mime: "image/png",
  bytes: 1536,
  viewable: true,
  sidecar_path: null,
  ...over,
});

describe("assetKind (routing)", () => {
  it("sends pdf and images to the viewer, notes to the editor, everything else nowhere", () => {
    expect(openKind("a/b.PDF")).toBe("pdf");
    for (const n of ["x.png", "x.JPG", "x.jpeg", "x.gif", "x.webp", "x.avif", "x.bmp", "x.svg"]) expect(openKind(n)).toBe("image");
    expect(openKind("n.md")).toBe("note");
    expect(openKind("n.pdf.md")).toBe("note"); // a sidecar is an ordinary note
    expect(openKind("noext")).toBe("other");
    expect(isViewerPath("x.pdf")).toBe(true);
    expect(isViewerPath("x.pdf.md")).toBe(false);
    expect(sidecarPathFor("d/x.pdf")).toBe("d/x.pdf.md");
  });
});

describe("assetUrl containment on the front end (NFR-SEC-06, ADR-0017 steps 3-4)", () => {
  it("refuses traversal, absolute and empty-segment paths on both platforms", () => {
    for (const p of ["../secret.png", "a/../../b.png", "a/./b.png", "/etc/passwd", "a//b.png", "", "a/..", "./a.png"]) {
      expect(() => assetUrl(p, "other"), p).toThrow();
      expect(() => assetUrl(p, "windows"), p).toThrow();
    }
  });

  it("treats a backslash as a separator (native VaultPath::new does), so a\\..\\b and .serat\\x are refused", () => {
    for (const p of ["a\\..\\b.png", "..\\x.png", ".serat\\index.db", "a\\\\b.png", "\\abs.png"]) {
      expect(() => assetUrl(p, "other"), p).toThrow();
    }
    // A backslash between ordinary names is still encoded, never emitted raw.
    const u = assetUrl("C:\\Windows\\x.png", "other");
    expect(u).toBe("serat-asset://localhost/C%3A%5CWindows%5Cx.png");
    expect(u).not.toContain("\\");
    // `%2e%2e` typed literally is a file named that, encoded once (the handler decodes once).
    expect(assetUrl("%2e%2e/x.png", "other")).toBe("serat-asset://localhost/%252e%252e/x.png");
  });

  it("refuses a path under an ignored directory (dotted parent, node_modules), in any case", () => {
    for (const p of [".serat/index.db", "a/.git/x.png", ".SERAT/x.png", "n/.Obsidian/p.png", ".trash/x.pdf", "a/.stfolder/x.png", "a/.gitx/x.png", "node_modules/p.png", "a/Node_Modules/p.png"]) {
      expect(isIgnoredAssetPath(p), p).toBe(true);
      expect(() => assetUrl(p, "other"), p).toThrow();
    }
    expect(isIgnoredAssetPath("serat/x.png")).toBe(false);
    expect(isIgnoredAssetPath("a/x.png")).toBe(false);
  });

  it("builds exactly the two ADR-0017 origins and the sanitiser's recogniser accepts only those", () => {
    expect(assetUrl("img/a b.png", "other")).toBe("serat-asset://localhost/img/a%20b.png");
    expect(assetUrl("img/a.png", "windows")).toBe("http://serat-asset.localhost/img/a.png");
    expect(isSeratAssetUrl(assetUrl("x.png", "other"))).toBe(true);
    expect(isSeratAssetUrl("http://serat-asset.localhost.evil.test/x.png")).toBe(false);
  });
});

describe("ViewerHost states (binary-viewers.md)", () => {
  beforeEach(() => {
    assetInfo.mockReset();
    writeFile.mockReset();
  });

  it("loading, then the image viewer with MIME and size from asset_info; the <img> src is the scheme URL", async () => {
    let resolve!: (v: AssetInfo) => void;
    assetInfo.mockReturnValue(new Promise((r) => (resolve = r)));
    const w = mount(ViewerHost, { props: { path: "p/x.png" } });
    expect(w.find('[data-testid="viewer-loading"]').exists()).toBe(true);
    resolve(info());
    await flushPromises();
    expect(assetInfo).toHaveBeenCalledWith({ path: "p/x.png" });
    expect(w.get('[data-testid="viewer-info"]').text()).toContain("image/png · 1.5 KiB");
    const img = w.get("img");
    expect(img.attributes("src")).toMatch(/^(serat-asset:\/\/localhost|http:\/\/serat-asset\.localhost)\/p\/x\.png$/);
    expect(w.find("object, iframe, embed").exists()).toBe(false);
    // Dimensions appear once the <img> reports them.
    Object.defineProperty(img.element, "naturalWidth", { value: 640 });
    Object.defineProperty(img.element, "naturalHeight", { value: 480 });
    await img.trigger("load");
    expect(w.get('[data-testid="viewer-info"]').text()).toContain("640×480");
  });

  it("missing on disk -> a plain statement, no viewer", async () => {
    assetInfo.mockRejectedValue({ kind: "not_found", path: "p/x.png" });
    const w = mount(ViewerHost, { props: { path: "p/x.png" } });
    await flushPromises();
    expect(w.get('[data-testid="viewer-missing"]').text()).toContain("no longer on disk");
    expect(w.find("img").exists()).toBe(false);
  });

  it("not viewable -> unsupported message with the MIME; no URL is built", async () => {
    assetInfo.mockResolvedValue(info({ viewable: false, mime: "image/heic" }));
    const w = mount(ViewerHost, { props: { path: "p/x.png" } });
    await flushPromises();
    expect(w.get('[data-testid="viewer-unsupported"]').text()).toContain("image/heic");
    expect(w.find("img").exists()).toBe(false);
  });

  it("an image over the 64 MiB serve cap is not requested; says so with a reveal action", async () => {
    assetInfo.mockResolvedValue(info({ bytes: 65 * 1024 * 1024 }));
    const w = mount(ViewerHost, { props: { path: "p/x.png" } });
    await flushPromises();
    expect(w.get('[data-testid="viewer-too-large"]').text()).toContain("couldn't be displayed");
    expect(w.find("img").exists()).toBe(false);
    await w.get('[data-testid="viewer-too-large"] button').trigger("click");
    expect(w.emitted("reveal")).toEqual([["p/x.png"]]);
  });

  it("a PDF that changes on disk restarts at the same page, re-reading asset_info, with a 4 s notice", async () => {
    vi.useFakeTimers();
    assetInfo.mockResolvedValue(info({ path: "d/x.pdf", mime: "application/pdf", bytes: 5000 }));
    const loads: Array<{ length: number; changed: () => void }> = [];
    const loader = vi.fn(async (_p: string, length: number, hooks: { onChanged: () => void }) => {
      loads.push({ length, changed: hooks.onChanged });
      return { numPages: 9, pageSize: async () => ({ width: 1, height: 1 }), render: async () => {}, destroy: () => {} };
    });
    const w = mount(ViewerHost, { props: { path: "d/x.pdf", pdfLoader: loader } });
    await flushPromises();
    expect(loads[0].length).toBe(5000);
    await w.get('[data-testid="pdf-page"]').setValue("4");
    await w.get('[data-testid="pdf-page"]').trigger("keydown", { key: "Enter" });
    assetInfo.mockResolvedValue(info({ path: "d/x.pdf", mime: "application/pdf", bytes: 7000 }));
    loads[0].changed();
    await flushPromises();
    expect(loads).toHaveLength(2);
    expect(loads[1].length).toBe(7000);
    expect((w.get('[data-testid="pdf-page"]').element as HTMLInputElement).value).toBe("4");
    expect(w.get('[data-testid="viewer-changed"]').text()).toContain("changed on disk");
    vi.advanceTimersByTime(4100);
    await w.vm.$nextTick();
    expect(w.find('[data-testid="viewer-changed"]').exists()).toBe(false);
    vi.useRealTimers();
  });

  it("a restart whose asset_info shows the same size is the error state, not another load", async () => {
    assetInfo.mockResolvedValue(info({ path: "d/x.pdf", mime: "application/pdf", bytes: 5000 }));
    const loads: Array<() => void> = [];
    const loader = vi.fn(async (_p: string, _l: number, hooks: { onChanged: () => void }) => {
      loads.push(hooks.onChanged);
      return new Promise<PdfDocumentHandle>(() => {});
    });
    const w = mount(ViewerHost, { props: { path: "d/x.pdf", pdfLoader: loader } });
    await flushPromises();
    loads[0]();
    await flushPromises();
    expect(loader).toHaveBeenCalledTimes(1);
    expect(w.find('[data-testid="viewer-error"]').exists()).toBe(true);
  });

  it("a path the URL builder refuses never reaches an <img>", async () => {
    assetInfo.mockResolvedValue(info({ path: ".serat/x.png" }));
    const w = mount(ViewerHost, { props: { path: ".serat/x.png" } });
    await flushPromises();
    expect(w.find("img").exists()).toBe(false);
    expect(w.find('[data-testid="viewer-error"]').exists()).toBe(true);
  });

  it("a stale answer for a superseded path is ignored", async () => {
    let first!: (v: AssetInfo) => void;
    assetInfo.mockReturnValueOnce(new Promise((r) => (first = r)));
    assetInfo.mockResolvedValueOnce(info({ path: "b.png", bytes: 10 }));
    const w = mount(ViewerHost, { props: { path: "a.png" } });
    await w.setProps({ path: "b.png" });
    await flushPromises();
    first(info({ path: "a.png", bytes: 99999 }));
    await flushPromises();
    expect(w.get("img").attributes("src")).toContain("b.png");
  });

  it("'Edit metadata note' opens the existing sidecar; with none it creates x.pdf.md (base_hash null) then opens it", async () => {
    assetInfo.mockResolvedValue(info({ path: "d/x.pdf", mime: "application/pdf", sidecar_path: "d/x.pdf.md" }));
    const loader = vi.fn(() => new Promise<PdfDocumentHandle>(() => {}));
    const w = mount(ViewerHost, { props: { path: "d/x.pdf", pdfLoader: loader } });
    await flushPromises();
    expect(w.get('[data-testid="viewer-edit-meta"]').text()).toBe("Edit metadata note");
    await w.get('[data-testid="viewer-edit-meta"]').trigger("click");
    expect(w.emitted("open-note")).toEqual([["d/x.pdf.md"]]);
    expect(writeFile).not.toHaveBeenCalled();

    assetInfo.mockResolvedValue(info({ path: "d/y.pdf", mime: "application/pdf", sidecar_path: null }));
    writeFile.mockResolvedValue({});
    const w2 = mount(ViewerHost, { props: { path: "d/y.pdf", pdfLoader: loader } });
    await flushPromises();
    await w2.get('[data-testid="viewer-edit-meta"]').trigger("click");
    await flushPromises();
    expect(writeFile).toHaveBeenCalledWith({ path: "d/y.pdf.md", text: "---\n\n---\n", base_hash: null });
    expect(w2.emitted("open-note")).toEqual([["d/y.pdf.md"]]);
  });

  it("creating over a sidecar that appeared meanwhile (stale_hash) just opens it; other failures say so", async () => {
    assetInfo.mockResolvedValue(info({ path: "d/y.pdf", mime: "application/pdf" }));
    const loader = () => new Promise<PdfDocumentHandle>(() => {});
    writeFile.mockRejectedValueOnce({ kind: "stale_hash", path: "d/y.pdf.md", current_hash: "h" });
    const w = mount(ViewerHost, { props: { path: "d/y.pdf", pdfLoader: loader } });
    await flushPromises();
    await w.get('[data-testid="viewer-edit-meta"]').trigger("click");
    await flushPromises();
    expect(w.emitted("open-note")).toEqual([["d/y.pdf.md"]]);

    writeFile.mockRejectedValueOnce({ kind: "io", path: "d/y.pdf.md", message: "x" });
    await w.get('[data-testid="viewer-edit-meta"]').trigger("click");
    await flushPromises();
    expect(w.emitted("open-note")).toHaveLength(1);
    expect(w.get('[data-testid="viewer-note"]').text()).toContain("could not be created");
  });
});

describe("ImageViewer", () => {
  const mountImg = () => mount(ImageViewer, { props: { url: "serat-asset://localhost/x.png", name: "x.png", mime: "image/png", bytes: 5 } });

  it("an undecodable image shows the stated fact, MIME and Reveal, never a broken glyph", async () => {
    const w = mountImg();
    await w.get("img").trigger("error");
    expect(w.get('[data-testid="image-failed"]').text()).toContain("This image couldn't be displayed");
    expect(w.get('[data-testid="image-mime"]').text()).toBe("image/png");
    expect(w.get("img").isVisible()).toBe(false);
    await w.get('[data-testid="image-reveal"]').trigger("click");
    expect(w.emitted("reveal")).toHaveLength(1);
  });

  it("zoom field: Enter applies a percent; actual size is 100%; the accessible name is the file name", async () => {
    const w = mountImg();
    const img = w.get("img");
    Object.defineProperty(img.element, "naturalWidth", { value: 200 });
    Object.defineProperty(img.element, "naturalHeight", { value: 100 });
    await img.trigger("load");
    expect(img.attributes("alt")).toBe("x.png");
    await w.get('[data-testid="image-zoom-field"]').setValue("250");
    await w.get('[data-testid="image-zoom-field"]').trigger("keydown", { key: "Enter" });
    expect(img.attributes("style")).toContain("width: 500px");
    await w.get('[data-testid="image-actual"]').trigger("click");
    expect(img.attributes("style")).toContain("width: 200px");
    await w.get('[data-testid="image-fit"]').trigger("click");
    expect(w.get('[data-testid="image-fit"]').attributes("aria-pressed")).toBe("true");
  });
});

describe("PdfViewer", () => {
  function fakeDoc(pages: number): PdfDocumentHandle & { rendered: number[]; destroyed: boolean } {
    const doc = {
      numPages: pages,
      rendered: [] as number[],
      destroyed: false,
      pageSize: async () => ({ width: 600, height: 800 }),
      render: async (n: number) => {
        doc.rendered.push(n);
      },
      destroy: () => {
        doc.destroyed = true;
      },
    };
    return doc;
  }

  it("loads through the injected loader with the URL and the asset_info length, shows page count, Enter jumps", async () => {
    const doc = fakeDoc(18);
    const loader = vi.fn().mockResolvedValue(doc);
    const w = mount(PdfViewer, { props: { path: "d/c.pdf", length: 123456, name: "c.pdf", loader } });
    expect(w.find('[data-testid="pdf-loading"]').exists()).toBe(true);
    await flushPromises();
    expect(loader).toHaveBeenCalledWith("d/c.pdf", 123456, expect.objectContaining({ onChanged: expect.any(Function), onFailed: expect.any(Function) }), expect.anything());
    expect(w.get('[data-testid="pdf-count"]').text()).toBe("of 18");
    const field = w.get('[data-testid="pdf-page"]');
    await field.setValue("4");
    await field.trigger("keydown", { key: "Enter" });
    await flushPromises();
    expect(doc.rendered.at(-1)).toBe(4);
    await w.get('[data-testid="pdf-next"]').trigger("click");
    expect(doc.rendered.at(-1)).toBe(5);
    expect((field.element as HTMLInputElement).value).toBe("5");
  });

  it("an out-of-range page reverts in place with the error flash and renders nothing new", async () => {
    vi.useFakeTimers();
    const doc = fakeDoc(3);
    const w = mount(PdfViewer, { props: { path: "d/c.pdf", length: 1, name: "c.pdf", loader: async () => doc } });
    await flushPromises();
    const before = doc.rendered.length;
    const field = w.get('[data-testid="pdf-page"]');
    await field.setValue("9");
    await field.trigger("keydown", { key: "Enter" });
    expect((field.element as HTMLInputElement).value).toBe("1");
    expect(field.classes()).toContain("flash");
    vi.advanceTimersByTime(130);
    await w.vm.$nextTick();
    expect(field.classes()).not.toContain("flash");
    expect(doc.rendered.length).toBe(before);
    vi.useRealTimers();
  });

  it("a load failure is stated, and unmounting destroys the document", async () => {
    const bad = mount(PdfViewer, { props: { path: "d/c.pdf", length: 1, name: "c.pdf", loader: async () => Promise.reject(new Error("x")) } });
    await flushPromises();
    expect(bad.get('[data-testid="pdf-error"]').text()).toContain("couldn't be displayed");
    const doc = fakeDoc(2);
    const ok = mount(PdfViewer, { props: { path: "d/c.pdf", length: 1, name: "c.pdf", loader: async () => doc } });
    await flushPromises();
    ok.unmount();
    expect(doc.destroyed).toBe(true);
  });

  it("a persistent read failure (size unchanged) is one load then the error state, no restart loop", async () => {
    const loader = vi.fn(async (_p: string, _l: number, hooks: { onFailed: () => void }) => {
      queueMicrotask(hooks.onFailed); // the transport's verdict
      return new Promise<PdfDocumentHandle>(() => {}); // pdf.js never settles: it is waiting on bytes
    });
    const w = mount(PdfViewer, { props: { path: "d/c.pdf", length: 10, name: "c.pdf", loader } });
    await flushPromises();
    expect(loader).toHaveBeenCalledTimes(1);
    expect(w.get('[data-testid="pdf-error"]').text()).toContain("couldn't be displayed");
    expect(w.emitted("changed")).toBeUndefined();
    expect((loader.mock.calls[0] as unknown[])[3]).toMatchObject({ aborted: true }); // the stuck load was released
  });

  it("unmounting while the load is still pending aborts it", async () => {
    let signal!: AbortSignal;
    const loader = vi.fn(async (_p: string, _l: number, _h: unknown, s?: AbortSignal) => {
      signal = s as AbortSignal;
      return new Promise<PdfDocumentHandle>(() => {});
    });
    const w = mount(PdfViewer, { props: { path: "d/c.pdf", length: 10, name: "c.pdf", loader } });
    await flushPromises();
    expect(signal.aborted).toBe(false);
    w.unmount();
    expect(signal.aborted).toBe(true);
  });

  it("arrow keys page when focus is not in the page field", async () => {
    const doc = fakeDoc(5);
    const w = mount(PdfViewer, { props: { path: "d/c.pdf", length: 1, name: "c.pdf", loader: async () => doc } });
    await flushPromises();
    await w.get("section").trigger("keydown", { key: "ArrowRight" });
    expect(doc.rendered.at(-1)).toBe(2);
    await w.get("section").trigger("keydown", { key: "ArrowLeft" });
    expect(doc.rendered.at(-1)).toBe(1);
  });
});
