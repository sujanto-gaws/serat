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
    // The leave snapshot survives the progress events (same kind); only a change of kind clears it.
    expect(live.textContent).toContain("Rebuilding index");
    // Non-waiting states carry their full text as the title.
    expect(wrapper.find('[data-testid="index-state"]').attributes("title")).toContain("Rebuilding index");
    await send({ kind: "ready", note_count: 3 });
    await flush();
    expect(announced).toEqual([]);
    expect(live.textContent).toBe("");

    observer.disconnect();
    wrapper.unmount();
    vi.mocked(listen).mockImplementation(() => new Promise(() => {}));
  });
});

describe("App status strip: held by another process (#155)", () => {
  it("renders text, title and describedby, then leaves through Opening{None} (not announced) to Ready (announced)", async () => {
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
    const announced: string[] = [];
    const observer = new MutationObserver(() => {
      if ((live.textContent ?? "") !== "") announced.push(live.textContent ?? "");
    });
    observer.observe(live, { childList: true, characterData: true, subtree: true });
    const flush = async () => {
      await new Promise<void>((r) => queueMicrotask(r));
      await new Promise<void>((r) => queueMicrotask(r));
    };
    const held = { kind: "opening", reason: "index_held_by_another_process" };
    await send(held);
    await flush();
    expect(announced).toEqual(["Another Serat session is using this vault's index — search will start when it's free"]);
    const strip = wrapper.find('[data-testid="index-state"]');
    expect(strip.attributes("title")).toContain("Only one Serat session can update a vault's index at a time.");
    expect(wrapper.find(`#${strip.attributes("aria-describedby")!}`).text()).toContain("You can keep editing.");
    for (let i = 0; i < 3; i++) await send(held);
    await flush();
    expect(announced).toHaveLength(1);
    await send({ kind: "opening" });
    await flush();
    expect(announced).toHaveLength(1); // the plain Opening after a cleared reason is not announced
    expect(wrapper.find('[data-testid="index-state"]').attributes("aria-describedby")).toBeUndefined();
    await send({ kind: "ready", note_count: 4 }); // the state after it is
    await flush();
    expect(announced).toHaveLength(2);
    expect(announced[1]).toBe("index: ready · 4 notes");
    observer.disconnect();
    wrapper.unmount();
    vi.mocked(listen).mockImplementation(() => new Promise(() => {}));
  });
});

describe("App status strip: index check found problems (#157)", () => {
  it("renders one quiet fact beside Rebuilding, announced once, cleared when the field goes absent", async () => {
    const { listen } = await import("@tauri-apps/api/event");
    const handlers = new Map<string, (e: { payload: unknown }) => void>();
    vi.mocked(listen).mockImplementation(((name: string, h: (e: { payload: unknown }) => void) => {
      handlers.set(name, h);
      return Promise.resolve(() => undefined);
    }) as never);
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    const send = async (state: unknown, extra: object = {}) => {
      handlers.get("index://status")!({ payload: { vault_id: "A", state, ...extra } });
      await wrapper.vm.$nextTick();
    };
    const live = wrapper.find('[data-testid="index-live"]').element;
    const announced: string[] = [];
    const observer = new MutationObserver(() => {
      if ((live.textContent ?? "") !== "") announced.push(live.textContent ?? "");
    });
    observer.observe(live, { childList: true, characterData: true, subtree: true });
    const flush = async () => {
      await new Promise<void>((r) => queueMicrotask(r));
      await new Promise<void>((r) => queueMicrotask(r));
    };
    const rebuilding = (done: number) => ({ kind: "rebuilding", reason: "corrupt", done, total: 10, walk_complete: false });
    expect(wrapper.find('[data-testid="index-check-problems"]').exists()).toBe(false);

    await send(rebuilding(0), { index_check_problems: 3 });
    await flush();
    const fact = wrapper.get('[data-testid="index-check-problems"]');
    expect(fact.text()).toBe("Index check found problems — rebuilding");
    expect(fact.text()).not.toMatch(/\d/);
    expect(fact.attributes("title")).toContain("3 problems");
    expect(wrapper.get(`#${fact.attributes("aria-describedby")!}`).text()).toContain("3 problems");
    expect(announced).toEqual(["Index check found problems — rebuilding"]);

    for (let done = 1; done < 6; done++) await send(rebuilding(done), { index_check_problems: 3 });
    await flush();
    expect(announced).toHaveLength(1);

    await send({ kind: "ready", note_count: 4 });
    await flush();
    expect(wrapper.find('[data-testid="index-check-problems"]').exists()).toBe(false);
    observer.disconnect();
    wrapper.unmount();
    vi.mocked(listen).mockImplementation(() => new Promise(() => {}));
  });
});
