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

1. **CI assertion exists again, partly (updated 2026-11-24).** Hosted runners executed `budgets.yml` on 2026-11-24 for the first time since 2026-11-09: run 415 attempt 2 (`87b4fed`) and run 417 (`9ba7cb5`, the head at this edit) reproduce each other exactly. Green on the head: NFR-PRF-03 (windows-latest, 1.4 ms p95 of 50, the hand-off interval), NFR-PRF-04 (Linux twin, ubuntu-latest, asserted), NFR-PRF-06 (windows-latest), FR-VLT-04 (Linux twin, ubuntu-latest, asserted), FR-VLT-01 (windows-latest). Red: NFR-PRF-01 (both jobs: one launch in each hung 60 s with no first paint, after 1, 2, 6 and 12 earlier launches had painted: intermittent inside a job) and FR-VLT-04 on windows-latest (failed to measure), so `budgets-gate` is red. None of the green rows is a gate number (windows-latest reports `gate eligible false`; ubuntu-latest is a regression gate, ADR-0019 §3). NFR-PRF-08 is **still not satisfied** while the gate is red. Figures not marked as run 415 or 417 were run **locally** or not at all. RISK-013's wording requires this report to say which budgets were CI-asserted: **NFR-PRF-03, NFR-PRF-04 (Linux twin), NFR-PRF-06, FR-VLT-01, FR-VLT-04 (Linux twin), on the head**; FR-VLT-01 and FR-VLT-04 are outside the NFR-PRF-01..09 count. Enforced for the gate under C-06 (ADR-0019 §8): **0**, since no RM-1 scanner-on figure exists. See `ci-windows-first-run-2026-11.md`.
2. **One P1 budget is attested; the rest are not.** NFR-PRF-03 is attested (17.01 ms p95, `attested 0`), supporting evidence (Linux container), not a gate figure. Every other figure was taken on a shared 4-CPU container under load, at P0 on a different machine, or in one CI run on 2026-11-09 at head `6a608b8` that is not re-run on the current head, and is reported as such.
3. **No figure here comes from the reference machine** (`docs/05-testing/reference-machine.md`, RM-1, Windows 11 / WebView2). NFR-PRF-02 is gate-eligible only there (PRD v1.6, architecture §9.3). That run has not been done. Three budgets now also carry a figure from a second machine, RM-2, a Linux desktop (2026-11-24): NFR-PRF-04 PASS, NFR-PRF-03 PASS, NFR-PRF-01 Serat term FAIL, each asserted there and none the gate figure; see the budget table's RM-2 column.
4. **The Demo and Inspect IDs (12 of the 33) cannot be witnessed by this document at all.** Only the owner's week witnesses them (protocol §Demo/Inspect). They are listed as pending, with automated proxies named, and are not counted as witnessed.

## Scope and integration head

