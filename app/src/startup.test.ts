import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useVaultStore } from "@/stores/vault";

const markFirstPaint = vi.fn();
const markVaultUsable = vi.fn();
const readFile = vi.fn();

vi.mock("@/ipc/client", () => ({
  markFirstPaint: (...args: unknown[]) => markFirstPaint(...args),
  markVaultUsable: (...args: unknown[]) => markVaultUsable(...args),
  readFile: (...args: unknown[]) => readFile(...args),
}));

// Imported after the mock so startup.ts resolves the mocked module.
const { hydrateBuffer, schedulePaintMark } = await import("./startup");

/**
 * A double `requestAnimationFrame`, faked with microtasks rather than real frames
 * so the test does not depend on a browser's paint scheduler. Each call gets a
 * distinct, increasing timestamp (10, 20, 30, ...) so `schedulePaintMark`'s tests can
 * tell `raf1_ms` and `raf2_ms` apart rather than both landing on the same stub value
 * — the same property crates/serat-app/src/startup.rs's `mark_first_paint` test
 * asserts on the Rust side ("Nothing here is a product feature" — this is purely a
 * scheduling contract).
 */
function stubDoubleRaf() {
  let calls = 0;
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    calls += 1;
    queueMicrotask(() => cb(calls * 10));
    return calls;
  });
}

/** The shape `markFirstPaint` is called with — matches `PaintEvidence`, kept local so this file does not import a type only to satisfy a cast. */
interface PaintEvidenceForTest {
  firstContentfulPaintMs: number | null;
  raf1Ms: number;
  raf2Ms: number;
  navDomContentLoadedMs: number | null;
}

/**
 * `schedulePaintMark` reads two different `performance.getEntriesByType` calls —
 * `"paint"` for `firstContentfulPaintMs` and `"navigation"` for
 * `navDomContentLoadedMs` — so a single blanket mock would make one test's fixture
 * leak into the other's assertion. This stubs both by type, defaulting to an empty
 * list for whichever the caller does not supply.
 */
function stubPerformanceEntries(byType: Partial<Record<string, PerformanceEntry[]>>) {
  vi.spyOn(performance, "getEntriesByType").mockImplementation(
    (type: string) => byType[type] ?? [],
  );
}

