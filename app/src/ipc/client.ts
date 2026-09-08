// The typed IPC boundary. Every function here is a thin wrapper over
// `@tauri-apps/api`'s `invoke`/`listen`, typed against the generated bindings in
// `./generated` (ADR-0007). Nothing here is hand-written wire shape — only the
// command name strings, which must match `crates/serat-app/src/commands.rs`
// (`#[tauri::command]` uses the function name) and `contracts/ipc.rs`
// (`INDEX_STATUS_EVENT`) exactly. If either drifts, the round trip fails at
// runtime, not at compile time — there is no generated command list yet, only
// generated argument/return types.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { FileContents } from "./generated/FileContents";
import type { FileWritten } from "./generated/FileWritten";
import type { IndexStatus } from "./generated/IndexStatus";
import type { IpcError } from "./generated/IpcError";
import type { OpenVaultArgs } from "./generated/OpenVaultArgs";
import type { ReadFileArgs } from "./generated/ReadFileArgs";
import type { VaultOpened } from "./generated/VaultOpened";
import type { WriteFileArgs } from "./generated/WriteFileArgs";

export type { IpcError };

/** `contracts/ipc.rs::INDEX_STATUS_EVENT`. See that file for why it is not "/". */
export const INDEX_STATUS_EVENT = "index://status";

export function openVault(args: OpenVaultArgs): Promise<VaultOpened> {
  return invoke("open_vault", { args });
}

export function readFile(args: ReadFileArgs): Promise<FileContents> {
  return invoke("read_file", { args });
}

export function writeFile(args: WriteFileArgs): Promise<FileWritten> {
  return invoke("write_file", { args });
}

export function indexStatus(): Promise<IndexStatus> {
  return invoke("index_status");
}

/**
 * Evidence that `mark_first_paint` was really taken from inside a double
 * `requestAnimationFrame`, not merely asserted by calling it — task #108, closing
 * F-377 (`crates/serat-app`'s guard, task #105, is what actually rejects implausible
 * evidence; this type only has to match its parameter names).
 *
 * Field names are snake_case, matching this file's one existing convention for wire
 * shapes (e.g. `WriteFileArgs.base_hash`) even though `mark_first_paint` sits outside
 * `contracts/ipc.rs` and this type is hand-written rather than ts-rs'd — the two
 * instrumentation commands are documented as "not in the contract"
 * (`commands.rs`), not as exempt from its wire convention.
 */
export interface PaintEvidence {
  /**
   * The **first** `requestAnimationFrame` callback's own `DOMHighResTimeStamp`
   * argument. Deliberately not a second `performance.now()` call inside the
   * callback: the value is the one the browser's scheduler produced for that
   * callback, not one this file computed after the fact.
   */
  raf1_ms: number;
  /** The second callback's timestamp — taken after the frame reached the compositor. */
  raf2_ms: number;
  /**
   * `PerformanceNavigationTiming.domContentLoadedEventEnd`, if the webview recorded
   * one. Unlike the two fields above, this is the platform's own instrumentation: a
   * rewrite of this file cannot move it, which is exactly why it is worth reporting
   * — it is the anchor ADR-0014's open objection to vue-frontend-dev asked for, and
   * splits `S_shell` into WebView2's navigation cost and this bundle's mount cost.
   * `null` when the entry is absent, which some engines or navigation types can
   * legitimately produce. Diagnostic only: the guard does not gate on it.
   */
  nav_dom_content_loaded_ms: number | null;
}

/**
 * The paint mark (NFR-PRF-01). Call this from inside a double
 * `requestAnimationFrame` and nowhere else — see `src/startup.ts`. Returns
 * milliseconds since t0, which `serat-app` computed and is not re-derived here.
 *
 * Can reject: `crates/serat-app`'s guard (task #105) is expected to refuse evidence
 * that could not have come from two real, distinct composited frames.
 */
export function markFirstPaint(evidence: PaintEvidence): Promise<number> {
  return invoke("mark_first_paint", { ...evidence });
}

/**
 * The vault-usable mark (NFR-PRF-01). `note` is what was rendered, or `null` if
 * nothing was — recorded by the Rust side so a fast number over an empty shell is
 * legible as one. See `src/startup.ts` for the ordering this must respect.
 */
export function markVaultUsable(note: string | null): Promise<number> {
  return invoke("mark_vault_usable", { note });
}

/**
 * Subscribes to `index://status`. A rebuild is a status line, not a modal and not
 * an error (architecture §3.4) — the caller decides how to render `IndexStatus`,
 * this only delivers it.
 */
export function onIndexStatus(handler: (status: IndexStatus) => void): Promise<UnlistenFn> {
  return listen<IndexStatus>(INDEX_STATUS_EVENT, (event) => handler(event.payload));
}

/** True when `err` is the shape `IpcError` promises — Tauri rejects with `unknown`. */
export function isIpcError(err: unknown): err is IpcError {
  return typeof err === "object" && err !== null && "kind" in err;
}
