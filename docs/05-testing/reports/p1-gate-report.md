---
project: Serat
artifact: P1 gate report — GO / NO-GO (task #28, second half; drafted under #163)
phase: 05-testing
owner: test-engineer
serves: NFR-CMP-01, NFR-CMP-02, NFR-CMP-04, NFR-USE-01, NFR-MNT-01, NFR-PRF-01, NFR-PRF-02, NFR-PRF-03, NFR-PRF-06, NFR-PRF-08, NFR-SEC-01, FR-VLT-08, FR-VLT-11, FR-EDT-02, FR-EDT-10, FR-BIN-03, FR-BIN-11, FR-BIN-12, FR-NAV-06, US-01, US-29, US-35, US-41, RISK-013
cites: ADR-0011, ADR-0015, ADR-0016, ADR-0017, F-384, RISK-013, RISK-015
---

# P1 gate report — an editor you could live in

**Status: DRAFT, status half only. The week-in-vault half is blank on purpose and the verdict is pending the owner's week.**
**Date drafted:** 2026-11-23 · **Run by:** test-engineer · **Template:** `docs/05-testing/reports/TEMPLATE-test-report.md` · **Modelled on:** `docs/05-testing/reports/t2-p0-gate-report.md` · **Protocol applied:** `docs/05-testing/p1-gate-protocol.md` ("What the gate report will contain", the Demo/Inspect table, the reach rules) and `docs/05-testing/test-plan.md` §Reports.

**Rule for this document:** no invented evidence. Every cell below cites a path or a commit, or says **not witnessed**. Where a figure comes only from a status document and no report or raw output is committed, the cell says so. The automated suites were **not re-run for this draft**; "passing" below means what the lead's per-merge gate recorded (`docs/01-planning/status-p1-editor.md`: `cargo test --workspace`, clippy, fmt, doclint, ts-rs drift, `npm test`, `vue-tsc` before every merge), not something this author observed today.

## Read this first

1. **Nothing in P1 was CI-asserted.** Jobs are created on every push but no hosted runner is ever assigned (F-384, RISK-013; correction of 2026-11-23, checked against GitHub Actions directly; see CI jobs below). Every budget job and every gating check named below was therefore run **locally** by the lead's gate or not at all. NFR-PRF-08 (budgets enforced in CI) is **not satisfied**, as it was not at P0. RISK-013's wording requires this report to say which budgets were CI-asserted: **none**.
2. **One P1 budget is attested; the rest are not.** NFR-PRF-03 is attested (17.01 ms p95, `attested 0`). Every other figure was taken on a shared 4-CPU container under load, or at P0 on a different machine, and is reported as such.
3. **No figure here comes from the reference machine** (`docs/05-testing/reference-machine.md`, RM-1, Windows 11 / WebView2). NFR-PRF-02 is gate-eligible only there (PRD v1.6, architecture §9.3). That run has not been done.
4. **The Demo and Inspect IDs (12 of the 33) cannot be witnessed by this document at all.** Only the owner's week witnesses them (protocol §Demo/Inspect). They are listed as pending, with automated proxies named, and are not counted as witnessed.

## Scope and integration head

| | |
|---|---|
| Gate (charter §Milestones) | The owner opens their own vault in Serat for a week without needing Obsidian |
| Integration branch | `origin/claude/great-mccarthy-jywddn` at **`e82eb01`** ("Merge task/p1-62-attested-save-tail: NFR-PRF-03 hub-note save attested at 17.01 ms p95 (#62)"); `origin/main` is the same commit at fetch time (`git rev-parse origin/main`) |
| Requirements in scope | **33** PRD IDs with Phase = P1 (`docs/02-requirements/prd.md`, requirement tables; matches the PRD header's "P1 33"). `docs/01-planning/tasks-p1-editor.md` still says 31 in its prose; its own list had 32 (`test-plan.md` §P1 coverage) and FR-VLT-11 / NFR-SEC-07 arrived later (PRD v1.7, v1.10) |
| Stories | US-01..09, US-18, US-19, US-29, US-34, US-35, plus US-41 (FR-VLT-11) and US-42 (NFR-SEC-07) |
| Index schema version on the head | 12 (`docs/01-planning/status-p1-editor.md`) |
| Benchmark vault | canonical 20k: seed 20260903, `tree_digest_sha256` `35fcb867759df4b8737af8914f798d81d77396dfe051f38140139d3779bd393e`, `VAULT_FORMAT_VERSION` 1 (`bench/vaultgen/src/lib.rs`); the digest is quoted from `docs/05-testing/p1-save-tail-2026-11.md`. The manifest file `bench/vaults/vault-20k.manifest.json` is generated, not committed, so it is not cited as evidence here |
| Measurement environment (P1 figures) | this container: 4 cores, Linux, shared with other agents' builds except for the NFR-PRF-03 attested run (`p1-save-tail-2026-11.md`). Real-time scanner and power plan: not applicable to this environment; not recorded for any P1 figure |
| Reference machine | **no P1 run.** RM-1 (`docs/05-testing/reference-machine.md`) was used for the P0 numbers only |
| Toolchain | Rust 1.92 pinned; Chromium 153 for the latency harness; pdfjs-dist 6.5.438 (`status-p1-editor.md`) |

## Task tally (`docs/01-planning/tasks-p1-editor.md`, refreshed against `git log` at the head)

| | Count | Source |
|---|---:|---|
| Rows in the P1 task table | 97 | `tasks-p1-editor.md` |
| Merged | 94 | same, status column |
| Partly merged | 1 | #28 (protocol merged; this report is its second half, drafted under #163) |
| Pending | 2 | #147 (quiet-machine NFR-PRF-02 re-run, needs CI runners) and #149 (P5 release: third-party notices) |
| Merged, not in the table | 1 | #62 (attested hub-save run), merge `e82eb01` |
| Unmerged branches on origin | 0 | `status-p1-editor.md` §Open branches |

Test cases: **103** `TC-nnn` rows in `docs/05-testing/test-plan.md` (TC-001..TC-148 with gaps; IDs are permanent and never reused). Test-plan §P1 coverage (task #27) maps every in-scope requirement to named tests; its summary was 29 COVERED, 3 PARTIAL (FR-BIN-11, FR-BIN-12, NFR-CMP-01), 0 MISSING of 32, and it predates NFR-SEC-07 (TC-129, TC-130, TC-148 cover it) and the P1 additions after #27.

## Requirement-by-requirement witness

Columns: **Verify** is the PRD method. **Witness** names the test case, bench report, CI job or merged doc. **State** is one of: *tested* (automated test present and recorded green by the merge gate), *reported, unattested* (a budget job exists and a loaded or P0 figure is recorded), *not witnessed*, or *pending the week* (Demo/Inspect; the protocol's evidence is the owner's week, section blank below). Paths are relative to the repository root. Test names are in `docs/05-testing/test-plan.md` §P1 coverage; only files are repeated here.

| ID | Verify | Witness | State |
|---|---|---|---|
| FR-VLT-01 | Bench | `crates/serat-index/tests/tree_rows.rs`, `crates/serat-vault/tests/tree_session.rs`, `crates/serat-core/tests/tree_stream.rs`, `app/src/stores/vaultId.test.ts`; budget job `p1-tree-path` (`bench/budget/src/bin/p1-tree-path.rs`, `.github/workflows/budgets.yml`). The 2 s tree budget is judged by the job; **no figure from it is recorded in any document I read** | functional tests: tested. Budget: **not witnessed** (no sourced figure; job never run in CI) |
| FR-VLT-03 | Test | `crates/serat-vault/src/patch_props.rs`, `crates/serat-vault/src/write_race_tests.rs` | tested |
| FR-VLT-04 | Bench | `crates/serat-index/tests/external_change.rs`, `crates/serat-app/src/consumer.rs`; budget `p1-external-change`. Single note p95 190 ms, max 454 ms of 500 ms (loaded); batch of 100 p95 723 ms, reported (`status-p1-editor.md` §Budget numbers; no standalone report) | reported, unattested |
| FR-VLT-05 | Test | `app/src/editor/reconcile.test.ts` | tested |
| FR-VLT-07 | Test | `crates/serat-vault/src/ignore_file_tests.rs`, `crates/serat-app/src/asset/tests.rs` | tested |
| FR-VLT-08 | Demo | proxy: `crates/serat-index/tests/assets_sidecars.rs` | pending the week |
| FR-VLT-11 | Test | `app/src/AppTreeOps.test.ts`, `app/src/tree/treeOps.test.ts`, `crates/serat-app/src/folder.rs` | tested |
| FR-EDT-01 | Test | `app/src/editor/preview.test.ts`, `app/src/editor/mdExtensions.test.ts` | tested |
| FR-EDT-02 | Demo | proxy: `app/src/editor/reveal.test.ts`, `app/src/editor/typography.test.ts` (TC-144..146), `app/src/styles/fonts.test.ts` (TC-147, TC-148). The protocol requires a watch on every one of the 7 days | pending the week |
| FR-EDT-03 | Test | `app/src/editor/links.test.ts`, `app/src/editor/previewLinks.test.ts` | tested |
| FR-EDT-10 | Demo | proxy: `app/src/editor/CodeMirrorEditor.test.ts` ("split preview fallback") | pending the week |
| FR-NAV-01 | Bench | `crates/serat-index/tests/switcher_corpus.rs`, `app/src/overlays/overlays.test.ts`; budget `p1-switcher` (see NFR-PRF-06) | reported, unattested |
| FR-NAV-02 | Test | `crates/serat-index/tests/read_paths.rs`, `crates/serat-app/src/reads.rs`, `app/src/panes/panes.test.ts` | tested |
| FR-NAV-04 | Test | `crates/serat-index/tests/read_paths.rs`, `app/src/panes/panes.test.ts` | tested |
| FR-NAV-06 | Inspect | proxy: `app/src/editor/commands.test.ts`, `app/src/overlays/overlays.test.ts` | pending the week |
| FR-STR-03 | Test | `crates/serat-index/tests/read_paths.rs`, `crates/serat-parse/tests/compat_frontmatter.rs` | tested |
| FR-BIN-01 | Test | `crates/serat-index/tests/assets_sidecars.rs` | tested |
| FR-BIN-02 | Test | `crates/serat-index/tests/assets_sidecars.rs`, `crates/serat-app/src/asset/tests.rs` | tested |
| FR-BIN-03 | Demo | proxy: `app/src/tree/tree.test.ts`, `crates/serat-index/tests/tree_rows.rs` | pending the week |
| FR-BIN-11 | Demo | proxy: `app/src/viewers/pdfTransport.test.ts`, `crates/serat-app/src/asset/tests.rs`. Page navigation and text selection in a real webview are not exercised in CI (MISSING-E2E-1, `test-plan.md`) | pending the week |
| FR-BIN-12 | Demo | proxy: `app/src/viewers/viewers.test.ts`. Zoom in a real webview not exercised (MISSING-E2E-1) | pending the week |
| NFR-PRF-02 | Bench | harness only: `bench/editor-latency/tests/` and `docs/05-testing/reports/nfr-prf-02-editor-latency-scenarios.md`; no budget evidence (see budget table) | **not witnessed** |
| NFR-PRF-06 | Bench | budget `p1-switcher`; `app/src/overlays/overlays.test.ts` | reported, unattested |
| NFR-SEC-01 | Test | `crates/serat-app/tests/no_network.rs`, `app/src/security/noNetwork.test.ts`, `docs/05-testing/no-network-evidence.md` (static). The runtime capture (protocol row, once in the week) has not been taken | tested (static); runtime capture pending the week |
| NFR-SEC-06 | Test | `crates/serat-app/src/asset/tests.rs`, `crates/serat-vault/src/write_race_tests.rs`; Windows twins `crates/serat-app/tests/windows_vault.rs` are written by reading and **have not run** (no runner) | tested on Linux; Windows not witnessed |
| NFR-SEC-07 | Test | TC-129 (`cargo deny`, `deny.toml`; 2026-11-12 on `5be5605`, exit 0), TC-130 (`bench/npm-deny/`), TC-148; `deny` and `npm-deny` jobs in `.github/workflows/ci.yml` (in `gate.needs`, never run on a runner) | tested locally |
| NFR-REL-01 | Test | `crates/serat-vault/src/patch_props.rs`, `crates/serat-vault/src/write_race_tests.rs`, `app/src/editor/reconcile.test.ts`, `app/src/editor/closing.test.ts` | tested |
| NFR-CMP-01 | Demo | proxy: `crates/serat-vault/tests/obsidian_untouched.rs`, `crates/serat-parse/tests/compat_frontmatter.rs`, `crates/serat-index/tests/compat_fold.rs`, TC-135, TC-143. The criterion is a session on a real Obsidian vault | pending the week |
| NFR-CMP-02 | Inspect | proxy: `crates/serat-app/tests/app_writes_plain.rs`, `crates/serat-vault/tests/patch_corpus.rs` | pending the week |
| NFR-CMP-03 | Test | `crates/serat-index/tests/open_or_rebuild.rs`, `crates/serat-index/tests/rebuild_equivalence.rs` | tested |
| NFR-CMP-04 | Demo | proxy: `crates/serat-app/src/gitignore.rs`, `crates/serat-vault/src/gitignore.rs` | pending the week |
| NFR-USE-01 | Inspect | proxy: `app/src/editor/commands.test.ts`, `app/src/overlays/overlays.test.ts`, `app/src/tree/contextMenuPlatforms.test.ts` | pending the week |
| NFR-MNT-01 | Inspect | `ipc-types` job in `.github/workflows/ci.yml` (regeneration diff; ran locally at #27, 57 files, no difference; **has not run on a runner**); not re-run for this draft | pending the week (two dated local logs required by the protocol) |

**Tally (33 P1 IDs).**

| State | Count | IDs |
|---|---:|---|
| Tested (automated, recorded green by the merge gate, not re-run here) | 16 | FR-VLT-03, FR-VLT-05, FR-VLT-07, FR-VLT-11, FR-EDT-01, FR-EDT-03, FR-NAV-02, FR-NAV-04, FR-STR-03, FR-BIN-01, FR-BIN-02, NFR-SEC-01 (static half), NFR-SEC-06 (Linux), NFR-SEC-07, NFR-REL-01, NFR-CMP-03 |
| Reported, unattested (bench job exists, loaded figure recorded) | 3 | FR-VLT-04, FR-NAV-01, NFR-PRF-06 |
| **Not witnessed** | 2 | FR-VLT-01 (budget: no sourced figure), NFR-PRF-02 (budget evidence) |
| Pending the owner's week (Demo 8: FR-VLT-08, FR-EDT-02, FR-EDT-10, FR-BIN-03, FR-BIN-11, FR-BIN-12, NFR-CMP-01, NFR-CMP-04; Inspect 4: FR-NAV-06, NFR-CMP-02, NFR-USE-01, NFR-MNT-01) | 12 | see the Demo/Inspect section |
| Witnessed in the sense of the gate (Test and Bench with attested evidence) | 0 of 5 Bench | no Bench ID has attested P1 evidence; NFR-PRF-03 is attested but is a P0-phase ID |

Caveats on the 16: the front-end tests are jsdom/vitest, not a real webview; FR-EDT-01 and FR-NAV-02 cover the P1 note-body scope only (FR-NAV-08 is P2); Windows and macOS paths were verified by reading only (`status-p1-editor.md` §Open risks).

## Budgets (NFR-PRF-01..09)

"Attested" means the run printed `attested 0` (no concurrent build processes for the whole run, `bench/budget/src/bin/environment.rs`) or equivalent evidence of a quiet machine. "gate-eligible" is the stricter charter condition (scanner state, reference machine).

| Budget | P1 figure | State | Source |
|---|---|---|---|
| **NFR-PRF-01** cold start, Serat term 350 ms p95 | P1 head: not re-run. P0 (2026-09-08, Windows/WebView2): Serat term 201.1 ms median / 233.8 ms p95; platform term 1116.7 / 1687.1 ms; aggregate `window_visible` p95 1575.6 ms (FAIL vs 400), `vault_usable` p95 1577.2 ms (INDETERMINATE vs 1000). The tree's first-screen term was added to the trace in #141; no reading with it is recorded | **unattested at P1** (P0 figure only; `gate_eligible` false at P0) | `docs/05-testing/reports/t2-p0-gate-report.md`, `docs/05-testing/reports/nfr-prf-01-cold-start-post-codemirror.md`; job `prf01-serat-term` |
| **NFR-PRF-02** editor latency, >=95% of keystrokes on time | Chromium 153 CI dry run: start and middle PASS, end-of-note INDETERMINATE (97.8 / 97.6%); positive-strict control exactly at the 99% bar. 2026-11-12 scenario run: the positive control read 98.3% (below its 99% bar), so every scenario figure is informational: table cell 93.6% strict, split preview 93.2% strict (p95 2 frames), the other scenarios 96.2 to 99.4% | **unattested; OPEN.** Not gateable headless; reference-machine run not done | `status-p1-editor.md` §Budget numbers; `docs/05-testing/reports/nfr-prf-02-editor-latency-scenarios.md`; task #147 pending |
| **NFR-PRF-03** save to index, 50 ms p95 | **17.01 ms p95** (p50 11.54, max 148.86), 1000 hub-note saves, in-degree 679, `attested 0` over 1013 s, 0 checkpoints on the save path, head `6655230`, 2026-11-23, vault digest as above, 4-core Linux container. P0 figure on Windows: 4.306 ms, `gate_eligible` false | **ATTESTED, PASS** (run 3). Not on the reference machine | `docs/05-testing/p1-save-tail-2026-11.md`; `docs/05-testing/reports/nfr-prf-03-save-to-index.md` (P0). The earlier 23.3 ms figure is unsourced and superseded |
| **NFR-PRF-04** full rebuild 20k, 30 s | 7.9 to 10.8 s on this container; 14.8 s median on the rusqlite 0.41 build under heavy load, indicative only, quiet A/B owed (C-06). P0 Windows: 27,693.5 ms median of 3, gate-quality PASS, `gate_eligible` false | **unattested at P1**; no standalone P1 report, the figures are in the status document only | `status-p1-editor.md`; `t2-p0-gate-report.md` |
| NFR-PRF-05 query latency | P2 scope | **not run** | PRD Phase column |
| **NFR-PRF-06** switcher keystroke, 50 ms | 20k default 12 to 13 ms, heading mode 16 to 18 ms (loaded); 50k 46 to 48 ms (reported, not gated, PRD v1.9). The 20 ms allocation of §9.2 is a printed finding | **unattested** (loaded; no attested run, no report file) | `status-p1-editor.md`; job `p1-switcher` |
| NFR-PRF-07 memory ceiling | P4 scope | **not run** | PRD Phase column |
| **NFR-PRF-08** budgets enforced in CI | Jobs exist in `.github/workflows/budgets.yml` (`prf01-cold-start`, `prf01-serat-term`, `prf03-save-to-index`, `prf04-full-rebuild`, `p1-read-paths`, `p1-tree-path`, `p1-external-change`, `p1-switcher`, `every-budget-has-a-job`, the report-only `editor-latency` and `editor-latency-real-ipc`). **None has executed since 2026-09-08 at the latest** (see CI jobs) | **not satisfied** (F-384, RISK-013). Needs the repository owner; nothing under `bench/` or `docs/` can fix it | `t2-p0-gate-report.md` §NFR-PRF-08; `docs/01-planning/risk-register.md` |
| NFR-PRF-09 extraction budget | P2 scope; TC-035 planned | **not run** | `docs/05-testing/test-plan.md` |

NFR-PRF-10 (scale ceiling, P5) is outside the 01..09 range and is not run.

**Budget tally (NFR-PRF-01..09):** attested 1 (NFR-PRF-03) · unattested 4 (NFR-PRF-01, 02, 04, 06) · not run 3 (NFR-PRF-05, 07, 09; out of P1 scope) · not satisfied 1 (NFR-PRF-08). CI-asserted: **0**.

P1-specific budgets that are not NFR-PRF IDs: FR-VLT-04 (500 ms at p95 over 100 single-note changes, PRD v1.9): 190 ms loaded, unattested. FR-VLT-01 (complete tree within 2 s p95 from first paint, PRD v1.6, ADR-0015): job exists, no figure sourced, **not witnessed**. Others reported without a budget: catch-up on an unchanged index 16 to 22 ms (sync-tool worst case 294 ms); attachments read p95 2.3 ms of a 20 ms expectation; backlinks and tags p50 8.0 / 8.7 ms of 20 ms (`status-p1-editor.md`).

### Re-baseline hand-off (owed to product-analyst and architect)

The P1 numbers that change a baseline or a design input, stated explicitly because they re-baseline on them:

1. NFR-PRF-03: 17.01 ms p95 attested, hub-note save, with the WAL checkpointer off the save path (ADR-0011 spacing); the tail is the FTS5 merge and its COMMIT, not a checkpoint. Supersedes the unsourced 23.3 ms.
2. NFR-PRF-06: 50k switcher reads 46 to 48 ms against a 50 ms binding at 20k; the 50k case is reported, not gated.
3. NFR-PRF-02: no gate-quality figure exists; the P1 gate cannot cite one until #147 and the reference-machine run.
4. NFR-PRF-04: no attested P1 figure; the 14.8 s loaded reading is not a baseline.

This draft does not message them; the lead relays (`status-p1-editor.md` §Process notes).

## CI jobs

| Workflow | Jobs relevant to P1 | Executed on a runner? |
|---|---|---|
| `.github/workflows/budgets.yml` | the nine budget jobs and two report-only editor-latency jobs listed under NFR-PRF-08 | **No** |
| `.github/workflows/ci.yml` | `fmt`, `deny`, `clippy`, `workspace`, `windows-compile`, `npm-deny`, `ipc-types`, `frontend-bundle`, `members`, `crate`, `location-independence`, `gate` | **No since 2026-09-08 at the latest**; it did run for real on 2026-09-06 |
| `.github/workflows/design-docs.yml`, `serat-app-tests.yml` | doclint checks and serat-app tests | **No** (same pattern) |

**What GitHub shows (checked directly by the lead, 2026-11-23; test-engineer did not query the API).** The repository is private and has 1,595 workflow runs. `ci.yml` ran for real on 2026-09-06: runs 72 and 80 succeeded (733 s and 840 s) and runs 82 to 85 ran 9 to 13 minutes and failed on content. From 2026-09-08 at the latest, every run of `ci`, `budgets`, `design-docs` and `serat-app-linux` completes with conclusion failure within 2 to 7 s; every job shows `runner_id` 0 and no log (the API returns 404 for job logs). That includes the latest run on `main` at `e82eb01`: ci run 428, 12 jobs failed in 2 to 3 s, 1 skipped (https://github.com/sujanto-gaws/serat/actions/runs/45197269660). So jobs are created and no hosted runner is ever assigned. All jobs use plain `ubuntu-latest` or `windows-latest`, so this is the billing / spending-limit / payment pattern for private repositories, not a workflow or runner-label defect. This replaces the earlier wording "CI produces no jobs" and "has never run"; the verdict does not change.

**Owner remedy:** check the account's Actions billing (minutes, spending limit, payment method) in GitHub Settings, Billing, then re-run `ci.yml` on `main`. Until then the lead's local gate is the only executed evidence.

Everything the jobs would assert has been run locally by the lead's per-merge gate; that is the substitute, and it is not CI.

## Known gaps

1. **#147 pending, needs CI runners.** Quiet-machine NFR-PRF-02 scenarios, triage of table-cell and split-preview, and of the bridge `save_patch` figure.
2. **Reference-machine run not done.** NFR-PRF-02 must be tested on WebView2 with and without scroll margins (`status-p1-editor.md` §Open risks; `docs/05-testing/reference-machine.md`).
3. **No CI job has executed since 2026-09-08 at the latest (F-384, RISK-013).** Jobs are created but never get a runner; see CI jobs for the facts and the owner remedy. NFR-PRF-08 unmet; Windows runner tests (`crates/serat-app/tests/windows_vault.rs`, 17 runner-only assumptions in `test-plan.md`) have never run on a runner; the `ipc-types` and `frontend-bundle` jobs have not run on a runner.
4. **Table-cell and split-preview typing are the weakest NFR-PRF-02 scenarios under load** (strict 93.6% and 93.2%, p95 2 frames). Recorded without a task of their own; #147 re-measures both first.
5. **Bridge-side `save_patch` p95 723 ms under load** (p50 455 ms, max 976 ms, n=40, load average 10 to 30 on 4 CPUs). Off the keystroke path; #147 re-measures it. Contrast the attested 17.01 ms for the in-process hub-note save: they are different harnesses and are not comparable.
6. **MISSING-E2E-1.** No real-webview test of PDF page navigation, text selection or image zoom (FR-BIN-11/12). Falls to the manual week.
7. **MISSING-WIN-1.** UNC, drive-relative, junction and NTFS name tests are written by reading and awaiting a runner. No Windows or macOS build exists in this environment.
8. **NFR-SEC-01 runtime capture** (once in the week, on the reference machine) has not been taken; CI evidence is static.
9. **Residual risks carried** (`status-p1-editor.md` §Open risks): the index lock is guaranteed only on a local filesystem (A-02); RISK-015 (a cloud-synced vault can hydrate silently, P1 gate vault is local only by owner decision); the committed webview bundle must be rebuilt with each `app/` change (`frontend-bundle` job); #149 third-party notices (P5).
10. The automated suites were not re-run for this draft (see the rule above); a re-run at the final head is part of closing the report.

No new failures were found while drafting, so no failure tasks were opened. Every gap above is either an existing task (#147, #149) or an infrastructure dependency on the owner.

## Owner rulings in force

| # | Ruling | Effect on the gate |
|---|---|---|
| 1 | Enter P1 with P0's gate not formally closed; F-384 and #123 carried as risks (2026-09-24) | NFR-PRF-08 stays unmet; no P1 figure is CI-asserted |
| 2 | Flat table view stays P2 (FR-VIW-01); FR-NAV-02 split, extracted content becomes FR-NAV-08 (P2) | a reach for the table view or PDF search is a post-P1 reach |
| 3 | Reaching for Obsidian for a post-P1 capability is logged and counted, not gate-failing | see reach rules |
| 4 | FR-VLT-01 re-defined: complete tree within 2 s p95 from first paint (PRD v1.6, ADR-0015) | the budget is judged by `p1-tree-path` |
| 5 | NFR-PRF-02 re-defined: >=95% of keystrokes painted on time under rule E0, per engine version, gate-eligible only on the reference machine (PRD v1.6, §9.3) | cannot be closed headless |
| 6 | Lead default (told, not objected to): a reach mapping to no PRD ID counts as missing P1 unless product-analyst records an ID or exclusion before the week | recorded in the protocol (#140) |
| 7 | Finished branches push to `main` as well as the integration branch (2026-10-21) | none |
| 8 | Open or switch the vault from inside Serat (#151, 2026-11-13) | in scope for the week |

## Reach rules applied to the week (from `docs/05-testing/p1-gate-protocol.md`)

- A reach for Obsidian, or any other tool, against the real vault or any copy of it (Obsidian mobile counts) is logged with what was attempted, why Serat did not serve it, and one of four dispositions: **missing P1 feature**, **post-P1 capability correctly out of P1 scope**, **defect**, **defect (trust)**.
- The disposition is read off the PRD's Phase column for the ID the reach maps to. P1 ID: missing P1 feature. P2, P3, P4, v2 or one of the four Won't exclusions: post-P1. No PRD ID at all: missing P1 feature by default, unless product-analyst records an ID or exclusion before the week starts.
- **NO-GO** on any reach logged as missing P1 feature, defect or defect (trust), including a safety-net check that finds nothing. Creating a note or folder through another tool is a missing P1 feature (FR-VLT-11).
- Post-P1 reaches do not fail the gate but every one is counted per capability (Dataview-style queries, table view, PDF search, block refs, graph view, lenses, any other).
- A gate-stopping breach restarts the 7-day count; the report shows the full history of attempts.
- A day counts only if daily items 2 to 4 (write, link plus backlinks, switch or search plus palette) were done.
- A week with zero or thin log entries is stated as such, not read as an uneventful week.

---

# Week-in-vault half (blank until the owner's week)

Everything below is intentionally empty. Nothing here may be filled from the synthetic benchmark vault, from CI, or from the status half above.

## Daily log (full, per `docs/05-testing/reports/p1-gate-log.md`)

| Day / date | Session start-end | Items 2 to 6 done | Reaches | Crashes, hangs, data-loss scares, wrong renders (severity) | Counts toward the 7? |
|---|---|---|---|---|---|
| | | | | | |

Attempt history (every restart after a NO-GO event, with its cause): blank.

## NFR-CMP-01 day-7 checklist (Obsidian reopens with no configuration loss)

| Check | Result |
|---|---|
| Day-0 baseline recorded (plugins, hotkeys, layout; `.obsidian/` tar, size, checksum including symlinks; git status; `git ls-files .obsidian`; `.trash/` listing and marker) | blank |
| 1. `.obsidian/` checksum at day 7 byte-identical to day 0 (or explained by an unbroken before/after chain) | blank |
| 2. Vault opens in Obsidian with no "needs repair" or re-index prompt | blank |
| 3. Plugins, hotkeys and layout from day-0 step 1 still enabled with the same settings; workspace restores | blank |
| 4. `.trash/` not created or written by Serat (`find .trash -newer <marker>` empty) | blank |
| 5. Pass/fail per item recorded here, not a single line | blank |

## NFR-CMP-04 (`.serat/` never enters version control silently)

| Step | Result |
|---|---|
| 1. `.gitignore` hash and `git check-ignore .serat` before first open (real vault, or throwaway clone if `.serat/` is already ignored or already on disk; or "not a git repository: not exercised") | blank |
| 2. Open in Serat; offer declined | blank |
| 3. `.gitignore` hash unchanged; `git status` clean with respect to `.gitignore` | blank |

## Demo / Inspect table with evidence

| ID | Verify | Evidence (log entries, count of days, screenshots) | Result |
|---|---|---|---|
| FR-VLT-08 | Demo | | blank |
| FR-EDT-02 | Demo (all 7 days) | | blank |
| FR-EDT-10 | Demo | | blank |
| FR-BIN-03 | Demo | | blank |
| FR-BIN-11 | Demo | | blank |
| FR-BIN-12 | Demo | | blank |
| NFR-CMP-01 | Demo | checklist above | blank |
| NFR-CMP-04 | Demo | section above | blank |
| FR-NAV-06 | Inspect | actions used vs palette listing | blank |
| NFR-CMP-02 | Inspect | every file with a changed mtime, opened in a plain text editor | blank |
| NFR-USE-01 | Inspect | actions performed by keyboard alone | blank |
| NFR-SEC-01 | Inspect (week observation) | connection capture with the process filter and the excluded-traffic list | blank |
| NFR-MNT-01 | Inspect | two dated local generated-types checks (before the week, day 7), marked "not CI-asserted" | blank |

## Obsidian-reach tally

| # | Date | Attempted | Why Serat did not serve it | Candidate PRD ID(s) and phase | Disposition | Defect task |
|---|---|---|---|---|---|---|
| | | | | | | |

Post-P1 reach counts per capability: Dataview-style queries blank · table view blank · PDF search blank · block refs blank · graph view blank · lenses blank · other blank.

## Verdict

**Pending the owner's week.** No GO or NO-GO is stated or implied by the status half above. The status half's findings that bear on the verdict are the gaps listed under Known gaps; none of them is a reach, and none can be closed by the week.

## Traceability

| ID | How this report serves it |
|---|---|
| NFR-PRF-01, 02, 03, 04, 06, 08 | The budget table: attested, unattested or not satisfied, each with its source. |
| NFR-PRF-05, 07, 09 | Stated as not run, with the phase that owns them. |
| Every P1 FR/NFR | The requirement-by-requirement witness table and tally. |
| NFR-CMP-01, NFR-CMP-04, NFR-CMP-02, NFR-USE-01, NFR-MNT-01, FR-NAV-06, FR-VLT-08, FR-EDT-02, FR-EDT-10, FR-BIN-03, FR-BIN-11, FR-BIN-12, NFR-SEC-01 | Blank week-in-vault sections with the protocol's evidence columns. |
| US-01, US-29, US-35, US-41 | Through the IDs above; US-01 is not complete until the NFR-CMP-01 week is filled. |
| RISK-013 | States that no budget or check was CI-asserted. |
