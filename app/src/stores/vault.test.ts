import { describe, expect, it } from "vitest";

import { createPinia, setActivePinia } from "pinia";

import type { IndexState } from "@/ipc/generated/IndexState";
import type { RebuildReason } from "@/ipc/generated/RebuildReason";

import {
  describeIndexCheckDetail,
  describeIndexDetail,
  describeIndexState,
  INDEX_CHECK_ANNOUNCEMENT,
  describeRebuildAnnouncement,
  describeRebuildDetail,
  INDEX_CHECK_TEXT,
  useVaultStore,
} from "./vault";

// Serves NFR-REL-01, NFR-REL-02, FR-VLT-04, US-01, US-07.
// Exhaustive over IndexState's four variants (contracts/ipc.rs). §3.4: every one of
// them renders as a status line, never a modal and never a blocking spinner — this
// asserts each has a defined, non-empty rendering rather than assuming the switch
// covers them all.
describe("describeIndexState", () => {
  it("renders opening", () => {
    expect(describeIndexState({ kind: "opening" })).toBe("Opening index…");
  });

  it("renders ready with the note count", () => {
    expect(describeIndexState({ kind: "ready", note_count: 20_200 })).toBe(
      "index: ready · 20200 notes",
    );
  });

  it("renders rebuilding before the walk completes as a running count", () => {
    const text = describeIndexState({
      kind: "rebuilding",
      reason: "no_index",
      done: 120,
      total: 4_000,
      walk_complete: false,
    });
    expect(text).toContain("Building the index");
    expect(text).toContain("120");
    expect(text).not.toContain("120/4000");
  });

  it("renders rebuilding after the walk completes as a fraction", () => {
    const text = describeIndexState({
      kind: "rebuilding",
      reason: "vault_moved",
      done: 4_000,
      total: 4_000,
      walk_complete: true,
    });
    expect(text).toContain("4000/4000");
  });

  it("renders unavailable with the operator-readable message", () => {
    const text = describeIndexState({
      kind: "unavailable",
      message: "task I6 not implemented",
      path: "/vault/.serat/index.sqlite3",
    });
    expect(text).toBe("index: unavailable — task I6 not implemented");
  });
});

describe("opening reason (P1 #117)", () => {
  const waiting = { kind: "opening", reason: "previous_index_still_closing" } as const;

  it("renders the reason copy, with no internal term or number", () => {
    const text = describeIndexState(waiting);
    expect(text).toBe("Finishing the previous session's index — search will start when it's done");
    // \d: no count or percentage for this state (brief, "No fake progress").
    expect(text).not.toMatch(/actor|writer|detached|index\.db|lock|\d/i);
    expect(describeIndexDetail(waiting)).toContain("You can keep editing.");
  });

  it("renders absent, null and unknown reasons as the plain text without throwing", () => {
    for (const s of [
      { kind: "opening" },
      { kind: "opening", reason: null },
      { kind: "opening", reason: "held_by_another_process" },
    ]) {
      expect(describeIndexState(s as unknown as IndexState)).toBe("Opening index…");
      expect(describeIndexDetail(s as unknown as IndexState)).toBeNull();
    }
  });

  it("clears the reason when the state moves to Ready or Rebuilding", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.setIndexState(waiting);
    expect(store.indexStatusDetail).not.toBeNull();
    store.setIndexState({ kind: "ready", note_count: 3 });
    expect(store.indexStatusText).toBe("index: ready · 3 notes");
    expect(store.indexStatusDetail).toBeNull();
  });
});

describe("seeding from VaultOpened (P1 #117)", () => {
  it("shows the waiting reason from vault://opened, announces it once, and a later Opening{None} clears it", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.vaultOpened({
      root: "/v",
      vault_id: "V",
      index: { kind: "opening", reason: "previous_index_still_closing" },
    } as never);
    expect(store.indexStatusText).toContain("Finishing the previous session's index");
    expect(store.indexStatusDetail).not.toBeNull();
    expect(store.indexAnnouncement).toBe(store.indexStatusText);
    store.setIndexState({ kind: "opening" });
    expect(store.indexStatusText).toBe("Opening index…");
    expect(store.indexStatusDetail).toBeNull();
  });
});

