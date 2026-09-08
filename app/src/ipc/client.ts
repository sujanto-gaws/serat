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
 * Evidence for `mark_first_paint` — task #108, closing F-377/F-388
 * (`crates/serat-app`'s guard, task #105, is what actually rejects implausible
 * evidence; this type only has to match its parameter names).
 *
 * **Field names are camelCase, not this file's usual snake_case.** Every other wire
 * shape here is a nested struct field (`WriteFileArgs.base_hash` and friends), which
 * ts-rs preserves verbatim from Rust and Tauri deserialises with no renaming. These
 * are **top-level command parameters**, and Tauri v2 renames a command's own
 * parameters to camelCase by default before matching them against the JS call —
 * confirmed against task #105's actual Rust signature
 * (`crates/serat-app/src/commands.rs`, branch `task/105-paint-mark-evidence-guard`),
 * which takes `first_contentful_paint_ms: f64` and is only reachable by sending
 * `firstContentfulPaintMs`. Sending the snake_case form here silently fails to match
 * the parameter — a bug this file shipped for one revision before catching it
 * against the real branch.
 */
export interface PaintEvidence {
  /**
   * `PerformancePaintTiming`'s `first-contentful-paint` entry, if the webview
   * recorded one — the browser's own record of when it painted actual content
   * (text, image, non-white canvas/SVG), which this app's `#app` div cannot produce
   * before Vue mounts. This is the argument task #105's guard checks: a call with
   * nothing real to send has nothing to offer in its place (`PaintEvidenceRejected`
   * on the Rust side). `null` only if the entry genuinely never landed — task #105's
   * current signature takes a plain `f64`, not an `Option`, so sending `null` should
   * be rare in practice (by the time the double `requestAnimationFrame` below
   * resolves, two frames have already elapsed and FCP will ordinarily have fired)
   * and is flagged to tauri-app-dev/architect as a case worth a decision rather than
   * assumed here.
   */
  firstContentfulPaintMs: number | null;
  /**
   * The **first** `requestAnimationFrame` callback's own `DOMHighResTimeStamp`
   * argument. Not part of task #105's landed guard (that branch dropped the frame
   * gate entirely in favour of the paint entry alone — see `src/startup.ts`'s module
   * note for the open question this raises against F-388's "conjunction, not
   * replacement" ratification). Sent anyway: harmless if unused, and immediately
   * available if the guard grows a frame-gap check back in.
   */
  raf1Ms: number;
  /** The second callback's timestamp — taken after the frame reached the compositor. */
  raf2Ms: number;
  /**
   * `PerformanceNavigationTiming.domContentLoadedEventEnd`, if the webview recorded
   * one. Fires on HTML parse completion, before any script (including Vue's mount)
   * has run — it is not paint evidence and plays no part in any guard. Kept because
   * it is the inner mark ADR-0014's open objection to vue-frontend-dev asked for,
   * splitting `S_shell` into WebView2's navigation cost and this bundle's mount
   * cost. `null` when the entry is absent. Diagnostic only.
   */
  navDomContentLoadedMs: number | null;
}

/**
 * The paint mark (NFR-PRF-01). Call this from inside a double
 * `requestAnimationFrame` and nowhere else — see `src/startup.ts`. Returns
 * milliseconds since t0, which `serat-app` computed and is not re-derived here.
 *
 * Can reject: `crates/serat-app`'s guard (task #105) refuses evidence that could not
 * have come from a real composited frame.
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
