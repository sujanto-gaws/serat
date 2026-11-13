// `vault.open` (task #151 vue half; US-01, FR-VLT-01, FR-NAV-06; ADR-0012, architecture §9.3): the palette
// command, the "No vault open" empty state, and the adopt path (#91/#103 id rules, #80's close-before-adopt).

import { flushPromises, mount } from "@vue/test-utils";
import { createPinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Handler = (event: { payload: unknown }) => void;
const handlers = new Map<string, Handler>();
let chooseImpl: () => Promise<unknown> = () => Promise.resolve(null);
const chooseCalls = vi.hoisted(() => ({ n: 0 }));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((cmd: string) => {
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