describe("opening reason: index held by another process (P1 #155)", () => {
  const held = { kind: "opening", reason: "index_held_by_another_process" } as const;
  const HELD_TEXT = "Another Serat session is using this vault's index — search will start when it's free";

  it("renders its own copy and tooltip, different from the previous-session reason, with no digits", () => {
    expect(describeIndexState(held)).toBe(HELD_TEXT);
    expect(describeIndexState(held)).not.toBe(describeIndexState({ kind: "opening", reason: "previous_index_still_closing" }));
    expect(describeIndexState(held)).not.toMatch(/\d/); // no fake progress
    expect(describeIndexDetail(held)).toContain("Only one Serat session can update a vault's index at a time.");
    expect(describeIndexDetail(held)).toContain("You can keep editing.");
  });

  it("falls back to the plain text for a prototype-key reason", () => {
    for (const reason of ["constructor", "toString", "__proto__"]) {
      expect(describeIndexState({ kind: "opening", reason } as unknown as IndexState)).toBe("Opening index…");
    }
  });

  it("announces on entering, skips the plain Opening{None}, announces the state after it, then the Ready that ends it", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.setIndexState(held);
    expect(store.indexAnnouncement).toBe(HELD_TEXT);
    store.setIndexState(held);
    expect(store.indexAnnouncement).toBe(HELD_TEXT);
    store.setIndexState({ kind: "opening" });
    expect(store.indexStatusDetail).toBeNull();
    expect(store.indexAnnouncement).toBe(""); // the plain Opening after a cleared reason is not announced
    store.setIndexState({ kind: "opening" });
    expect(store.indexAnnouncement).toBe("");
    store.setIndexState({ kind: "rebuilding", reason: "no_index", done: 0, total: 5, walk_complete: false });
    expect(store.indexAnnouncement).toContain("Building the index"); // the state after it is
    store.setIndexState({ kind: "rebuilding", reason: "no_index", done: 1, total: 5, walk_complete: false });
    expect(store.indexAnnouncement).toContain("Building the index");
    store.setIndexState({ kind: "ready", note_count: 2 });
    expect(store.indexAnnouncement).toBe("index: ready · 2 notes"); // ends an announced Rebuilding
  });
});

describe("index check found problems (P1 #157)", () => {
  const rebuilding = (done: number) =>
    ({ kind: "rebuilding", reason: "corrupt", done, total: 10, walk_complete: false }) as const;

  it("the visible text has no digit; the count is only in the description", () => {
    expect(INDEX_CHECK_TEXT).toBe("Index check found problems");
    expect(INDEX_CHECK_TEXT).not.toMatch(/rebuilding/i); // the Rebuilding line beside it says so
    expect(INDEX_CHECK_TEXT).not.toMatch(/\d/);
    expect(describeIndexCheckDetail(3)).toContain("found 3 problems, so it is rebuilding the index from your notes");
    expect(describeIndexCheckDetail(3)).toContain("When Serat last closed unexpectedly");
    expect(describeIndexCheckDetail(1)).toContain("1 problem,");
  });

  it("shows while the field is present, announces once, and clears when it goes absent", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.applyIndexStatus({ state: rebuilding(0), index_check_problems: 2 }, "event");
    expect(store.indexCheckProblems).toBe(2);
    expect(store.indexAnnouncement).toBe(INDEX_CHECK_ANNOUNCEMENT);
    for (let done = 1; done < 5; done++) {
      store.applyIndexStatus({ state: rebuilding(done), index_check_problems: 2 }, "event");
    }
    expect(store.indexAnnouncement).toBe(INDEX_CHECK_ANNOUNCEMENT); // held, not re-announced
    store.applyIndexStatus({ state: { kind: "ready", note_count: 4 } }, "event");
    expect(store.indexCheckProblems).toBeNull();
  });

  it("the Unavailable that follows clears the fact and does not repeat its announcement", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.applyIndexStatus({ state: rebuilding(0), index_check_problems: 2 }, "event");
    store.applyIndexStatus({ state: { kind: "unavailable", message: "x", path: "/p" } }, "event");
    expect(store.indexCheckProblems).toBeNull();
    expect(store.indexStatusText).toBe("index: unavailable — x");
    expect(store.indexAnnouncement).toBe("index: unavailable — x"); // announced once, visible text
  });

  it("a status without the field, or with 0, shows nothing", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.applyIndexStatus({ state: rebuilding(0) }, "event");
    expect(store.indexCheckProblems).toBeNull();
    store.applyIndexStatus({ state: rebuilding(1), index_check_problems: 0 }, "event");
    expect(store.indexCheckProblems).toBeNull();
    expect(store.indexAnnouncement).not.toBe(INDEX_CHECK_ANNOUNCEMENT);
  });

  it("the late-subscriber poll supplies it too, until an event has spoken", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.applyIndexStatus({ state: rebuilding(0), index_check_problems: 1 }, "poll");
    expect(store.indexCheckProblems).toBe(1);
  });
});

