import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";

import { useVaultStore } from "./vault";

describe("refused names status fact", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("is unknown until the status carries a count, then a single number", () => {
    const store = useVaultStore();
    expect(store.refusedNames).toBeNull();
    store.setRefusedNames(3);
    expect(store.refusedNames).toBe(3);
  });
});
