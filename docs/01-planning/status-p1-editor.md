---
project: Serat
artifact: P1 status
phase: 01-planning
owner: lead
status: in progress — 2026-11-09 (evening) — gate NOT reached; core, app plumbing, editor reconciliation and the three panes on main; switcher/viewers/latency job in review
---

# P1 — An editor you could live in — Status

**Gate (charter §Milestones):** owner opens their own vault in Serat for a week without needing Obsidian. Protocol: `docs/05-testing/p1-gate-protocol.md` (merged).

**Where P1 stands.** On `main`: the Rust core and serat-app plumbing (tree, search, backlinks, patch-set saves, assets and sidecars, switcher index, always-mtime catch-up with the window stamped before the watcher is armed, WAL checkpointer off the save path, asset scheme, open/close/switch with the editor's bounded flush), the editor's external-change reconciliation UI, the file tree / backlinks / search panes, CI budget jobs for FR-VLT-01, FR-VLT-04 and NFR-PRF-06, the #79 contract amendments (ahead of implementation), and PRD v1.9. In review or round 2: the quick switcher + command palette (#25), the PDF/image viewers (#26), the report-only NFR-PRF-02 CI job (#60). In progress: the #79 implementation tasks (#86 link field keys / INDEX_VERSION 12, #88 search-hit staleness, #90 `vault://opened`). Not started: #20, #27, #35, #39, #41, #43, #49; the week-in-vault report; the reference-machine NFR-PRF-02 run; the attested hub-save run (#62).

`main` == integration branch `claude/great-mccarthy-jywddn` == **9ce1c6c**. Every merge goes through the full gate first: `cargo test --workspace`, `cargo clippy --workspace --all-targets -D warnings`, `cargo fmt --check`, `cargo check -p serat-app --release`, doclint, `npm test` and `vue-tsc` in `app/`. Index schema version on main: **11** (12 arrives with #86). Toolchain: pinned Rust 1.92; Chromium 153 for the latency harness.

## Owner decisions recorded during P1

1. Enter P1 with P0's gate not formally closed; F-384 (no CI jobs) and #123 (scanner-on re-run) carried as risks.
2. Flat table view stays P2 (FR-VIW-01). FR-NAV-02 split: note bodies in P1, extracted/PDF content becomes FR-NAV-08 (P2).
3. Gate rule: reaching for Obsidian for any post-P1 capability is logged and counted, not gate-failing. A missing-P1-feature, defect or trust reach is NO-GO and restarts the 7 days.
4. FR-VLT-01 re-defined (PRD v1.6, ADR-0015): complete tree within 2 s p95 from the window's first paint; empty folders reported, not gated.
5. NFR-PRF-02 re-defined (PRD v1.6, §9.3): ≥95% of keystrokes painted on time under the E0 rule; measured per engine version; gate-eligible only on the reference machine.
6. Lead default (told, not objected to): a gate reach mapping to no PRD ID counts as missing P1 unless product-analyst records an ID or exclusion first.
7. Finished branches push to `main` as well as the integration branch (owner, 2026-10-21).

## Design rulings made during implementation (all merged)

- **ADR-0015 §2:** the open-time catch-up always compares mtimes (a `git pull` while Serat is closed must be seen); `clean_close` only schedules the background `quick_check`. Coarse-mtime gap closed by `notes.size` plus a racy window `caught_up_from_ms` (#66); the window is stamped immediately before the watcher is armed (#78). `Vault::read` stats the open handle before reading; `TouchMtime` is hash-guarded.
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
| 63 | editor-bounded flush before close: `app://closing` → `flush_ack`, serialised on `opening` | approved after 2 rounds |
| 66 (+66b) | INDEX_VERSION 11: `notes.size`, `caught_up_from_ms` racy window, `min(started_at, oldest unread)` | approved after 2 rounds |
| 71 | `Written { hash, mtime_ms, size }` from the temp handle before rename | approved after 2 rounds |
| 72 | `strip_verbatim` shared; `\\?\UNC\` roots | approved (Windows by reading only; #77) |
| 67 | three load-sensitive tests deterministic; `FaultInjector` hold / fail-until-flush seams, all `cfg(test)` | approved after 2 rounds; 20 runs under load, 0 failures |
| 78 | catch-up window stamped before the watcher is armed; two-session test with negative control | approved after 2 rounds |
| 22/23 | editor: clean-close flush + ack, vault-scoped events, completion guards, external-change UI (Updated externally, minimised dot, Compare view, Not saved line) | approved after 2 rounds |
| 24 | file tree, backlinks pane, search panel per the screen specs; roving tabindex; `tree://status` live region | approved after 3 rounds |
| 81 | PRD v1.9: FR-VLT-04 "500 ms at p95 over 100 single-note changes", Bench; 50k switcher finding | approved after 2 rounds |
| 79 | contract ahead of implementation: `Backlink.frontmatter_keys`, `SearchHit.changed_since_indexed`, `BootInfo.vault_id`, `vault://opened`; §3.4 vault-id delivery | approved after 2 rounds |
| 9/10 | CI budget jobs p1-tree-path, p1-external-change, p1-switcher with `--assert` at 20k; `P1_GATED` sabotage-proof test | approved after 3 rounds |

## Open branches

| # | Branch @ head | State | Next |
|---|---|---|---|
| 25 | `task/p1-25-switcher-palette` @ 21734b6 | NOT APPROVED round 1 (second command registry breaks §9.3; CRLF heading offset; mac Ctrl double-fire; FR-NAV-06 not -05) | round 2, re-review, merge |
| 26 | `task/p1-26-binary-viewers` @ 82b0793 | in review (pdf.js lazy chunk, same-origin worker, CSP unchanged) | review, merge |
| 60/62 | `task/p1-60-prf02-ci` @ 689aadd | in review; #62 NOT closed (both runs unattested under load; p95 42–44 ms) | review, merge; quiet-machine run owed |
| 86 | `task/p1-86-link-field-key` | in progress (INDEX_VERSION 12, `links.field_key`, R2 keys, folds #87) | review, merge |
| 88 | `task/p1-88-search-hit-staleness` | in progress (R1 returns mtime/size; stat ≤ 50 hits; folds #89) | review, merge |
| 90 | `task/p1-90-vault-opened` | in progress (`BootInfo.vault_id`, emit in `open_vault_with`, ordering tests) | review, merge; then #91 (front end adopts it) |
| 12 | `task/p1-12-compat-corpus` @ 1a081c7 | pre-reset WIP | resume |
| 28b | `task/p1-28b-no-id-reach` @ e9564c4 | pre-reset, unreviewed | review |
| 31 | `task/p1-31-tree-first-screen-mark` @ d8e2547 | pre-reset CR + lead WIP | finish, re-review |
| 40 | `task/p1-40-block-widgets` @ b5d3424 | pre-reset WIP | resume |
| — | `task/p1-design-merge-preview` @ 87e6ace, `task/p1-09-tree-budget`, `task/p1-10-switcher-budget`, `task/p1-79-contract-gaps`, `task/p1-79b-contract-gaps` | superseded | owner deletes |

## Not started (P1 scope)

#20 no-network test · #27 test cases per P1 ID + hostile suite · #35 refused names · #39 context menu Win/mac · #41 cargo-deny · #43 tree ops UI (also replaces the App.vue no-op handlers from #24, enables the create buttons and the switcher's Create-note row) · #49 read/write off main thread · #91 front end adopts `BootInfo.vault_id` / `vault://opened` · #92 render frontmatter keys and the search "approximate" marker.

## Follow-ups filed this run

#61 remaining 5 PRF-02 scenarios + CRLF/BOM + real IPC · #68 TreeSession replay on reload · #69 background quick_check · #75 48 px scroll-margin token · #77 Windows CI run · #80 vault-switch id handling in the front end · #82 editor-side 400 ms destination-block flash on backlink navigate · #83 tabs-dependent notice behaviour · #84 Compare diff view (per-hunk, ↓/↑ between ranges, assertive live region, virtualise) · #85 Mod-Shift-K/O/D notice bindings · #93 ⌘R rebind editor + `set_keybinding` + conflict UI (NFR-USE-01) · NFR-PRF-06 keystroke-to-paint browser bench · #26 follow-ups: PDF annotation links, find, changed-on-disk restart (`PDFDataRangeTransport` + `readAssetRange` fallback), cMaps/fonts, `AssetInfo` pixel dimensions, FileTree reveal-by-path.

## Budget numbers (20k vault unless stated)

| Budget | Figure | Status |
|---|---|---|
| NFR-PRF-03 hub-note save | p95 23.3 ms (attested quiet, pre-spacing — **unsourced**: the run output was never committed, the figure first appears in e6f6738); 42.0 / 44.5 ms loaded post-spacing, NOT attested (#62 open; 0 checkpoints on the save path) | PASS claim rests on an unsourced figure; #62's quiet run replaces it |
| NFR-PRF-06 switcher keystroke 3 (`Switcher::query`, 20 ms allocation of 50 ms) | 20k: default 12–13 ms, heading 16–18 ms (loaded); 50k: 46–48 ms (reported, not gated; PRD v1.9 finding) | CI gates 50 ms per mode; allocation findings printed |
| FR-VLT-04 external change visible | single note p95 190 ms, max 454 ms of 500 ms (loaded); batch-of-100 p95 723 ms reported | PASS (gated in CI at p95, PRD v1.9) |
| NFR-PRF-04 full rebuild | 7.9–10.8 s of 30 s | PASS |
| NFR-PRF-02 keystrokes | Chromium 153 (CI dry run): start/middle PASS, end-of-note INDETERMINATE (97.8 / 97.6%); positive-strict control exactly at the 99% bar; 153 and 141 figures not comparable (§9.3) | OPEN (reference machine); re-baseline to architect/product-analyst |
| catch-up on unchanged index | 16–22 ms; sync-tool worst case 294 ms | reported |
| R5 attachments read | p95 2.3 ms of 20 ms | PASS |

## Open risks

- **No Windows or macOS build in this environment.** #19's Windows paths, #72's UNC handling and #34/#35's name rules are verified by reading only. A Windows CI run is owed before any release claim.
- **NFR-PRF-02 is not gateable headless.** The reference-machine run (WebView2) must test the editor with and without scroll margins.
- **RISK-013 / F-384:** CI still produces no jobs; #60 cannot gate until it does.
- **Shared target directory:** a TaskCompleted hook or any second cargo in the integration checkout relinks test binaries under the lead's gate (SIGBUS, F-234's shape). The lead now completes tasks only while no gate runs. The three load-sensitive tests are deterministic since #67.
- **Disk:** the session allowance overflowed once with three concurrent Rust builds; `~/.cargo/config.toml` sets incremental=false and debug=0, and each agent uses a uniquely named target dir.

## Process notes

- Teammates cannot message each other or edit the task list; the lead relays and records. Agents read their spec from the brief.
- A container reset wipes worktrees, the session task list and the cargo config; branches on origin are the record. Never remove a worktree whose agent may still be resumed.
- Merged `task/p1-*` branches remain on origin because branch deletion is denied to the lead; the owner may delete them (`git push --delete origin <branch>`); all are contained in `main`. `task/p1-79-contract-gaps` and `-79b` were re-authored as `-79c` because the architect agent's own instructions set a different commit trailer; the owner should decide whether to align `.claude/agents/architect.md`.
- Teammates without a shell (product-analyst) hand edits back uncommitted; the lead commits them verbatim after doclint.