describe("index check fact: scope and ending (P1 #157 review)", () => {
  it("another vault's open clears the previous vault's fact", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.vaultOpened({ root: "/a", vault_id: "A", index: { kind: "opening" } } as never);
    store.applyIndexStatus(
      { state: { kind: "rebuilding", reason: "corrupt", done: 0, total: 1, walk_complete: false }, index_check_problems: 2 },
      "event",
    );
    expect(store.indexCheckProblems).toBe(2);
    store.vaultOpened({ root: "/b", vault_id: "B", index: { kind: "opening" } } as never);
    expect(store.indexCheckProblems).toBeNull();
  });

  it("the fact is rendered and announced only beside Rebuilding", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.applyIndexStatus({ state: { kind: "ready", note_count: 1 }, index_check_problems: 4 }, "event");
    expect(store.indexCheckProblems).toBeNull();
    expect(store.indexAnnouncement).toBe("");
  });

  it("the Ready that ends the rebuild is announced once, with its visible text, then nothing more", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.applyIndexStatus(
      { state: { kind: "rebuilding", reason: "corrupt", done: 0, total: 1, walk_complete: false }, index_check_problems: 2 },
      "event",
    );
    store.applyIndexStatus({ state: { kind: "ready", note_count: 4 } }, "event");
    expect(store.indexAnnouncement).toBe("index: ready · 4 notes");
    store.applyIndexStatus({ state: { kind: "ready", note_count: 4 } }, "event");
    expect(store.indexAnnouncement).toBe("index: ready · 4 notes");
  });
});

