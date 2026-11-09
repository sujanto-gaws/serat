// Task #91 (FR-VLT-01, FR-VLT-04, US-06; ADR-0016; architecture 3.4 "Which vault an event belongs to"):
// where the front end's vault id comes from, and what may change it.

import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";

import type { VaultOpened } from "@/ipc/generated/VaultOpened";
import { useVaultStore } from "@/stores/vault";

const opened = (vault_id: string, root = "/v"): VaultOpened =>
  ({ root, vault_id, index: { kind: "opening" } }) as VaultOpened;

beforeEach(() => setActivePinia(createPinia()));

describe("vault id sources", () => {
  it("launch with a BootInfo id: an index_status with another id is ignored", () => {
    const s = useVaultStore();
    s.adoptBoot("A");
    s.adoptFromIndexStatus("B");
    expect(s.vaultId).toBe("A");
  });

  it("launch without a BootInfo id: the first index_status sets it; a later vault://opened replaces it; index_status can no longer change it", () => {
    const s = useVaultStore();
    s.adoptBoot(undefined);
    expect(s.vaultId).toBeNull();
    s.adoptFromIndexStatus("A");
    expect(s.vaultId).toBe("A");
    s.vaultOpened(opened("B", "/b"));
    expect(s.vaultId).toBe("B");
    expect(s.vaultRoot).toBe("/b");
    s.adoptFromIndexStatus("A"); // a poll computed before the switch's close
    s.adoptFromIndexStatus("");
    s.adoptFromIndexStatus("C");
    expect(s.vaultId).toBe("B");
  });

  it("an empty id is never adopted, from any source", () => {
    const s = useVaultStore();
    s.adoptBoot("");
    s.adoptFromIndexStatus("");
    expect(s.vaultId).toBeNull();
    s.adoptFromIndexStatus("A");
    s.vaultOpened(opened(""));
    expect(s.vaultId).toBe("A");
    s.adoptFromIndexStatus(""); // and the empty poll answer never clears it either
    expect(s.vaultId).toBe("A");
  });

  it("an empty BootInfo id leaves the fallback open (absent means absent)", () => {
    const s = useVaultStore();
    s.adoptBoot("");
    s.adoptFromIndexStatus("A");
    expect(s.vaultId).toBe("A");
  });

  it("vault://opened then a failed open: the window has no vault, and a stale index_status with the announced id is dropped", () => {
    const s = useVaultStore();
    s.adoptBoot("A");
    s.vaultOpened(opened("B", "/b"));
    s.openFailed(); // what #80 calls when choose_vault answers Err
    expect(s.vaultId).toBeNull();
    expect(s.vaultRoot).toBeNull();
    s.adoptFromIndexStatus("B"); // the stale poll carrying the announced id
    s.adoptFromIndexStatus("A");
    expect(s.vaultId).toBeNull();
    s.vaultOpened(opened("D")); // a later successful open recovers
    expect(s.vaultId).toBe("D");
  });

  it("a BootInfo launch vault is replaced by vault://opened too", () => {
    const s = useVaultStore();
    s.adoptBoot("A");
    s.vaultOpened(opened("B"));
    expect(s.vaultId).toBe("B");
  });
});
