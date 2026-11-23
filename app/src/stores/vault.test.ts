import { describe, expect, it } from "vitest";

import { createPinia, setActivePinia } from "pinia";

import type { IndexState } from "@/ipc/generated/IndexState";

import { describeIndexCheckDetail, describeIndexDetail, describeIndexState, INDEX_CHECK_TEXT, useVaultStore } from "./vault";

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
    expect(text).toContain("Rebuilding index (no_index)");
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

  it("announces on entering, skips the plain Opening{None}, announces the state after it, then Ready clears", () => {
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
    expect(store.indexAnnouncement).toContain("Rebuilding index"); // the state after it is
    store.setIndexState({ kind: "rebuilding", reason: "no_index", done: 1, total: 5, walk_complete: false });
    expect(store.indexAnnouncement).toContain("Rebuilding index");
    store.setIndexState({ kind: "ready", note_count: 2 });
    expect(store.indexAnnouncement).toBe("");
  });
});

describe("index check found problems (P1 #157)", () => {
  const rebuilding = (done: number) =>
    ({ kind: "rebuilding", reason: "corrupt", done, total: 10, walk_complete: false }) as const;

  it("the visible text has no digit; the count is only in the description", () => {
    expect(INDEX_CHECK_TEXT).toBe("Index check found problems — rebuilding");
    expect(INDEX_CHECK_TEXT).not.toMatch(/\d/);
    expect(describeIndexCheckDetail(3)).toContain("3 problems");
    expect(describeIndexCheckDetail(1)).toContain("1 problem,");
  });

  it("shows while the field is present, announces once, and clears when it goes absent", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.applyIndexStatus({ state: rebuilding(0), index_check_problems: 2 }, "event");
    expect(store.indexCheckProblems).toBe(2);
    expect(store.indexAnnouncement).toBe(INDEX_CHECK_TEXT);
    for (let done = 1; done < 5; done++) {
      store.applyIndexStatus({ state: rebuilding(done), index_check_problems: 2 }, "event");
    }
    expect(store.indexAnnouncement).toBe(INDEX_CHECK_TEXT); // held, not re-announced
    store.applyIndexStatus({ state: { kind: "ready", note_count: 4 } }, "event");
    expect(store.indexCheckProblems).toBeNull();
  });

  it("a status without the field, or with 0, shows nothing", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.applyIndexStatus({ state: rebuilding(0) }, "event");
    expect(store.indexCheckProblems).toBeNull();
    store.applyIndexStatus({ state: rebuilding(1), index_check_problems: 0 }, "event");
    expect(store.indexCheckProblems).toBeNull();
    expect(store.indexAnnouncement).toBe("");
  });

  it("the late-subscriber poll supplies it too, until an event has spoken", () => {
    setActivePinia(createPinia());
    const store = useVaultStore();
    store.applyIndexStatus({ state: rebuilding(0), index_check_problems: 1 }, "poll");
    expect(store.indexCheckProblems).toBe(1);
  });
});
