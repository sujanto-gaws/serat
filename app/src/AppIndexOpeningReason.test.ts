// The status strip's Opening-reason rendering and its one-shot live region (P1 #117; NFR-REL-01,
// NFR-REL-02, FR-VLT-04, US-01, US-07; index-state-copy.md "Announcement").

import { mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => new Promise(() => {})),
  Channel: class {
    onmessage: unknown = null;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => new Promise(() => {})),
}));
vi.mock("@/startup", () => ({
  schedulePaintMark: vi.fn(() => new Promise<number | null>(() => {})),
  hydrateBuffer: vi.fn(() => new Promise<void>(() => {})),
}));

const { default: App } = await import("./App.vue");

beforeEach(() => {
  vi.useFakeTimers();
  window.__SERAT_BOOT__ = { vault: "/vault", vault_id: "A", open: null, exit_after_ready: false };
  vi.stubGlobal("requestAnimationFrame", () => 0);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const WAITING = "Finishing the previous session's index — search will start when it's done";

describe("App status strip: Opening reason", () => {
  it("binds the text, title and description, and announces entering and leaving exactly once each", async () => {
    const { listen } = await import("@tauri-apps/api/event");
    const handlers = new Map<string, (e: { payload: unknown }) => void>();
    vi.mocked(listen).mockImplementation(((name: string, h: (e: { payload: unknown }) => void) => {
      handlers.set(name, h);
      return Promise.resolve(() => undefined);
    }) as never);
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    const send = async (state: unknown) => {
      handlers.get("index://status")!({ payload: { vault_id: "A", state } });
      await wrapper.vm.$nextTick();
    };
    const live = wrapper.find('[data-testid="index-live"]').element;
    let announced: string[] = [];
    // One entry per mutation batch that leaves non-empty text (clearing to "" is not an announcement).
    const observer = new MutationObserver(() => {
      if ((live.textContent ?? "") !== "") announced.push(live.textContent ?? "");
    });
    observer.observe(live, { childList: true, characterData: true, subtree: true });
    const flush = async () => {
      await new Promise<void>((r) => queueMicrotask(r));
      await new Promise<void>((r) => queueMicrotask(r));
    };
    // Drain anything from startup, then start counting.
    observer.takeRecords();
    announced = [];

    const waiting = { kind: "opening", reason: "previous_index_still_closing" };
    await send(waiting);
    await flush();
    expect(announced).toEqual([WAITING]);
    const strip = wrapper.find('[data-testid="index-state"]');
    expect(strip.text()).toBe(WAITING);
    expect(strip.attributes("title")).toContain("You can keep editing.");
    const describedBy = strip.attributes("aria-describedby")!;
    expect(wrapper.find(`#${describedBy}`).text()).toContain("You can keep editing.");

    announced = [];
    for (let i = 0; i < 5; i++) await send(waiting);
    await flush();
    expect(announced).toEqual([]);

    await send({ kind: "rebuilding", reason: "no_index", done: 1, total: 10, walk_complete: false });
    await flush();
    expect(announced).toHaveLength(1);
    expect(announced[0]).toContain("Rebuilding index");
    expect(wrapper.find('[data-testid="index-state"]').attributes("aria-describedby")).toBeUndefined();

    announced = [];
    for (let done = 2; done < 7; done++) {
      await send({ kind: "rebuilding", reason: "no_index", done, total: 10, walk_complete: false });
    }
    await flush();
    expect(announced).toEqual([]);
    expect(live.textContent).toBe("");
    // Non-waiting states carry their full text as the title.
    expect(wrapper.find('[data-testid="index-state"]').attributes("title")).toContain("Rebuilding index");

    observer.disconnect();
    wrapper.unmount();
    vi.mocked(listen).mockImplementation(() => new Promise(() => {}));
  });
});