| | |
|---|---|
| Gate (charter §Milestones) | The owner opens their own vault in Serat for a week without needing Obsidian |
| Integration branch | `origin/claude/great-mccarthy-jywddn` at **`e82eb01`** ("Merge task/p1-62-attested-save-tail: NFR-PRF-03 hub-note save attested at 17.01 ms p95 (#62)"); `origin/main` is the same commit at fetch time (`git rev-parse origin/main`) |
| Requirements in scope | **33** PRD IDs with Phase = P1 (`docs/02-requirements/prd.md`, requirement tables; matches the PRD header's "P1 33"). `docs/01-planning/tasks-p1-editor.md` still says 31 in its prose; FR-VLT-11 (PRD v1.7) and NFR-SEC-07 (v1.10) were added after it, giving 33; `test-plan.md` §P1 coverage's 32 includes FR-VLT-11 but predates NFR-SEC-07 |
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
| Merged | 95 | same, status column; includes #147 (this branch, merging; the container re-run is done, the reference-machine run is owed) |
| Partly merged | 1 | #28 (protocol merged; this report is its second half, drafted under #163) |
| Pending | 1 | #149 (P5 release: third-party notices) |
| Merged, not in the table | 1 | #62 (attested hub-save run), merge `e82eb01` |
| Unmerged branches on origin | 0 | `status-p1-editor.md` §Open branches |

Test cases: **103** `TC-nnn` rows in `docs/05-testing/test-plan.md` (TC-001..TC-148 with gaps; IDs are permanent and never reused). Test-plan §P1 coverage (task #27) maps every in-scope requirement to named tests; its summary was 29 COVERED, 3 PARTIAL (FR-BIN-11, FR-BIN-12, NFR-CMP-01), 0 MISSING of 32, and it predates NFR-SEC-07 (TC-129, TC-130, TC-148 cover it) and the P1 additions after #27.

## Requirement-by-requirement witness

Columns: **Verify** is the PRD method. **Witness** names the test case, bench report, CI job or merged doc. **State** is one of: *tested* (automated test present and recorded green by the merge gate), *reported, unattested* (a budget job exists and a loaded or P0 figure is recorded), *not witnessed*, or *pending the week* (Demo/Inspect; the protocol's evidence is the owner's week, section blank below). Paths are relative to the repository root. Test names are in `docs/05-testing/test-plan.md` §P1 coverage; only files are repeated here.

| ID | Verify | Witness | State |
|---|---|---|---|
| FR-VLT-01 | Bench | `crates/serat-index/tests/tree_rows.rs`, `crates/serat-vault/tests/tree_session.rs`, `crates/serat-core/tests/tree_stream.rs`, `app/src/stores/vaultId.test.ts`; budget job `p1-tree-path` (`bench/budget/src/bin/p1-tree-path.rs`, `.github/workflows/budgets.yml`). The 2 s tree budget is judged by the job. CI run 316 (budgets, id 42676247715, head `6a608b8`, windows-latest, 2026-11-09): warm p95 47.6 ms, no-index 105.9 ms against 2000 ms, PASS (last run; windows-latest, 2 cores, realtime scanner off, harness prints `gate eligible false`, not gate-eligible (RISK-012); 4 PASS of 4 runs) | functional tests: tested. Budget: **CI PASS on 2026-11-09 (`6a608b8`), 4 of 4 runs, not gate-eligible, not re-run on the current head** |
| FR-VLT-03 | Test | `crates/serat-vault/src/patch_props.rs`, `crates/serat-vault/src/write_race_tests.rs` | tested |
| FR-VLT-04 | Bench | `crates/serat-index/tests/external_change.rs`, `crates/serat-app/src/consumer.rs`; budget `p1-external-change`. Single note p95 190 ms, max 454 ms of 500 ms (loaded); batch of 100 p95 723 ms, reported (`status-p1-editor.md` §Budget numbers; no standalone report). **CI, windows-latest, 2 cores, realtime scanner off, harness prints `gate eligible false`, not gate-eligible (RISK-012): 3 PASS / 1 FAIL over runs 311 (9ce1c6c) 422.1 ms, 312 (c24943a) 417.0 ms, 315 (cf93c32) 376.1 ms (max 1261.2), and 316 (last run, `6a608b8`) FAIL 615.28 ms > 500 ms (max 792.0; batch p95 7017 ms)**; intermittent, task #165; report-only on hosted Windows (#166); asserted on ubuntu-latest by `p1-external-change-linux` (#168), unrun | **CI-asserted on ubuntu-latest only** (`p1-external-change-linux`, #168, green on `9ba7cb5` and `87b4fed`; run 419 Linux-twin job 138509620000 success, id from `gh run view 45412287846 --json jobs`). **Hosted Windows, report-only, not gate-eligible, not CI-asserted:** run 418 (`3e35ce2`, windows-latest, 4 cores, scanner off) single p95 **412.7 ms**, max 617.4 ms, "within" 500 ms; run 419 (`e9b6576`) 396.4 ms, max 834.1 ms; job green. Earlier: 3 of 4 pass at 376-422 ms, run 316 (2 cores) FAIL 615.3 ms. Batch of 100 (ungated) 3.3 s median; cost coincides with WAL checkpoint overlap (task #180, `ci-windows-first-run-2026-11.md`) |
| FR-VLT-05 | Test | `app/src/editor/reconcile.test.ts` | tested |
| FR-VLT-07 | Test | `crates/serat-vault/src/ignore_file_tests.rs`, `crates/serat-app/src/asset/tests.rs` | tested |
| FR-VLT-08 | Demo | proxy: `crates/serat-index/tests/assets_sidecars.rs` | pending the week |
| FR-VLT-11 | Test | `app/src/AppTreeOps.test.ts`, `app/src/tree/treeOps.test.ts`, `crates/serat-app/src/folder.rs` | tested |
| FR-EDT-01 | Test | `app/src/editor/preview.test.ts`, `app/src/editor/mdExtensions.test.ts` | tested |
| FR-EDT-02 | Demo | proxy: `app/src/editor/reveal.test.ts`, `app/src/editor/typography.test.ts` (TC-144), `app/src/styles/fonts.test.ts` (TC-147); TC-145 and TC-146 serve NFR-USE-01 and TC-148 serves NFR-SEC-07. The protocol requires a watch on every one of the 7 days | pending the week |
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
| NFR-PRF-02 | Bench | harness only: `bench/editor-latency/tests/`, `docs/05-testing/reports/nfr-prf-02-editor-latency-scenarios.md` and the quiet re-run `docs/05-testing/reports/nfr-prf-02-quiet-rerun-2026-11.md` (Linux container, supporting evidence); no budget evidence (see budget table) | **not witnessed** |
| NFR-PRF-06 | Bench | budget `p1-switcher`; `app/src/overlays/overlays.test.ts` | reported, unattested |
| NFR-SEC-01 | Test | `crates/serat-app/tests/no_network.rs`, `app/src/security/noNetwork.test.ts`, `docs/05-testing/no-network-evidence.md` (static). The runtime capture (protocol row, once in the week) has not been taken | tested (static); runtime capture pending the week |
| NFR-SEC-06 | Test | `crates/serat-app/src/asset/tests.rs`, `crates/serat-vault/src/write_race_tests.rs`; Windows twins `crates/serat-app/tests/windows_vault.rs` are written by reading and **have not run** (no runner) | tested on Linux; Windows not witnessed |
| NFR-SEC-07 | Test | TC-129 (`cargo deny`, `deny.toml`; 2026-11-12 on `5be5605`, exit 0), TC-130 (`bench/npm-deny/`), TC-148; `deny` and `npm-deny` jobs in `.github/workflows/ci.yml` (in `gate.needs`, not recorded as run on a runner) | tested locally |
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
| Reported, not attested on the current head (bench job exists; a loaded figure or the 2026-11-09 CI result is recorded) | 4 | FR-VLT-01 (CI PASS), FR-VLT-04 (**CI intermittent, last run FAIL**; report-only on hosted Windows (#166); asserted on ubuntu-latest by `p1-external-change-linux` (#168), unrun), FR-NAV-01, NFR-PRF-06 (CI within budget) |
| **Not witnessed** | 1 | NFR-PRF-02 (budget evidence) |
| Pending the owner's week (Demo 8: FR-VLT-08, FR-EDT-02, FR-EDT-10, FR-BIN-03, FR-BIN-11, FR-BIN-12, NFR-CMP-01, NFR-CMP-04; Inspect 4: FR-NAV-06, NFR-CMP-02, NFR-USE-01, NFR-MNT-01) | 12 | see the Demo/Inspect section |
| Non-additive memo (not part of the 33): Bench IDs with attested evidence | 0 of 5 Bench | no Bench ID has attested P1 evidence; NFR-PRF-03 is attested but is a P0-phase ID |

Caveats on the 16: the front-end tests are jsdom/vitest, not a real webview; FR-EDT-01 and FR-NAV-02 cover the P1 note-body scope only (FR-NAV-08 is P2); Windows and macOS paths were verified by reading only (`status-p1-editor.md` §Open risks).

## Budgets (NFR-PRF-01..09)

"Attested" means the run printed `attested 0` (no concurrent build processes for the whole run, `bench/budget/src/bin/environment.rs`) or equivalent evidence of a quiet machine. "gate-eligible" is the stricter charter condition (scanner state, reference machine).

| Budget | P1 figure | State | CI-asserted (runner, head, result) | Gate figure (RM-1, scanner on, gate head) | RM-2 (Linux desktop gaws-dev-05, gate-eligible, 2026-11-24; not the gate figure) | Source |
|---|---|---|---|---|---|---|
| **NFR-PRF-01** cold start, Serat term 350 ms p95 | P1 head: not re-run. P0 (2026-09-08, Windows/WebView2): Serat term 201.1 ms median / 233.8 ms p95; platform term 1116.7 / 1687.1 ms; aggregate `window_visible` p95 1575.6 ms (FAIL vs 400), `vault_usable` p95 1577.2 ms (INDETERMINATE vs 1000). The tree's first-screen term was added to the trace in #141; no reading with it is recorded | **unattested at P1** (P0 figure only; `gate_eligible` false at P0) | **Report-only on hosted windows from task #190 (ADR-0019 point 2, amended 2026-11-25): not CI-asserted.** `prf01-serat-term` measures and fails only when a measurement cannot be taken; the product limit is asserted at the gate on RM-1 (`--profile reference`, scanner on). No Linux twin exists (the 350 ms is Windows/WebView2's, PRD v1.2 F-96). **C-06 is NOT satisfied in CI for NFR-PRF-01 (lead's reading): no CI job asserts its limit, so it counts as enforced only by an RM-1 scanner-on reference run, which is owed. An open gate item, not a pass, until that run exists or a gate-eligible Windows runner class asserts it.** Run 427 (main `83b616d`, the first hang-free run after #186, 20 of 20 launches painted): Serat term p95 **546.3 ms** of 350, n=20, min 237.5, mean 309.2, max 711.6, spread 199.6%, platform term p95 709.7, scanner OFF, 4 cores, INDETERMINATE (now REPORT-ONLY by the harness): a second data point beside RM-2's 1,310.4 ms, not a gate figure and not compared with it. `prf01-cold-start`'s scale-independence check stays asserted and passed. Runs 431 and 432 (task #192): three launches exited 2 about 0.5 s after a frame-gate refusal that the #186 retry had cleared, a race in the benchmark exit (it read the trace one IPC round trip before the buffer mark), fixed by waiting for the buffer mark too; one launch spent 55 to 79 s between setup_done and window_created (inside build() is inferred, no log shows it; diagnostic added, cause open); see `ci-windows-first-run-2026-11.md`. Earlier: `prf01-serat-term` and `prf01-cold-start` ran with a binary for the first time (run 417 (head `9ba7cb5`, 2026-11-24) and run 415 attempt 2 (`87b4fed`)) and both **failed**: one launch in each job hung 60 s with no `window_visible` mark (exit 2, a failure to measure), after 1, 2, 6 and 12 earlier launches in those jobs had painted and exited. Intermittent inside a job, reproduced on two heads; triage in `ci-windows-first-run-2026-11.md`. **Attributed (task #179, 2026-11-25):** every hang (main run 45532921895 on `fe85178`, and 6 in a diagnostic dispatch, run 45580019582) carries `serat: paint evidence rejected ... the second requestAnimationFrame timestamp (N) did not follow the first (N)` and a snapshot with every mark set except `window_visible` and `vault_usable`: a one-shot refusal the page never retries, a product defect under investigation (#186). Per-launch WebView2 data folders and an exit wait did not remove it (serat-term arms: folder only 3 hangs in 28 launches, folder plus browser flags 3 in 7; baseline 6 in 34 including the main run; a small sample), so it is not a shared-profile or exiting-process artefact (RM-1 behaviour is unknown); WebView2 157.0.4341.46. | Owed: no RM-1 scanner-on run. The P0 figure (scanner off) is not a gate number. | **FAIL, asserted** (Linux desktop gaws-dev-05, 4 cores, 2026-11-24, `gate_eligible` true, n=20): Serat term **p95 1,310.4 ms of 350**; platform term 503.1, window-visible 1,556.1, vault-usable 1,796.5, tree-first-screen 1,796.5 ms, reported beside it and not asserted (the two separate legacy jobs, also reported only, read p95 1,603.4 and 1,897.9 ms). A Linux finding, not yet attributed: the 350 ms is bounded to Windows/WebView2 (PRD v1.2, F-96), so this is not a verdict on the Windows budget. Not RM-1, not the gate figure | `docs/05-testing/reports/t2-p0-gate-report.md`, `docs/05-testing/reports/nfr-prf-01-cold-start-post-codemirror.md`; job `prf01-serat-term`; RM-2: `docs/05-testing/reports/reference-run-gaws-dev-05-2026-11.md` |
| **NFR-PRF-02** editor latency, >=95% of keystrokes on time | Chromium 153 "CI dry run" (#60, task #95; architecture.md §9.3 "Readings on Chromium 153"; that section does not say whether it ran on a hosted runner or locally, and the run history in CI jobs leaves it unconfirmed): start and middle PASS, end-of-note INDETERMINATE (97.8 / 97.6%); positive-strict control exactly at the 99% bar. 2026-11-12 scenario run: the positive control read 98.3% (below its 99% bar), so every scenario figure is informational: table cell 93.6% strict, split preview 93.2% strict (p95 2 frames), the other scenarios 96.2 to 99.4%. **#147 quiet re-run, 2026-11-24, supporting evidence only (4-core Linux container, headless Chromium 153.0.8010.12, software rendering; not WebView2, not RM-1, not a gate number):** load 0.1 to 0.92, controls 99.7 to 100.0%; all eight scenarios PASS on E0 at 99.6 to 100.0% on time (strict and swept), p95 1 frame, table cell 100.0% / 99.8%, split preview 100.0% / 100.0%; real-IPC arm 100.0% / 99.8% (dev bridge), 100.0% / 100.0% (optimised bridge) | **unattested; OPEN.** Not gateable headless; reference-machine run not done | None. `editor-latency` is a report-only headless job on ubuntu-latest, not gate eligible by design. | Owed: the WebView2 run on RM-1 (the binding measurement; #147's container figures do not substitute for it). | n/a: not run on RM-2 | `status-p1-editor.md` §Budget numbers; `docs/05-testing/reports/nfr-prf-02-editor-latency-scenarios.md`; `docs/05-testing/reports/nfr-prf-02-quiet-rerun-2026-11.md` (task #147, done on the container) |
| **NFR-PRF-03** save to index, 50 ms p95 | **17.01 ms p95** (p50 11.54, max 148.86), 1000 hub-note saves, in-degree 679, `attested 0` over 1013 s, 0 checkpoints on the save path, head `6655230`, 2026-11-23, vault digest as above, 4-core Linux container. P0 figure on Windows: 4.306 ms, `gate_eligible` false | **ATTESTED, PASS** (run 3). Not on the reference machine | `prf03-save-to-index`, windows-latest (4 cores, scanner OFF, not a gate number), **green** on `9ba7cb5` (run 417): p95 1.4 ms of 50 ms, n=1000, 1000 submitted, 0 refused. The interval is the call into the index from outside (the hand-off), not the apply-to-flush hub-note interval of `p1-save-tail`. Also green on `87b4fed` (run 415, figure in the artifact). | Owed. The 17.01 ms on the Linux container is supporting evidence only, not the gate figure. | **PASS, asserted** (gaws-dev-05, `gate_eligible` true, n=1000): **p95 20.247 ms of 50** (mean 11.7, max 111.5; census 1000 submitted, 0 refused). A different quantity from the attested 17.01 ms hub-note figure (1,000 distinct notes, not one note 1,000 times). Not RM-1, not the gate figure | `docs/05-testing/p1-save-tail-2026-11.md`; `docs/05-testing/reports/nfr-prf-03-save-to-index.md` (P0). The earlier 23.3 ms figure is unsourced and superseded; RM-2: `docs/05-testing/reports/reference-run-gaws-dev-05-2026-11.md` |
| **NFR-PRF-04** full rebuild 20k, 30 s | 7.9 to 10.8 s on the 4-core Linux container (not the reference machine); 14.8 s median on the rusqlite 0.41 build under heavy load, indicative only, quiet A/B owed (C-06). P0 Windows: 27,693.5 ms median of 3, gate-quality PASS, `gate_eligible` false | **unattested**; no standalone P1 report. CI, windows-latest, 2 cores, realtime scanner off, harness says NOT A GATE NUMBER, not gate-eligible (RISK-012): 22 FAIL / 14 PASS on windows-latest; last run 316 (2026-11-09, `6a608b8`) median of 3 **17,812.4 ms vs 30 s, PASS**; worst run 279 at 80123ce median 51,260.5 ms and run 298 at 7b06763 median 55,224.6 ms; run 306 at 6857025 undecided, 28,394 ms, spread 192 %. Task #166 ruled: hosted Windows is never gate eligible, so the job is report-only and the assertion is the reference machine's; none re-run on the current head | `prf04-full-rebuild-linux` asserts 30 s on ubuntu-latest (ADR-0019 §3, #168) and is **green** on `9ba7cb5` (run 417) and `87b4fed` (run 415). `prf04-full-rebuild` on windows-latest is report-only, green, and counts as not CI-asserted. | Owed. RM-1's only figure is the P0 27.7 s, scanner off, which is not a gate number. | **PASS, asserted** (gaws-dev-05, `gate_eligible` true, n=3): median **9,152.9 ms of 30,000** (min 9,105.6, max 12,718.8; first sample 12,718.8). Phases walk 73 / read_hash 474 / parse 2,904 / producer_wait 1,718 / write_fts 1,515 ms. Linux, not RM-1, not the gate figure | `status-p1-editor.md`; `t2-p0-gate-report.md`; RM-2: `docs/05-testing/reports/reference-run-gaws-dev-05-2026-11.md` |
| NFR-PRF-05 query latency | P2 scope | **not run** | n/a (P2 scope) | n/a (P2 scope) | n/a: not run on RM-2 | PRD Phase column |
| **NFR-PRF-06** switcher keystroke, 50 ms | 20k default 12 to 13 ms, heading mode 16 to 18 ms (loaded); 50k 46 to 48 ms (reported, not gated, PRD v1.9). The 20 ms allocation of §9.2 is a printed finding | **unattested** (CI, windows-latest, 2 cores, realtime scanner off, harness prints `gate eligible false`, not gate-eligible (RISK-012), 4 PASS of 4 runs; last run 316 (2026-11-09, `6a608b8`): default p95 8.53 ms, heading 11.48 ms, within 50 ms; not re-run on the current head; local figures loaded; no attested run, no report file) | `p1-switcher` asserts on windows-latest and is **green** on `9ba7cb5` (run 417) and `87b4fed` (run 415). Not a gate number (scanner OFF). | Owed: no RM-1 scanner-on run. | n/a: not run on RM-2 | `status-p1-editor.md`; job `p1-switcher` |
| NFR-PRF-07 memory ceiling | P4 scope | **not run** | n/a (P4 scope) | n/a (P4 scope) | n/a: not run on RM-2 | PRD Phase column |
| **NFR-PRF-08** budgets enforced in CI | `.github/workflows/budgets.yml` has 17 jobs (task #168 added `prf04-full-rebuild-linux` and `p1-external-change-linux`), among them `prf01-cold-start`, `prf01-serat-term`, `prf03-save-to-index`, `prf04-full-rebuild`, `p1-read-paths`, `p1-tree-path`, `p1-external-change`, `p1-switcher`, `every-budget-has-a-job` and the report-only `editor-latency` jobs. About 38 `budgets.yml` runs got hosted runners (runs 279-297 on 2026-10-20/21, 298-316 on 2026-11-09); none was green. Per-job tally under CI jobs | **not satisfied** (F-384, RISK-013). Needs the repository owner; nothing under `bench/` or `docs/` can fix it | n/a: this is the CI requirement itself. `budgets.yml` has 17 jobs; on `9ba7cb5` (run 417) 13 ran green, three are red, `budgets-gate` is red and one NFR-PRF-02 Linux job was unfinished when checked; the three red (`prf01-cold-start`, `prf01-serat-term`, `p1-external-change` on windows-latest), so `budgets-gate` is red. | n/a | n/a: not run on RM-2 | `t2-p0-gate-report.md` §NFR-PRF-08; `docs/01-planning/risk-register.md` |
| NFR-PRF-09 extraction budget | P2 scope; TC-035 planned | **not run** | n/a (P2 scope) | n/a (P2 scope) | n/a: not run on RM-2 | `docs/05-testing/test-plan.md` |
| FR-VLT-04 external change (500 ms p95) | See "P1-specific budgets" below | **unattested**; no gate figure | `p1-external-change-linux` asserts on ubuntu-latest (#168) and is **green** on `9ba7cb5` and `87b4fed`. The windows-latest job is report-only: it **failed to measure** on `9ba7cb5` and `87b4fed` ("REJECTED: only 1 of 100 edits were indexed in 10 s", exit 2: the 100-note batch shape, not the gated single series) and is **green from run 418** with the batch limit at 60 s: single p95 412.7 ms (run 419: 396.4 ms), report-only, not CI-asserted; resolution in `ci-windows-first-run-2026-11.md` (#180) | Owed | Owed: `p1-external-change` was not part of the run | `status-p1-editor.md`; jobs `p1-external-change`, `p1-external-change-linux` |
| FR-VLT-01 file tree complete (2 s p95) | CI PASS 2026-11-09 (`6a608b8`), warm p95 47.6 ms | **unattested at the head** | `p1-tree-path` is an `Always` gated job on windows-latest and is **green** on `9ba7cb5` (run 417) and `87b4fed` (run 415) | Owed | n/a: not run on RM-2 | job `p1-tree-path` |

NFR-PRF-10 (scale ceiling, P5) is outside the 01..09 range and is not run.

**Two columns, never merged (ADR-0019 §8).** "CI-asserted" means a job asserted this budget on the gate head, was green, and the runner is named. A report-only job counts as **not CI-asserted**. "Gate figure" means a determinate, gate-eligible `--profile reference` figure on RM-1 with the scanner on, taken on the gate head. A CI assertion is the regression half of C-06 and cannot substitute for the gate figure; a figure from any other machine is supporting evidence, not the gate figure. NFR-PRF-04 and FR-VLT-04 each have a report-only windows-latest job and an asserting ubuntu-latest job (`prf04-full-rebuild-linux`, `p1-external-change-linux`, task #168); the Linux jobs are regression gates on Serat's own code and are green on `87b4fed` and `9ba7cb5` (runs 415 and 417, 2026-11-24). **P1 counts no budget as enforced under C-06 until the RM-1 scanner-on runs exist, and today that is none of them.** A green report-only job or a Linux figure is not enforcement.

**The RM-2 column (added 2026-11-24, task #177).** The repository owner ran `--profile reference` on a Linux desktop, gaws-dev-05 (4 cores), on 2026-11-24 (`reference-machine.md` §RM-2; raw JSON under `docs/05-testing/reports/reference-runs/gaws-dev-05-2026-11/`; analysis in `docs/05-testing/reports/reference-run-gaws-dev-05-2026-11.md`). The harness rates that machine gate eligible (`Environment::gate_eligible()` is true on Linux), so its figures are asserting figures for a Linux reference, and they are printed as PASS or FAIL. They are not the gate figure: the product ships on Windows, ADR-0019 §8 says a figure from a machine other than RM-1 is supporting evidence, and the "Gate figure (RM-1, scanner on)" column therefore stays **Owed** for every row. A budget is never compared across machines: RM-2's figures are RM-2's. The `environment` bin was not run, so RM-2's power plan, toolchain and load are unrecorded, and `p1-external-change` and `p1-save-tail` were not part of the run.

**Budget tally (NFR-PRF-01..09):** attested 1 (NFR-PRF-03) · unattested 4 (NFR-PRF-01, 02, 04, 06) · not run 3 (NFR-PRF-05, 07, 09; out of P1 scope) · not satisfied 1 (NFR-PRF-08). CI-asserted on the current head `9ba7cb5`: **5 budgets** (NFR-PRF-03, NFR-PRF-04 and FR-VLT-04 as Linux twins, NFR-PRF-06, FR-VLT-01; the two FR-VLT entries are outside the NFR-PRF-01..09 count), with NFR-PRF-01 red and `budgets-gate` red. Enforced for the gate under C-06 (ADR-0019 §8): **0**, since no RM-1 scanner-on figure exists. The tally is by RM-1 evidence and is not changed by the RM-2 column.

P1-specific budgets that are not NFR-PRF IDs: FR-VLT-04 (500 ms at p95 over 100 single-note changes, PRD v1.9): 190 ms loaded; **hosted Windows, report-only, not gate-eligible, run 418: p95 412.7 ms, max 617.4 ms (run 419: 396.4 / 834.1 ms), job green; CI-asserted on the Linux twin only (task #180)**; the 2026-11-09 history was **CI intermittent: 3 of 4 runs pass (376 to 422 ms), run 316 FAIL at 615.3 ms** (windows-latest, 2 cores, realtime scanner off, harness prints `gate eligible false`, not gate-eligible (RISK-012); task #165). FR-VLT-01 (complete tree within 2 s p95 from first paint, PRD v1.6, ADR-0015): CI PASS on 2026-11-09 (`6a608b8`), warm p95 47.6 ms, 4 of 4 runs (windows-latest, 2 cores, realtime scanner off, harness prints `gate eligible false`, not gate-eligible (RISK-012)), not re-run on the current head. Others reported without a budget: catch-up on an unchanged index 16 to 22 ms (sync-tool worst case 294 ms); attachments read p95 2.3 ms of a 20 ms expectation; backlinks and tags p50 8.0 / 8.7 ms of 20 ms (`status-p1-editor.md`).

### Re-baseline hand-off (owed to product-analyst and architect)

The P1 numbers that change a baseline or a design input, stated explicitly because they re-baseline on them:

1. NFR-PRF-03: 17.01 ms p95 attested, hub-note save, with the WAL checkpointer off the save path (ADR-0011 spacing); the tail is the FTS5 merge and its COMMIT, not a checkpoint. Supersedes the unsourced 23.3 ms.
2. NFR-PRF-06: 50k switcher reads 46 to 48 ms against a 50 ms binding at 20k; the 50k case is reported, not gated.
3. NFR-PRF-02: no gate-quality figure exists; the P1 gate cannot cite one until the reference-machine run (the #147 container re-run is supporting evidence, not a gate figure).
4. NFR-PRF-04: no attested P1 figure; the 14.8 s loaded reading is not a baseline; the one CI reading is 17.8 s on windows-latest, **report-only there** (task #166): hosted Windows is never gate eligible, NFR-PRF-04 is asserted on the reference machine only and is not CI-asserted on this head.
5. FR-VLT-04: CI is intermittent (3 of 4 pass at 376 to 422 ms; 615.3 ms in the last run) against the loaded local 190 ms; do not re-baseline on any of them until #165 is resolved. Likewise NFR-PRF-04: the 22 breaches on windows-latest were 20 runs before the #56 merge (`1ec7a9e`, medians 51.3 to 131.8 s, writer blocked the producer 38 to 102 s) and 2 indeterminate after it; 14 PASS (15.9 to under 30 s), 2 INDETERMINATE (299: median 30.2 s; 306: 28.4 s, spread 192 %). Not a regression and not gate-eligible: ruling in nfr-prf-04-gate-quality.md (#166), report-only on hosted Windows.
6. **RM-2 (Linux desktop gaws-dev-05, 2026-11-24, task #177), Linux-only, not Windows and not a baseline for any Windows budget:** NFR-PRF-04 median 9,152.9 ms of 30,000 (n=3); NFR-PRF-03 hub-note p95 20.247 ms of 50 (n=1000); NFR-PRF-01 Serat term p95 1,310.4 ms of 350 (n=20), platform term 503.1 ms. No target is re-baselined by these. The NFR-PRF-01 figure is a Linux finding, not yet attributed, that PRD v1.2 already bounds out of the 350 ms (F-96); the owner decides whether the budget binds Linux in P1, and RISK-016 records it.

This draft does not message them; the lead relays (`status-p1-editor.md` §Process notes).

## CI jobs

| Workflow | Jobs relevant to P1 | Executed on a runner? |
|---|---|---|
| `.github/workflows/budgets.yml` | the budget jobs listed under NFR-PRF-08 (13 jobs in the file) | **Yes, about 38 runs** (279-297 on 2026-10-20/21, 298-316 on 2026-11-09); none green; **no** on the current head |
| `.github/workflows/ci.yml` | `fmt`, `deny`, `clippy`, `workspace`, `windows-compile`, `npm-deny`, `ipc-types`, `frontend-bundle`, `members`, `crate`, `location-independence`, `gate` | **Yes** on 2026-09-06 (runs 72, 80 green) and 2026-10-20/21 (runs 306, 309 green on the integration branch, windows-latest `cargo test` succeeded); **no** on the current head |
| `.github/workflows/design-docs.yml`, `serat-app-tests.yml` | doclint checks and serat-app tests | runner-less pattern as below |

**What GitHub shows (checked by the lead against the GitHub API, 2026-11-23; test-engineer did not query it).** The repository is private and has 1,595 workflow runs. `ci.yml` ran for real on 2026-09-06: runs 72 and 80 succeeded (733 s and 840 s) and runs 82 to 85 ran 9 to 13 minutes and failed on content. Most runs since ci run 87 (2026-09-06, about 16:32Z) got **no runner**, except as below: `runner_id` 0, failure within seconds to under a minute (ci runs 98, 224 and 239 took about 42 s), no job log (the API returns 404). The exceptions are 2026-10-20/21 (ci runs 306 and 309 green on the integration branch with real runner ids; `windows-latest` `cargo test` succeeded) and 2026-11-09 up to about 18:52Z. About 38 `budgets.yml` runs got hosted runners (runs 279-297 on 2026-10-20/21 and 298-316 on 2026-11-09) and executed every job in the file at that head (6 jobs at runs 279-297, more later); none was green. The last, run 316 (id 42676247715, head `6a608b8`, windows-latest), is tabulated first. The latest run on `main` at `e82eb01` (ci run 428, https://github.com/sujanto-gaws/serat/actions/runs/45197269660) again had no runner: 12 jobs failed in 2 to 3 s, 1 skipped. All jobs use plain `ubuntu-latest` or `windows-latest`, which is the billing / spending-limit / payment pattern for private repositories rather than a workflow or runner-label defect. This replaces the earlier wording "CI produces no jobs" and "has never run".

**budgets run 316, the last run (2026-11-09, `6a608b8`, windows-latest, 2 cores, realtime scanner off, harness prints `gate eligible false`, not gate-eligible (RISK-012); not re-run on the current head).**

| Job / budget | Result |
|---|---|
| FR-VLT-01 `p1-tree-path` | PASS: warm p95 47.6 ms, no-index 105.9 ms vs 2000 ms |
| NFR-PRF-04 `prf04-full-rebuild` | REPORT-ONLY from #166 (hosted Windows is not gate eligible); the run-316 figure was a PASS: median of 3 17,812.4 ms vs 30 s |
| NFR-PRF-06 `p1-switcher` | within budget: default p95 8.53 ms, heading 11.48 ms |
| FR-VLT-04 `p1-external-change` | **FAIL** (intermittent, see tally): single-note p95 615.28 ms > 500 ms (max 792.0 ms; batch p95 7017 ms); task #165; report-only on hosted Windows (#166); asserted on ubuntu-latest by `p1-external-change-linux` (#168), unrun |
| NFR-PRF-03 `prf03-save-to-index`, both NFR-PRF-01 jobs | **FAIL without measuring** ("there is no application to start yet ... looked for target/release/serat.exe"): a job defect, not a measurement; task #164 |
| NFR-MNT-02 vault determinism | PASS |
| NFR-PRF-08 "every budget has a job" (`every-budget-has-a-job`) | PASS (the job check; the requirement stays unsatisfied) |
| `editor-latency` (report-only) | job succeeded; figures are informational |

**Per-job tally over all runner-executed `budgets.yml` runs (about 38, 2026-10-20/21 and 2026-11-09; lead's API check, reviewer logs).**

| Job / budget | Runs and result | Notes |
|---|---|---|
| NFR-PRF-04 full rebuild | 22 FAIL / 14 PASS, windows-latest; a step at #56 (20 FAIL before, 14 PASS + 2 INDETERMINATE after), a step improvement at #56 plus residual runner noise, not a regression; report-only from #166 | worst: run 279 at 80123ce median 51,260.5 ms; run 298 at 7b06763 median 55,224.6 ms; run 306 at 6857025 undecided, 28,394 ms, spread 192 %. Task #166 |
| FR-VLT-04 external change | 3 PASS / 1 FAIL | run 311 (9ce1c6c) 422.1 ms, 312 (c24943a) 417.0 ms, 315 (cf93c32) 376.1 ms (max 1261.2), 316 615.3 ms. **CI intermittent (3 of 4 pass, 376 to 615 ms)**; report-only on hosted Windows (#166); asserted on ubuntu-latest by `p1-external-change-linux` (#168), unrun. Task #165 |
| FR-VLT-01 tree path | 4 PASS | |
| NFR-PRF-06 switcher | 4 PASS | |
| NFR-PRF-03 | 36 FAIL | all the `serat.exe` job defect, task #164 |
| NFR-PRF-01 (both jobs) | 72 FAIL | same defect, task #164 |
| NFR-MNT-02 vault determinism | 36 PASS | |
| FR-NAV-02 read paths (`p1-read-paths`) | 30 PASS | report-only expectations, not budgets |

Headline: since ci run 87 runners executed jobs only on 2026-10-20/21 and 2026-11-09; no `budgets.yml` run has been green, so nothing is CI-asserted on the current head, and NFR-PRF-08 is still not satisfied. Every CI figure above is on "windows-latest, 2 cores, realtime scanner off, harness prints `gate eligible false`, not gate-eligible (RISK-012)" and is not gate-eligible. The cause line changes from the earlier draft; the verdict does not. Tasks #164, #165 and #166 are created by the lead.

**Owner remedy, in order:** (1) check the account's Actions billing (minutes, spending limit, payment method) in GitHub Settings, Billing; (2) check repository Settings, Actions, General; (3) re-run `ci.yml` and `budgets.yml` on `main`. Question for the owner: what changed around 2026-10-20 and 2026-11-09? Until runners return, the lead's local gate is the only executed evidence for the current head.

Apart from the 2026-11-09 results above, everything the jobs would assert has been run locally by the lead's per-merge gate; that is the substitute, and it is not CI.

## Known gaps

1. **#147 done on the container, 2026-11-24** (`reports/nfr-prf-02-quiet-rerun-2026-11.md`): quiet-machine scenarios, the real-IPC arm and the triage. Supporting evidence only. Three follow-up tasks are proposed in the report (T1 contain the split-preview pane, T2 stop flattening the note per keystroke, T3 a release-buildable bridge); the harness stub was repaired (the editor was hidden behind "No vault open").
2. **RM-1 reference-machine run not done.** NFR-PRF-02 must be tested on WebView2 with and without scroll margins (`status-p1-editor.md` §Open risks; `docs/05-testing/reference-machine.md`). **The Linux reference run (RM-2, gaws-dev-05, 2026-11-24, task #177) does not close this and does not close #167:** it is the Linux figure, the Windows scanner-on runs of NFR-PRF-01, 03, 04 and FR-VLT-04 are still owed, and so is the `environment` stamp, `p1-external-change` and `p1-save-tail` on RM-2 itself (`reports/reference-run-gaws-dev-05-2026-11.md`). It opens one gap: NFR-PRF-01's Serat term reads 1,310.4 ms p95 against 350 on Linux (asserted FAIL on a gate-eligible machine), a platform the PRD bounds the 350 ms away from; the owner decision is RISK-016.
3. **CI has not executed on the current head (F-384, RISK-013).** Since ci run 87 runners were assigned only on 2026-10-20/21 and 2026-11-09 (about 38 `budgets.yml` runs, none green); see CI jobs for the facts and the per-job tally (FR-VLT-04 intermittent, #165; NFR-PRF-04 breaching 30 s on 22 of 36 CI runs, #166; the NFR-PRF-03 and NFR-PRF-01 jobs failing without measuring, #164) and the owner remedy. NFR-PRF-08 unmet; the Windows runner tests (`crates/serat-app/tests/windows_vault.rs`, 17 runner-only assumptions in `test-plan.md`) are not recorded as having run; the `ipc-types` and `frontend-bundle` jobs are not recorded as having run on a runner.
4. **Table-cell and split-preview typing were the weakest NFR-PRF-02 scenarios under load** (strict 93.6% and 93.2%, p95 2 frames); quiet they read 100.0% (#147). Split preview keeps the least margin (about 5 ms/key of layout, pre-paint and hit-test for an uncontained 10,214-element preview): task T1 in the report.
5. **Bridge-side `save_patch` p95 723 ms under load** (p50 455 ms, max 976 ms, n=40, load average 10 to 30 on 4 CPUs). Off the keystroke path. #147 re-measured it quiet: 272.7 / 317.3 / 354.2 ms (p50 / p95 / max) in the dev-profile bridge, 38.7 / 59.7 / 62.0 ms with the bridge at `opt-level=3`. #172 built the bridge in release and timed it per phase (`docs/05-testing/reports/save-patch-bridge-release-2026-11.md`, Linux-container supporting evidence, no budget attached): **48.8 / 68.8 / 82.0 ms** (n=60, quiet), of which `submit_to_index` is 42.7 ms (87.5%), the index hand-off's synchronous read and parse of the 5,000-line note on the command thread (about 31 ms of it the parse); `Vault::write_patch` is 4.6 ms, the second `apply_edits` plus `hash_bytes` 1.28 ms (2.6%, under the quarter threshold, no follow-up). #147's guess that hash and edit application carry the tail is withdrawn. Contrast the attested 17.01 ms for the in-process hub-note save: a different quantity on a small note; NFR-PRF-03 on a 5,000-line note is unmeasured.
6. **MISSING-E2E-1.** No real-webview test of PDF page navigation, text selection or image zoom (FR-BIN-11/12). Falls to the manual week.
7. **MISSING-WIN-1.** UNC, drive-relative, junction and NTFS name tests are written by reading and awaiting a runner. No Windows or macOS build exists in this environment.
8. **NFR-SEC-01 runtime capture** (once in the week, on the reference machine) has not been taken; CI evidence is static.
9. **Residual risks carried** (`status-p1-editor.md` §Open risks): the index lock is guaranteed only on a local filesystem (A-02: `status-p1-editor.md` rows 154b and 156, and the PRD §Assumptions); RISK-015 (a cloud-synced vault can hydrate silently, P1 gate vault is local only by owner decision); the committed webview bundle must be rebuilt with each `app/` change (`frontend-bundle` job); #149 third-party notices (P5; `docs/01-planning/tasks-p1-editor.md`).
10. The automated suites were not re-run for this draft (see the rule above); a re-run at the final head is part of closing the report.
11. **Windows test leg red since #127; `main` definition-of-done red through ci runs 442-457 (task #191).** `cargo test --workspace (windows-latest)` did not compile `serat-app`'s lib tests from #127 (commit 2a61d35, merged in 2b97a8e on 2026-11-13: `body_text` in `asset/tests.rs` was `cfg(not(windows))` but used ungated). No Windows result exists for any merge since, including #188's TC-167 hostile test; treat every Windows-side line in this report that rests on `cargo test --workspace` as unrun. `cargo deny check` was also red (`cookie` 0.18.2, fixed by the lock bump to 0.18.3), and `windows-compile` failed on a toolchain mismatch (target installed for `@stable`, cargo runs 1.92). Fixes are in #191 (TC-172); a green Windows run on `main` is still owed. Windows leg ran on 2026-11-25 (run 460); one Windows-only test defect found (`reads::tests` search-slot `remove_file` on an open `index.db`), fixed in #193 (425 passed, 6 ignored, 1 failed before the fix). Run 460 also failed `cargo deny` (url 2.5.8, lock bump to 2.5.9 in #193) and `windows-compile` (target still not installed; explicit `rustup target add` from the repo root added in #193). `windows-compile` first went green on `main` in ci run 462 (0a4b3fe; run 463 also green) and is now gating (#194, TC-176). A fully green Windows test leg is still owed.
12. **FR-VLT-04 and NFR-PRF-03 on hosted Windows: no code lever left (#201).** A checkpoint costs per frame on that runner's disk (about 3 MB/s, run 447), so checkpoint constants only trade run count for size; writing fewer frames per edit would tune the product to the runner (ADR-0019 point 6). The lever is a gate-eligible Windows runner (ADR-0019 point 4(i), RISK-013, RISK-017); until then FR-VLT-04 stays report-only there (p95 370.4 of 500 ms).

No failures were found by running anything for this draft. Failures taken from the 2026-11-09 CI run are tasks created by the lead: #164 (the NFR-PRF-03 and NFR-PRF-01 jobs fail without measuring, a job defect; owner rust-core-dev or test-engineer to be assigned by the lead) #165 (FR-VLT-04 CI intermittent, 376 to 615 ms against 500 ms; reproduction: budgets.yml job `p1-external-change` on windows-latest, runs 311, 312, 315, 316) and #166 (NFR-PRF-04 CI breaches: runner class / gate-eligibility decision). The other gaps are existing tasks (#147, #149) or an infrastructure dependency on the owner.

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

## Daily log (full, per `docs/05-testing/reports/p1-gate-log.md`, created on day 0, protocol step 7)

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

**Pending the owner's week.** No GO or NO-GO is stated or implied by the status half above. The status half's findings that bear on the verdict are the gaps listed under Known gaps; none of them is a reach; gaps 6 and 8 (no real-webview PDF and image check, no runtime network capture) are closed by the week, the rest are not.

## Traceability

| ID | How this report serves it |
|---|---|
| NFR-PRF-01, 02, 03, 04, 06, 08 | The budget table: attested, unattested or not satisfied, each with its source. |
| NFR-PRF-05, 07, 09 | Stated as not run, with the phase that owns them. |
| Every P1 FR/NFR | The requirement-by-requirement witness table and tally. |
| NFR-CMP-01, NFR-CMP-04, NFR-CMP-02, NFR-USE-01, NFR-MNT-01, FR-NAV-06, FR-VLT-08, FR-EDT-02, FR-EDT-10, FR-BIN-03, FR-BIN-11, FR-BIN-12, NFR-SEC-01 | Blank week-in-vault sections with the protocol's evidence columns. |
| US-01, US-29, US-35, US-41 | Through the IDs above; US-01 is not complete until the NFR-CMP-01 week is filled. |
| RISK-013 | States that no budget or check was CI-asserted. |
