// npm dependency policy for app/ (NFR-SEC-07, US-42, NFR-PRF-08): the npm counterpart of
// deny.toml. Three checks, policy in /npm-deny.json:
//   audit     npm audit, judged per advisory against a severity floor, with recorded exceptions
//   licenses  every package in package-lock.json against the SPDX allow-list
//   sources   every package resolves from the one registry; no git, file, link or other host
// Usage: node bench/npm-deny/npm-deny.mjs <audit|licenses|sources|all>
// The decision functions are pure and exported; npm-deny.test.mjs plants a failure in each.
// Every check fails closed: an unknown severity, an unparseable licence expression, a blank
// exception or an exception that matches nothing is a failure, not a pass.

import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const RANK = { info: 0, low: 1, moderate: 2, high: 3, critical: 4 };
const GHSA = /^GHSA(-[23456789cfghjmpqrvwx]{4}){3}$/;
const ISO_DATE = /^\d{4}-\d{2}-\d{2}$/;

/** A calendar date that round-trips: `2026-02-30` is rolled to March by Date.parse, so it fails here. */
const isRealDate = (text) => {
  const d = new Date(`${text}T00:00:00Z`);
  return !Number.isNaN(d.getTime()) && d.toISOString().slice(0, 10) === text;
};

/** Problems with the exception list itself; an empty array means it is acceptable. */
export function validateExceptions(exceptions) {
  const problems = [];
  const seen = new Set();
  (exceptions ?? []).forEach((e, i) => {
    const at = `exception ${i} (${e?.id ?? "no id"})`;
    const blank = (field) => typeof e?.[field] !== "string" || e[field].trim() === "";
    if (typeof e?.id !== "string" || !GHSA.test(e.id)) problems.push(`${at}: id is not a GHSA id`);
    else if (seen.has(e.id)) problems.push(`${at}: listed twice`);
    else seen.add(e.id);
    if (blank("package")) problems.push(`${at}: package is empty`);
    if (blank("reason") || e.reason.trim().length < 20)
      problems.push(`${at}: reason is empty or shorter than 20 characters`);
    if (blank("owner")) problems.push(`${at}: owner is empty`);
    if (typeof e?.recorded !== "string" || !ISO_DATE.test(e.recorded) || !isRealDate(e.recorded))
      problems.push(`${at}: recorded is not an ISO date`);
  });
  return problems;
}

/** Every advisory object in an `npm audit --json` report: {id, name, severity, title}. */
export function advisories(report) {
  if (report.error) throw new Error(`npm audit failed: ${JSON.stringify(report.error)}`);
  const found = new Map();
  for (const vuln of Object.values(report.vulnerabilities ?? {})) {
    for (const via of vuln.via ?? []) {
      if (typeof via !== "object") continue; // a pointer to another vulnerable package
      const id = String(via.url ?? via.source).split("/").pop();
      found.set(`${id}\u0000${via.name}`, { id, name: via.name, severity: via.severity, title: via.title });
    }
  }
  return [...found.values()];
}

const excepts = (exceptions, a) => exceptions.some((e) => e.id === a.id && e.package === a.name);

/** Advisories that breach `level` and are not excepted (by id and package). Unknown severity fails. */
export function auditBreaches(report, level, exceptions = []) {
  if (!(level in RANK)) throw new Error(`unknown severity '${level}'`);
  const out = [];
  for (const a of advisories(report)) {
    if (excepts(exceptions, a)) continue;
    if (!(a.severity in RANK)) out.push(`${a.name} unknown severity '${a.severity}' ${a.id}: ${a.title}`);
    else if (RANK[a.severity] >= RANK[level]) out.push(`${a.name} ${a.severity} ${a.id}: ${a.title}`);
  }
  return out;
}

/** Exceptions that match no advisory in the report: stale, to be removed (like cargo-deny's unmatched ignore). */
export function unmatchedExceptions(report, exceptions = []) {
  const present = advisories(report);
  return exceptions
    .filter((e) => !present.some((a) => a.id === e.id && a.name === e.package))
    .map((e) => `exception ${e.id} (${e.package}) matches no advisory; remove it`);
}

/**
 * Does an SPDX expression pass the allow-list? Recursive descent:
 *   expr := term (OR term)*     term := atom (AND atom)*
 *   atom := id [WITH id] | '(' expr ')'
 * `WITH` makes one identifier ("Apache-2.0 WITH LLVM-exception") that must itself be allowed.
 * Anything that does not parse is not allowed.
 */
export function licenseOk(expression, allow) {
  const set = new Set(allow);
  const tokens = String(expression).match(/\(|\)|[^\s()]+/g) ?? [];
  let pos = 0;
  const peek = () => tokens[pos];
  const fail = () => {
    throw new SyntaxError("bad SPDX expression");
  };
  function atom() {
    const t = tokens[pos++];
    if (t === undefined || t === ")" || t === "AND" || t === "OR" || t === "WITH") fail();
    if (t === "(") {
      const v = expr();
      if (tokens[pos++] !== ")") fail();
      return v;
    }
    let id = t;
    if (peek() === "WITH") {
      pos++;
      const exception = tokens[pos++];
      if (exception === undefined || exception === "(" || exception === ")" || /^(AND|OR|WITH)$/.test(exception)) fail();
      id = `${t} WITH ${exception}`;
    }
    return set.has(id);
  }
  function term() {
    let v = atom();
    while (peek() === "AND") {
      pos++;
      const r = atom();
      v = v && r;
    }
    return v;
  }
  function expr() {
    let v = term();
    while (peek() === "OR") {
      pos++;
      const r = term();
      v = v || r;
    }
    return v;
  }
  try {
    const v = expr();
    if (pos !== tokens.length) return false;
    return v;
  } catch {
    return false;
  }
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
    results.push(["audit exceptions are well formed", validateExceptions(exceptions)]);
    if (results[0][1].length === 0) {
      // Every exception is for a dev-only dependency, so the production check honours none.
      results.push([`audit (production, >= ${prod}, no exceptions)`, auditBreaches(audit(true), prod, [])]);
      const everything = audit(false);
      results.push([`audit (all, >= ${all})`, auditBreaches(everything, all, exceptions)]);
      results.push(["audit exceptions all match an advisory", unmatchedExceptions(everything, exceptions)]);
    }
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
