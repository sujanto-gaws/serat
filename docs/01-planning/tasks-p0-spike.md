---
project: Serat
artifact: P0 spike task list
phase: 01-planning
owner: lead
---

# P0 Spike — Task List

**Gate (charter §Milestones):** full index of 20,000 notes < 30 s · save→index < 50 ms p95 · cold start < 1 s (window visible < 400 ms).
**Requirements in scope (7, the whole P0 load):** FR-VLT-02, NFR-PRF-01, NFR-PRF-03, NFR-PRF-04, NFR-PRF-08, NFR-REL-02, NFR-MNT-02.
**Stories:** US-35 (primary), US-01, US-07. **ADRs:** 0001, 0002, 0003, 0006, 0007.
**Also settled by P0:** assumption A-06, architecture §1.2, open design question 1 (parse pool vs index writer), RISK-002.

Each teammate works in a git worktree under `.worktrees/<role>` and branches per task as `task/<id>-<slug>` from `main`; the lead merges after code-reviewer approval. See `docs/04-implementation/coding-standards.md`. Wave 1 (D1, B1, I1) was already in flight when the repository was created and lands directly on `main`. Status is maintained by the lead.

| # | Subject | IDs / ADRs | Owner | Artifact | Depends on | Status |
|---|---|---|---|---|---|---|
| D1 | `[P0/design] Write schema/001_init.sql from architecture §4` | FR-VLT-02, NFR-PRF-03, NFR-PRF-04, NFR-REL-02 · ADR-0001/0002/0003 | architect | `schema/001_init.sql` | — | pending |
| D2 | `[P0/design] Specify the cold-start path and index integrity detection` | NFR-PRF-01, NFR-REL-02 · ADR-0002 | architect | `docs/03-design/architecture.md` §3.4 + §Traceability | — | pending |
| D3 | `[P0/design] Cut the P0 subset of contracts/ipc.rs` | FR-VLT-02, NFR-PRF-01, NFR-MNT-01 · ADR-0007 | architect | `contracts/ipc.rs` | D1 | pending |
| B1 | `[P0/test] Build the deterministic 20k/50k benchmark vault generator` | NFR-MNT-02, US-35 | test-engineer | `bench/` | — | pending |
| B2 | `[P0/test] Add the budget harness and CI jobs asserting NFR-PRF-01/03/04` | NFR-PRF-08, US-35 | test-engineer | `bench/`, CI workflow | B1 | pending |
| I1 | `[P0/impl] Scaffold the Cargo workspace and a mechanical dependency-rule check` | NFR-MNT-02 · architecture §2.1 | rust-core-dev | `crates/`, `Cargo.toml` | — | pending |
| I2 | `[P0/impl] serat-vault walk, watcher, hash gate and atomic write` | FR-VLT-02, NFR-PRF-03 · architecture §3.1 | rust-core-dev | `crates/serat-vault` | I1 | pending |
| I3 | `[P0/impl] serat-parse comrak to blocks, fields, links, tasks` | NFR-PRF-04 · architecture §3.1 | rust-core-dev | `crates/serat-parse` | I1 | pending |
| I4 | `[P0/impl] serat-index single-writer actor, schema bootstrap, full rebuild` | NFR-PRF-04, FR-VLT-02 · ADR-0001/0002 | rust-core-dev | `crates/serat-index` | D1, I1, I3 | pending |
| I5 | `[P0/impl] Incremental save-to-index path over the rayon parse pool` | NFR-PRF-03 · ADR-0001 | rust-core-dev | `crates/serat-index` | I2, I3, I4 | pending |
| I6 | `[P0/impl] Detect a corrupt or missing index on start and rebuild automatically` | NFR-REL-02, US-07 · ADR-0002 | rust-core-dev | `crates/serat-index` | D2, I4 | pending |
| S1 | `[P0/impl] Tauri v2 shell: open vault, read/write one file, index-status events` | NFR-PRF-01, FR-VLT-02 · ADR-0007 | tauri-app-dev | `crates/serat-app`, `contracts/ipc.rs` | D3, I5 | pending |
| S2 | `[P0/impl] Mount CodeMirror 6 as a plain text box, no live preview` | NFR-PRF-01 · ADR-0006 | vue-frontend-dev | `app/` | S1 | pending |
| T1 | `[P0/test] Rebuild-equivalence: delete .serat/, assert identical results` | FR-VLT-02, NFR-REL-02, US-01, US-07 | test-engineer | `crates/serat-index/tests`, `docs/05-testing` | I4, I6 | pending |
| T2 | `[P0/test] Run the four gate benchmarks and record the numbers` | NFR-PRF-01, NFR-PRF-03, NFR-PRF-04, NFR-PRF-08, US-35 | test-engineer | `docs/05-testing/reports/` | B2, I5, I6, S2 | pending |
| R1 | `[P0/review] Review every P0 artifact against C-01..C-07 and ADR-0001/0002` | all P0 IDs | code-reviewer | review notes | per artifact | pending |
| D4 | `[P0/design] Close open design question 1 and §1.2 with the P0 numbers` | NFR-PRF-03, NFR-PRF-04 · ADR-0001 | architect | `docs/03-design/architecture.md` | T2 | pending |
| Q1 | `[P0/req] Re-baseline the NFR-PRF targets from the P0 measurements` | NFR-PRF-01, NFR-PRF-03, NFR-PRF-04, US-35 | product-analyst | `docs/02-requirements/prd.md` | T2, D4 | pending |

## Out of scope for P0

No product features (charter). No live preview (P1, ADR-0006), no wikilink resolution UI, no SQ, no extraction, no AI, no packaging. Nothing on the charter's "deliberately not" list is touched.
