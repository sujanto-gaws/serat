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

describe("loading and failures", () => {
  const mountPanel = (count: number | null, load: () => Promise<RefusedName[]>) =>
    mount(RefusedNames, { props: { count, load }, attachTo: document.body });
  const deferred = <T,>() => {
    let resolve!: (v: T) => void;
    let reject!: (e: unknown) => void;
    const promise = new Promise<T>((res, rej) => ((resolve = res), (reject = rej)));
    return { promise, resolve, reject };
  };
  const status = (w: ReturnType<typeof mount>) => w.get('[data-testid="refused-panel-status"]');

  it("first open: no loading line for a fast read; 'Loading the list…' only after 150 ms, announced once, then the result", async () => {
    const fast = mountPanel(1, () => Promise.resolve([row("a:b")]));
    await fast.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    expect(fast.find('[data-testid="refused-loading"]').exists()).toBe(false);
    expect(status(fast).text()).toMatch(/^1 file in your vault folder/);
    fast.unmount();

    const d = deferred<RefusedName[]>();
    const w = mountPanel(2, () => d.promise);
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(149);
    expect(w.find('[data-testid="refused-loading"]').exists()).toBe(false);
    await vi.advanceTimersByTimeAsync(2);
    expect(w.get('[data-testid="refused-loading"]').text()).toBe("Loading the list…");
    expect(status(w).text()).toBe("Loading the list");
    expect(status(w).attributes("role")).toBe("status");
    expect(w.find(".spinner, [role='progressbar']").exists()).toBe(false);
    d.resolve([row("a:b"), row("c:d")]);
    await vi.advanceTimersByTimeAsync(0);
    expect(w.find('[data-testid="refused-loading"]').exists()).toBe(false);
    expect(status(w).text()).toMatch(/^2 files in your vault folder/);
    w.unmount();
  });

  it("first open that finds nothing announces the empty line", async () => {
    const w = mountPanel(1, () => Promise.resolve([]));
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    expect(status(w).text()).toBe("Every name in this vault can be shown.");
    w.unmount();
  });

  it("first-open failure shows the load-failure state and announces it", async () => {
    const w = mountPanel(1, () => Promise.reject(new Error("x")));
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(500);
    expect(w.get('[data-testid="refused-failed"]').text()).toContain("Couldn't load the list.");
    expect(status(w).text()).toContain("Couldn't load the list.");
    w.unmount();
  });

  it("a re-fetch with an old list shows no loading state; if it fails the old list stays with a line and Retry above it", async () => {
    let mode: "ok" | "fail" | "slow" = "ok";
    const gate = deferred<RefusedName[]>();
    const load = vi.fn(() => (mode === "fail" ? Promise.reject(new Error("x")) : mode === "slow" ? gate.promise : Promise.resolve([row("a:b")])));
    const w = mountPanel(1, load);
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);

    mode = "slow";
    await w.setProps({ count: 2 });
    await vi.advanceTimersByTimeAsync(500 + 300);
    expect(load).toHaveBeenCalledTimes(2);
    expect(w.find('[data-testid="refused-loading"]').exists()).toBe(false); // old list, no loading state
    expect(w.findAll('[data-testid="refused-path"]')).toHaveLength(1);
    gate.resolve([row("a:b"), row("c:d")]);
    await vi.advanceTimersByTimeAsync(0);

    mode = "fail";
    await w.setProps({ count: 3 });
    await vi.advanceTimersByTimeAsync(600);
    expect(load).toHaveBeenCalledTimes(3);
    expect(w.findAll('[data-testid="refused-path"]')).toHaveLength(2); // the old list stays
    expect(w.get('[data-testid="refused-refresh-failed"]').text()).toBe("Couldn't refresh the list. It may be out of date.");
    expect(w.get('[data-testid="refused-sentence"]').text()).toMatch(/^2 files/); // header keeps the old count
    expect(status(w).text()).toBe("Couldn't refresh the list. It may be out of date.");
    // the line sits between the header sentence and the first row
    const html = w.get('[data-testid="refused-panel"]').html();
    expect(html.indexOf("refused-sentence")).toBeLessThan(html.indexOf("refused-refresh-failed"));
    expect(html.indexOf("refused-refresh-failed")).toBeLessThan(html.indexOf("refused-list"));
    await vi.advanceTimersByTimeAsync(5000);
    expect(load).toHaveBeenCalledTimes(3); // no retry loop

    mode = "ok";
    await w.get('[data-testid="refused-retry"]').trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    expect(w.find('[data-testid="refused-refresh-failed"]').exists()).toBe(false);
    expect(w.findAll('[data-testid="refused-path"]')).toHaveLength(1);
    w.unmount();
  });

  it("a later count change re-fetches and clears the failure line when it succeeds", async () => {
    let fail = false;
    const load = vi.fn(() => (fail ? Promise.reject(new Error("x")) : Promise.resolve([row("a:b")])));
    const w = mountPanel(1, load);
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    fail = true;
    await w.setProps({ count: 2 });
    await vi.advanceTimersByTimeAsync(600);
    expect(w.find('[data-testid="refused-refresh-failed"]').exists()).toBe(true);
    fail = false;
    await w.setProps({ count: 3 });
    await vi.advanceTimersByTimeAsync(600);
    expect(w.find('[data-testid="refused-refresh-failed"]').exists()).toBe(false);
    w.unmount();
  });
});

