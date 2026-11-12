// Self-test for npm-deny.mjs (NFR-SEC-07, US-42): each check is shown to fail on a planted
// breach and to pass on its absence. Run: node --test bench/npm-deny/npm-deny.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import {
  auditBreaches,
  licenseBreaches,
  licenseOk,
  sourceBreaches,
  unmatchedExceptions,
  validateExceptions,
} from "./npm-deny.mjs";

const advisory = (id, severity, name = "pkg") => ({
  name,
  severity,
  title: "t",
  url: `https://github.com/advisories/${id}`,
  source: 1,
});
const report = (...vias) => ({ vulnerabilities: { pkg: { via: vias } } });
const A = "GHSA-ffff-ffff-ffff";
const B = "GHSA-cccc-cccc-cccc";
const good = {
  id: A,
  package: "pkg",
  reason: "dev-only test runner; fixed by a major bump",
  owner: "someone",
  recorded: "2026-11-12",
};

test("audit: an advisory at or above the floor fails; below it passes", () => {
  assert.equal(auditBreaches(report(advisory(A, "high")), "high").length, 1);
  assert.equal(auditBreaches(report(advisory(A, "critical")), "high").length, 1);
  assert.equal(auditBreaches(report(advisory(A, "moderate")), "high").length, 0);
  assert.equal(auditBreaches({ vulnerabilities: {} }, "high").length, 0);
});

test("audit: an exception passes that advisory in that package and nothing else", () => {
  const ex = [good];
  assert.equal(auditBreaches(report(advisory(A, "critical")), "high", ex).length, 0);
  assert.equal(auditBreaches(report(advisory(B, "critical")), "high", ex).length, 1);
  // The same id in another package is not the advisory that was reviewed.
  assert.equal(auditBreaches(report(advisory(A, "critical", "other")), "high", ex).length, 1);
});

test("audit: a transitive pointer is not an advisory; an npm error is not a pass; an unknown severity fails", () => {
  assert.equal(auditBreaches(report("tinypool"), "high").length, 0);
  assert.throws(() => auditBreaches({ error: { code: "ENOAUDIT" } }, "high"));
  assert.throws(() => auditBreaches({}, "severe"));
  assert.equal(auditBreaches(report(advisory(A, "catastrophic")), "high").length, 1);
});

test("audit: an exception that matches no advisory is reported as stale", () => {
  assert.equal(unmatchedExceptions(report(advisory(A, "critical")), [good]).length, 0);
  assert.equal(unmatchedExceptions(report(advisory(B, "critical")), [good]).length, 1);
  assert.equal(unmatchedExceptions(report(advisory(A, "critical", "other")), [good]).length, 1);
  assert.equal(unmatchedExceptions({ vulnerabilities: {} }, [good]).length, 1);
});

test("exceptions: a well-formed entry passes; each missing or blank field is a problem", () => {
  assert.deepEqual(validateExceptions([good]), []);
  assert.deepEqual(validateExceptions([]), []);
  for (const field of ["id", "package", "reason", "owner", "recorded"]) {
    const missing = { ...good };
    delete missing[field];
    assert.equal(validateExceptions([missing]).length, 1, `missing ${field}`);
    assert.equal(validateExceptions([{ ...good, [field]: "   " }]).length, 1, `blank ${field}`);
  }
  assert.equal(validateExceptions([{ ...good, reason: "too short" }]).length, 1);
  assert.equal(validateExceptions([{ ...good, id: "GHSA-not-real" }]).length, 1);
  assert.equal(validateExceptions([{ ...good, id: "GHSA-cccc-cccc-ccco" }]).length, 1); // 'o' is not in the alphabet
  assert.equal(validateExceptions([{ ...good, recorded: "12 Nov 2026" }]).length, 1);
  assert.equal(validateExceptions([{ ...good, recorded: "2026-13-45" }]).length, 1);
  assert.equal(validateExceptions([{ ...good, recorded: "2026-02-30" }]).length, 1); // rolls to March
  assert.equal(validateExceptions([{ ...good, recorded: "2025-02-29" }]).length, 1); // not a leap year
  assert.deepEqual(validateExceptions([{ ...good, recorded: "2028-02-29" }]), []); // a leap day is real
  assert.equal(validateExceptions([good, good]).length, 1); // listed twice
});

