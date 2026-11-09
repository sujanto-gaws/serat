// The status-strip half of #23 (US-06, FR-VLT-04/05): "Updated externally" for 3 s after a merged
// external edit (external-change-notice.md outcome table) and the persistent "Not saved - file changed
// on disk" line with a working Compare (editor.md, StaleHash sequencing).

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
const { default: CodeMirrorEditor } = await import("@/editor/CodeMirrorEditor.vue");

beforeEach(() => {
  vi.useFakeTimers();
  window.__SERAT_BOOT__ = { vault: "/vault", open: null, exit_after_ready: false };
  vi.stubGlobal("requestAnimationFrame", () => 0);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("App status strip: external changes", () => {
  it("external-merged shows 'Updated externally' for 3 s, then the normal state returns", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] } });
    const editor = wrapper.findComponent(CodeMirrorEditor);
    expect(wrapper.find('[data-testid="updated-externally"]').exists()).toBe(false);
    editor.vm.$emit("external-merged", { path: "a.md" });
    await wrapper.vm.$nextTick();
    expect(wrapper.find('[data-testid="updated-externally"]').text()).toBe("Updated externally");
    await vi.advanceTimersByTimeAsync(2900);
    expect(wrapper.find('[data-testid="updated-externally"]').exists()).toBe(true);
    await vi.advanceTimersByTimeAsync(200);
    expect(wrapper.find('[data-testid="updated-externally"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("a second merge restarts the 3 s window", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] } });
    const editor = wrapper.findComponent(CodeMirrorEditor);
    editor.vm.$emit("external-merged", { path: "a.md" });
    await vi.advanceTimersByTimeAsync(2000);
    editor.vm.$emit("external-merged", { path: "a.md" });
    await vi.advanceTimersByTimeAsync(2000);
    expect(wrapper.find('[data-testid="updated-externally"]').exists()).toBe(true);
    await vi.advanceTimersByTimeAsync(1100);
    expect(wrapper.find('[data-testid="updated-externally"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("save-blocked shows the persistent line with a Compare that reaches the editor; clearing it removes the line", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] } });
    const editor = wrapper.findComponent(CodeMirrorEditor);
    editor.vm.$emit("save-blocked", true);
    await wrapper.vm.$nextTick();
    const line = wrapper.find('[data-testid="save-blocked"]');
    expect(line.text()).toContain("Not saved — file changed on disk");
    await vi.advanceTimersByTimeAsync(10000);
    expect(wrapper.find('[data-testid="save-blocked"]').exists()).toBe(true); // persistent, not a toast

    // End to end through the default adapter: Compare reads the disk and opens the in-place diff.
    const { invoke } = await import("@tauri-apps/api/core");
    vi.mocked(invoke).mockImplementation(((cmd: string) =>
      cmd === "read_file"
        ? Promise.resolve({ path: "a.md", text: "ON DISK", hash: "hD", mtime_ms: 0, bytes: 7 })
        : Promise.resolve(undefined)) as never);
    await (editor.vm as unknown as { loadNote(t: string, p: string, h: string): Promise<boolean> }).loadNote("mine", "a.md", "h0");
    await wrapper.find('[data-testid="blocked-compare"]').trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    expect(wrapper.find('[data-testid="compare-disk"]').text()).toContain("ON DISK");
    expect(wrapper.find('[data-testid="compare-mine"]').text()).toContain("mine");
    vi.mocked(invoke).mockImplementation(() => new Promise(() => {}));

    editor.vm.$emit("save-blocked", false);
    await wrapper.vm.$nextTick();
    expect(wrapper.find('[data-testid="save-blocked"]').exists()).toBe(false);
    wrapper.unmount();
  });
});

describe("App wiring of the vault id sources (#91)", () => {
  it("reads BootInfo.vault_id at launch, then follows vault://opened; index_status cannot override either", async () => {
    window.__SERAT_BOOT__ = { vault: "/vault", vault_id: "A", open: null, exit_after_ready: false };
    const { listen } = await import("@tauri-apps/api/event");
    const { invoke } = await import("@tauri-apps/api/core");
    const handlers = new Map<string, (e: { payload: unknown }) => void>();
    vi.mocked(listen).mockImplementation(((name: string, h: (e: { payload: unknown }) => void) => {
      handlers.set(name, h);
      return Promise.resolve(() => undefined);
    }) as never);
    vi.mocked(invoke).mockImplementation(((cmd: string) =>
      cmd === "index_status"
        ? Promise.resolve({ vault_id: "OTHER", state: { kind: "opening" } })
        : new Promise(() => {})) as never);
    const pinia = createPinia();
    const wrapper = mount(App, { global: { plugins: [pinia] } });
    const { useVaultStore } = await import("@/stores/vault");
    const store = useVaultStore(pinia);
    await vi.advanceTimersByTimeAsync(0);
    expect(store.vaultId).toBe("A");
    handlers.get("vault://opened")!({ payload: { root: "/b", vault_id: "B", index: { kind: "opening" } } });
    expect(store.vaultId).toBe("B");
    handlers.get("index://status")!({ payload: { vault_id: "A", state: { kind: "opening" } } });
    expect(store.vaultId).toBe("B");
    wrapper.unmount();
    vi.mocked(listen).mockImplementation(() => new Promise(() => {}));
    vi.mocked(invoke).mockImplementation(() => new Promise(() => {}));
  });
});

describe("App drops index status from another vault (#91, IndexStatus.vault_id)", () => {
  it("applies a status for the current vault; drops another vault's and the empty id; stays dropped after a failed open", async () => {
    window.__SERAT_BOOT__ = { vault: "/vault", vault_id: "A", open: null, exit_after_ready: false };
    const { listen } = await import("@tauri-apps/api/event");
    const handlers = new Map<string, (e: { payload: unknown }) => void>();
    vi.mocked(listen).mockImplementation(((name: string, h: (e: { payload: unknown }) => void) => {
      handlers.set(name, h);
      return Promise.resolve(() => undefined);
    }) as never);
    const pinia = createPinia();
    const wrapper = mount(App, { global: { plugins: [pinia] } });
    const { useVaultStore } = await import("@/stores/vault");
    const store = useVaultStore(pinia);
    await vi.advanceTimersByTimeAsync(0);
    const send = (vault_id: string, note_count: number) =>
      handlers.get("index://status")!({ payload: { vault_id, state: { kind: "ready", note_count } } });
    send("A", 1);
    expect(store.indexStatusText).toContain("1 notes");
    send("OTHER", 2);
    send("", 3);
    expect(store.indexStatusText).toContain("1 notes");
    handlers.get("vault://opened")!({ payload: { root: "/b", vault_id: "B", index: { kind: "opening" } } });
    store.openFailed();
    send("A", 4); // the old vault's late status after the failed open
    send("B", 5); // the announced id's own status, emitted before the Err and delivered after (#103)
    expect(store.indexStatusText).toContain("1 notes");
    expect(store.vaultRoot).toBeNull();
    handlers.get("vault://opened")!({ payload: { root: "/c", vault_id: "C", index: { kind: "opening" } } });
    send("B", 6);
    expect(store.indexStatusText).toContain("1 notes");
    send("C", 7);
    expect(store.indexStatusText).toContain("7 notes");
    wrapper.unmount();
    vi.mocked(listen).mockImplementation(() => new Promise(() => {}));
  });
});
