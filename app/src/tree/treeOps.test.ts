// Task #43 (FR-VLT-11, US-41; file-tree.md; ADR-0016, ADR-0008): creating a note or folder from the tree
// and the switcher, and the tree's context menu. Rename, move and trash are P3 (FR-VLT-09) with no
// command in contracts/ipc.rs, so their menu entries are asserted Disabled-with-a-reason, and nothing
// else is built for them.

import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import QuickSwitcher from "@/overlays/QuickSwitcher.vue";
import type { PaneIpcAdapter } from "@/ipc/paneAdapter";
import type { SwitcherQueryArgs } from "@/ipc/generated/SwitcherQueryArgs";
import type { TreeEntry } from "@/ipc/generated/TreeEntry";
import type { TreeMessage } from "@/ipc/generated/TreeMessage";
import { describeCreateError, noteFileName, uniqueName } from "@/tree/createEntry";
import FileTree from "@/tree/FileTree.vue";

const note = (name: string): TreeEntry => ({ name, kind: "note", sidecar: false });
const dir = (name: string): TreeEntry => ({ name, kind: "dir", sidecar: false });
const asset = (name: string, sidecar = false): TreeEntry => ({ name, kind: "asset", sidecar });
const listing = (d: string, entries: TreeEntry[]): TreeMessage => ({ kind: "listing", dir: d, entries });

