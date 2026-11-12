// npm dependency policy for app/ (NFR-SEC-07, US-42, NFR-PRF-08): the npm counterpart of
// deny.toml. Three checks, policy in /npm-deny.json:
//   audit     npm audit, judged per advisory against a severity floor, with recorded exceptions
//   licenses  every package in package-lock.json against the SPDX allow-list
//   sources   every package resolves from the one registry; no git, file, link or other host
// Usage: node bench/npm-deny/npm-deny.mjs <audit|licenses|sources|all>
// The decision functions are pure and exported; npm-deny.test.mjs plants a failure in each.

import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const RANK = { info: 0, low: 1, moderate: 2, high: 3, critical: 4 };

/** Advisories in an `npm audit --json` report that breach `level` and are not excepted. */
export function auditBreaches(report, level, exceptions = []) {
  if (!(level in RANK)) throw new Error(`unknown severity '${level}'`);
  if (report.error) throw new Error(`npm audit failed: ${JSON.stringify(report.error)}`);
  const excepted = new Set(exceptions.map((e) => e.id));
  const found = new Map();
  for (const vuln of Object.values(report.vulnerabilities ?? {})) {
    for (const via of vuln.via ?? []) {
      if (typeof via !== "object") continue; // a pointer to another vulnerable package
      const id = String(via.url ?? via.source).split("/").pop();
      if (excepted.has(id) || !(via.severity in RANK)) continue;
      if (RANK[via.severity] >= RANK[level]) found.set(id, `${via.name} ${via.severity} ${id}: ${via.title}`);
    }
  }
  return [...found.values()];
}

/** Does an SPDX expression pass the allow-list? `OR` any arm, `AND` every arm. */
export function licenseOk(expression, allow) {
  const set = new Set(allow);
  const text = String(expression).trim().replace(/^\(|\)$/g, "");
  return text
    .split(/\s+OR\s+/)
    .some((arm) =>
      arm
        .trim()
        .replace(/^\(|\)$/g, "")
        .split(/\s+AND\s+/)
        .every((id) => set.has(id.trim())),
    );
}

const nameOf = (path) => path.replace(/^.*node_modules\//, "");

/** Packages whose licence is absent or not allowed. */
export function licenseBreaches(lock, allow) {
  const out = [];
  for (const [path, pkg] of Object.entries(lock.packages ?? {})) {
    if (path === "") continue;
    if (!pkg.license) out.push(`${nameOf(path)}: no license field`);
    else if (!licenseOk(typeof pkg.license === "string" ? pkg.license : JSON.stringify(pkg.license), allow))
      out.push(`${nameOf(path)}: ${pkg.license}`);
  }
  return out;
}

/** Packages not resolved from the registry (git, file, link, http, another host, none). */
export function sourceBreaches(lock, registry) {
  const out = [];
  for (const [path, pkg] of Object.entries(lock.packages ?? {})) {
    if (path === "") continue;
    if (pkg.link) out.push(`${nameOf(path)}: a link`);
    else if (!pkg.resolved) out.push(`${nameOf(path)}: no resolved field`);
    else if (!pkg.resolved.startsWith(registry)) out.push(`${nameOf(path)}: ${pkg.resolved}`);
  }
  return out;
}

function main() {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
  const policy = JSON.parse(readFileSync(join(root, "npm-deny.json"), "utf8"));
  const lock = JSON.parse(readFileSync(join(root, "app", "package-lock.json"), "utf8"));
  const mode = process.argv[2] ?? "all";
  const audit = (omitDev) => {
    const args = ["audit", "--package-lock-only", "--json", ...(omitDev ? ["--omit=dev"] : [])];
    let text;
    try {
      text = execFileSync("npm", args, { cwd: join(root, "app"), encoding: "utf8" });
    } catch (e) {
      text = e.stdout; // npm audit exits 1 when it finds anything
    }
    return JSON.parse(text);
  };
  const results = [];
  if (mode === "audit" || mode === "all") {
    const { production_level: prod, all_level: all, exceptions } = policy.audit;
    results.push([`audit (production, >= ${prod})`, auditBreaches(audit(true), prod, exceptions)]);
    results.push([`audit (all, >= ${all})`, auditBreaches(audit(false), all, exceptions)]);
  }
  if (mode === "licenses" || mode === "all")
    results.push(["licenses", licenseBreaches(lock, policy.licenses.allow)]);
  if (mode === "sources" || mode === "all")
    results.push(["sources", sourceBreaches(lock, policy.sources.registry)]);
  let failed = false;
  for (const [name, breaches] of results) {
    console.log(`${breaches.length === 0 ? "ok" : "FAILED"}  ${name}`);
    for (const b of breaches) console.log(`      ${b}`);
    failed ||= breaches.length > 0;
  }
  process.exit(failed ? 1 : 0);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
