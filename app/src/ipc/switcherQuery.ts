// The switcher's sequence discipline, in one place (architecture.md §9.2;
// `SwitcherQueryArgs`): every query carries a monotonically increasing `seq`;
// only the answer to the newest one is ever used. A `discarded` answer means
// "superseded, ignore" — never "no matches". Shared by the quick switcher and
// wikilink completion (`editor/links.ts`), which is the same corpus.

import type { SwitcherQueryArgs } from "./generated/SwitcherQueryArgs";
import type { SwitcherResults } from "./generated/SwitcherResults";

export type SwitcherQueryFn = (args: SwitcherQueryArgs) => Promise<SwitcherResults>;

export interface SwitcherQuerier {
  /**
   * Sends `query` with the next `seq`. Resolves to the answer only if it is
   * still the newest query's and was actually computed; `null` otherwise
   * (superseded, discarded or out of order). A transport failure rejects.
   */
  run(query: string, limit?: number): Promise<SwitcherResults | null>;
  /** The newest `seq` sent so far. */
  readonly latest: number;
}

export function createSwitcherQuerier(query: SwitcherQueryFn): SwitcherQuerier {
  let seq = 0;
  return {
    get latest() {
      return seq;
    },
    async run(text, limit = 50) {
      const mine = ++seq;
      const results = await query({ seq: mine, query: text, limit });
      if (results.discarded || results.seq !== seq) return null;
      return results;
    },
  };
}
