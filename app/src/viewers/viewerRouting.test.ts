// Task #26 routing through App: activating a PDF/image opens the viewer (FR-BIN-01, US-18),
// a note opens the editor again, and the sidecar opens as an ordinary note (FR-BIN-02).
import { mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const channels = vi.hoisted(() => [] as Array<{ onmessage: ((m: unknown) => void) | null }>);
const calls = vi.hoisted(() => [] as Array<{ cmd: string; args: unknown }>);
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string, args?: { args?: { path?: string } }) => {
    calls.push({ cmd, args: args?.args });
    const path = args?.args?.path ?? "";
    if (cmd === "read_file") return Promise.resolve({ path, text: "# hi", hash: "h1" });
    if (cmd === "asset_info")
      return Promise.resolve({
        path,
        mime: path.endsWith(".pdf") ? "application/pdf" : "image/png",
        bytes: 2048,
        viewable: true,
        sidecar_path: path === "docs/c.pdf" ? "docs/c.pdf.md" : null,
      });
    return new Promise(() => {});
  }),
  Channel: class {
    onmessage: ((m: unknown) => void) | null = null;
    constructor() {
      channels.push(this);
    }
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => new Promise(() => {})) }));
vi.mock("@/startup", () => ({
  schedulePaintMark: () => new Promise(() => {}),
  hydrateBuffer: () => new Promise(() => {}),
}));
// pdf.js is not exercised here (jsdom has no canvas); the viewer's loader is injected in viewers.test.ts.
vi.mock("@/viewers/pdfEngine", () => ({ loadPdf: () => new Promise(() => {}) }));

const { default: App } = await import("@/App.vue");

const treePush = (m: unknown) => channels[channels.length - 1].onmessage?.(m);

beforeEach(() => {
  window.__SERAT_BOOT__ = { vault: "/vault", open: "a.md", exit_after_ready: false };
  vi.stubGlobal("requestAnimationFrame", () => 0);
  calls.length = 0;
});
afterEach(() => vi.unstubAllGlobals());

describe("App routing to the viewers (FR-BIN-01)", () => {
  it("a PDF opens the viewer, not the editor; a note brings the editor back; read_file is never used for the binary", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    treePush({
      kind: "listing",
      dir: "docs",
      entries: [],
    });
    treePush({
      kind: "listing",
      dir: "",
      entries: [
        { name: "docs", kind: "dir", sidecar: false },
        { name: "pic.png", kind: "asset", sidecar: false },
        { name: "a.md", kind: "note", sidecar: false },
      ],
    });
    await wrapper.vm.$nextTick();
    expect(wrapper.find('[data-testid="viewer-host"]').exists()).toBe(false);

    await wrapper.get('[data-path="pic.png"]').trigger("click");
    await vi.waitFor(() => expect(wrapper.find('[data-testid="image-viewer"]').exists()).toBe(true));
    expect(wrapper.get(".serat-editor-surface").attributes("style")).toContain("display: none");
    expect(calls.some((c) => c.cmd === "read_file" && (c.args as { path: string }).path === "pic.png")).toBe(false);
    expect(wrapper.get('[data-testid="image-el"]').attributes("src")).toMatch(/^(serat-asset:\/\/localhost|http:\/\/serat-asset\.localhost)\/pic\.png$/);

    await wrapper.get('[data-path="a.md"]').trigger("click");
    await vi.waitFor(() => expect(wrapper.find('[data-testid="viewer-host"]').exists()).toBe(false));
    wrapper.unmount();
  });

  it("'Edit metadata note' opens the sidecar as an ordinary note in the editor (FR-BIN-02)", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    treePush({ kind: "listing", dir: "", entries: [{ name: "docs", kind: "dir", sidecar: false }] });
    treePush({ kind: "listing", dir: "docs", entries: [{ name: "c.pdf", kind: "asset", sidecar: true }] });
    await wrapper.vm.$nextTick();
    // expand docs
    await wrapper.get('[data-path="docs"]').trigger("click");
    await vi.waitFor(() => expect(wrapper.find('[data-path="docs/c.pdf"]').exists()).toBe(true));
    await wrapper.get('[data-path="docs/c.pdf"]').trigger("click");
    await vi.waitFor(() => expect(wrapper.find('[data-testid="pdf-viewer"]').exists()).toBe(true));

    await wrapper.get('[data-testid="viewer-edit-meta"]').trigger("click");
    await vi.waitFor(() => expect(wrapper.find('[data-testid="viewer-host"]').exists()).toBe(false));
    expect(calls.some((c) => c.cmd === "read_file" && (c.args as { path: string }).path === "docs/c.pdf.md")).toBe(true);
    wrapper.unmount();
  });
});
