// Task #39 (FR-VLT-01, NFR-USE-01, NFR-CMP-01, US-05): the tree context menu's platform conventions, and which
// surfaces keep the webview's native menu. The tree is the only surface the specs give a Serat context menu
// (file-tree.md; editor.md, the backlinks pane and the search panel specify none, so none exists).

import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { allowsNativeMenu, installNativeMenuPolicy } from "@/nativeMenu";
import type { PaneIpcAdapter } from "@/ipc/paneAdapter";
import type { TreeEntry } from "@/ipc/generated/TreeEntry";
import type { TreeMessage } from "@/ipc/generated/TreeMessage";
import FileTree from "@/tree/FileTree.vue";

const note = (name: string): TreeEntry => ({ name, kind: "note", sidecar: false });
const listing = (d: string, entries: TreeEntry[]): TreeMessage => ({ kind: "listing", dir: d, entries });

async function mountTree(props: Record<string, unknown> = {}) {
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
  const wrapper = mount(FileTree, {
    props: {
      ipc,
      createEntry: vi.fn().mockResolvedValue(null),
      bindings: { "new-note": "Mod-n", "new-folder": "Mod-Shift-n", "insert-link": "Mod-Shift-l", rename: "F2" },
      ...props,
    },
    attachTo: document.body,
  });
  await vi.advanceTimersByTimeAsync(0);
  push(listing("", [note("a.md"), note("b.md")]));
  await wrapper.vm.$nextTick();
  return wrapper;
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

describe("macOS: Ctrl-click and right-click", () => {
  it("a Ctrl-click (delivered as a click with ctrlKey) opens the menu and does NOT open the note", async () => {
    const wrapper = await mountTree({ mac: true });
    await wrapper.get('[data-path="a.md"]').trigger("click", { ctrlKey: true, clientX: 10, clientY: 20 });
    expect(wrapper.findAll('[role="menu"]')).toHaveLength(1);
    expect(wrapper.emitted("open")).toBeUndefined();
    expect(wrapper.get('[data-path="a.md"]').attributes("aria-selected")).toBe("true");
    wrapper.unmount();
  });

  it("right-click opens it once, and WebKit's contextmenu after a Ctrl-click does not stack a second menu", async () => {
    const wrapper = await mountTree({ mac: true });
    await wrapper.get('[data-path="b.md"]').trigger("contextmenu", { clientX: 5, clientY: 5 });
    expect(wrapper.findAll('[role="menu"]')).toHaveLength(1);
    await wrapper.get('[data-path="a.md"]').trigger("click", { ctrlKey: true });
    await wrapper.get('[data-path="a.md"]').trigger("contextmenu");
    expect(wrapper.findAll('[role="menu"]')).toHaveLength(1);
    wrapper.unmount();
  });

  it("a plain click still opens the note on macOS", async () => {
    const wrapper = await mountTree({ mac: true });
    await wrapper.get('[data-path="a.md"]').trigger("click");
    expect(wrapper.emitted("open")?.[0][0]).toMatchObject({ path: "a.md" });
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    wrapper.unmount();
  });
});

describe("Windows / Linux: keyboard and Ctrl-click", () => {
  it("the Menu key and Shift-F10 open the menu for the focused row", async () => {
    const wrapper = await mountTree({ mac: false });
    const tree = wrapper.get('[role="tree"]');
    await tree.trigger("keydown", { key: "ArrowDown" });
    await tree.trigger("keydown", { key: "ContextMenu" });
    expect(wrapper.findAll('[role="menu"]')).toHaveLength(1);
    await wrapper.get('[role="menu"]').trigger("keydown", { key: "Escape" });
    await tree.trigger("keydown", { key: "F10", shiftKey: true });
    expect(wrapper.findAll('[role="menu"]')).toHaveLength(1);
    wrapper.unmount();
  });

  it("Ctrl-click is NOT a secondary click here: it opens the note like any click (it is a selection chord elsewhere)", async () => {
    const wrapper = await mountTree({ mac: false });
    await wrapper.get('[data-path="a.md"]').trigger("click", { ctrlKey: true });
    expect(wrapper.find('[role="menu"]').exists()).toBe(false);
    expect(wrapper.emitted("open")?.[0][0]).toMatchObject({ path: "a.md" });
    wrapper.unmount();
  });
});

describe("accelerators in the menu agree with the palette's chips, per platform", () => {
  async function accels(mac: boolean) {
    const wrapper = await mountTree({ mac, openPath: "b.md" });
    await wrapper.get('[data-path="a.md"]').trigger("contextmenu", { clientX: 1, clientY: 1 });
    const read = (id: string) => ({
      glyphs: wrapper.get(`[data-item="${id}"] [data-accel]`).text(),
      hidden: wrapper.get(`[data-item="${id}"] [data-accel]`).attributes("aria-hidden"),
      spoken: wrapper.get(`[data-item="${id}"] [data-spoken]`).text(),
    });
    const out = { newFolder: read("new-folder"), newNote: read("new-note"), link: read("insert-link"), rename: read("rename") };
    wrapper.unmount();
    return out;
  }

  it("macOS: ⌘ ⇧ glyphs, spoken as Command / Shift", async () => {
    const a = await accels(true);
    expect(a.newFolder.glyphs).toBe("⌘ ⇧ N");
    expect(a.newNote.glyphs).toBe("⌘ N");
    expect(a.link.glyphs).toBe("⌘ ⇧ L");
    expect(a.rename.glyphs).toBe("F2");
    expect(a.newFolder.spoken).toBe(", shortcut Command Shift N");
    expect(a.newFolder.hidden).toBe("true"); // glyphs are not read out; the spoken span is
  });

  it("Windows / Linux: Ctrl Shift names, spoken as Control / Shift", async () => {
    const a = await accels(false);
    expect(a.newFolder.glyphs).toBe("Ctrl Shift N");
    expect(a.newNote.glyphs).toBe("Ctrl N");
    expect(a.link.glyphs).toBe("Ctrl Shift L");
    expect(a.newFolder.spoken).toBe(", shortcut Control Shift N");
  });

  it("a rebinding shows in the menu, and an entry with no binding shows none", async () => {
    const wrapper = await mountTree({ mac: false, bindings: { "new-note": "Mod-Alt-j", "new-folder": null } });
    await wrapper.get('[data-path="a.md"]').trigger("contextmenu", { clientX: 1, clientY: 1 });
    expect(wrapper.get('[data-item="new-note"] [data-accel]').text()).toBe("Ctrl Alt J");
    expect(wrapper.find('[data-item="new-folder"] [data-accel]').exists()).toBe(false);
    wrapper.unmount();
  });
});

describe("the native webview menu", () => {
  const fire = (target: Element | ShadowRoot): boolean => {
    const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, composed: true });
    (target as Element).dispatchEvent(event);
    return event.defaultPrevented;
  };

  it("allowsNativeMenu: text fields and editable text yes; buttons, plain chrome, checkboxes, read-only editor no", () => {
    const mk = (html: string) => {
      const host = document.createElement("div");
      host.innerHTML = html;
      document.body.appendChild(host);
      return host.firstElementChild as Element;
    };
    expect(allowsNativeMenu(mk('<input type="text">'))).toBe(true);
    expect(allowsNativeMenu(mk("<input>"))).toBe(true);
    expect(allowsNativeMenu(mk('<input type="search">'))).toBe(true);
    expect(allowsNativeMenu(mk("<textarea></textarea>"))).toBe(true);
    expect(allowsNativeMenu(mk('<div contenteditable="true"><span>x</span></div>').firstElementChild)).toBe(true);
    expect(allowsNativeMenu(mk('<input type="checkbox">'))).toBe(false);
    expect(allowsNativeMenu(mk("<button>b</button>"))).toBe(false);
    expect(allowsNativeMenu(mk("<p>chrome</p>"))).toBe(false);
    expect(allowsNativeMenu(mk('<div contenteditable="false">ro</div>'))).toBe(false);
    expect(allowsNativeMenu(null)).toBe(false);
  });

  it("with the policy installed: suppressed on chrome and tree rows, allowed in text inputs, the naming prompt and the editor's shadow DOM", async () => {
    const uninstall = installNativeMenuPolicy();
    const wrapper = await mountTree({ mac: false });

    // Chrome and the tree: suppressed (and the tree's own menu still opens: its handler runs first).
    const p = document.createElement("p");
    document.body.appendChild(p);
    expect(fire(p)).toBe(true);
    expect(fire(wrapper.get('[data-path="a.md"]').element)).toBe(true);
    await wrapper.vm.$nextTick();
    expect(wrapper.findAll('[role="menu"]')).toHaveLength(1);
    await wrapper.get('[role="menu"]').trigger("keydown", { key: "Escape" });

    // A native contextmenu with no row under it (WebView2 Menu key-up) still reaches the tree handler: the
    // policy runs in the bubbling phase, after it.
    await wrapper.get('[role="tree"]').trigger("keydown", { key: "ArrowDown" });
    expect(fire(wrapper.get('[role="tree"]').element)).toBe(true);
    await wrapper.vm.$nextTick();
    expect(wrapper.findAll('[role="menu"]')).toHaveLength(1);

    // Text input (the tree filter) and the naming prompt: native menu allowed.
    expect(fire(wrapper.get('[data-testid="tree-filter"]').element)).toBe(false);
    (wrapper.vm as unknown as { newNote(): void }).newNote();
    await vi.advanceTimersByTimeAsync(0);
    expect(fire(wrapper.get('[data-testid="tree-prompt-input"]').element)).toBe(false);

    // The editor: contenteditable inside an open shadow root.
    const host = document.createElement("div");
    document.body.appendChild(host);
    const inner = host.attachShadow({ mode: "open" });
    const editable = document.createElement("div");
    editable.setAttribute("contenteditable", "true");
    inner.appendChild(editable);
    expect(fire(editable)).toBe(false);
    const readOnly = document.createElement("div");
    readOnly.setAttribute("contenteditable", "false");
    inner.appendChild(readOnly);
    expect(fire(readOnly)).toBe(true);

    uninstall();
    expect(fire(p)).toBe(false); // uninstalled: nothing is touched any more
    wrapper.unmount();
  });
});
