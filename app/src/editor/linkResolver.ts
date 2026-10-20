// Batched, cached link resolution (architecture.md §9.3: "one batched command
// per viewport change. Results are cached for the session and invalidated by
// `index.changed`"). Nothing here runs on the keystroke path: callers (the
// links plugin) schedule `resolve` after the user pauses, and a target already
// answered costs no IPC at all.

import { linkKey } from "@/editor/linkState";
import type { LinkIpcAdapter } from "@/ipc/linkAdapter";
import type { LinkResolution } from "@/ipc/generated/LinkResolution";

export class LinkResolver {
  private cache = new Map<string, LinkResolution>();
  private inflight = new Map<string, Promise<LinkResolution | undefined>>();
  private generation = 0;

  constructor(private readonly ipc: LinkIpcAdapter) {}

  private cacheKey(source: string, target: string): string {
    return `${source}\0${linkKey(target)}`;
  }

  /** Cached answer, if any — synchronous, for the render path. */
  peek(source: string, target: string): LinkResolution | undefined {
    return this.cache.get(this.cacheKey(source, target));
  }

  /** True when every target already has an answer (no IPC would be made). */
  hasAll(source: string, targets: readonly string[]): boolean {
    return targets.every((t) => this.cache.has(this.cacheKey(source, t)));
  }

  /**
   * Resolves `targets` written in `source`: cached ones immediately, the rest
   * in ONE `resolve_links` call. An IPC failure leaves those targets
   * unanswered (they are asked again at the next scan) — it never throws, so
   * a transport hiccup cannot disturb typing.
   */
  async resolve(source: string, targets: readonly string[]): Promise<LinkResolution[]> {
    const wanted = new Map<string, string>(); // key -> first spelling
    for (const target of targets) {
      const key = linkKey(target);
      if (!wanted.has(key)) wanted.set(key, target);
    }
    const missing: Array<[string, string]> = [];
    const pending: Array<Promise<LinkResolution | undefined>> = [];
    const results: LinkResolution[] = [];
    for (const [key, spelling] of wanted) {
      const hit = this.cache.get(`${source}\0${key}`);
      if (hit) {
        results.push(hit);
        continue;
      }
      const running = this.inflight.get(`${source}\0${key}`);
      if (running) pending.push(running);
      else missing.push([key, spelling]);
    }

    if (missing.length > 0) {
      const generation = this.generation;
      const request = this.ipc
        .resolveLinks({ source_path: source, targets: missing.map(([, spelling]) => spelling) })
        .then(
          (answers) => new Map(answers.map((a) => [linkKey(a.target), a] as const)),
          () => new Map<string, LinkResolution>(),
        );
      for (const [key] of missing) {
        const slot = `${source}\0${key}`;
        const one = request.then((answers) => {
          const answer = answers.get(key);
          // An answer from before an invalidation is returned but not cached.
          if (answer && generation === this.generation) this.cache.set(slot, answer);
          this.inflight.delete(slot);
          return answer;
        });
        this.inflight.set(slot, one);
        pending.push(one);
      }
    }

    for (const answer of await Promise.all(pending)) if (answer) results.push(answer);
    return results;
  }

  /** `index://changed`: drop every answer; in-flight ones become uncacheable. */
  invalidate(): void {
    this.cache.clear();
    this.inflight.clear();
    this.generation += 1;
  }
}
