// `vault.open` (task #151 vue half; US-01, FR-VLT-01, FR-NAV-06; ADR-0012, architecture §9.3): the palette
// command, the "No vault open" empty state, and the adopt path (#91/#103 id rules, #80's close-before-adopt).

import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Handler = (event: { payload: unknown }) => void;
const handlers = new Map<string, Handler>();
let chooseImpl: () => Promise<unknown> = () => Promise.resolve(null);
const chooseCalls = vi.hoisted(() => ({ n: 0, openTree: 0 }));
const treeChannels = vi.hoisted(() => [] as Array<{ onmessage: ((m: unknown) => void) | null }>);
let savePatch: () => Promise<unknown> = () => Promise.reject(new Error("disk full"));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string, args?: { onMessage?: { onmessage: ((m: unknown) => void) | null } }) => {
    if (cmd === "open_tree") {
      chooseCalls.openTree += 1;
      if (args?.onMessage) treeChannels.push(args.onMessage);
    }
    if (cmd === "save_patch") return savePatch();
    if (cmd === "choose_vault") {
      chooseCalls.n += 1;
      return chooseImpl();
    }
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
const { default: CodeMirrorEditor } = await import("@/editor/CodeMirrorEditor.vue");
const { useVaultStore } = await import("@/stores/vault");

const OPENED_B = { root: "/b", vault_id: "B", index: { kind: "opening" } };

beforeEach(() => {
  vi.useFakeTimers();
  handlers.clear();
  chooseCalls.n = 0;
  chooseCalls.openTree = 0;
  treeChannels.length = 0;
  savePatch = () => Promise.reject(new Error("disk full"));
  chooseImpl = () => Promise.resolve(null);
  window.__SERAT_BOOT__ = { vault: "/a", vault_id: "A", open: null, exit_after_ready: false } as never;
  vi.stubGlobal("requestAnimationFrame", () => 0);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

async function paletteRow(wrapper: ReturnType<typeof mount>) {
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }));
  await flushPromises();
  await wrapper.get('[data-testid="command-palette"] input').setValue("Open vault");
  return wrapper.findAll('[data-testid="command-palette"] [role="option"]').find((r) => r.text().includes("Open vault…"));
}

describe("vault.open in the palette", () => {
  it("is listed with no chord and available both with a vault open and with none", async () => {
    for (const boot of [{ vault: "/a", vault_id: "A" }, {}]) {
      window.__SERAT_BOOT__ = { ...boot, open: null, exit_after_ready: false } as never;
      const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
      await flushPromises();
      const row = await paletteRow(wrapper);
      expect(row, JSON.stringify(boot)).toBeDefined();
      expect(row!.classes()).not.toContain("unavailable");
      expect(row!.find("kbd, .chord").exists()).toBe(false);
      wrapper.unmount();
      document.body.innerHTML = "";
    }
  });

  it("running it calls choose_vault", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await flushPromises();
    await paletteRow(wrapper);
    await wrapper.get('[data-testid="command-palette"] input').trigger("keydown", { key: "Enter" });
    await flushPromises();
    expect(chooseCalls.n).toBe(1);
    wrapper.unmount();
  });
});

describe("the No vault open empty state", () => {
  it("is absent with a vault open and present with none; its button calls choose_vault", async () => {
    const withVault = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await flushPromises();
    expect(withVault.find('[data-testid="no-vault"]').exists()).toBe(false);
    withVault.unmount();

    window.__SERAT_BOOT__ = { open: null, exit_after_ready: false } as never;
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await flushPromises();
    expect(wrapper.get('[data-testid="no-vault"]').text()).toContain("No vault open");
    await wrapper.get('[data-testid="no-vault-open"]').trigger("click");
    await flushPromises();
    expect(chooseCalls.n).toBe(1);
    wrapper.unmount();
  });

  it("appears after a failed open (#103) once the announced vault was dropped", async () => {
    const pinia = createPinia();
    const wrapper = mount(App, { global: { plugins: [pinia] }, attachTo: document.body });
    await flushPromises();
    chooseImpl = () => {
      handlers.get("vault://opened")?.({ payload: OPENED_B });
      return Promise.reject({ kind: "vault_not_found", path: "/b" });
    };
    await wrapper.vm.$nextTick();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }));
    await flushPromises();
    await wrapper.get('[data-testid="command-palette"] input').setValue("Open vault");
    await wrapper.get('[data-testid="command-palette"] input').trigger("keydown", { key: "Enter" });
    await flushPromises();
    expect(useVaultStore(pinia).vaultOpen).toBe(false);
    expect(wrapper.find('[data-testid="no-vault"]').exists()).toBe(true);
    wrapper.unmount();
  });
});