describe("focus is never dropped to the body", () => {
  const mountPanel = (count: number | null, load: () => Promise<RefusedName[]>) =>
    mount(RefusedNames, { props: { count, load }, attachTo: document.body });

  it("Retry succeeding moves focus to the dialog, and Esc still closes", async () => {
    const load = vi.fn().mockRejectedValueOnce(new Error("x")).mockResolvedValue([row("a:b")]);
    const w = mountPanel(1, load);
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(500);
    (w.get('[data-testid="refused-retry"]').element as HTMLElement).focus();
    await w.get('[data-testid="refused-retry"]').trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    expect(w.find('[data-testid="refused-retry"]').exists()).toBe(false);
    expect(document.activeElement).toBe(w.get('[role="dialog"]').element);
    await w.get('[role="dialog"]').trigger("keydown", { key: "Escape" });
    await vi.advanceTimersByTimeAsync(0);
    expect(w.find('[role="dialog"]').exists()).toBe(false);
    w.unmount();
  });

  it("the focused list emptying on a re-fetch moves focus to the dialog", async () => {
    let answer = [row("a:b")];
    const w = mountPanel(1, () => Promise.resolve(answer));
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    (w.get('[data-testid="refused-list"]').element as HTMLElement).focus();
    answer = [];
    await w.setProps({ count: 0 });
    await vi.advanceTimersByTimeAsync(600);
    expect(w.find('[data-testid="refused-list"]').exists()).toBe(false);
    expect(document.activeElement).toBe(w.get('[role="dialog"]').element);
    await w.get('[role="dialog"]').trigger("keydown", { key: "Escape" });
    await vi.advanceTimersByTimeAsync(0);
    expect(w.find('[role="dialog"]').exists()).toBe(false);
    w.unmount();
  });
});

describe("the leaving panel", () => {
  it("is inert, hidden and id-free during the fade, so one #refused-title remains after a quick reopen", async () => {
    const real = window.getComputedStyle.bind(window);
    vi.spyOn(window, "getComputedStyle").mockImplementation(((el: Element, pseudo?: string | null) => {
      const style = real(el, pseudo);
      return new Proxy(style, {
        get: (target, prop) => {
          if (prop === "transitionDuration") return "0.16s";
          if (prop === "transitionDelay") return "0s";
          if (prop === "transitionProperty") return "opacity";
          const v = Reflect.get(target, prop);
          return typeof v === "function" ? v.bind(target) : v;
        },
      });
    }) as typeof window.getComputedStyle);
    const w = mount(RefusedNames, {
      props: { count: 1, load: () => Promise.resolve([row("a:b")]) },
      attachTo: document.body,
      global: { stubs: { transition: false } },
    });
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(300);
    await w.get('[data-testid="refused-close"]').trigger("click");
    await vi.advanceTimersByTimeAsync(10);
    const leaving = document.querySelector<HTMLElement>('[data-testid="refused-panel"]');
    expect(leaving).not.toBeNull();
    expect(leaving?.hasAttribute("inert")).toBe(true);
    expect(leaving?.getAttribute("aria-hidden")).toBe("true");
    expect(leaving?.querySelector("[id]")).toBeNull();
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(10);
    expect(document.querySelectorAll("#refused-title")).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(400);
    expect(document.querySelectorAll('[data-testid="refused-panel"]')).toHaveLength(1);
    w.unmount();
    vi.restoreAllMocks();
  });

  it("a repeated identical failure is announced again, and Retry leaves the failure state while loading", async () => {
    const load = vi.fn(() => Promise.reject(new Error("x")));
    const w = mount(RefusedNames, { props: { count: 1, load }, attachTo: document.body });
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(200);
    const status = () => w.get('[data-testid="refused-panel-status"]').text();
    expect(status()).toContain("Couldn't load the list.");
    await w.get('[data-testid="refused-retry"]').trigger("click");
    expect(status()).toBe(""); // cleared before the call, so the same sentence is a change
    expect(w.find('[data-testid="refused-failed"]').exists()).toBe(false);
    await vi.advanceTimersByTimeAsync(200);
    expect(status()).toContain("Couldn't load the list.");
    w.unmount();
  });
});

