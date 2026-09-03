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

## Findings so far (P0 in progress)

Recorded as they land, so the gate report is assembled from evidence rather than memory. Each names who measured it.

| # | Finding | Evidence | Status |
|---|---|---|---|
| F-01 | **FTS5 is present in the SQLite Serat ships.** The DDL applies in full through rusqlite with no statement erroring, so architecture.md §4's "runs as delivered" is true of the bundled build (SQLite 3.53.2, `ENABLE_FTS5`). The earlier failure at line 292 came from the system `sqlite3` 3.42 on PATH — the Android platform-tools binary, built without fts5. A fact about that binary, not about Serat. Supersedes the caveat in commit d3b024a. | rust-core-dev, measured through rusqlite | Closed |
| F-02 | **The FTS5 external-content delete path holds.** Parent note deleted, block removed by `ON DELETE CASCADE`, blocks after cascade 0, FTS hits 0, and `INSERT INTO blocks_fts(blocks_fts) VALUES ('integrity-check')` returns Ok — internally consistent, not merely quiet. This is the case §4.4 singles out as breaking silently. | rust-core-dev | Closed |
| F-03 | **NFR-PRF-03 is unsatisfiable as written — arithmetically, not as a performance problem.** No amount of fast code closes a gap that exists before the code runs. The requirement is 50 ms p95 save-to-index; §3.1 step 1 coalesces watcher events per path over 150 ms. End to end as worded, the design misses by construction — 150 > 50. Resolution is one of: the requirement means event-dequeue-to-row and its wording is wrong (Q1 re-baseline); Serat's own writes get a route skipping the debounce, as they are not external events (design change); or the 150 ms window is wrong. The gate report must state which interval the 50 ms claim rests on. | rust-core-dev, building the I4 measurement seam | Open — architect to rule, product-analyst at Q1 |
| F-04 | **`rebuild()` has no legal home under §2.1.** It walks, parses and writes, and no product crate may do two of those; the budget harness and T1 must both run without Tauri. Lead's interim ruling: build `rebuild(source: &dyn NoteSource, parser: &dyn NoteParser, index_dir: &Path)` in serat-index — traits-in-core is the mechanism §2.1 prescribes and adds no dependency edge. The *home* (serat-index vs a new serat-pipeline crate) stays architect's, because the signature is expensive and the file move is not. | rust-core-dev; ruled by lead | Open — architect to ratify or relocate |
| F-05 | **ADR-0001's channel is not fire-and-forget.** `flush()` had to exist on day one: the rebuild runs ANALYZE after the last batch and every test needs read-after-write. The true shape is *one writer, asynchronous, with an explicit and expensive barrier*. Invisible in P0; surfaces in I5 when the UI reports "indexed" after a save, and in P2 when a write-back refreshes the row it changed. §2.2 describes a frictionless channel and should be amended. | rust-core-dev | Open — architect to amend §2.2 |
| F-06 | **Backpressure was undefined and had to be decided in code.** A full bounded queue drops the path, latches a rescan flag, and lets the consumer walk and reconcile. Blocking the producer instead blocks the watcher's coalescer, overflowing the OS event buffer — losing events with no record that anything was lost, the one failure C-02 cannot absorb. "Bounded" in §3.1 step 2 reads as a memory statement when it is a correctness one. | rust-core-dev, forced by I2 | Open — ADR-0011 candidate |
| F-07 | **A-06 / RISK-002: the Rust is not the risk.** serat-vault compiled first try, 35 of 40 tests passed on the first run, two clippy findings across two tasks, rusqlite bundled built SQLite unattended. Nothing about the language cost an hour. All three real costs were coordination: a workspace-wide gate red because another teammate's crate was mid-edit; the repository appearing mid-task; and untracked `bench/` breaking `cargo metadata` in every worktree at once. The risk is that the workspace is one shared object with many owners, and that cost scales with teammates rather than lines of code. | rust-core-dev | Open — informs the RISK-002 re-rating at gate |
| F-08 | **`PRAGMA foreign_keys` is ON by default in our build** (`DEFAULT_FOREIGN_KEYS` is compiled in), so cascade fires without setting it. Set it explicitly anyway — depending on a vendored build flag is what changes silently at a version bump — but the reason is defensiveness, not necessity, and the stated reason matters to whoever debugs it next. | rust-core-dev, proved by running the cascade without the pragma | Closed |
| F-09 | **§1.2's three assumptions: P0 has produced evidence on none of them.** (a) *Write contention is the bottleneck* — not tested; first measurable in I4. What writing the types showed is structural, not empirical: the shape is sound in that no message needs a reply, and weaker than §2.2 claims in that `flush()` is a mandatory serialising barrier. Record as neither confirmed nor contradicted. (b) *Parsing dominates rebuild time* — not tested; this is open design question 1 and needs I4, I5 and B2 against the 20k vault. (c) *Brute-force vector search at 200,000 chunks* — untouched, a P4 question, and ADR-0009 was written to be reversed cheaply. The A-06 coordination finding (F-07) is real but is a finding about how the team works, not about the three structures §1.2 puts at risk. A status document implying otherwise overstates the spike. | rust-core-dev | Open — settled by T2 |
| F-10 | **A single rebuild number cannot answer open design question 1.** The rebuild's cost splits four ways — walk, read+hash, parse, write — because the walk enumerates without hashing. Reported as one number, "the index writer dominates" and "reading 39.4 MiB off disk dominates" are indistinguishable, and they imply completely different fixes. B2 must report the four phases separately before it hardens; otherwise the P0 gate produces a number that passes or fails without saying what to change. | rust-core-dev, from I2's walk | Open — B2 to implement before T2 |