describe("adopting a new vault", () => {
  async function withStaleBuffer() {
    const pinia = createPinia();
    const wrapper = mount(App, { global: { plugins: [pinia] }, attachTo: document.body });
    await flushPromises();
    const editor = wrapper.findComponent(CodeMirrorEditor);
    await (editor.vm as unknown as { loadNote(t: string, p: string, h: string): Promise<boolean> }).loadNote("old note", "a.md", "h1");
    expect((editor.vm as unknown as { getText(): string }).getText()).toBe("old note");
    return { pinia, wrapper, editor: editor.vm as unknown as { getText(): string } };
  }

  it("a non-null result closes the previous vault's buffer, then adopts the new id", async () => {
    const { pinia, wrapper, editor } = await withStaleBuffer();
    const store = useVaultStore(pinia);
    store.setOpenBuffer("a.md", "h1");
    chooseImpl = () => Promise.resolve(OPENED_B);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }));
    await flushPromises();
    await wrapper.get('[data-testid="command-palette"] input').setValue("Open vault");
    await wrapper.get('[data-testid="command-palette"] input').trigger("keydown", { key: "Enter" });
    await flushPromises();
    expect(editor.getText()).toBe("");
    expect(store.openNote).toBeNull();
    expect(store.vaultId).toBe("B");
    expect(store.vaultRoot).toBe("/b");
    wrapper.unmount();
  });

  it("the vault://opened event arriving first does the same", async () => {
    const { pinia, wrapper, editor } = await withStaleBuffer();
    handlers.get("vault://opened")?.({ payload: OPENED_B });
    await flushPromises();
    expect(editor.getText()).toBe("");
    expect(useVaultStore(pinia).vaultId).toBe("B");
    wrapper.unmount();
  });

  it("null (cancelled) changes nothing: buffer, id and root stay", async () => {
    const { pinia, wrapper, editor } = await withStaleBuffer();
    const store = useVaultStore(pinia);
    chooseImpl = () => Promise.resolve(null);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }));
    await flushPromises();
    await wrapper.get('[data-testid="command-palette"] input').setValue("Open vault");
    await wrapper.get('[data-testid="command-palette"] input').trigger("keydown", { key: "Enter" });
    await flushPromises();
    expect(chooseCalls.n).toBe(1);
    expect(editor.getText()).toBe("old note");
    expect(store.vaultId).toBe("A");
    expect(store.vaultRoot).toBe("/a");
    wrapper.unmount();
  });
});

describe("the file tree follows the chosen vault (#151 round 2)", () => {
  async function runCommand(wrapper: ReturnType<typeof mount>) {
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }));
    await flushPromises();
    await wrapper.get('[data-testid="command-palette"] input').setValue("Open vault");
    await wrapper.get('[data-testid="command-palette"] input').trigger("keydown", { key: "Enter" });
    await flushPromises();
  }

  it("open_tree is invoked again after a non-null answer, from the no-vault state", async () => {
    window.__SERAT_BOOT__ = { open: null, exit_after_ready: false } as never;
    chooseImpl = () => Promise.resolve(OPENED_B);
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await flushPromises();
    const before = chooseCalls.openTree;
    await wrapper.get('[data-testid="no-vault-open"]').trigger("click");
    await flushPromises();
    expect(chooseCalls.openTree).toBe(before + 1);
    wrapper.unmount();
  });

  it("on a switch, and not for null or an Err", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await flushPromises();
    const before = chooseCalls.openTree;
    await runCommand(wrapper); // null
    expect(chooseCalls.openTree).toBe(before);
    chooseImpl = () => Promise.resolve(OPENED_B);
    await runCommand(wrapper);
    expect(chooseCalls.openTree).toBe(before + 1);
    wrapper.unmount();
  });

  it("refuses to switch while the open note has unsaved text, and says why", async () => {
    const pinia = createPinia();
    const wrapper = mount(App, { global: { plugins: [pinia] }, attachTo: document.body });
    await flushPromises();
    const editor = wrapper.findComponent(CodeMirrorEditor).vm as unknown as {
      loadNote(t: string, p: string, h: string): Promise<boolean>;
      getView(): { dispatch(spec: unknown): void } | undefined;
      getText(): string;
    };
    await editor.loadNote("old", "a.md", "h1");
    editor.getView()?.dispatch({ changes: { from: 3, insert: "!" } });
    chooseImpl = () => Promise.resolve(OPENED_B);
    await runCommand(wrapper);
    expect(chooseCalls.n).toBe(0);
    expect(useVaultStore(pinia).marksText).toContain("not switching vault");
    expect(editor.getText()).toBe("old!");
    wrapper.unmount();
  });
});