describe("untrusted text edge cases", () => {
  it("a literal escape-looking name does not render like a real U+202E", () => {
    const literal = escapeUntrusted("a" + String.fromCodePoint(0x27e8) + "U+202E" + String.fromCodePoint(0x27e9) + "b");
    const real = escapeUntrusted("a" + String.fromCodePoint(0x202e) + "b");
    expect(literal).not.toBe(real);
    expect(literal).toContain("U+27E8");
  });

  it("escapes tag characters and the soft hyphen too", () => {
    expect(escapeUntrusted("a" + String.fromCodePoint(0xad) + "b")).toContain("U+00AD");
    expect(escapeUntrusted(String.fromCodePoint(0xe0041))).toContain("U+E0041");
  });

  it("an unpaired backtick in a reason stays literal text", async () => {
    const w = mount(RefusedNames, {
      props: { count: 1, load: () => Promise.resolve([row("a:b", "contains a `:` and a stray ` here")]) },
      attachTo: document.body,
    });
    await w.get("button").trigger("click");
    await vi.advanceTimersByTimeAsync(0);
    const reason = w.get('[data-testid="refused-reason"]');
    expect(reason.text()).toBe("Contains a : and a stray ` here");
    expect(reason.findAll("code")).toHaveLength(1);
    w.unmount();
  });
});

describe("store: vault switch and poll ordering", () => {
  beforeEach(() => setActivePinia(createPinia()));
  const ready = { kind: "ready", note_count: 1 } as const;

  it("a different vault id clears the count; the same id keeps it", () => {
    const store = useVaultStore();
    store.vaultOpened({ vault_id: "v1", root: "/a" } as never);
    store.setRefusedNames(4);
    store.vaultOpened({ vault_id: "v1", root: "/a" } as never);
    expect(store.refusedNames).toBe(4);
    store.vaultOpened({ vault_id: "v2", root: "/b" } as never);
    expect(store.refusedNames).toBeNull();
  });

  it("a poll answer after an event is ignored; before any event it applies", () => {
    const store = useVaultStore();
    store.applyIndexStatus({ state: ready, refused_names: 2 }, "poll");
    expect(store.refusedNames).toBe(2);
    store.applyIndexStatus({ state: ready, refused_names: 5 }, "event");
    store.applyIndexStatus({ state: ready, refused_names: 1 }, "poll");
    expect(store.refusedNames).toBe(5);
  });
});

describe("store: status event and poll share one entry", () => {
  beforeEach(() => setActivePinia(createPinia()));
  const ready = { kind: "ready", note_count: 1 } as const;

  it("takes the count from the status, and None clears it", () => {
    const store = useVaultStore();
    store.applyIndexStatus({ state: ready, refused_names: 3 }, "event");
    expect(store.refusedNames).toBe(3);
    store.applyIndexStatus({ state: ready, refused_names: 0 }, "event");
    expect(store.refusedNames).toBe(0);
    store.applyIndexStatus({ state: ready }, "event");
    expect(store.refusedNames).toBeNull();
  });
});
