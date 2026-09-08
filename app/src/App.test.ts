import { mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// `@tauri-apps/api` is mocked to never resolve, so these assertions hold under the
// worst case for the property under test: the editor must exist and be interactive
// even while every IPC round trip is still in flight. That is the P0 placeholder
// shell's own property (crates/serat-app/webview/index.html: "the editor accepts
// keystrokes before this, because it is in the static markup") — Vue mounting the
// DOM instead of the browser parsing it is the honest cost task #98 was asked to
// report, and this test is what would catch that cost regressing further, e.g. an
// `await` landing before the editor is created.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => new Promise(() => {})),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => new Promise(() => {})),
}));

const { default: App } = await import("./App.vue");

beforeEach(() => {
  window.__SERAT_BOOT__ = { vault: "/vault", open: "a.md", exit_after_ready: false };
  vi.stubGlobal("requestAnimationFrame", () => 0);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("App", () => {
  it("mounts a focusable editor before any IPC round trip has resolved", () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] } });

    const content = wrapper.find(".cm-content");
    expect(content.exists()).toBe(true);
    expect(content.attributes("contenteditable")).toBe("true");
  });
});
