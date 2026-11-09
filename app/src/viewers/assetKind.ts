// Which surface a vault path opens in (FR-BIN-11, FR-BIN-12, US-19). Extension-only and front-end-side:
// the authority on what the scheme will serve is `asset_info.viewable` / ADR-0017 step 5.

/** ADR-0017 §9: an image above this is not served (404), so the viewer does not even ask. */
export const IMAGE_SERVE_CAP = 64 * 1024 * 1024;

export type OpenKind = "pdf" | "image" | "note" | "other";

const IMAGE_EXT = new Set(["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "svg"]);

export function extensionOf(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  return dot < 0 ? "" : name.slice(dot + 1).toLowerCase();
}

export function openKind(path: string): OpenKind {
  const ext = extensionOf(path);
  if (ext === "md") return "note";
  if (ext === "pdf") return "pdf";
  if (IMAGE_EXT.has(ext)) return "image";
  return "other";
}

/** A PDF or image: opens in a viewer, never in the editor. */
export function isViewerPath(path: string): boolean {
  const kind = openKind(path);
  return kind === "pdf" || kind === "image";
}

/** ADR-0005: the sidecar's name is the binary's full file name with `.md` appended, byte-exact. */
export function sidecarPathFor(assetPath: string): string {
  return `${assetPath}.md`;
}
