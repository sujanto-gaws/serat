// Task #114: `tree.insertLink` is a registry command (FR-NAV-06, NFR-USE-01, US-41, architecture §9.3).
// A rebinding moves BOTH the key that works in the tree and the chip in the context menu.

import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { computed, defineComponent, h, ref } from "vue";

import { CommandRegistry, COMMANDS_KEY } from "@/editor/commands";
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
  const wrapper = mount(Host, { attachTo: document.body, global: { provide: { [COMMANDS_KEY as symbol]: registry } } });
  await vi.advanceTimersByTimeAsync(0);
  push({
    kind: "listing",
    dir: "",
    entries: [
      { name: "a.md", kind: "note", sidecar: false },
      { name: "b.md", kind: "note", sidecar: false },
      { name: "pic.png", kind: "asset", sidecar: false },
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
    (el.element as HTMLElement).focus();
    registry.register({ id: "a.other", name: "A other", exec: () => undefined });
    await el.trigger("keydown", { key: "ArrowDown" });
    await el.trigger("keydown", { key: "ArrowDown" });
    expect(tree.get('[aria-selected="true"]').attributes("data-path")).toBe("b.md");
    await el.trigger("keydown", { key: "L", ctrlKey: true, shiftKey: true });
    expect(tree.emitted("insert-link")?.[0][0]).toEqual({ path: "b.md", text: "[[b]]" });
    await el.trigger("keydown", { key: "ArrowDown" });
    await el.trigger("keydown", { key: "L", ctrlKey: true, shiftKey: true });
    expect(tree.emitted("insert-link")?.[1][0]).toEqual({ path: "pic.png", text: "![[pic.png]]" });
    expect(registry.ranked()[0].id).toBe(TREE_INSERT_LINK_ID); // chord presses count as runs
    await el.trigger("keydown", { key: "ArrowUp" });
    expect(registry.setOverride(TREE_INSERT_LINK_ID, "Mod-Alt-j")).toBe(true);
    await wrapper.vm.$nextTick();
    await el.trigger("keydown", { key: "L", ctrlKey: true, shiftKey: true });
    expect(tree.emitted("insert-link") ?? []).toHaveLength(2); // old chord: nothing
    await el.trigger("keydown", { key: "j", ctrlKey: true, altKey: true });
    expect(tree.emitted("insert-link")?.[2][0]).toEqual({ path: "b.md", text: "[[b]]" });
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
    await tree.get('[data-path="b.md"]').trigger("contextmenu", { clientX: 1, clientY: 1 });
    expect(tree.find('[data-item="insert-link"] [data-spoken]').exists()).toBe(false);
    expect(tree.get('[data-item="insert-link"]').text()).toBe("Insert link");
    wrapper.unmount();
  });

  it("is inactive without tree focus or a selected file row", async () => {
    const { wrapper, tree, registry } = await mountHost();
    const command = registry.get(TREE_INSERT_LINK_ID)!;
    const here = () => ({ focus: document.activeElement });
    expect(command.unavailable?.(here())).toBe("Focus the file tree first");
    const el = tree.get('[role="tree"]');
    (el.element as HTMLElement).focus();
    expect(command.unavailable?.(here())).toBe("Select a file in the tree first");
    await el.trigger("keydown", { key: "ArrowDown" });
    expect(command.unavailable?.(here())).toBeNull();
    expect(await registry.execute(TREE_INSERT_LINK_ID, null)).toBeNull();
    expect(tree.emitted("insert-link") ?? []).toHaveLength(1);
    (el.element as HTMLElement).blur();
    expect(await registry.execute(TREE_INSERT_LINK_ID, null)).toBe("Focus the file tree first");
    wrapper.unmount();
  });

  it("the palette always lists it; unavailable it is greyed and Enter says why; availability follows where it opened", async () => {
    const { wrapper, tree, registry } = await mountHost();
    const el = tree.get('[role="tree"]');

    // Opened with the tree unfocused: listed, greyed, Enter shows the reason.
    const cold = mount(CommandPalette, { props: { registry }, attachTo: document.body });
    await cold.get("input").setValue("insert link");
    const row = cold.findAll('[role="option"]').find((r) => r.text().includes("Insert link to selected file"));
    expect(row?.classes()).toContain("unavailable");
    await cold.get("input").trigger("keydown", { key: "Enter" });
    expect(cold.get('[data-testid="palette-reason"]').text()).toContain("Focus the file tree first");
    cold.unmount();

    // Opened from the tree with a file selected: available, and stays so while the user types.
    (el.element as HTMLElement).focus();
    await el.trigger("keydown", { key: "ArrowDown" });
    const warm = mount(CommandPalette, { props: { registry }, attachTo: document.body });
    const rowIn = async () => {
      await warm.get("input").setValue("insert link");
      return warm.findAll('[role="option"]').find((r) => r.text().includes("Insert link to selected file"));
    };
    expect((await rowIn())?.classes()).not.toContain("unavailable");
    warm.unmount();

    // Opened from the tree with no row selected: reason names the selection.
    wrapper.unmount();
    const second = await mountHost();
    (second.tree.get('[role="tree"]').element as HTMLElement).focus();
    const noRow = mount(CommandPalette, { props: { registry: second.registry }, attachTo: document.body });
    await noRow.get("input").setValue("insert link");
    await noRow.get("input").trigger("keydown", { key: "Enter" });
    expect(noRow.get('[data-testid="palette-reason"]').text()).toContain("Select a file in the tree first");
    noRow.unmount();
    second.wrapper.unmount();
  });

  it("refuses a rebinding onto a live global chord, including a rebound one", async () => {
    const { wrapper, registry } = await mountHost();
    registry.register({ id: "tree.newNote", name: "New note", defaultBinding: "Mod-n", exec: () => undefined });
    registry.register({ id: "search.open", name: "Search vault", defaultBinding: "Mod-Shift-f", exec: () => undefined });
    expect(registry.setOverride(TREE_INSERT_LINK_ID, "Mod-n")).toBe(false);
    expect(registry.setOverride(TREE_INSERT_LINK_ID, "Mod-Shift-f")).toBe(false);
    expect(registry.setOverride("search.open", "Mod-Alt-s")).toBe(true);
    expect(registry.setOverride(TREE_INSERT_LINK_ID, "Mod-Alt-s")).toBe(false);
    expect(registry.setOverride(TREE_INSERT_LINK_ID, "Mod-Shift-f")).toBe(true); // freed by the rebind
    wrapper.unmount();
  });

  it("refuses a rebinding onto a key the tree owns (Enter still opens)", async () => {
    const { wrapper, tree, registry } = await mountHost();
    for (const key of ["Enter", "F2", "Mod-Enter", "ArrowDown", "Mod-f", "Shift-F10", "ContextMenu"]) {
      expect(registry.setOverride(TREE_INSERT_LINK_ID, key)).toBe(false);
    }
    expect(registry.effectiveBinding(TREE_INSERT_LINK_ID)).toBe("Mod-Shift-l");
    const el = tree.get('[role="tree"]');
    await el.trigger("keydown", { key: "ArrowDown" });
    await el.trigger("keydown", { key: "Enter" });
    expect(tree.emitted("open")).toHaveLength(1);
    expect(tree.emitted("insert-link")).toBeUndefined();
    // a settings file naming a reserved chord is dropped the same way
    expect(registry.setOverrides([{ command_id: TREE_INSERT_LINK_ID, key: "Enter" }])).toEqual([TREE_INSERT_LINK_ID]);
    wrapper.unmount();
  });
});
