---
project: Serat
artifact: P1 task list
phase: 01-planning
owner: lead
---

# P1 — An editor you could live in — Task List

**Gate (charter §Milestones):** owner opens their own vault in Serat for a week without needing Obsidian. Protocol: `docs/05-testing/p1-gate-protocol.md` (task #28).

**Entry decision (owner, 2026-09-24):** proceed from P0 with its gate not formally closed. PRD v1.2–v1.4 carry the P0 re-baseline (NFR-PRF-01 split per ADR-0014; NFR-PRF-03/04 pass). Carried as risks: F-384 / RISK-013 (no CI jobs, NFR-PRF-08 unsatisfiable), #123 (scanner-enabled re-run), RISK-014 (FR-VLT-01 4x over).

**Scope rulings (owner, 2026-09-24):** flat table view stays P2 (FR-VIW-01); FR-NAV-02 split — note bodies in P1, PDF/extracted content to P2 under a new ID (task #1).

**Requirements in scope (31):** FR-VLT-01/03/04/05/07/08 · FR-EDT-01/02/03/10 · FR-NAV-01/02(notes)/04/06 · FR-STR-03 · FR-BIN-01/02/03/11/12 · NFR-PRF-02/06 · NFR-SEC-01/06 · NFR-REL-01 · NFR-CMP-01/02/03/04 · NFR-USE-01 · NFR-MNT-01.
**Stories:** US-01..09, US-18, US-19, US-29, US-34, US-35. **ADRs:** 0001, 0005, 0006, 0007, 0008, 0011, 0012, 0014 (+0015 new, tree).

**Working rules:** one worktree per teammate; branch `task/p1-<nn>-<slug>` from `claude/great-mccarthy-jywddn`; lead merges only after code-reviewer approval; nobody commits another teammate's work; a finding is recorded only if it changes a number, a design or a task.

| # | Subject | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | `[P1/req] Split FR-NAV-02 and confirm FR-BIN-02 is a P1 field lookup` | product-analyst | — | **merged** — PRD v1.5, FR-NAV-08 new (P2) |
| 2 | `[P1/design] Make the FR-VLT-01 file tree meet 2 s at 20k notes` (ADR-0015) | architect | — | **merged** e624c20 |
| 3 | `[P1/design] Design the quick switcher and its NFR-PRF-06 budget` | architect | — | **merged** 487abc3 |
| 4 | `[P1/design] Specify the P1 read paths: note search, block backlinks, frontmatter lookup` | architect | 1 | **merged** b2d2f1c |
| 5 | `[P1/design] Specify editor integration: reconciliation, links/embeds, command registry, split preview` | architect | — | **merged** d0d5300 |
| 6 | `[P1/design] Specify assets, sidecars and the viewers' file-serving path` | architect | 1 | **merged** (this commit) |
| 7 | `[P1/design] Cut the P1 subset of contracts/ipc.rs` | architect | 2–6 | **merged** be8baa7 |
| 8 | `[P1/design] Write P1 screen specs and tokens` | frontend-designer | 3, 5 | **merged** 8fe20a3 |
| 9 | `[P1/test] Budget jobs for FR-VLT-01 tree ≤2 s and FR-VLT-04 ≤500 ms` | test-engineer | 2 | **open** @ 1098c60, fixed-unreviewed; reconcile with #14's p1-external-change bench |
| 10 | `[P1/test] Budget jobs for the quick switcher (FR-NAV-01, NFR-PRF-06)` | test-engineer | 3 | **open** @ e54986a, fixed-unreviewed; reconcile with #16's p1-switcher bench |
| 11 | `[P1/test] NFR-PRF-02 editor keystroke-latency harness` | test-engineer | 5 | **merged** 6521013 (3/8 scenarios; #60 CI, #61 scenarios follow) |
| 12 | `[P1/test] Obsidian-compatibility corpus and write fault-injection suite` | test-engineer | 5 | **open** @ 1a081c7, WIP |
| 13 | `[P1/impl] FR-VLT-01 tree path + F-12 walk fix (+F-33/F-34)` | rust-core-dev | 2, 9 | **merged** 1e15a3d |
| 14 | `[P1/impl] Ignore file and external-change indexing` | rust-core-dev | 9 | **merged** db877e5 (+#58; INDEX_VERSION 10) |
| 15 | `[P1/impl] Search, backlinks and frontmatter read paths` | rust-core-dev | 4 | **merged** |
| 16 | `[P1/impl] Quick-switcher index` | rust-core-dev | 3, 10 | **merged** 81d7ef9 |
| 17 | `[P1/impl] Index assets and associate sidecars` | rust-core-dev | 6 | **merged** (approved at dbfa2b5) |
| 18 | `[P1/impl] P1 IPC commands/events and generated TS types` | tauri-app-dev | 7, 13–17 | **merged** rounds 1–3; last 61c3054 (24b4073). Still blocked: resolve_links/note_anchors, choose_vault |
| 19 | `[P1/impl] Contained asset serving + .serat/ gitignore offer` | tauri-app-dev | 6, 7 | **merged** fd7f9c6 (Windows by reading only) |
| 20 | `[P1/test] Verify no outbound network requests` | tauri-app-dev + test-engineer | 18 | pending |
| 21 | `[P1/impl] Hybrid live-preview editor + split fallback` | vue-frontend-dev | 8, 11 | **merged** (via #42 lineage) |
| 22 | `[P1/impl] Wikilink autocompletion and embeds` | vue-frontend-dev | 18, 21 | pending — honour SwitcherResults.discarded and app://closing flush |
| 23 | `[P1/impl] Reconcile external edits into the open note` | vue-frontend-dev | 12, 18, 21 | pending — note://external-change and request_external_diff exist |
| 24 | `[P1/impl] File tree, backlinks pane, search panel` | vue-frontend-dev | 8, 18 | pending — open_tree, tree://status, search, backlinks exist |
| 25 | `[P1/impl] Quick switcher and command palette` | vue-frontend-dev | 8, 18 | pending — switcher_query exists |
| 26 | `[P1/impl] PDF and image viewers` | vue-frontend-dev | 8, 19 | pending — assetInfo/readAssetRange/assetUrl exist |
| 27 | `[P1/test] Test cases for every P1 ID + hostile suite` | test-engineer | 12–17, 22–26 | pending |
| 28 | `[P1/test] Gate protocol (early) and GO/NO-GO report (last)` | test-engineer | 20, 27 | protocol merged; report last |
| 30 | `[P1/design] Account for FR-NAV-08 in architecture.md so doclint passes` | architect | 1 | **merged** db1d967 |
| 31 | `[P1/test] Update and re-run the NFR-PRF-01 cold-start bench for ADR-0015's tree mark` | test-engineer | 2 | **open** @ d8e2547, CR + lead WIP; TREE_DELIVERED can flip |
| 32 | `[P1/req] Amend FR-VLT-01 and NFR-PRF-02 to the owner-approved definitions` (PRD v1.6) | product-analyst | 2, 5 | **merged** 857af57 |
| 33 | `[P1/impl] Implement ADR-0016 patch-set application and edit-script in serat-vault` | rust-core-dev | 5 | **merged** (with #55) |
| 34 | `[P1/impl] Close the Windows drive-relative path escape in VaultPath and Vault::resolve` — **live security defect** (NFR-SEC-06, T-07, T-10). Scope: reject `:` in any segment; reject Windows device names and trailing dot/space; `starts_with(root)` after resolve; case-fold the ignored-dir check. ADR-0017 relies on all four. | rust-core-dev | — | in progress |
| 35 | `[P1/impl] Report, never drop, vault files whose names Serat refuses` — Windows-only name rules apply on Windows only; refused names are reported with a count (FR-VLT-01, NFR-SEC-06) | rust-core-dev | 34 | in progress |
| 36 | `[P1/design] Correct ADR-0017's false remote-origin control claim` — real controls: (a) top-level navigation guard, (b) sandbox CSP on scheme responses, (c) app CSP `frame-src 'none'; object-src 'none'` + sanitiser | architect | 6 | **merged** |
| 37 | `[P1/impl] Install the webview navigation guard, scope the opener to http/https/mailto, pin frame-src/object-src 'none', refuse downloads, disable release context menu` — implements controls (a) and (c) with tests (NFR-SEC-01, NFR-SEC-06, T-09, T-10) | tauri-app-dev | 5, 36 | in progress |

Task 29 was folded into 15. Every branch is reviewed by code-reviewer before merge.

**Owner rulings recorded during P1 (2026-09-24):** gate reaches for any post-P1 capability are logged and counted, not gate-failing; FR-VLT-01 is re-defined as the complete tree timed from window paint (ADR-0015); NFR-PRF-02 is re-defined as ≥95% of keystrokes painted in the first frame (architecture §9.3), because the old "p95 < 16 ms" is unmeetable at 60 Hz (a bare contenteditable measures 17.1 ms).
