# npm-deny: the npm counterpart of deny.toml

Serves NFR-SEC-07, US-42, NFR-PRF-08. Policy lives in `/npm-deny.json`, beside `deny.toml`; this directory holds the script that applies it and its self-test. CI runs both in the `npm-deny` job, which is in `gate.needs`.

```
node --test bench/npm-deny/npm-deny.test.mjs     # plants a failure in each check
node bench/npm-deny/npm-deny.mjs all             # or: audit | licenses | sources
```

| Check | Policy | Source of truth |
|---|---|---|
| audit | `npm audit --package-lock-only`, judged per advisory (not per package, so a transitive pointer does not inherit a severity). Production dependencies (`--omit=dev`) fail at `audit.production_level` (high); the whole tree fails at `audit.all_level` (critical). An advisory is passed only by an entry in `audit.exceptions`, which names the GHSA id, the package, the reason, an owner and the date. | the registry's advisory endpoint (the only network use) |
| licenses | every package in `app/package-lock.json`, production and dev, must carry a `license` that the SPDX allow-list accepts. `A OR B` passes on any allowed arm, `A AND B` needs both. A missing field fails. | the lockfile |
| sources | every package must have a `resolved` URL beginning `sources.registry`. A git, file, http or other-host URL, a link, or a missing `resolved` fails. | the lockfile |

Fail-closed rules (review round 2): the production audit honours no exceptions; an exception needs a GHSA id, the package, a reason of 20 or more characters, an owner and an ISO date, and is matched by id and package; an exception that matches no advisory in the whole-tree report fails the job, so it is removed when its fix lands; an unknown severity fails; a licence expression is parsed (`AND` binds tighter than `OR`, parentheses and `WITH` are honoured) and one that does not parse is not allowed. Licence rulings (lead, 2026-11-12): PSF-2.0 and BlueOak-1.0.0 are accepted, and OFL-1.1 for the three bundled @fontsource-variable font packages (task #162; licence text in each package's LICENSE); all are recorded in `npm-deny.json`.

Changing the policy is a change to `npm-deny.json`, reviewed like any other; an exception is never added to make a build pass without an owner and a reason. The exceptions are dev-only and expire when the dependency they name is bumped.