// P1 #158: one line per RebuildReason. The test iterates the contract's members, so a member added without a
// row fails it (the generated type is the list; `Record<RebuildReason, ...>` in the store fails vue-tsc too).
describe("rebuild reason copy (P1 #158)", () => {
  const PHRASES: Record<RebuildReason, string> = {
    no_index: "Building the index",
    index_version: "Updating the index for this version of Serat",
    interrupted: "Completing an unfinished index rebuild",
    vault_moved: "Rebuilding the index for this vault",
    corrupt: "Repairing the index",
    user_requested: "Rebuilding the index, as you asked",
  };
  const rebuilding = (reason: string, done = 3): IndexState =>
    ({ kind: "rebuilding", reason, done, total: 10, walk_complete: true }) as IndexState;

  for (const reason of Object.keys(PHRASES) as RebuildReason[]) {
    it(`${reason}: the line starts with its phrase and no visible string or tooltip carries a wire value`, () => {
      const text = describeIndexState(rebuilding(reason));
      expect(text.startsWith(`${PHRASES[reason]} — search and queries are incomplete until this finishes · 3/10`)).toBe(true);
      const detail = describeRebuildDetail(rebuilding(reason)) ?? "";
      const announcement = describeRebuildAnnouncement(reason);
      for (const wire of Object.keys(PHRASES)) {
        for (const visible of [text, detail, announcement]) expect(visible).not.toContain(wire);
      }
      expect(detail).toMatch(/You can keep editing\.$/);
      expect(announcement).toBe(`${PHRASES[reason]}. Search and queries are incomplete until it finishes. You can keep editing.`);
    });
  }

  it("an unknown reason renders the generic line and tooltip without throwing", () => {
    for (const reason of ["something_new", "constructor", "__proto__"]) {
      expect(describeIndexState(rebuilding(reason)).startsWith("Rebuilding index — search")).toBe(true);
      expect(describeRebuildDetail(rebuilding(reason))).toContain("Serat is rebuilding the search index from your notes.");
    }
  });

  it("entering announces once, progress is silent, a reason change announces once", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.setIndexState(rebuilding("no_index", 0));
    expect(store.indexAnnouncement).toBe(describeRebuildAnnouncement("no_index"));
    store.indexAnnouncement = "sentinel";
    store.setIndexState(rebuilding("no_index", 4));
    expect(store.indexAnnouncement).toBe("sentinel"); // progress did not write
    store.setIndexState(rebuilding("interrupted", 0));
    expect(store.indexAnnouncement).toBe(describeRebuildAnnouncement("interrupted"));
  });

  it("the Ready or Unavailable that ends an announced Rebuilding is announced once; after an unannounced one, silence", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.setIndexState(rebuilding("no_index", 0));
    store.setIndexState({ kind: "ready", note_count: 5 });
    expect(store.indexAnnouncement).toBe("index: ready · 5 notes");
    store.indexAnnouncement = "sentinel";
    store.setIndexState({ kind: "ready", note_count: 5 });
    expect(store.indexAnnouncement).toBe("sentinel"); // a repeat is not re-announced

    store.setIndexState(rebuilding("corrupt", 0));
    store.setIndexState({ kind: "unavailable", message: "gone", path: "/p" });
    expect(store.indexAnnouncement).toBe("index: unavailable — gone");

    setActivePinia(createPinia());
    const fresh = useVaultStore();
    fresh.setIndexState({ kind: "ready", note_count: 1 }); // no Rebuilding before it
    expect(fresh.indexAnnouncement).toBe("");
  });

  it("the check fact's utterance replaces the Rebuilding one in the same update", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.applyIndexStatus({ state: rebuilding("corrupt", 0), index_check_problems: 2 }, "event");
    expect(store.indexAnnouncement).toBe(INDEX_CHECK_ANNOUNCEMENT);
    store.applyIndexStatus({ state: { kind: "ready", note_count: 1 } }, "event");
    expect(store.indexAnnouncement).toBe("index: ready · 1 note");
  });
});

describe("index session reset (P1 #158 review)", () => {
  const rebuilding = { kind: "rebuilding", reason: "no_index", done: 0, total: 3, walk_complete: false } as const;
  const OPENED = { root: "/a", vault_id: "A", index: { kind: "opening" } } as never;

  it("a failed open then a same-id reopen: Rebuilding is announced once, as an entry", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.vaultOpened(OPENED);
    store.setIndexState(rebuilding);
    store.openFailed();
    store.vaultOpened(OPENED);
    store.indexAnnouncement = "sentinel";
    store.setIndexState(rebuilding);
    expect(store.indexAnnouncement).toBe(describeRebuildAnnouncement("no_index"));
    store.indexAnnouncement = "sentinel";
    store.setIndexState({ ...rebuilding, done: 2 });
    expect(store.indexAnnouncement).toBe("sentinel"); // progress silent
  });

  it("a failed open then a Ready alone is silent (no announced Rebuilding ended)", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.vaultOpened(OPENED);
    store.setIndexState(rebuilding);
    store.openFailed();
    store.vaultOpened(OPENED);
    store.indexAnnouncement = "";
    store.setIndexState({ kind: "ready", note_count: 2 });
    expect(store.indexAnnouncement).toBe("");
  });

  it("every open, same id or not, starts a new session", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.vaultOpened(OPENED);
    store.applyIndexStatus({ state: rebuilding, index_check_problems: 1 }, "event");
    store.vaultOpened(OPENED);
    expect(store.indexCheckProblems).toBeNull();
    store.setIndexState({ kind: "ready", note_count: 2 });
    expect(store.indexAnnouncement).not.toBe("index: ready · 2 notes");
  });
});
