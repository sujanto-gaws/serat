// Creating a note or a folder from the tree (FR-VLT-11, US-41; ADR-0016 for notes, `create_folder`
// for folders). Pure helpers only; the IPC calls live in App.vue. Name RULES (invalid characters,
// reserved names, escapes) belong to the core (#34/#35): this module only picks a unique default and
// turns the core's refusal into a sentence. Rename, move and delete are P3 (FR-VLT-09) and have no
// command in contracts/ipc.rs, so nothing here touches them.

import type { TreeModel } from "@/tree/treeModel";

export type CreateKind = "note" | "folder";

export interface CreateRequest {
  kind: CreateKind;
  /** Vault-relative parent directory; `""` is the vault root. */
  dir: string;
  /** What the user typed: no `.md` suffix needed for a note (it is added). */
  name: string;
}

/** The names already listed in `dir`, lower-cased for a case-insensitive comparison: macOS and Windows
 * file systems treat `Note.md` and `note.md` as one file (contracts/ipc.rs, `create_folder`). Empty when the
 * folder is not listed yet. */
export function takenNames(model: TreeModel, dir: string): Set<string> {
  return new Set((model.listing(dir) ?? []).map((entry) => entry.name.toLowerCase()));
}

/** `base`, else `base 2`, `base 3`, ... the first that is free (case-insensitively). */
export function uniqueName(taken: ReadonlySet<string>, base: string, ext = ""): string {
  let candidate = `${base}${ext}`;
  for (let n = 2; taken.has(candidate.toLowerCase()); n += 1) candidate = `${base} ${n}${ext}`;
  return candidate;
}

/** The file name a typed note name becomes: trimmed, `.md` added unless already there. `null` when empty. */
export function noteFileName(typed: string): string | null {
  const name = typed.trim();
  if (name === "" || name.toLowerCase() === ".md") return null;
  return /\.md$/i.test(name) ? name : `${name}.md`;
}

export function folderName(typed: string): string | null {
  const name = typed.trim();
  return name === "" ? null : name;
}

function kindOf(error: unknown): string | null {
  return typeof error === "object" && error !== null ? ((error as { kind?: string }).kind ?? null) : null;
}

/** The core's refusal as one sentence the user can act on; nothing is swallowed. */
export function describeCreateError(error: unknown, kind: CreateKind, name: string): string {
  const what = kind === "note" ? "note" : "folder";
  const e = error as { path?: string; message?: string };
  switch (kindOf(error)) {
    case "stale_hash": // write_file with `base_hash: null` on a path that exists
      return `A ${what} named “${name}” already exists here.`;
    case "not_found":
      return `The folder ${e.path ? `“${e.path}” ` : ""}does not exist.`;
    case "path_escapes_vault":
      return `Serat refused that name: it is not a valid ${what} name here.`;
    case "not_writable":
      return `Serat does not write there${e.path ? ` (${e.path})` : ""}.`;
    case "vault_not_found":
      return "No vault is open.";
    case "io":
      return `Could not create the ${what}: ${e.message ?? "the file system refused"}`;
    default:
      return `Could not create the ${what}: ${typeof error === "string" ? error : JSON.stringify(error)}`;
  }
}
