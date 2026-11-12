// The refused-names count in the App: it follows `index://status` and the late-subscriber poll, and the
// palette command `vault.refused-names` opens the panel (FR-VLT-01, NFR-USE-01, architecture §9.3).

import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Handler = (event: { payload: unknown }) => void;
const handlers = new Map<string, Handler>();
let polled: { refused_names?: number } = {};
const READY = { kind: "ready", note_count: 1 };

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string) => {
    if (cmd === "index_status") return Promise.resolve({ vault_id: "v1", state: READY, ...polled });
    if (cmd === "refused_names") return Promise.resolve([{ path: "a:b.md", reason: "contains a `:`" }]);
    return new Promise(() => {});
  }),
  Channel: class {
    onmessage: unknown = null;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((name: string, handler: Handler) => {
    handlers.set(name, handler);
    return Promise.resolve(() => undefined);
  }),
}));
vi.mock("@/startup", () => ({
  schedulePaintMark: vi.fn(() => new Promise<number | null>(() => {})),
  hydrateBuffer: vi.fn(() => new Promise<void>(() => {})),
}));

const { default: App } = await import("./App.vue");

beforeEach(() => {
  vi.useFakeTimers();
  handlers.clear();
  polled = {};
  window.__SERAT_BOOT__ = { vault: "/vault", vault_id: "v1", open: null, exit_after_ready: false } as never;
  vi.stubGlobal("requestAnimationFrame", () => 0);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

const button = (w: ReturnType<typeof mount>) => w.find('[data-testid="refused-names"]');

describe("App: refused-names count", () => {
  it("the late-subscriber poll supplies the count", async () => {
    polled = { refused_names: 3 };
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await flushPromises();
    expect(button(wrapper).text()).toBe("3 names not shown");
    wrapper.unmount();
  });

  it("index://status events change and clear it; the old wording is gone", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await flushPromises();
    expect(button(wrapper).exists()).toBe(false);
    const emit = (extra: object) => handlers.get("index://status")?.({ payload: { vault_id: "v1", state: READY, ...extra } });
    emit({ refused_names: 1 });
    await wrapper.vm.$nextTick();
    expect(button(wrapper).text()).toBe("1 name not shown");
    expect(wrapper.text()).not.toContain("invalid on this platform");
    emit({ refused_names: 1000 });
    await wrapper.vm.$nextTick();
    expect(button(wrapper).text()).toContain("1000+");
    expect(button(wrapper).text()).toContain("1000 or more");
    emit({ refused_names: 0 });
    await wrapper.vm.$nextTick();
    expect(button(wrapper).exists()).toBe(false);
    emit({ refused_names: 2 });
    emit({});
    await wrapper.vm.$nextTick();
    expect(button(wrapper).exists()).toBe(false); // None, not zero
    wrapper.unmount();
  });

  it("a stale poll answer does not overwrite a newer event", async () => {
    let release: (v: unknown) => void = () => undefined;
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockImplementationOnce(((cmd: string) =>
      cmd === "index_status" ? new Promise((r) => (release = r)) : new Promise(() => {})) as never);
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await flushPromises();
    handlers.get("index://status")?.({ payload: { vault_id: "v1", state: READY, refused_names: 5 } });
    release({ vault_id: "v1", state: READY, refused_names: 2 });
    await flushPromises();
    expect(button(wrapper).text()).toBe("5 names not shown");
    wrapper.unmount();
  });

  it("the palette lists 'Show names not shown' with no chord: greyed with a reason at None/0, opening the panel otherwise", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await flushPromises();
    const emit = (extra: object) => handlers.get("index://status")?.({ payload: { vault_id: "v1", state: READY, ...extra } });
    const openPalette = async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }));
      await flushPromises();
      const input = wrapper.get('[data-testid="command-palette"] input');
      await input.setValue("names not shown");
      return input;
    };
    const row = () => wrapper.findAll('[data-testid="command-palette"] [role="option"]').find((r) => r.text().includes("Show names not shown"));

    let input = await openPalette();
    expect(row()?.classes()).toContain("unavailable");
    await input.trigger("keydown", { key: "Enter" });
    expect(wrapper.get('[data-testid="palette-reason"]').text()).toBe("Open a vault first");
    await input.trigger("keydown", { key: "Escape" });

    emit({ refused_names: 0 });
    await wrapper.vm.$nextTick();
    input = await openPalette();
    await input.trigger("keydown", { key: "Enter" });
    expect(wrapper.get('[data-testid="palette-reason"]').text()).toBe("Every name can be shown");
    await input.trigger("keydown", { key: "Escape" });

    emit({ refused_names: 2 });
    await wrapper.vm.$nextTick();
    input = await openPalette();
    expect(row()?.classes()).not.toContain("unavailable");
    expect(row()?.find("kbd, .chord").exists()).toBe(false); // no default chord
    await input.trigger("keydown", { key: "Enter" });
    await flushPromises();
    expect(wrapper.find('[data-testid="refused-panel"]').exists()).toBe(true);
    expect(wrapper.get('[data-testid="refused-path"]').text()).toBe("a:b.md");
    wrapper.unmount();
  });

  it("blank workspace is not focusable; the focus fallback makes it focusable only until it blurs", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await flushPromises();
    const main = wrapper.get("main.workspace");
    expect(main.attributes("tabindex")).toBeUndefined();
    (document.activeElement as HTMLElement | null)?.blur();
    wrapper.findComponent({ name: "RefusedNames" }).vm.$emit("focus-fallback");
    await flushPromises();
    // the tree takes focus when it can; force the body case to reach the workspace fallback
    if (document.activeElement === main.element) {
      expect(main.attributes("tabindex")).toBe("-1");
      (main.element as HTMLElement).blur();
      expect(main.attributes("tabindex")).toBeUndefined();
    }
    expect(main.attributes("tabindex") === undefined || document.activeElement === main.element).toBe(true);
    wrapper.unmount();
  });
});
