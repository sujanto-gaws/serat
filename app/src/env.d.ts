/// <reference types="vite/client" />

declare module "*.vue" {
  import type { DefineComponent } from "vue";
  const component: DefineComponent<Record<string, never>, Record<string, never>, unknown>;
  export default component;
}

/**
 * What `serat-app::run`'s `initialization_script` puts on `window` before any page
 * script runs (crates/serat-app/src/lib.rs's `BootInfo`). Read once, at startup —
 * see src/startup.ts.
 */
interface SeratBoot {
  /** Canonical vault root, if one was opened at start. */
  vault: string | null;
  /** Vault-relative note to render, if any. */
  open: string | null;
  /** Whether this process is a benchmark run. */
  exit_after_ready: boolean;
}

declare global {
  interface Window {
    __SERAT_BOOT__?: SeratBoot;
    /** NFR-PRF-02's benchmark harness seam — see `App.vue`'s `onMounted`
     * for why this exists and what it does. Not present outside a page
     * that mounted `App.vue`, and only ever defined in a `--mode bench` build
     * (`npm run build:bench`); a release build has no such global. */
    __seratBenchOpenNote?: (text: string) => void;
  }
}

export {};
