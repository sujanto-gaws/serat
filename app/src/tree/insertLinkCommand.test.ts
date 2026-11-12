// Task #114: `tree.insertLink` is a registry command (FR-NAV-06, NFR-USE-01, US-41, architecture §9.3).
// A rebinding moves BOTH the key that works in the tree and the chip in the context menu.

import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { computed, defineComponent, h, ref } from "vue";

import { CommandRegistry } from "@/editor/commands";
import type { PaneIpcAdapter } from "@/ipc/paneAdapter";
import type { TreeMessage } from "@/ipc/generated/TreeMessage";
import CommandPalette from "@/overlays/CommandPalette.vue";
import FileTree from "@/tree/FileTree.vue";
import { registerTreeInsertLink, TREE_INSERT_LINK_ID } from "@/tree/insertLinkCommand";

async function mountHost() {
  let push: (m: TreeMessage) => void = () => undefined;
  const ipc = {
    openTree: vi.fn(async (onMessage: (m: TreeMessage) => void) => {
      push = onMessage;
    }),
    markTree: vi.fn().mockResolvedValue(undefined),
    search: vi.fn(),
    backlinks: vi.fn(),
    onIndexChanged: vi.fn().mockResolvedValue(() => undefined),
  } as unknown as PaneIpcAdapter;
  const registry = new CommandRegistry();
  const version = ref(0);
  registry.subscribe(() => (version.value += 1));
  const treeRef = ref<InstanceType<typeof FileTree> | null>(null);
  registerTreeInsertLink(registry, () => treeRef.value);
  const Host = defineComponent({
    setup() {
      const bindings = computed(() => {
        void version.value;
        return { "insert-link": registry.effectiveBinding(TREE_INSERT_LINK_ID), rename: "F2" };
      });
      return () => h(FileTree, { ref: treeRef, ipc, mac: false, openPath: "a.md", bindings: bindings.value });
    },
  });
  const wrapper = mount(Host, { attachTo: document.body });
  await vi.advanceTimersByTimeAsync(0);
  push({
    kind: "listing",
    dir: "",
    entries: [
      { name: "a.md", kind: "note", sidecar: false },
      { name: "b.md", kind: "note", sidecar: false },
    ],
  });
  await wrapper.vm.$nextTick();
  return { wrapper, tree: wrapper.getComponent(FileTree), registry };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0) as unknown as number);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

describe("tree.insertLink", () => {
  it("is registered with the default Mod-Shift-l and the spec title", async () => {
    const { wrapper, registry } = await mountHost();
    expect(registry.get(TREE_INSERT_LINK_ID)).toMatchObject({ name: "Insert link to selected file", defaultBinding: "Mod-Shift-l" });
    wrapper.unmount();
  });

  it("rebinding changes the key that works in the tree; the old chord stops working", async () => {
    const { wrapper, tree, registry } = await mountHost();
    const el = tree.get('[role="tree"]');
    await el.trigger("keydown", { key: "ArrowDown" });
    await el.trigger("keydown", { key: "L", ctrlKey: true, shiftKey: true });
    expect(tree.emitted("insert-link") ?? []).toHaveLength(1);
    expect(registry.setOverride(TREE_INSERT_LINK_ID, "Mod-Alt-j")).toBe(true);
    await wrapper.vm.$nextTick();
    await el.trigger("keydown", { key: "L", ctrlKey: true, shiftKey: true });
    expect(tree.emitted("insert-link") ?? []).toHaveLength(1); // old chord: nothing
    await el.trigger("keydown", { key: "j", ctrlKey: true, altKey: true });
    expect(tree.emitted("insert-link") ?? []).toHaveLength(2);
    wrapper.unmount();
  });

  it("rebinding changes the chip shown in the context menu; unbinding removes it; Rename stays F2", async () => {
    const { wrapper, tree, registry } = await mountHost();
    const chip = async () => {
      await tree.get('[data-path="b.md"]').trigger("contextmenu", { clientX: 1, clientY: 1 });
      const accel = tree.find('[data-item="insert-link"] [data-accel]');
      const rename = tree.get('[data-item="rename"] [data-accel]').text();
      const text = accel.exists() ? accel.text() : null;
      await tree.get('[role="menu"]').trigger("keydown", { key: "Escape" });
      return { text, rename };
    };
    expect(await chip()).toEqual({ text: "Ctrl Shift L", rename: "F2" });
    registry.setOverride(TREE_INSERT_LINK_ID, "Mod-Alt-j");
    await wrapper.vm.$nextTick();
    expect(await chip()).toEqual({ text: "Ctrl Alt J", rename: "F2" });
    registry.setOverride(TREE_INSERT_LINK_ID, null);
    await wrapper.vm.$nextTick();
    expect((await chip()).text).toBeNull();
    wrapper.unmount();
  });

  it("is inactive without tree focus or a selected file row, and the palette lists it only when active", async () => {
    const { wrapper, tree, registry } = await mountHost();
    const command = registry.get(TREE_INSERT_LINK_ID)!;
    expect(command.unavailable?.()).not.toBeNull(); // nothing focused
    const el = tree.get('[role="tree"]');
    (el.element as HTMLElement).focus();
    expect(command.unavailable?.()).not.toBeNull(); // focused, no row selected
    await el.trigger("keydown", { key: "ArrowDown" });
    expect(command.unavailable?.()).toBeNull(); // active
    expect(await registry.execute(TREE_INSERT_LINK_ID, null)).toBeNull();
    expect(tree.emitted("insert-link") ?? []).toHaveLength(1);

    const listed = () => {
      const palette = mount(CommandPalette, { props: { registry }, attachTo: document.body });
      const text = palette.text();
      palette.unmount();
      return text;
    };
    expect(listed()).toContain("Insert link to selected file");
    (el.element as HTMLElement).blur();
    expect(listed()).not.toContain("Insert link to selected file");
    expect(await registry.execute(TREE_INSERT_LINK_ID, null)).toMatch(/focus/i);
    wrapper.unmount();
  });
});