function fakeIpc() {
  let push: (m: TreeMessage) => void = () => undefined;
  const ipc: PaneIpcAdapter = {
    openTree: vi.fn(async (onMessage) => {
      push = onMessage;
    }),
    markTree: vi.fn().mockResolvedValue(undefined),
    search: vi.fn(),
    backlinks: vi.fn(),
    onIndexChanged: vi.fn().mockResolvedValue(() => undefined),
  };
  return { ipc, push: (m: TreeMessage) => push(m) };
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

describe("create helpers", () => {
  it("uniqueName is case-insensitive and numbers from 2", () => {
    expect(uniqueName(new Set(), "Untitled", ".md")).toBe("Untitled.md");
    expect(uniqueName(new Set(["untitled.md"]), "Untitled", ".md")).toBe("Untitled 2.md");
    expect(uniqueName(new Set(["untitled.md", "untitled 2.md"]), "Untitled", ".md")).toBe("Untitled 3.md");
    expect(uniqueName(new Set(["new folder"]), "New folder")).toBe("New folder 2");
  });
  it("noteFileName adds .md once and refuses empty", () => {
    expect(noteFileName(" idea ")).toBe("idea.md");
    expect(noteFileName("idea.MD")).toBe("idea.MD");
    expect(noteFileName("  ")).toBeNull();
    expect(noteFileName(".md")).toBeNull();
  });
  it("describeCreateError says what the core refused, for every kind it can answer", () => {
    expect(describeCreateError({ kind: "stale_hash", path: "a.md", current_hash: "h" }, "note", "a.md")).toContain("already exists");
    expect(describeCreateError({ kind: "not_found", path: "nope" }, "note", "x")).toContain("does not exist");
    expect(describeCreateError({ kind: "path_escapes_vault", path: "../x" }, "folder", "../x")).toContain("refused that name");
    expect(describeCreateError({ kind: "not_writable", path: ".obsidian/x" }, "folder", "x")).toContain(".obsidian/x");
    expect(describeCreateError({ kind: "io", path: "a", message: "disk full" }, "note", "a")).toContain("disk full");
    expect(describeCreateError("weird", "note", "a")).toContain("weird");
  });
});

async function mountTree(createEntry?: (r: { kind: "note" | "folder"; dir: string; name: string }) => Promise<string | null>, extra = {}) {
  const { ipc, push } = fakeIpc();
  const wrapper = mount(FileTree, { props: { ipc, createEntry, ...extra }, attachTo: document.body });
  await vi.advanceTimersByTimeAsync(0);
  push(listing("", [dir("Projects"), note("Untitled.md"), asset("pic.png")]));
  push(listing("Projects", [note("roadmap.md")]));
  await wrapper.vm.$nextTick();
  return { wrapper, push };
}

describe("New note / New folder in the tree", () => {
  it("the toolbar buttons are enabled when the host can create, and disabled with a reason when it cannot", async () => {
    const off = await mountTree(undefined);
    expect(off.wrapper.get('[data-testid="tree-new-note"]').attributes("disabled")).toBeDefined();
    expect(off.wrapper.get('[data-testid="tree-new-note"]').attributes("title")).toContain("not available");
    off.wrapper.unmount();
    const on = await mountTree(vi.fn().mockResolvedValue(null));
    expect(on.wrapper.get('[data-testid="tree-new-note"]').attributes("disabled")).toBeUndefined();
    expect(on.wrapper.get('[data-testid="tree-new-folder"]').attributes("disabled")).toBeUndefined();
    on.wrapper.unmount();
  });

  it("New note prefills a unique name (Untitled exists at the root), creates in the vault root, and closes on success", async () => {
    const createEntry = vi.fn().mockResolvedValue(null);
    const { wrapper } = await mountTree(createEntry);
    await wrapper.get('[data-testid="tree-new-note"]').trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    const input = wrapper.get('[data-testid="tree-prompt-input"]');
    expect((input.element as HTMLInputElement).value).toBe("Untitled 2");
    expect(document.activeElement).toBe(input.element);
    expect(wrapper.get('[data-testid="tree-prompt"]').text()).toContain("the vault root");
    await wrapper.get('[data-testid="tree-prompt"]').trigger("submit");
    await vi.advanceTimersByTimeAsync(0);
    expect(createEntry).toHaveBeenCalledWith({ kind: "note", dir: "", name: "Untitled 2" });
    expect(wrapper.find('[data-testid="tree-prompt"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("creates in the selected folder, and in an entry's folder when a file is selected", async () => {
    const createEntry = vi.fn().mockResolvedValue(null);
    const { wrapper } = await mountTree(createEntry);
    const tree = wrapper.get('[role="tree"]');
    await tree.trigger("keydown", { key: "ArrowDown" }); // Projects
    await wrapper.get('[data-testid="tree-new-folder"]').trigger("click");
    await wrapper.get('[data-testid="tree-prompt"]').trigger("submit");
    await vi.advanceTimersByTimeAsync(0);
    expect(createEntry).toHaveBeenLastCalledWith({ kind: "folder", dir: "Projects", name: "New folder" });
    await wrapper.get('[data-path="Projects"]').trigger("click"); // expand
    await wrapper.get('[data-path="Projects/roadmap.md"]').trigger("click"); // select a file inside
    await wrapper.get('[data-testid="tree-new-note"]').trigger("click");
    await wrapper.get('[data-testid="tree-prompt"]').trigger("submit");
    await vi.advanceTimersByTimeAsync(0);
    expect(createEntry).toHaveBeenLastCalledWith({ kind: "note", dir: "Projects", name: "Untitled" });
    expect((wrapper.vm as unknown as { currentDir(): string }).currentDir()).toBe("Projects");
    wrapper.unmount();
  });

  it("the core's refusal is shown inline, the prompt stays open with what was typed, and typing clears it", async () => {
    const createEntry = vi.fn().mockResolvedValue("Serat refused that name: it is not a valid note name here.");
    const { wrapper } = await mountTree(createEntry);
    await wrapper.get('[data-testid="tree-new-note"]').trigger("click");
    const input = wrapper.get('[data-testid="tree-prompt-input"]');
    await input.setValue("a/../b");
    await wrapper.get('[data-testid="tree-prompt"]').trigger("submit");
    await vi.advanceTimersByTimeAsync(0);
    expect(wrapper.get('[data-testid="tree-prompt-error"]').text()).toContain("refused that name");
    expect(wrapper.get('[data-testid="tree-prompt-error"]').attributes("role")).toBe("alert");
    expect((wrapper.get('[data-testid="tree-prompt-input"]').element as HTMLInputElement).value).toBe("a/../b");
    await input.setValue("fine");
    expect(wrapper.find('[data-testid="tree-prompt-error"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("a name that differs only by case from a listed entry is refused before any command is sent", async () => {
    const createEntry = vi.fn().mockResolvedValue(null);
    const { wrapper } = await mountTree(createEntry);
    await wrapper.get('[data-testid="tree-new-note"]').trigger("click");
    await wrapper.get('[data-testid="tree-prompt-input"]').setValue("UNTITLED");
    await wrapper.get('[data-testid="tree-prompt"]').trigger("submit");
    await vi.advanceTimersByTimeAsync(0);
    expect(createEntry).not.toHaveBeenCalled();
    expect(wrapper.get('[data-testid="tree-prompt-error"]').text()).toContain("already exists");
    await wrapper.get('[data-testid="tree-prompt-input"]').setValue("  ");
    await wrapper.get('[data-testid="tree-prompt"]').trigger("submit");
    expect(wrapper.get('[data-testid="tree-prompt-error"]').text()).toContain("Give it a name");
    wrapper.unmount();
  });

  it("Esc cancels and returns focus to the tree; nothing is created", async () => {
    const createEntry = vi.fn().mockResolvedValue(null);
    const { wrapper } = await mountTree(createEntry);
    await wrapper.get('[data-testid="tree-new-note"]').trigger("click");
    await wrapper.get('[data-testid="tree-prompt"]').trigger("keydown", { key: "Escape" });
    await vi.advanceTimersByTimeAsync(0);
    expect(wrapper.find('[data-testid="tree-prompt"]').exists()).toBe(false);
    expect(document.activeElement).toBe(wrapper.get('[role="tree"]').element);
    expect(createEntry).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("the tree is not edited optimistically: a created note appears only when the stream says so", async () => {
    const createEntry = vi.fn().mockResolvedValue(null);
    const { wrapper, push } = await mountTree(createEntry);
    await wrapper.get('[data-testid="tree-new-note"]').trigger("click");
    await wrapper.get('[data-testid="tree-prompt"]').trigger("submit");
    await vi.advanceTimersByTimeAsync(0);
    expect(wrapper.find('[data-path="Untitled 2.md"]').exists()).toBe(false);
    push({ kind: "upsert", dir: "", entry: note("Untitled 2.md"), after: "Untitled.md" } as TreeMessage);
    await wrapper.vm.$nextTick();
    expect(wrapper.find('[data-path="Untitled 2.md"]').exists()).toBe(true);
    wrapper.unmount();
  });
});

describe("the tree context menu", () => {
  const menuIds = (w: ReturnType<typeof mount>) => w.findAll('[role="menuitem"]').map((i) => i.attributes("data-item"));

  it("right-click opens a role=menu with the entries; rename, move and trash are Disabled (not hidden) with the reason", async () => {
    const { wrapper } = await mountTree(vi.fn().mockResolvedValue(null), { openPath: "Untitled.md" });
    await wrapper.get('[data-path="Untitled.md"]').trigger("contextmenu", { clientX: 40, clientY: 60 });
    const menu = wrapper.get('[role="menu"]');
    expect(menu.attributes("aria-label")).toBe("File actions");
    expect(menuIds(wrapper)).toEqual(["insert-link", "new-note", "new-folder", "rename", "move", "trash"]);
    for (const id of ["rename", "move", "trash"]) {
      const item = wrapper.get(`[data-item="${id}"]`);
      expect(item.attributes("aria-disabled")).toBe("true");
      expect(item.attributes("title")).toContain("P3");
    }
    expect(wrapper.get('[data-item="new-note"]').attributes("aria-disabled")).toBe("false");
    wrapper.unmount();
  });

  it("Shift-F10 opens it for the selected row; arrows wrap; Home/End jump; focus is in the menu", async () => {
    const { wrapper } = await mountTree(vi.fn().mockResolvedValue(null), { openPath: "Untitled.md" });
    const tree = wrapper.get('[role="tree"]');
    await tree.trigger("keydown", { key: "ArrowDown" });
    await tree.trigger("keydown", { key: "ArrowDown" }); // Untitled.md
    await tree.trigger("keydown", { key: "F10", shiftKey: true });
    const menu = wrapper.get('[role="menu"]');
    expect(document.activeElement).toBe(menu.element);
    const active = () => menu.attributes("aria-activedescendant");
    expect(active()).toBe("serat-tree-menu-0");
    await menu.trigger("keydown", { key: "ArrowUp" });
    expect(active()).toBe("serat-tree-menu-5"); // wraps to the last
    await menu.trigger("keydown", { key: "ArrowDown" });
    expect(active()).toBe("serat-tree-menu-0");
    await menu.trigger("keydown", { key: "End" });
    expect(active()).toBe("serat-tree-menu-5");
    await menu.trigger("keydown", { key: "Home" });
    expect(active()).toBe("serat-tree-menu-0");
    wrapper.unmount();
  });

  it("the Menu key opens it too, and Esc closes it and returns focus to the tree", async () => {
    const { wrapper } = await mountTree(vi.fn().mockResolvedValue(null));
    const tree = wrapper.get('[role="tree"]');
    await tree.trigger("keydown", { key: "ArrowDown" });
    await tree.trigger("keydown", { key: "ContextMenu" });
    expect(wrapper.find('[role="menu"]').exists()).toBe(true);
    await wrapper.get('[role="menu"]').trigger("keydown", { key: "Escape" });
    await vi.advanceTimersByTimeAsync(0);
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    expect(document.activeElement).toBe(tree.element);
    wrapper.unmount();
  });

  it("Enter on 'New note here' on a folder opens the prompt for THAT folder; Enter on a disabled entry does nothing", async () => {
    const createEntry = vi.fn().mockResolvedValue(null);
    const { wrapper } = await mountTree(createEntry);
    await wrapper.get('[data-path="Projects"]').trigger("contextmenu", { clientX: 1, clientY: 1 });
    const menu = wrapper.get('[role="menu"]');
    await menu.trigger("keydown", { key: "End" }); // trash (disabled)
    await menu.trigger("keydown", { key: "Enter" });
    expect(wrapper.find('[role="menu"]').exists()).toBe(true); // still open, nothing happened
    expect(wrapper.find('[data-testid="tree-prompt"]').exists()).toBe(false);
    await menu.trigger("keydown", { key: "Home" }); // folders have no Insert link: first entry is New note here
    await menu.trigger("keydown", { key: "Enter" });
    await vi.advanceTimersByTimeAsync(0);
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    expect(wrapper.get('[data-testid="tree-prompt"]').text()).toContain("Projects");
    await wrapper.get('[data-testid="tree-prompt"]').trigger("submit");
    await vi.advanceTimersByTimeAsync(0);
    expect(createEntry).toHaveBeenCalledWith({ kind: "note", dir: "Projects", name: "Untitled" });
    wrapper.unmount();
  });

  it("Insert link is disabled with a reason when no note is open, and emits the link when one is", async () => {
    const closed = await mountTree(vi.fn().mockResolvedValue(null));
    await closed.wrapper.get('[data-path="pic.png"]').trigger("contextmenu", { clientX: 1, clientY: 1 });
    expect(closed.wrapper.get('[data-item="insert-link"]').attributes("aria-disabled")).toBe("true");
    expect(closed.wrapper.get('[data-item="insert-link"]').attributes("title")).toContain("Open a note first");
    closed.wrapper.unmount();

    const open = await mountTree(vi.fn().mockResolvedValue(null), { openPath: "Untitled.md" });
    await open.wrapper.get('[data-path="pic.png"]').trigger("contextmenu", { clientX: 1, clientY: 1 });
    await open.wrapper.get('[role="menu"]').trigger("keydown", { key: "Enter" });
    expect(open.wrapper.emitted("insert-link")?.[0][0]).toEqual({ path: "pic.png", text: "![[pic.png]]" });
    open.wrapper.unmount();
  });

  it("a click outside closes the menu", async () => {
    const { wrapper } = await mountTree(vi.fn().mockResolvedValue(null));
    await wrapper.get('[data-path="Projects"]').trigger("contextmenu", { clientX: 1, clientY: 1 });
    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    await wrapper.vm.$nextTick();
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    wrapper.unmount();
  });
});

describe("the switcher's Create note row", () => {
  const empty = async ({ seq }: SwitcherQueryArgs) => ({ seq, corpus: "index" as const, items: [], discarded: false });

  async function open(createNote?: (n: string) => Promise<string | null>) {
    const w = mount(QuickSwitcher, { props: { query: empty, createNote }, attachTo: document.body });
    await vi.advanceTimersByTimeAsync(0);
    const input = w.get("input");
    await input.setValue("brand new idea");
    await vi.advanceTimersByTimeAsync(0);
    return { w, input };
  }

  it("on a settled no-match, offers Create note and Enter runs it, then closes", async () => {
    const createNote = vi.fn().mockResolvedValue(null);
    const { w, input } = await open(createNote);
    expect(w.get('[data-testid="switcher-create"]').text()).toContain("Create note “brand new idea”");
    expect(input.attributes("aria-activedescendant")).toBe("qs-create");
    await input.trigger("keydown", { key: "Enter" });
    await vi.advanceTimersByTimeAsync(0);
    expect(createNote).toHaveBeenCalledWith("brand new idea");
    expect(w.emitted("close")).toBeTruthy();
    w.unmount();
  });

  it("shows the core's refusal inline and stays open", async () => {
    const createNote = vi.fn().mockResolvedValue("A note named “x.md” already exists here.");
    const { w, input } = await open(createNote);
    await input.trigger("keydown", { key: "Enter" });
    await vi.advanceTimersByTimeAsync(0);
    expect(w.get('[data-testid="switcher-create-error"]').text()).toContain("already exists");
    expect(w.emitted("close")).toBeFalsy();
    w.unmount();
  });

  it("is not offered when the host cannot create", async () => {
    const { w } = await open(undefined);
    expect(w.find('[data-testid="switcher-create"]').exists()).toBe(false);
    w.unmount();
  });
});

describe("round 2: focus, close reasons, a11y", () => {
  it("Esc on the prompt returns focus to where it was before (the editor stand-in), not to the tree", async () => {
    const outside = document.createElement("textarea");
    document.body.appendChild(outside);
    const { wrapper } = await mountTree(vi.fn().mockResolvedValue(null));
    outside.focus();
    (wrapper.vm as unknown as { newNote(): void }).newNote();
    await vi.advanceTimersByTimeAsync(0);
    expect(document.activeElement).toBe(wrapper.get('[data-testid="tree-prompt-input"]').element);
    await wrapper.get('[data-testid="tree-prompt"]').trigger("keydown", { key: "Escape" });
    await vi.advanceTimersByTimeAsync(0);
    expect(document.activeElement).toBe(outside);
    wrapper.unmount();
  });

  it("falls back to the tree when the origin is gone", async () => {
    const outside = document.createElement("textarea");
    document.body.appendChild(outside);
    const { wrapper } = await mountTree(vi.fn().mockResolvedValue(null));
    outside.focus();
    (wrapper.vm as unknown as { newNote(): void }).newNote();
    await vi.advanceTimersByTimeAsync(0);
    outside.remove();
    await wrapper.get('[data-testid="tree-prompt"]').trigger("keydown", { key: "Escape" });
    await vi.advanceTimersByTimeAsync(0);
    expect(document.activeElement).toBe(wrapper.get('[role="tree"]').element);
    wrapper.unmount();
  });

  it("the chord again while the prompt is open refocuses it and keeps what was typed", async () => {
    const { wrapper } = await mountTree(vi.fn().mockResolvedValue(null));
    const api = wrapper.vm as unknown as { newNote(): void };
    api.newNote();
    await vi.advanceTimersByTimeAsync(0);
    await wrapper.get('[data-testid="tree-prompt-input"]').setValue("half typed");
    (document.activeElement as HTMLElement).blur();
    api.newNote();
    await vi.advanceTimersByTimeAsync(0);
    const input = wrapper.get('[data-testid="tree-prompt-input"]').element as HTMLInputElement;
    expect(input.value).toBe("half typed");
    expect(document.activeElement).toBe(input);
    wrapper.unmount();
  });

  it("menu close reasons: Esc refocuses the tree; Tab and an outside press do not force it", async () => {
    const outside = document.createElement("button");
    document.body.appendChild(outside);
    const { wrapper } = await mountTree(vi.fn().mockResolvedValue(null));
    const tree = wrapper.get('[role="tree"]').element as HTMLElement;

    await wrapper.get('[data-path="Projects"]').trigger("contextmenu", { clientX: 1, clientY: 1 });
    await wrapper.get('[role="menu"]').trigger("keydown", { key: "Tab" });
    await vi.advanceTimersByTimeAsync(0);
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    expect(document.activeElement).not.toBe(tree);

    await wrapper.get('[data-path="Projects"]').trigger("contextmenu", { clientX: 1, clientY: 1 });
    outside.focus();
    outside.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(0);
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    expect(document.activeElement).toBe(outside);

    await wrapper.get('[data-path="Projects"]').trigger("contextmenu", { clientX: 1, clientY: 1 });
    await wrapper.get('[role="menu"]').trigger("keydown", { key: "Escape" });
    await vi.advanceTimersByTimeAsync(0);
    expect(document.activeElement).toBe(tree);
    wrapper.unmount();
  });

  it("disabled entries carry the reason as aria-description too, not only a tooltip", async () => {
    const { wrapper } = await mountTree(vi.fn().mockResolvedValue(null));
    await wrapper.get('[data-path="Projects"]').trigger("contextmenu", { clientX: 1, clientY: 1 });
    expect(wrapper.get('[data-item="trash"]').attributes("aria-description")).toContain("P3");
    expect(wrapper.get('[data-item="new-note"]').attributes("aria-description")).toBeUndefined();
    wrapper.unmount();
  });

  it("a native contextmenu on the tree (WebView2 Menu key / Shift-F10 key-up) opens our menu for the focused row, once", async () => {
    const { wrapper } = await mountTree(vi.fn().mockResolvedValue(null));
    const tree = wrapper.get('[role="tree"]');
    await tree.trigger("keydown", { key: "ArrowDown" }); // Projects
    await tree.trigger("contextmenu");
    expect(wrapper.findAll('[role="menu"]')).toHaveLength(1);
    // A right-click on a row is handled by the row alone: still one menu, for that row.
    await wrapper.get('[data-path="pic.png"]').trigger("contextmenu", { clientX: 2, clientY: 2 });
    expect(wrapper.findAll('[role="menu"]')).toHaveLength(1);
    expect(wrapper.get('[data-path="pic.png"]').attributes("aria-selected")).toBe("true");
    wrapper.unmount();
  });
});
