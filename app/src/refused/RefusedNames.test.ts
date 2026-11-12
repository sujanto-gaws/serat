// Refused-names strip count and panel (file-tree.md "Names not shown"; FR-VLT-01, NFR-SEC-06, NFR-USE-01, US-01).

import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { RefusedName } from "@/ipc/generated/RefusedName";
import RefusedNames from "@/refused/RefusedNames.vue";
import { escapeUntrusted, refusedCountText, REFUSED_NAMES_CAP } from "@/refused/refusedText";
import { useVaultStore } from "@/stores/vault";

const row = (path: string, reason = "Contains a \":\"."): RefusedName => ({ path, reason });

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("count wording", () => {
  it("is absent at None and 0, singular at 1, plural at N, capped at 1000", () => {
    expect(refusedCountText(null)).toBeNull();
    expect(refusedCountText(0)).toBeNull();
    expect(refusedCountText(1)).toBe("1 name not shown");
    expect(refusedCountText(12)).toBe("12 names not shown");
    expect(refusedCountText(999)).toBe("999 names not shown");
    expect(refusedCountText(REFUSED_NAMES_CAP)).toBe("1000+ names not shown");
  });

  it("renders no button at None/0 but keeps the polite live region mounted", () => {
    for (const count of [null, 0]) {
      const w = mount(RefusedNames, { props: { count } });
      expect(w.find('[data-testid="refused-names"]').exists()).toBe(false);
      const live = w.get('[role="status"]');
      expect(live.classes()).toContain("sr-only");
      expect(live.attributes("aria-live")).toBe("polite");
      expect(live.attributes("aria-atomic")).toBe("true");
      expect(live.text()).toBe("");
      w.unmount();
    }
  });

  it("the button carries the wording, aria-haspopup, and a screen-reader 'or more' at the cap", () => {
    const one = mount(RefusedNames, { props: { count: 1 } });
    expect(one.get("button").text()).toBe("1 name not shown");
    expect(one.get("button").attributes("aria-haspopup")).toBe("dialog");
    expect(one.get("button").attributes("aria-expanded")).toBe("false");
    one.unmount();
    const cap = mount(RefusedNames, { props: { count: 1000 } });
    expect(cap.get('[aria-hidden="true"]:not(.refused-dot)').text()).toBe("1000+");
    expect(cap.get("button .sr-only").text()).toBe("1000 or more");
    cap.unmount();
  });

  it("coalesces changes through a 1 s trailing debounce, in the live region", async () => {
    const w = mount(RefusedNames, { props: { count: 1 } });
    await w.setProps({ count: 2 });
    await w.setProps({ count: 3 });
    await vi.advanceTimersByTimeAsync(900);
    expect(w.get('[role="status"]').text()).toBe("1 name not shown");
    expect(w.get("button").text()).toBe("3 names not shown"); // the button is immediate; only the announcement waits
    expect(w.get("button").element.closest('[role="status"]')).toBeNull();
    await vi.advanceTimersByTimeAsync(200);
    expect(w.get('[role="status"]').text()).toBe("3 names not shown");
    await w.setProps({ count: null });
    await vi.advanceTimersByTimeAsync(1100);
    expect(w.find("button").exists()).toBe(false);
    w.unmount();
  });
});

describe("escapes", () => {
  it("shows control and bidi characters as visible escapes and leaves ordinary text alone", () => {
    expect(escapeUntrusted("a‮exe.md")).toBe("a⟨U+202E⟩exe.md");
    expect(escapeUntrusted("x\u0007y\u007f")).toBe("x⟨U+0007⟩y⟨U+007F⟩");
    expect(escapeUntrusted("notes/Café 日本.md")).toBe("notes/Café 日本.md");
  });
});