beforeEach(() => {
  setActivePinia(createPinia());
  stubDoubleRaf();
  // Deterministic default: no PerformanceObserver at all, so `whenFirstContentfulPaint`
  // takes its "platform cannot ever produce one" branch unless a test stubs otherwise.
  // Real jsdom's own support for the Paint Timing API is not something this suite
  // should depend on incidentally.
  vi.stubGlobal("PerformanceObserver", undefined);
  markFirstPaint.mockReset();
  markVaultUsable.mockReset();
  readFile.mockReset();
  window.__SERAT_BOOT__ = undefined;
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("schedulePaintMark", () => {
  it("does not call mark_first_paint synchronously", () => {
    markFirstPaint.mockResolvedValue(12.5);
    const store = useVaultStore();
    schedulePaintMark(store);
    expect(markFirstPaint).not.toHaveBeenCalled();
  });

  it("resolves with the mark and records it on the store", async () => {
    markFirstPaint.mockResolvedValue(12.5);
    const store = useVaultStore();
    const ms = await schedulePaintMark(store);
    expect(ms).toBe(12.5);
    expect(store.marksText).toBe("painted 12.5 ms");
  });

  it("sends the two callbacks' own timestamps as evidence, not a recomputed pair", async () => {
    // task #108 / F-377: the point of PaintEvidence is that raf1Ms and raf2Ms are
    // the browser's own callback arguments. If schedulePaintMark ever starts calling
    // performance.now() a second time instead of using the callback argument, this
    // still passes as long as the two values it sends came from the two distinct
    // scheduled callbacks in order.
    markFirstPaint.mockResolvedValue(1.0);
    const store = useVaultStore();
    await schedulePaintMark(store);

    expect(markFirstPaint).toHaveBeenCalledTimes(1);
    const evidence = markFirstPaint.mock.calls[0][0] as PaintEvidenceForTest;
    expect(evidence.raf1Ms).toBe(10);
    expect(evidence.raf2Ms).toBe(20);
    expect(evidence.raf2Ms).toBeGreaterThan(evidence.raf1Ms);
  });

  it("sends camelCase keys, matching Tauri's default rename of command parameters", async () => {
    // Confirmed against task #105's actual Rust signature (first_contentful_paint_ms),
    // which Tauri only matches when the JS side sends firstContentfulPaintMs — the
    // snake_case form this file shipped for one revision would have silently failed
    // to reach the parameter at all. See PaintEvidence's own doc comment.
    markFirstPaint.mockResolvedValue(1.0);
    const store = useVaultStore();
    await schedulePaintMark(store);

    const evidence = markFirstPaint.mock.calls[0][0] as Record<string, unknown>;
    expect(Object.keys(evidence).sort()).toEqual(
      ["firstContentfulPaintMs", "navDomContentLoadedMs", "raf1Ms", "raf2Ms"].sort(),
    );
  });

  it("reports the first-contentful-paint entry, not the frame gate", async () => {
    // F-388: a frame gate alone is forgeable by a mutation that honestly waits for
    // two real, early frames before this app has mounted anything visible.
    // firstContentfulPaintMs is the evidence that closes that gap, and it is not the
    // same signal as navDomContentLoadedMs (which fires on HTML parse, before any
    // script runs, and cannot see whether real content ever painted).
    stubPerformanceEntries({
      paint: [{ name: "first-contentful-paint", startTime: 17 } as PerformancePaintTiming],
    });
    markFirstPaint.mockResolvedValue(1.0);

    const store = useVaultStore();
    await schedulePaintMark(store);

    const evidence = markFirstPaint.mock.calls[0][0] as PaintEvidenceForTest;
    expect(evidence.firstContentfulPaintMs).toBe(17);
  });

  it("reports null for first-contentful-paint when the platform has no PerformanceObserver", async () => {
    // Set explicitly by this test even though beforeEach already does — the property
    // under test is that "no way to ever get one" resolves null rather than hanging.
    vi.stubGlobal("PerformanceObserver", undefined);
    stubPerformanceEntries({});
    markFirstPaint.mockResolvedValue(1.0);

    const store = useVaultStore();
    await schedulePaintMark(store);

    const evidence = markFirstPaint.mock.calls[0][0] as PaintEvidenceForTest;
    expect(evidence.firstContentfulPaintMs).toBeNull();
  });

  it("waits for the entry rather than reading it too early — the real bug this file shipped once", async () => {
    // A real WebView2 run found that a synchronous performance.getEntriesByType
    // check inside the second rAF callback was too early: the entry was not yet in
    // the buffer even though the frame had genuinely painted. This fakes exactly
    // that shape — nothing available synchronously, PerformanceObserver delivers the
    // entry on its own schedule — and asserts markFirstPaint is not called until it
    // does, with the delivered value.
    stubPerformanceEntries({});
    let deliverEntry!: (entries: PerformanceEntry[]) => void;
    class FakePaintObserver {
      static supportedEntryTypes = ["paint"];
      #callback: PerformanceObserverCallback;
      constructor(callback: PerformanceObserverCallback) {
        this.#callback = callback;
      }
      observe() {
        deliverEntry = (entries) => {
          this.#callback({ getEntries: () => entries, getEntriesByType: () => entries } as never, this as never);
        };
      }
      disconnect() {}
    }
    vi.stubGlobal("PerformanceObserver", FakePaintObserver);
    markFirstPaint.mockResolvedValue(1.0);

    const store = useVaultStore();
    const done = schedulePaintMark(store);

    await Promise.resolve();
    await Promise.resolve();
    expect(markFirstPaint).not.toHaveBeenCalled();

    deliverEntry([{ name: "first-contentful-paint", startTime: 184 } as PerformancePaintTiming]);
    await done;

    const evidence = markFirstPaint.mock.calls[0][0] as PaintEvidenceForTest;
    expect(evidence.firstContentfulPaintMs).toBe(184);
  });

  it("reports the navigation-timing anchor when the webview recorded one", async () => {
    stubPerformanceEntries({
      navigation: [{ domContentLoadedEventEnd: 42 } as PerformanceNavigationTiming],
    });
    markFirstPaint.mockResolvedValue(1.0);

    const store = useVaultStore();
    await schedulePaintMark(store);

    const evidence = markFirstPaint.mock.calls[0][0] as PaintEvidenceForTest;
    expect(evidence.navDomContentLoadedMs).toBe(42);
  });

  it("reports null rather than throwing when there is no navigation entry", async () => {
    stubPerformanceEntries({});
    markFirstPaint.mockResolvedValue(1.0);

    const store = useVaultStore();
    await schedulePaintMark(store);

    const evidence = markFirstPaint.mock.calls[0][0] as PaintEvidenceForTest;
    expect(evidence.navDomContentLoadedMs).toBeNull();
  });

  it("resolves with null and records the failure rather than throwing", async () => {
    markFirstPaint.mockRejectedValue({ kind: "io", message: "boom" });
    const store = useVaultStore();
    const ms = await schedulePaintMark(store);
    expect(ms).toBeNull();
    expect(store.marksText).toContain("paint mark failed");
  });
});

describe("hydrateBuffer", () => {
  it("with nothing to open, waits for the paint mark before calling mark_vault_usable(null)", async () => {
    window.__SERAT_BOOT__ = { vault: "/vault", open: null, exit_after_ready: false };
    markVaultUsable.mockResolvedValue(20);

    // A paint mark that has not resolved yet: the ordering assertion below only
    // means something if hydrateBuffer is actually made to wait for it.
    let resolvePaint!: (ms: number | null) => void;
    const paintMark = new Promise<number | null>((resolve) => {
      resolvePaint = resolve;
    });

    const editor = { setText: vi.fn() };
    const store = useVaultStore();
    const done = hydrateBuffer(store, editor, paintMark);

    await Promise.resolve();
    await Promise.resolve();
    expect(markVaultUsable).not.toHaveBeenCalled();

    resolvePaint(9.0);
    await done;

    expect(markVaultUsable).toHaveBeenCalledWith(null);
    expect(store.marksText).toContain("usable 20.0 ms");
  });

  it("reads the boot note into the editor and reports it as the usable note", async () => {
    window.__SERAT_BOOT__ = { vault: "/vault", open: "a.md", exit_after_ready: false };
    readFile.mockResolvedValue({
      path: "a.md",
      text: "# hello",
      hash: "deadbeef",
      mtime_ms: 0,
      bytes: 7,
    });
    markVaultUsable.mockResolvedValue(31);

    const editor = { setText: vi.fn() };
    const store = useVaultStore();
    await hydrateBuffer(store, editor, Promise.resolve(9.0));

    expect(readFile).toHaveBeenCalledWith({ path: "a.md" });
    expect(editor.setText).toHaveBeenCalledWith("# hello");
    expect(store.openNote).toBe("a.md");
    expect(store.baseHash).toBe("deadbeef");
    expect(markVaultUsable).toHaveBeenCalledWith("a.md");
  });

  it("shows a refused read as text in the buffer rather than a modal (§9.1)", async () => {
    window.__SERAT_BOOT__ = { vault: "/vault", open: "missing.md", exit_after_ready: false };
    readFile.mockRejectedValue({ kind: "not_found", path: "missing.md" });
    markVaultUsable.mockResolvedValue(5);

    const editor = { setText: vi.fn() };
    const store = useVaultStore();
    await hydrateBuffer(store, editor, Promise.resolve(1.0));

    expect(editor.setText).toHaveBeenCalledWith(expect.stringContaining("missing.md"));
    expect(store.openNote).toBeNull();
    // Still reported usable: an unreadable note is not a blocked shell (§9.1).
    expect(markVaultUsable).toHaveBeenCalledWith(null);
  });
});

/**
 * The call-site count (#125, closing the front-end half of F-481/F-482/F-504) — the
 * source-side twin of the count `crates/serat-app/src/lib.rs` takes over the shipped
 * bundle.
 *
 * F-392 closed on an assertion that the paint mark is invoked *exactly once*. F-481
 * found that assertion is not in the repository and was dropped rather than moved,
 * and F-504 then reproduced what its absence costs against a real WebView2 window: a
 * second `mark_first_paint` call carrying fabricated but plausible evidence
 * (`firstContentfulPaintMs: 1, raf1Ms: 0, raf2Ms: 1`) is **accepted, recorded
 * verbatim in the audit field, and the honest call is silently no-opped** by the
 * Rust side's first-wins rule. The mark is stamped when the call *arrives*, so the
 * number that results is **unbounded in both directions** — F-504's own placement
 * came out worse than honest (`t0_to_window_visible_ms` 2687 against 2458) because
 * module evaluation happens after window creation, and a site hoisted earlier would
 * come out better. What this check buys is that a second site cannot be added
 * without something going red. It does **not** make the guard able to tell
 * fabricated evidence from real: one call carrying plausible-positive values is
 * still accepted on its own say-so (F-482), and that is ADR-0014's to settle.
 *
 * Why this is a twin and not a duplicate. tauri-app-dev's count matches the command
 * *name string* in `crates/serat-app/webview/boot.js`, the artefact that actually
 * ships — so it catches a literal hand-edited into the committed bundle, which
 * vitest never reads (F-493: the bundle is where a weakening survives review by
 * default). It cannot catch a second call written as `markFirstPaint({...})` against
 * the wrapper already in `app/src/ipc/client.ts`, because that route leaves the
 * command name at one occurrence and passes. This check reads source, so it catches
 * the wrapper route at authoring time — and F-470's two-build hazard does not reach
 * it, since no rebuild stands between the edit and the observation.
 *
 * That division was measured rather than reasoned, because it is the whole argument
 * for a second check. `markFirstPaint({ firstContentfulPaintMs: 1, raf1Ms: 0,
 * raf2Ms: 1, ... })` was added to `main.ts` — F-504's sharp form through the existing
 * wrapper — and `npm run build` run: the shipped `boot.js` **still names
 * `mark_first_paint` exactly once**, so the bundle-side count passes, while the
 * fabricated object is in it verbatim. Reverted, and the rebuild reproduced the
 * committed bundle byte-identically.
 *
 * What it cannot see, said here rather than left to be found: a call inside a
 * `*.test.ts` file (excluded below, and none of it ships); a call reached through a
 * dynamic property access such as `client["markFirstPaint"]()`; and anything in the
 * bundle that is not in `app/src/`, which is the other half's job.
 */
describe("the startup marks have exactly one call site each", () => {
  // Derived from the module path rather than with `new URL(".", import.meta.url)`:
  // under the jsdom environment `URL` is jsdom's, and it resolves the relative part
  // against the *document* base (`http://localhost:3000`) instead of the module.
  const sourceRoot = dirname(fileURLToPath(import.meta.url));

  /** Every shipping source file under `app/src/`. Tests are excluded — see the note above. */
  function shippingSources(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true })
      .sort((a, b) => a.name.localeCompare(b.name))
      .flatMap((entry) => {
        const full = join(dir, entry.name);
        if (entry.isDirectory()) return shippingSources(full);
        if (entry.name.endsWith(".test.ts")) return [];
        return /\.(ts|vue)$/.test(entry.name) ? [full] : [];
      });
  }

  /**
   * Counted **without** stripping comments first. A comment that writes the name
   * with parentheses will trip this, which is a false alarm with a one-word fix
   * (drop the parentheses); a stripper is the other trade, and it would let a call
   * hide behind anything it mistook for a comment. A guard for a forgeable mark
   * should fail loudly rather than quietly.
   */
  function occurrences(text: string, pattern: RegExp): number {
    return text.match(pattern)?.length ?? 0;
  }

  const files = shippingSources(sourceRoot).map((path) => ({
    path,
    relative: relative(sourceRoot, path).replaceAll("\\", "/"),
    text: readFileSync(path, "utf8"),
  }));

  it("scans a plausible number of files, so an empty scan cannot pass vacuously", () => {
    // Without this, a bad `shippingSources` (wrong root, wrong extension test) makes
    // every assertion below true over an empty set — the shape of guard this project
    // has recorded fifteen of.
    expect(files.length).toBeGreaterThan(3);
    expect(files.map((file) => file.relative)).toContain("startup.ts");
    expect(files.map((file) => file.relative)).toContain("ipc/client.ts");
  });

  /** Every call of `wrapper` in `app/src/`, by file. The wrapper's own definition in
   * `ipc/client.ts` is a declaration, not a call, and is subtracted. */
  function callSites(wrapper: string): { relative: string; count: number }[] {
    const call = new RegExp(String.raw`\b${wrapper}\s*\(`, "g");
    const declaration = new RegExp(String.raw`\bfunction\s+${wrapper}\s*\(`, "g");
    return files
      .map((file) => ({
        relative: file.relative,
        count: occurrences(file.text, call) - occurrences(file.text, declaration),
      }))
      .filter((file) => file.count > 0);
  }

  function describeSites(sites: { relative: string; count: number }[]): string {
    return sites.map((site) => `${site.relative}×${site.count}`).join(", ") || "nowhere";
  }

  it("markFirstPaint is called from exactly one site", () => {
    const sites = callSites("markFirstPaint");
    const total = sites.reduce((sum, site) => sum + site.count, 0);

    expect(
      total,
      `markFirstPaint is called ${total} times across app/src (${describeSites(sites)}); ` +
        `exactly one is the guarantee. Zero means the page stopped taking the mark, and a ` +
        `green cold-start job would then be measuring nothing. More than one means a second ` +
        `call can claim mark_first_paint first and silently no-op the honest one (first-wins), ` +
        `with the resulting number unbounded in both directions — F-392, F-481, F-504.`,
    ).toBe(1);
    expect(sites[0]?.relative).toBe("startup.ts");
  });

  /**
   * `markVaultUsable` gets confinement rather than a count, and the difference is a
   * fact about the code rather than a softer standard: `hydrateBuffer` marks usable
   * from each of its two mutually exclusive branches (a restored note, or nothing to
   * restore), so *two* is the correct number and pinning one would be asserting
   * something false. The count that does hold for it is the one in
   * `crates/serat-app/src/lib.rs` — the shipped bundle names the command once,
   * because both branches reach it through the single wrapper. Confinement is what
   * this side can add: the mark is taken by the startup path and by nothing else, so
   * a component or store that decides to report the vault usable goes red here.
   */
  it("markVaultUsable is called only from the startup path", () => {
    const sites = callSites("markVaultUsable");

    expect(
      sites.map((site) => site.relative),
      `markVaultUsable is called from ${describeSites(sites)}; only startup.ts may take ` +
        `the mark. Its two sites there are hydrateBuffer's two mutually exclusive branches ` +
        `— see this block's note for why this is confinement and not a count.`,
    ).toEqual(["startup.ts"]);
    expect(sites[0]?.count).toBeGreaterThan(0);
  });

  it.each(["markFirstPaint", "markVaultUsable"])(
    "%s is never imported under another name, which is the one static route past the count",
    (wrapper) => {
      // `import { markFirstPaint as mfp }` then `mfp(...)` would leave the count
      // above at one and the command-name count in boot.js at one as well.
      const aliased = files.filter((file) =>
        new RegExp(String.raw`\b${wrapper}\s+as\s+`).test(file.text),
      );
      expect(aliased.map((file) => file.relative)).toEqual([]);
    },
  );

  it.each(["mark_first_paint", "mark_vault_usable"])(
    "%s is invoked by name from exactly one place, the ipc/client.ts wrapper",
    (command) => {
      // The other route to a second call site: a fresh `invoke("mark_first_paint")`
      // beside the wrapper rather than through it. boot.js's count catches this one
      // too; catching it here names the offending source file instead of reporting a
      // number over a minified bundle.
      const invocations = files
        .map((file) => ({
          relative: file.relative,
          count: occurrences(file.text, new RegExp(String.raw`invoke\(\s*"${command}"`, "g")),
        }))
        .filter((file) => file.count > 0);

      expect(invocations).toEqual([{ relative: "ipc/client.ts", count: 1 }]);
    },
  );
});
