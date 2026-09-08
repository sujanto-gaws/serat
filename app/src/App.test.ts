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

// `@/startup` is mocked separately from the `@tauri-apps/api` mock above so the
// second test below can tell "schedulePaintMark ran" apart from "CodeMirror's own
// internal requestAnimationFrame usage ran" — a global rAF spy cannot make that
// distinction (verified: it stayed green under the ordering bug this test exists to
// catch, because CodeMirror 6 calls requestAnimationFrame on its own during
// construction, independent of anything App.vue does).
const schedulePaintMark = vi.fn((..._args: unknown[]) => new Promise<number | null>(() => {}));
const hydrateBuffer = vi.fn((..._args: unknown[]) => new Promise<void>(() => {}));

vi.mock("@/startup", () => ({
  schedulePaintMark: (...args: unknown[]) => schedulePaintMark(...args),
  hydrateBuffer: (...args: unknown[]) => hydrateBuffer(...args),
}));

const { default: App } = await import("./App.vue");

beforeEach(() => {
  window.__SERAT_BOOT__ = { vault: "/vault", open: "a.md", exit_after_ready: false };
  // CodeMirror 6 uses requestAnimationFrame internally; this only has to exist and
  // not throw, since the assertions below no longer depend on it being called.
  vi.stubGlobal("requestAnimationFrame", () => 0);
  schedulePaintMark.mockClear();
  hydrateBuffer.mockClear();
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

  it("schedules the paint mark synchronously, ahead of the index-status subscription", () => {
    // code-reviewer's SHOULD-FIX on #98: onMounted used to `await onIndexStatus(...)`
    // — an IPC round trip — before calling schedulePaintMark, so the double rAF that
    // call schedules could miss the frame that actually painted first, inflating
    // S_shell (ADR-0014's binding Serat term) with time that has nothing to do with
    // painting. `listen` is mocked to a promise that never resolves, so under the
    // old ordering `schedulePaintMark` would never run in this test at all —
    // execution would be stuck behind that `await` forever. That
    // `schedulePaintMark` is called here, synchronously during `mount` and before
    // `onIndexStatus`'s underlying `listen` call could possibly have settled, is
    // only possible because it now runs first.
    mount(App, { global: { plugins: [createPinia()] } });
    expect(schedulePaintMark).toHaveBeenCalledTimes(1);
  });
});