test("licenses: OR passes on any allowed arm, AND needs every arm, unknown fails", () => {
  const allow = ["MIT", "ISC", "Apache-2.0", "Apache-2.0 WITH LLVM-exception"];
  assert.ok(licenseOk("MIT", allow));
  assert.ok(licenseOk("GPL-3.0-only OR MIT", allow));
  assert.ok(!licenseOk("GPL-3.0-only", allow));
  assert.ok(!licenseOk("MIT AND GPL-3.0-only", allow));
  assert.ok(licenseOk("(MIT AND ISC)", allow));
  assert.ok(licenseOk("Apache-2.0 WITH LLVM-exception", allow));
  assert.ok(!licenseOk("MIT WITH Classpath-exception-2.0", allow));
});

test("licenses: a parenthesised OR does not launder an AND with a disallowed licence", () => {
  const allow = ["MIT", "Apache-2.0"];
  assert.ok(!licenseOk("(MIT OR Apache-2.0) AND GPL-3.0-only", allow));
  assert.ok(!licenseOk("GPL-3.0-only AND (MIT OR Apache-2.0)", allow));
  assert.ok(licenseOk("(MIT OR GPL-3.0-only) AND Apache-2.0", allow));
  assert.ok(licenseOk("MIT AND (GPL-3.0-only OR Apache-2.0)", allow));
  assert.ok(!licenseOk("MIT AND (GPL-3.0-only OR AGPL-3.0-only)", allow));
  // AND binds tighter than OR.
  assert.ok(licenseOk("GPL-3.0-only AND MIT OR Apache-2.0", allow));
  assert.ok(!licenseOk("GPL-3.0-only AND (MIT OR Apache-2.0) OR AGPL-3.0-only", allow));
});

test("licenses: an expression that does not parse is not allowed", () => {
  const allow = ["MIT"];
  for (const bad of ["", "MIT AND", "OR MIT", "(MIT", "MIT)", "MIT MIT", "MIT WITH", "()", "MIT AND OR MIT"])
    assert.ok(!licenseOk(bad, allow), bad);
});

test("licenses: a disallowed or missing licence in a lockfile is reported", () => {
  const lock = {
    packages: {
      "": {},
      "node_modules/a": { license: "MIT" },
      "node_modules/b": { license: "AGPL-3.0-only" },
      "node_modules/x/node_modules/c": {},
    },
  };
  assert.deepEqual(licenseBreaches(lock, ["MIT"]), ["b: AGPL-3.0-only", "c: no license field"]);
});

test("sources: git, file, link, http, another host and a missing resolved are reported", () => {
  const reg = "https://registry.npmjs.org/";
  const lock = {
    packages: {
      "": {},
      "node_modules/ok": { resolved: `${reg}ok/-/ok-1.0.0.tgz` },
      "node_modules/g": { resolved: "git+ssh://git@github.com/x/g.git#abc" },
      "node_modules/f": { resolved: "file:../f" },
      "node_modules/l": { link: true },
      "node_modules/h": { resolved: "http://registry.npmjs.org/h/-/h-1.tgz" },
      "node_modules/o": { resolved: "https://evil.example/o.tgz" },
      "node_modules/n": {},
    },
  };
  const bad = sourceBreaches(lock, reg).map((s) => s.split(":")[0]).sort();
  assert.deepEqual(bad, ["f", "g", "h", "l", "n", "o"]);
  assert.deepEqual(sourceBreaches({ packages: { "node_modules/ok": { resolved: `${reg}ok.tgz` } } }, reg), []);
});
