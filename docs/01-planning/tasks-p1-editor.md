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
| 2 | `[P1/design] Make the FR-VLT-01 file tree meet 2 s at 20k notes` (ADR-0015) | architect | — | in progress |
| 3 | `[P1/design] Design the quick switcher and its NFR-PRF-06 budget` | architect | — | pending |
| 4 | `[P1/design] Specify the P1 read paths: note search, block backlinks, frontmatter lookup` | architect | 1 | pending |
| 5 | `[P1/design] Specify editor integration: reconciliation, links/embeds, command registry, split preview` | architect | — | pending |
| 6 | `[P1/design] Specify assets, sidecars and the viewers' file-serving path` | architect | 1 | pending |
| 7 | `[P1/design] Cut the P1 subset of contracts/ipc.rs` | architect | 2–6 | pending |
| 8 | `[P1/design] Write P1 screen specs and tokens` | frontend-designer | 3, 5 | pending |
| 9 | `[P1/test] Budget jobs for FR-VLT-01 tree ≤2 s and FR-VLT-04 ≤500 ms` | test-engineer | 2 | pending |
| 10 | `[P1/test] Budget jobs for the quick switcher (FR-NAV-01, NFR-PRF-06)` | test-engineer | 3 | pending |
| 11 | `[P1/test] NFR-PRF-02 editor keystroke-latency harness` | test-engineer | 5 | pending |
| 12 | `[P1/test] Obsidian-compatibility corpus and write fault-injection suite` | test-engineer | 5 | pending |
| 13 | `[P1/impl] FR-VLT-01 tree path + F-12 walk fix (+F-33/F-34)` | rust-core-dev | 2, 9 | pending |
| 14 | `[P1/impl] Ignore file and external-change indexing` | rust-core-dev | 9 | pending |
| 15 | `[P1/impl] Search, backlinks and frontmatter read paths` | rust-core-dev | 4 | pending |
| 16 | `[P1/impl] Quick-switcher index` | rust-core-dev | 3, 10 | pending |
| 17 | `[P1/impl] Index assets and associate sidecars` | rust-core-dev | 6 | pending |
| 18 | `[P1/impl] P1 IPC commands/events and generated TS types` | tauri-app-dev | 7, 13–17 | pending |
| 19 | `[P1/impl] Contained asset serving + .serat/ gitignore offer` | tauri-app-dev | 6, 7 | pending |
| 20 | `[P1/test] Verify no outbound network requests` | tauri-app-dev + test-engineer | 18 | pending |
| 21 | `[P1/impl] Hybrid live-preview editor + split fallback` | vue-frontend-dev | 8, 11 | pending |
| 22 | `[P1/impl] Wikilink autocompletion and embeds` | vue-frontend-dev | 18, 21 | pending |
| 23 | `[P1/impl] Reconcile external edits into the open note` | vue-frontend-dev | 12, 18, 21 | pending |
| 24 | `[P1/impl] File tree, backlinks pane, search panel` | vue-frontend-dev | 8, 18 | pending |
| 25 | `[P1/impl] Quick switcher and command palette` | vue-frontend-dev | 8, 18 | pending |
| 26 | `[P1/impl] PDF and image viewers` | vue-frontend-dev | 8, 19 | pending |
| 27 | `[P1/test] Test cases for every P1 ID + hostile suite` | test-engineer | 12–17, 22–26 | pending |
| 28 | `[P1/test] Gate protocol (early) and GO/NO-GO report (last)` | test-engineer | 20, 27 | protocol in progress |

Every branch is reviewed by code-reviewer before merge.
