// Self-test for npm-deny.mjs (NFR-SEC-07, US-42): each check is shown to fail on a planted
// breach and to pass on its absence. Run: node --test bench/npm-deny/
import test from "node:test";
import assert from "node:assert/strict";
import { auditBreaches, licenseOk, licenseBreaches, sourceBreaches } from "./npm-deny.mjs";

const advisory = (id, severity, name = "pkg") => ({
  name,
  severity,
  title: "t",
  url: `https://github.com/advisories/${id}`,
  source: 1,
});
const report = (...vias) => ({ vulnerabilities: { pkg: { via: vias } } });

test("audit: an advisory at or above the floor fails; below it passes", () => {
  assert.equal(auditBreaches(report(advisory("GHSA-a", "high")), "high").length, 1);
  assert.equal(auditBreaches(report(advisory("GHSA-a", "critical")), "high").length, 1);
  assert.equal(auditBreaches(report(advisory("GHSA-a", "moderate")), "high").length, 0);
  assert.equal(auditBreaches({ vulnerabilities: {} }, "high").length, 0);
});

test("audit: a recorded exception passes that advisory and no other", () => {
  const ex = [{ id: "GHSA-a" }];
  assert.equal(auditBreaches(report(advisory("GHSA-a", "critical")), "high", ex).length, 0);
  assert.equal(auditBreaches(report(advisory("GHSA-b", "critical")), "high", ex).length, 1);
});

test("audit: a transitive pointer (a string) is not an advisory; an npm error is not a pass", () => {
  assert.equal(auditBreaches(report("tinypool"), "high").length, 0);
  assert.throws(() => auditBreaches({ error: { code: "ENOAUDIT" } }, "high"));
  assert.throws(() => auditBreaches({}, "severe"));
});

test("licenses: OR passes on any allowed arm, AND needs every arm, unknown fails", () => {
  const allow = ["MIT", "ISC"];
  assert.ok(licenseOk("MIT", allow));
  assert.ok(licenseOk("GPL-3.0-only OR MIT", allow));
  assert.ok(!licenseOk("GPL-3.0-only", allow));
  assert.ok(!licenseOk("MIT AND GPL-3.0-only", allow));
  assert.ok(licenseOk("(MIT AND ISC)", allow));
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
