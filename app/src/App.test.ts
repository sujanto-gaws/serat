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
// `read_file` answers (a tree click opens a note); every other command stays in flight.
const channels = vi.hoisted(() => [] as Array<{ onmessage: ((m: unknown) => void) | null }>);
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string, args?: { args?: { path?: string } }) =>
    cmd === "read_file"
      ? Promise.resolve({ path: args?.args?.path ?? "", text: "# hi", hash: "h1" })
      : new Promise(() => {}),
  ),
  Channel: class {
    onmessage: ((m: unknown) => void) | null = null;
    constructor() {
      channels.push(this);
    }
  },
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
  void import("@tauri-apps/api/event").then(({ listen }) => vi.mocked(listen).mockImplementation(() => new Promise(() => {})));
});

describe("App", () => {
  it("mounts the editor synchronously, read-only until a note buffer is installed (C-04)", () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] } });

    // CodeMirror mounts into a shadow root (code-reviewer-2's CSP finding), so
    // `wrapper.find` — light-DOM only, like `querySelector` — cannot see `.cm-content`
    // directly; go through the host element's own `shadowRoot`.
    const host = wrapper.find(".cm-editor-host").element as HTMLElement;
    const content = host.shadowRoot?.querySelector(".cm-content");
    expect(content).not.toBeNull();
    // Not editable yet: typing before `loadNote` would have no base to patch against.
    expect(content?.getAttribute("contenteditable")).toBe("false");
  });

  it("accepts keystrokes from the moment hydrateBuffer installs the document, before the usable mark (NFR-PRF-01)", async () => {
    const { listen } = await import("@tauri-apps/api/event");
    // Every `listen` call resolves in this test (a later task adds more subscribers).
    vi.mocked(listen).mockImplementation(() => Promise.resolve(() => undefined));
    let loaded: Promise<boolean> | null = null;
    // hydrateBuffer installs the note right after reading it, then waits for
    // the paint mark / usable mark (which never resolve here).
    hydrateBuffer.mockImplementationOnce((_store: unknown, editor: unknown) => {
      loaded = (editor as { loadNote(t: string, p: string, h: string): Promise<boolean> }).loadNote("# hi", "a.md", "h0");
      return new Promise<void>(() => {});
    });
    const wrapper = mount(App, { global: { plugins: [createPinia()] } });
    await vi.waitFor(() => expect(loaded).not.toBeNull());
    await loaded;

    const host = wrapper.find(".cm-editor-host").element as HTMLElement;
    const content = host.shadowRoot?.querySelector(".cm-content");
    expect(content?.getAttribute("contenteditable")).toBe("true");
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

  it("Mod-Shift-F opens and focuses search and never toggles it closed; Esc closes it and returns focus", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    const press = (init: KeyboardEventInit) =>
      window.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
    expect(wrapper.find('[data-testid="search-panel"]').exists()).toBe(false);
    press({ key: "F", ctrlKey: true, shiftKey: true });
    await vi.waitFor(() => expect(wrapper.find('[data-testid="search-panel"]').exists()).toBe(true));
    await wrapper.vm.$nextTick();
    expect(document.activeElement).toBe(wrapper.get('[data-testid="search-input"]').element);
    press({ key: "F", ctrlKey: true, shiftKey: true });
    await wrapper.vm.$nextTick();
    expect(wrapper.find('[data-testid="search-panel"]').exists()).toBe(true); // second press focuses, does not close
    await wrapper.get('[data-testid="search-input"]').trigger("keydown", { key: "Escape" });
    expect(wrapper.find('[data-testid="search-panel"]').exists()).toBe(false);
    wrapper.unmount();
  });

  // CodeMirror also asks matchMedia("print") and subscribes, so the stub carries the listener API.
  const mq = (matches: boolean) => ({
    matches,
    addListener: () => undefined,
    removeListener: () => undefined,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  });
  const treePush = (m: unknown) => channels[channels.length - 1].onmessage?.(m);
  const ent = (name: string, kind: "note" | "dir") => ({ name, kind, sidecar: false });

  it("Mod-3 focuses backlinks; Mod-Shift-N is a prevented no-op (new folder is task #43)", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "3", ctrlKey: true, bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(document.activeElement).toBe(wrapper.get('[data-testid="backlinks"]').element));
    const ev = new KeyboardEvent("keydown", { key: "N", ctrlKey: true, shiftKey: true, bubbles: true, cancelable: true });
    window.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(true);
    expect(wrapper.find(".marks").text()).toBe("");
    wrapper.unmount();
  });

  it("flyover below 720 px: Mod-1 opens it (tree-open), opening a note closes it and focuses the editor", async () => {
    vi.stubGlobal("matchMedia", (q: string) => mq(q.includes("719px")));
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    expect(wrapper.get(".workspace").classes()).not.toContain("tree-open");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "1", ctrlKey: true, bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(wrapper.get(".workspace").classes()).toContain("tree-open"));
    treePush({ kind: "listing", dir: "", entries: [ent("a.md", "note")] });
    await wrapper.vm.$nextTick();
    await wrapper.get('[data-path="a.md"]').trigger("click");
    await vi.waitFor(() => expect(wrapper.get(".workspace").classes()).not.toContain("tree-open"));
    const host = wrapper.get(".cm-editor-host").element;
    await vi.waitFor(() => expect(host === document.activeElement || host.contains(document.activeElement)).toBe(true));
    wrapper.unmount();
  });

  it("Mod-1 docked (>= 720 px) focuses the tree without opening a flyover", async () => {
    vi.stubGlobal("matchMedia", () => mq(false));
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "1", ctrlKey: true, bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(document.activeElement).toBe(wrapper.get('[role="tree"]').element));
    expect(wrapper.get(".workspace").classes()).not.toContain("tree-open");
    wrapper.unmount();
  });

  it("the tree's remove of the open note shows 'no longer exists'; an upsert of the same path clears it", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    treePush({ kind: "listing", dir: "", entries: [ent("a.md", "note")] });
    await wrapper.vm.$nextTick();
    await wrapper.get('[data-path="a.md"]').trigger("click");
    await vi.waitFor(() => expect(wrapper.get('[data-testid="backlinks"]').text()).not.toContain("no longer exists"));
    treePush({ kind: "remove", dir: "", name: "a.md" });
    await vi.waitFor(() => expect(wrapper.get('[data-testid="backlinks"]').text()).toContain("This note no longer exists"));
    treePush({ kind: "upsert", dir: "", entry: ent("a.md", "note"), after: null });
    await vi.waitFor(() => expect(wrapper.get('[data-testid="backlinks"]').text()).not.toContain("no longer exists"));
    wrapper.unmount();
  });

  it("selecting a pending folder shows 'Listing…' in the context pane until its listing lands", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    treePush({ kind: "listing", dir: "", entries: [ent("Later", "dir")] });
    await wrapper.vm.$nextTick();
    await wrapper.get('[data-path="Later"]').trigger("click");
    await vi.waitFor(() => expect(wrapper.get('[data-testid="listing-note"]').text()).toBe("Listing…"));
    treePush({ kind: "listing", dir: "Later", entries: [ent("x.md", "note")] });
    await vi.waitFor(() => expect(wrapper.find('[data-testid="listing-note"]').exists()).toBe(false));
    wrapper.unmount();
  });
});
