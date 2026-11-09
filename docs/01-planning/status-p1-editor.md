---
project: Serat
artifact: P1 status
phase: 01-planning
owner: lead
status: in progress — 2026-11-09 — gate NOT reached; core and app plumbing on main, frontend and gate work open
---

# P1 — An editor you could live in — Status

**Gate (charter §Milestones):** owner opens their own vault in Serat for a week without needing Obsidian. Protocol: `docs/05-testing/p1-gate-protocol.md` (merged).

**Where P1 stands.** The Rust core and the serat-app plumbing for P1 are on `main`: tree, search, backlinks, saves as patch sets, assets and sidecars, the quick-switcher index, external-change indexing with an always-mtime catch-up, the WAL checkpointer off the save path, the asset scheme, and the app's open/close/switch sequence. The editor's keystroke stall is fixed and the NFR-PRF-02 harness exists and passes 3 of 8 scenarios on headless Chromium. Not yet on main: the editor's own UI wiring (tree, panels, switcher UI, viewers), the remaining latency scenarios, CI wiring for the latency gate, and the week-in-vault report.

`main` == integration branch `claude/great-mccarthy-jywddn` == **217b75e**. Every merge goes through the full gate first: `cargo test --workspace`, `cargo clippy --workspace --all-targets -D warnings`, `cargo fmt --check`, `cargo check -p serat-app --release`, doclint, `npm test` and `vue-tsc` in `app/`. Index schema version on main: **10** (11 arrives with #66). Toolchain: pinned Rust 1.92; Chromium 153 for the latency harness.

## Owner decisions recorded during P1

1. Enter P1 with P0's gate not formally closed; F-384 (no CI jobs) and #123 (scanner-on re-run) carried as risks.
2. Flat table view stays P2 (FR-VIW-01). FR-NAV-02 split: note bodies in P1, extracted/PDF content becomes FR-NAV-08 (P2).
3. Gate rule: reaching for Obsidian for any post-P1 capability is logged and counted, not gate-failing. A missing-P1-feature, defect or trust reach is NO-GO and restarts the 7 days.
4. FR-VLT-01 re-defined (PRD v1.6, ADR-0015): complete tree within 2 s p95 from the window's first paint; empty folders reported, not gated.
5. NFR-PRF-02 re-defined (PRD v1.6, §9.3): ≥95% of keystrokes painted on time under the E0 rule; measured per engine version; gate-eligible only on the reference machine.
6. Lead default (told, not objected to): a gate reach mapping to no PRD ID counts as missing P1 unless product-analyst records an ID or exclusion first.
7. Finished branches push to `main` as well as the integration branch (owner, 2026-10-21).

## Design rulings made during implementation (all merged)

- **ADR-0015 §2:** the open-time catch-up always compares mtimes (a `git pull` while Serat is closed must be seen); `clean_close` only schedules the background `quick_check`. Coarse-mtime gap closed by `notes.size` plus a racy window `caught_up_from_ms` (#66, in review). `Vault::read` stats the open handle before reading; `TouchMtime` is hash-guarded.
- **§3.1.1 hub-note save (#53 → #56):** the WAL auto-checkpoint landed on every second hub save; it now runs on its own thread with a 250 ms idle gate, 2 s idle spacing and a 4,000-frame bound; search connection cache 64 MiB. Attested quiet run: **p95 23.3 ms vs 50 ms**.
- **§9.2 quick switcher:** 20k gates, 50k reported; empty query falls back to most-recent-by-mtime; per-query DP cell budget; history written by `Vault::update_history`.
- **§9.3 NFR-PRF-02:** E0 rule, two per-run invariants (keydown present; entry-end spread ≤8 ms, derived), engine version on every figure, graded arm informational and per-version, key window [t−0.01, t+4]. Headless end-of-document stall recorded; scroll margins are load-bearing for the measurement until the reference-machine run tests both builds.
- **ADR-0017:** handler accepts `serat-asset://localhost/…` on every platform (wry reverts its Windows rewrite); step 7 reads the handle's real path via `filepath`; `use_https_scheme(false)` pinned on the window builder.
- **#63 clean close:** `app://closing` → editor flush → `flush_ack`, bounded 2 s, only when a note is open; close and switch serialised on `opening`.
- **#58:** a parser-refused note is a bare row on every path (C-02). **#57:** block-style frontmatter lists are indexed (aliases were 0 rows before). **#59:** no unsafe carve-out for cache diagnostics.

## Merged on main (this run)

| # | What | Review |
|---|---|---|
| 14 | `.seratignore`, watcher-driven indexing, DirChanged rescan, clean-close marker, (path,hash) echo, one walk at open; INDEX_VERSION 10 | approved after 3 rounds |
| 16 | quick-switcher index + bench | approved |
| 17 | assets and sidecars, schema v8 | approved |
| 18 (round 3) | index://changed consumer, app-owned open-time catch-up, close_vault, switcher_query | approved after 3 rounds |
| 19 | serat-asset scheme, asset_info/read_asset_range, handle-path containment | approved (Windows by reading only) |
| 56 | WAL checkpointer, page-cache sizes, 2 s idle spacing | approved |
| 57 | block-style frontmatter lists; INDEX_VERSION 9 | approved |
| 58 | bare rows for parser-refused notes | approved (with #14) |
| 65 | always-mtime catch-up, stat-before-read, hash-guarded TouchMtime | approved |
| 11 | NFR-PRF-02 harness (unit → controls → subject projects, 500 keys) | approved after 3 rounds |
| 42 | editor keystroke stall: #app bounded to the window, CodeMirror owns the scroll | approved; **3/8 scenarios PASS, NFR-PRF-02 stays open** |
| docs | ADR-0015 §2 amendment, §9.3 rewrite, ADR-0017 wording, editor.md scrolling, #63 clean-close design, #59 ruling | doclint green |

## Open branches

| # | Branch @ head | State | Next |
|---|---|---|---|
| 63 | `task/p1-63-flush-before-close` @ a80365e | round 2 in re-review | merge; editor half in #22/#23 |
| 66 | `task/p1-66-size-and-window` @ bedd1e3 | NOT APPROVED (window advances past skipped files; rescan must not write); round 2 in progress | fix, re-review, merge (INDEX_VERSION 11) |
| 67 | `task/p1-67-watcher-test-flake` | in progress: three load-sensitive tests made deterministic | review, merge |
| 71 | `task/p1-71-filewritten-mtime` @ a9e4f18 | in review | merge |
| 72 | `task/p1-72-unc-root` | in review (Windows unexercised) | merge; Windows CI run owed |
| 9, 10 | `task/p1-09-tree-budget` @ 1098c60, `task/p1-10-switcher-budget` @ e54986a | pre-reset, fixed-unreviewed; #14 and #16 shipped their own benches since | reconcile with `p1-external-change` / `p1-switcher`, review, merge |
| 12 | `task/p1-12-compat-corpus` @ 1a081c7 | pre-reset WIP | resume |
| 28b | `task/p1-28b-no-id-reach` @ e9564c4 | pre-reset, unreviewed | review |
| 31 | `task/p1-31-tree-first-screen-mark` @ d8e2547 | pre-reset CR + lead WIP; `startup::TREE_DELIVERED` can flip now | finish, re-review |
| 40 | `task/p1-40-block-widgets` @ b5d3424 | pre-reset WIP, was paused for #21 (now merged) | resume |
| — | `task/p1-design-merge-preview` @ 87e6ace | unknown | inspect, delete if stale |

## Not started (P1 scope)

#20 no-network test · #22 wikilink completion (partly on the merged #21/#42 lineage; must honour `SwitcherResults.discarded` and the `app://closing` flush) · #23 reconcile external edits (`note://external-change`, `request_external_diff` exist) · #24 tree/backlinks/search panels (`open_tree`, `tree://status`, search, backlinks exist) · #25 switcher + palette UI · #26 PDF/image viewers (`assetInfo`/`readAssetRange`/`assetUrl` exist) · #27 test cases per P1 ID + hostile suite · #35 refused names · #39 context menu Win/mac · #41 cargo-deny · #43 tree ops UI · #49 read/write off main thread.

## Follow-ups filed this run

#60 latency job in CI · #61 remaining 5 PRF-02 scenarios + CRLF/BOM + real IPC · #62 attested post-spacing hub-save run · #68 TreeSession replay on reload · #69 background quick_check · #73 (done) · #75 48 px scroll-margin token · #18c follow-up: capture the window timestamp before arming the watcher.

## Budget numbers (20k vault unless stated)

| Budget | Figure | Status |
|---|---|---|
| NFR-PRF-03 hub-note save | p95 23.3 ms (attested quiet, pre-spacing); 32.6 ms loaded post-spacing | PASS; #62 re-attests |
| NFR-PRF-06 switcher keystroke 3 | p95 16–18 ms of 20 ms; heading mode up to 97% | PASS, thin |
| FR-VLT-04 external change visible | single note p95 192 ms of 500 ms (unattested) | PASS |
| NFR-PRF-04 full rebuild | 7.9–10.8 s of 30 s | PASS |
| NFR-PRF-02 keystrokes | 3/8 scenarios ≥95% on headless 153; lowest cell 95.0% | OPEN (reference machine) |
| catch-up on unchanged index | 16–22 ms; sync-tool worst case 294 ms | reported |
| R5 attachments read | p95 2.3 ms of 20 ms | PASS |

## Open risks

- **No Windows or macOS build in this environment.** #19's Windows paths, #72's UNC handling and #34/#35's name rules are verified by reading only. A Windows CI run is owed before any release claim.
- **NFR-PRF-02 is not gateable headless.** The reference-machine run (WebView2) must test the editor with and without scroll margins.
- **RISK-013 / F-384:** CI still produces no jobs; #60 cannot gate until it does.
- **Load-sensitive tests** (#67): three tests flake under concurrent builds; the lead's gate treats a frontend test failure as blocking from now on.
- **Disk:** the session allowance overflowed once with three concurrent Rust builds; `~/.cargo/config.toml` sets incremental=false and debug=0, and each agent uses a uniquely named target dir.

## Process notes

- Teammates cannot message each other or edit the task list; the lead relays and records. Agents read their spec from the brief.
- A container reset wipes worktrees, the session task list and the cargo config; branches on origin are the record. Never remove a worktree whose agent may still be resumed.
- 63 merged `task/p1-*` branches remain on origin because branch deletion is denied to the lead; the owner may delete them (`git push --delete origin <branch>`); all are contained in `main`.