describe("the editor is read-only from the closing ack until the new vault is adopted (#151)", () => {
  it("freezes on app://closing, thaws on adopt", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await flushPromises();
    const editor = wrapper.findComponent(CodeMirrorEditor).vm as unknown as {
      loadNote(t: string, p: string, h: string): Promise<boolean>;
      getView(): { state: { readOnly: boolean } } | undefined;
    };
    await editor.loadNote("old", "a.md", "h1");
    expect(editor.getView()?.state.readOnly).toBe(false);
    handlers.get("app://closing")?.({ payload: { vault_id: "A" } });
    await flushPromises();
    expect(editor.getView()?.state.readOnly).toBe(true);
    handlers.get("vault://opened")?.({ payload: OPENED_B });
    await flushPromises();
    await editor.loadNote("new", "b.md", "h2");
    expect(editor.getView()?.state.readOnly).toBe(false);
    wrapper.unmount();
  });
});

describe("a failed switch leaves no rows, and a pending autosave is not a refusal (#151)", () => {
  const LISTING = { kind: "listing", dir: "", entries: [{ name: "old.md", kind: "file", sidecar: false }] };

  it("clears the old vault's rows under No vault open without calling open_tree", async () => {
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await flushPromises();
    treeChannels[0].onmessage?.(LISTING);
    await flushPromises();
    const tree = wrapper.findComponent({ name: "FileTree" }).vm as unknown as { model: { fileCount: number } };
    expect(tree.model.fileCount).toBe(1);
    const opens = chooseCalls.openTree;
    chooseImpl = () => {
      handlers.get("vault://opened")?.({ payload: OPENED_B });
      return Promise.reject({ kind: "vault_not_found", path: "/b" });
    };
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }));
    await flushPromises();
    await wrapper.get('[data-testid="command-palette"] input').setValue("Open vault");
    await wrapper.get('[data-testid="command-palette"] input').trigger("keydown", { key: "Enter" });
    await flushPromises();
    expect(tree.model.fileCount).toBe(0);
    expect(chooseCalls.openTree).toBe(opens);
    expect(wrapper.find('[data-testid="no-vault"]').exists()).toBe(true);
    wrapper.unmount();
  });

  it("a dirty buffer is saved first and the switch proceeds when the save succeeds", async () => {
    savePatch = () => Promise.resolve({ kind: "saved", hash: "h2" });
    const wrapper = mount(App, { global: { plugins: [createPinia()] }, attachTo: document.body });
    await flushPromises();
    const editor = wrapper.findComponent(CodeMirrorEditor).vm as unknown as {
      loadNote(t: string, p: string, h: string): Promise<boolean>;
      getView(): { dispatch(spec: unknown): void } | undefined;
    };
    await editor.loadNote("old", "a.md", "h1");
    editor.getView()?.dispatch({ changes: { from: 3, insert: "!" } });
    chooseImpl = () => Promise.resolve(null);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }));
    await flushPromises();
    await wrapper.get('[data-testid="command-palette"] input').setValue("Open vault");
    await wrapper.get('[data-testid="command-palette"] input').trigger("keydown", { key: "Enter" });
    await flushPromises();
    expect(chooseCalls.n).toBe(1);
    wrapper.unmount();
  });
});