describe("reasons", () => {
  it("are capitalised, with backticked spans as inline mono and no backticks, and escaped like paths", async () => {
    const w = mount(RefusedNames, {
      props: { count: 1, load: () => Promise.resolve([row("a:b", "contains a `:`, which is a drive prefix\u202E")]) },
      attachTo: document.body,
    });
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    const reason = w.get('[data-testid="refused-reason"]');
    expect(reason.text()).toBe("Contains a :, which is a drive prefix\u27e8U+202E\u27e9");
    expect(reason.get("code").text()).toBe(":");
    expect(reason.text()).not.toContain("`");
    w.unmount();
  });

  it("palette opener: focus returns to the element focused when the panel opened, else the host fallback", async () => {
    const opener = document.createElement("button");
    document.body.append(opener);
    const w = mount(RefusedNames, { props: { count: 1, load: () => Promise.resolve([row("a:b")]) }, attachTo: document.body });
    opener.focus();
    await (w.vm as unknown as { open(): Promise<void> }).open();
    await vi.advanceTimersByTimeAsync(0);
    await w.get('[role="dialog"]').trigger("keydown", { key: "Escape" });
    await vi.advanceTimersByTimeAsync(0);
    expect(document.activeElement).toBe(opener);
    expect(w.emitted("focus-fallback")).toBeUndefined();
    w.unmount();
  });
});

