// ADR-0017: vault bytes reach the webview through one read-only custom scheme.
// Tauri presents a custom scheme differently per platform: macOS/Linux use
// `serat-asset://localhost/<path>`, Windows (WebView2) uses
// `http://serat-asset.localhost/<path>`. This is the only place that knows
// both forms; `sanitize.ts` accepts exactly the origins listed here, and the
// widgets build URLs only through `assetUrl`.
//
// The path argument must be a RESOLVED vault-relative path (from
// `resolve_links`), never a name as written. Containment is the scheme
// handler's job (ADR-0017 steps 3-4); this helper only encodes each segment.

export type AssetPlatform = "windows" | "other";

export const SERAT_ASSET_ORIGINS = ["serat-asset:", "http://serat-asset.localhost"] as const;

/** True for a URL on either presentation of the scheme. The http form must be
 * exactly that origin (`http://serat-asset.localhost.evil.test/` is not it). */
export function isSeratAssetUrl(value: string): boolean {
  if (value.startsWith("serat-asset:")) return true;
  const origin = "http://serat-asset.localhost";
  return value.startsWith(origin) && (value.length === origin.length || "/?#".includes(value[origin.length]));
}

export function detectAssetPlatform(): AssetPlatform {
  const platform = typeof navigator === "undefined" ? "" : navigator.platform ?? "";
  return /Win/.test(platform) ? "windows" : "other";
}

const SEPARATORS = /[\\/]/;

/**
 * The static part of serat-vault's ignore rules, mirrored so the front end does not even ask
 * (the handler stays the authority; `.seratignore` and configured extra dirs are unknown here):
 * any PARENT directory starting with `.` (`.serat`, `.git`, `.obsidian`, `.trash`, `.stfolder`,
 * `.stversions`, ...) and `node_modules`, case-folded. A backslash counts as a separator because
 * the native side turns `\` into `/` after its single decode.
 */
export function isIgnoredAssetPath(vaultRelativePath: string): boolean {
  const parents = vaultRelativePath.split(SEPARATORS).slice(0, -1);
  return parents.some((segment) => segment.startsWith(".") || segment.toLowerCase() === "node_modules");
}

/**
 * `.`, `..` and empty segments (an absolute path starts with an empty one) are refused rather
 * than encoded: `encodeURIComponent` leaves dots alone and a URL parser collapses `a/../b`
 * before the handler sees it. A backslash is a separator for this check (native `VaultPath::new`
 * normalises `\` to `/` after one decode), so `a\..\b.png` is refused too.
 */
export function assetUrl(vaultRelativePath: string, platform: AssetPlatform = detectAssetPlatform()): string {
  const segments = vaultRelativePath.split("/");
  if (vaultRelativePath.split(SEPARATORS).some((segment) => segment === "" || segment === "." || segment === "..")) {
    throw new Error(`not a vault-relative asset path: ${JSON.stringify(vaultRelativePath)}`);
  }
  if (isIgnoredAssetPath(vaultRelativePath)) {
    throw new Error(`not a servable asset path: ${JSON.stringify(vaultRelativePath)}`);
  }
  const encoded = segments.map((segment) => encodeURIComponent(segment)).join("/");
  return platform === "windows"
    ? `http://serat-asset.localhost/${encoded}`
    : `serat-asset://localhost/${encoded}`;
}
