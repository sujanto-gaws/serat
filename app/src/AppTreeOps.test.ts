// Task #43 at App level (FR-VLT-11, US-41; ADR-0016): the IPC the create flows use, the chords, the palette
// command, the note opening after creation, and F2 saying why rename is not here yet.

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
const { default: FileTree } = await import("@/tree/FileTree.vue");
const { invoke } = await import("@tauri-apps/api/core");
const { useVaultStore } = await import("@/stores/vault");

type CreateEntry = (r: { kind: "note" | "folder"; dir: string; name: string }) => Promise<string | null>;

function answers(overrides: Record<string, (args: unknown) => unknown> = {}) {
  vi.mocked(invoke).mockImplementation(((cmd: string, payload?: { args?: unknown }) => {
    const hook = overrides[cmd];
    if (hook) return Promise.resolve(hook(payload?.args));
    if (cmd === "index_status") return Promise.resolve({ vault_id: "v", state: { kind: "opening" } });
    if (cmd === "read_file") return Promise.resolve({ path: (payload?.args as { path: string }).path, text: "", hash: "h1", mtime_ms: 0, bytes: 0 });
    return Promise.resolve(undefined);
  }) as never);
}

beforeEach(() => {
  window.__SERAT_BOOT__ = { vault: "/vault", open: null, exit_after_ready: false };
  vi.stubGlobal("requestAnimationFrame", () => 0);
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.mocked(invoke).mockImplementation(() => new Promise(() => {}));
});

function mountApp() {
  const pinia = createPinia();
  const wrapper = mount(App, { global: { plugins: [pinia] }, attachTo: document.body });
  const create = wrapper.findComponent(FileTree).props("createEntry") as CreateEntry;
  return { wrapper, create, store: useVaultStore(pinia) };
}

describe("App: creating notes and folders", () => {
  it("a note is write_file with base_hash null and empty text (no template), then opens in the editor", async () => {
    answers({ write_file: () => ({ path: "d/idea.md", hash: "h1", mtime_ms: 0, bytes: 0 }) });
    const { wrapper, create, store } = mountApp();
    expect(await create({ kind: "note", dir: "d", name: "idea" })).toBeNull();
    expect(invoke).toHaveBeenCalledWith("write_file", { args: { path: "d/idea.md", text: "", base_hash: null } });
    await vi.waitFor(() => expect(store.openNote).toBe("d/idea.md"));
    expect(invoke).toHaveBeenCalledWith("read_file", { args: { path: "d/idea.md" } });
    wrapper.unmount();
  });

  it("a folder is create_folder; an existing one (created: false) is reported, not hidden", async () => {
    answers({ create_folder: () => ({ path: "Ideas", created: true }) });
    const { wrapper, create } = mountApp();
    expect(await create({ kind: "folder", dir: "", name: "Ideas" })).toBeNull();
    expect(invoke).toHaveBeenCalledWith("create_folder", { args: { path: "Ideas" } });
    answers({ create_folder: () => ({ path: "Ideas", created: false }) });
    expect(await create({ kind: "folder", dir: "", name: "Ideas" })).toContain("already exists");
    wrapper.unmount();
  });

  it("the core's refusals come back as sentences and no note opens", async () => {
    const { wrapper, create, store } = mountApp();
    vi.mocked(invoke).mockRejectedValueOnce({ kind: "path_escapes_vault", path: "../x.md" });
    expect(await create({ kind: "note", dir: "", name: "../x" })).toContain("refused that name");
    vi.mocked(invoke).mockRejectedValueOnce({ kind: "stale_hash", path: "a.md", current_hash: "h" });
    expect(await create({ kind: "note", dir: "", name: "a" })).toContain("already exists");
    vi.mocked(invoke).mockRejectedValueOnce({ kind: "not_found", path: "nope" });
    expect(await create({ kind: "folder", dir: "nope", name: "x" })).toContain("does not exist");
    expect(store.openNote).toBeNull();
    wrapper.unmount();
  });

  it("an empty name is refused before any command", async () => {
    answers();
    const { wrapper, create } = mountApp();
    vi.mocked(invoke).mockClear();
    expect(await create({ kind: "note", dir: "", name: "  " })).toBe("Give it a name.");
    expect(invoke).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("Ctrl-N and Ctrl-Shift-N open the tree's prompt (the registry's tree.newNote / tree.newFolder)", async () => {
    answers();
    const { wrapper } = mountApp();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "n", ctrlKey: true, bubbles: true }));
    await vi.waitFor(() => expect(wrapper.find('[data-testid="tree-prompt"]').exists()).toBe(true));
    expect(wrapper.get('[data-testid="tree-prompt"]').text()).toContain("New note");
    await wrapper.get('[data-testid="tree-prompt"]').trigger("keydown", { key: "Escape" });
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "N", ctrlKey: true, shiftKey: true, bubbles: true }));
    await vi.waitFor(() => expect(wrapper.get('[data-testid="tree-prompt"]').text()).toContain("New folder"));
    wrapper.unmount();
  });

  it("the toolbar buttons are enabled in the running app", () => {
    answers();
    const { wrapper } = mountApp();
    expect(wrapper.get('[data-testid="tree-new-note"]').attributes("disabled")).toBeUndefined();
    expect(wrapper.get('[data-testid="tree-new-folder"]').attributes("disabled")).toBeUndefined();
    wrapper.unmount();
  });

  it("F2 says rename arrives with link repair instead of doing nothing", async () => {
    answers();
    const { wrapper, store } = mountApp();
    wrapper.findComponent(FileTree).vm.$emit("rename", { path: "a.md" });
    expect(store.marksText).toContain("link repair");
    wrapper.unmount();
  });
});
