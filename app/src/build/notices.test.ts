// @vitest-environment node
// A licence file that ships in the webview bundle cannot go unlisted in docs/06-release/THIRD-PARTY-NOTICES.md
// (NFR-SEC-07, P5 release readiness, task #149; ADR-0017 Amendment 1). The webview directory is committed build
// output, so this reads the committed files, not a fresh build.

import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const webview = join(repo, "crates", "serat-app", "webview");
const notices = readFileSync(join(repo, "docs", "06-release", "THIRD-PARTY-NOTICES.md"), "utf8");

function walk(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? walk(join(dir, e.name)) : [join(dir, e.name)],
  );
}

const licenceFiles = walk(webview)
  .filter((p) => /^LICENSE/.test(p.split(sep).pop() as string))
  .map((p) => `crates/serat-app/webview/${relative(webview, p).split(sep).join("/")}`)
  .sort();

describe("third-party notices", () => {
  it("finds the licence files the bundle ships (the scan is not vacuous)", () => {
    expect(licenceFiles).toEqual(
      expect.arrayContaining([
        "crates/serat-app/webview/cmaps/LICENSE",
        "crates/serat-app/webview/standard_fonts/LICENSE_FOXIT",
        "crates/serat-app/webview/standard_fonts/LICENSE_LIBERATION",
        "crates/serat-app/webview/assets/LICENSE_source-serif-4",
        "crates/serat-app/webview/assets/LICENSE_bricolage-grotesque",
        "crates/serat-app/webview/assets/LICENSE_jetbrains-mono",
        "crates/serat-app/webview/assets/LICENSE_pdfjs",
      ]),
    );
  });

  it("names every LICENSE* file shipped under the webview directory", () => {
    const unlisted = licenceFiles.filter((f) => !notices.includes(`\`${f}\``));
    expect(unlisted, `add these to docs/06-release/THIRD-PARTY-NOTICES.md: ${unlisted.join(", ")}`).toEqual([]);
  });

  it("names no webview licence path that is not shipped", () => {
    const named = [...notices.matchAll(/`(crates\/serat-app\/webview\/[^`]*LICENSE[^`]*)`/g)].map((m) => m[1]);
    expect(named.length).toBeGreaterThan(0);
    expect(named.filter((n) => !licenceFiles.includes(n))).toEqual([]);
  });

  it("states the pdfjs-dist version the lockfile pins", () => {
    const lock = JSON.parse(readFileSync(join(repo, "app", "package-lock.json"), "utf8"));
    const version: string = lock.packages["node_modules/pdfjs-dist"].version;
    expect(notices).toContain(`pdfjs-dist ${version}`);
  });
});