describe("panel", () => {
  const mountPanel = (count: number | null, load: () => Promise<RefusedName[]>) =>
    mount(RefusedNames, { props: { count, load }, attachTo: document.body });

  it("opens non-modal with its labelled dialog, focuses it, and lists rows as text", async () => {
    const load = vi.fn().mockResolvedValue([row("notes/meeting: friday.md", "Contains a \":\"."), row("<b>x</b>.md")]);
    const w = mountPanel(2, load);
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    const dialog = w.get('[role="dialog"]');
    expect(dialog.attributes("aria-modal")).toBe("false");
    expect(dialog.attributes("aria-labelledby")).toBe("refused-title");
    expect(document.activeElement).toBe(dialog.element);
    expect(w.get("button").attributes("aria-expanded")).toBe("true");
    expect(w.get('[data-testid="refused-sentence"]').text()).toBe(
      "2 files in your vault folder can't be listed on this computer. Serat leaves them as they are.",
    );
    expect(w.findAll('[data-testid="refused-path"]').map((p) => p.text())).toEqual(["notes/meeting: friday.md", "<b>x</b>.md"]);
    expect(w.find(".refused-list b").exists()).toBe(false); // text, never HTML
    expect(w.get('[role="list"]').attributes("tabindex")).toBe("0");
    expect(w.get(".refused-foot").text()).toBe(
      "A name stays listed until Serat next scans its folder, even if you have since renamed or deleted the file.",
    );
    w.unmount();
  });

  it("uses singular wording for one file and the cap wording at 1000", async () => {
    const w = mountPanel(1, () => Promise.resolve([row("a:b.md")]));
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    expect(w.get('[data-testid="refused-sentence"]').text()).toBe(
      "1 file in your vault folder can't be listed on this computer. Serat leaves it as it is.",
    );
    w.unmount();
    const many = Array.from({ length: 1000 }, (_, i) => row(`f${i}:.md`));
    const c = mountPanel(1000, () => Promise.resolve(many));
    await c.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    expect(c.get('[data-testid="refused-sentence"]').text()).toContain("This list shows 1000 of them.");
    c.unmount();
  });

  it("renders a path with U+202E as a visible escape, not a reordering", async () => {
    const w = mountPanel(1, () => Promise.resolve([row("evil‮fdp.exe", "bad\u0007reason")]));
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    expect(w.get('[data-testid="refused-path"]').text()).toBe("evil⟨U+202E⟩fdp.exe");
    expect(w.get('[data-testid="refused-reason"]').text()).toBe("Bad⟨U+0007⟩reason");
    expect(w.get('[data-testid="refused-path"]').element.textContent).not.toContain("‮");
    w.unmount();
  });

  it("Esc, x and the toggle close it and return focus to the strip button", async () => {
    const w = mountPanel(1, () => Promise.resolve([row("a:b")]));
    const button = w.get("button").element as HTMLElement;
    for (const close of ["esc", "x", "toggle"]) {
      await w.get("button").trigger("click");
      await vi.advanceTimersByTimeAsync(0);
      expect(w.find('[role="dialog"]').exists()).toBe(true);
      if (close === "esc") await w.get('[role="dialog"]').trigger("keydown", { key: "Escape" });
      else if (close === "x") await w.get('[data-testid="refused-close"]').trigger("click");
      else await w.get("button").trigger("click");
      await vi.advanceTimersByTimeAsync(0);
      expect(w.find('[role="dialog"]').exists()).toBe(false);
      expect(document.activeElement).toBe(button);
    }
    w.unmount();
  });

  it("is not pulled back when focus left on its own, and a pointer press outside closes it", async () => {
    const outside = document.createElement("input");
    document.body.append(outside);
    const w = mountPanel(1, () => Promise.resolve([row("a:b")]));
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    outside.focus();
    await vi.advanceTimersByTimeAsync(0);
    expect(w.find('[role="dialog"]').exists()).toBe(false);
    expect(document.activeElement).toBe(outside);

    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    outside.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    await vi.advanceTimersByTimeAsync(0);
    expect(w.find('[role="dialog"]').exists()).toBe(false);
    w.unmount();
  });

  it("an empty list shows the empty state and stays open for Esc", async () => {
    const w = mountPanel(1, () => Promise.resolve([]));
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    expect(w.get('[data-testid="refused-empty"]').text()).toBe("Every name in this vault can be shown.");
    expect(w.find('[role="dialog"]').exists()).toBe(true);
    w.unmount();
  });

  it("a load failure says so, with Retry, and no retry loop", async () => {
    const load = vi.fn().mockRejectedValueOnce(new Error("boom")).mockResolvedValue([row("a:b")]);
    const w = mountPanel(1, load);
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(5000);
    expect(w.get('[data-testid="refused-failed"]').text()).toBe("Couldn't load the list. The count in the status strip is still correct.");
    expect(load).toHaveBeenCalledTimes(1);
    await w.get('[data-testid="refused-retry"]').trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    expect(load).toHaveBeenCalledTimes(2);
    expect(w.findAll('[data-testid="refused-path"]')).toHaveLength(1);
    w.unmount();
  });

  it("re-fetches once, 500 ms after the count changes while open, keeping the old list until it arrives", async () => {
    let answer = [row("a:b")];
    const load = vi.fn(() => Promise.resolve(answer));
    const w = mountPanel(1, load);
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    const list = w.get('[data-testid="refused-list"]').element as HTMLElement;
    list.scrollTop = 40;
    list.focus();
    answer = [row("a:b"), row("c:d")];
    await w.setProps({ count: 2 });
    await w.setProps({ count: 2 });
    await vi.advanceTimersByTimeAsync(400);
    expect(load).toHaveBeenCalledTimes(1);
    expect(w.findAll('[data-testid="refused-path"]')).toHaveLength(1); // old list, no flash
    await vi.advanceTimersByTimeAsync(150);
    expect(load).toHaveBeenCalledTimes(2);
    expect(w.findAll('[data-testid="refused-path"]')).toHaveLength(2);
    expect(w.get('[data-testid="refused-sentence"]').text()).toMatch(/^2 files/);
    expect(w.get('[data-testid="refused-list"]').element).toBe(list); // same container: scroll and focus kept
    expect(list.scrollTop).toBe(40);
    expect(document.activeElement).toBe(list);
    w.unmount();
  });

  it("a count that falls to 0 ends on the empty state; None closes and hands focus to the host", async () => {
    let answer = [row("a:b")];
    const w = mountPanel(1, () => Promise.resolve(answer));
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    answer = [];
    await w.setProps({ count: 0 });
    await vi.advanceTimersByTimeAsync(600);
    expect(w.find('[data-testid="refused-empty"]').exists()).toBe(true);
    expect(w.find('[role="dialog"]').exists()).toBe(true);
    await w.setProps({ count: null });
    await vi.advanceTimersByTimeAsync(1100);
    expect(w.find('[role="dialog"]').exists()).toBe(false);
    expect(w.emitted("focus-fallback")).toHaveLength(1);
    w.unmount();
  });
});

describe("store: status event and poll share one entry", () => {
  beforeEach(() => setActivePinia(createPinia()));
  const ready = { kind: "ready", note_count: 1 } as const;

  it("takes the count from the status, and None clears it", () => {
    const store = useVaultStore();
    store.applyIndexStatus({ state: ready, refused_names: 3 });
    expect(store.refusedNames).toBe(3);
    store.applyIndexStatus({ state: ready, refused_names: 0 });
    expect(store.refusedNames).toBe(0);
    store.applyIndexStatus({ state: ready });
    expect(store.refusedNames).toBeNull();
  });
});
