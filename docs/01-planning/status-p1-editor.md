---
project: Serat
artifact: P1 status
phase: 01-planning
owner: lead
status: in progress — 2026-11-23 — gate NOT reached; #68 tree replay, #69 integrity check after an unclean open, #157 its strip fact and #158 plain-words rebuild reasons landed; every branch on origin is merged or superseded: the 2026-11-12/13 review batch, open/switch vault from inside Serat (#151), the SIGBUS fix on both sides (#153 fixtures, #154 index writer lock) and the held-lock wait (#155); owner actions: delete listed branches, CI runners (F-384), week-in-vault
---

# P1 — An editor you could live in — Status

**Gate (charter §Milestones):** owner opens their own vault in Serat for a week without needing Obsidian. Protocol: `docs/05-testing/p1-gate-protocol.md` (merged).

**Where P1 stands.** On `main`: the Rust core and serat-app plumbing (tree, search, backlinks, patch-set saves, assets and sidecars, switcher index, always-mtime catch-up with the window stamped before the watcher is armed, WAL checkpointer off the save path, asset scheme with the size-mtime validator, open/close/switch with the editor's bounded flush, `BootInfo.vault_id` and `vault://opened`), the editor with external-change reconciliation, wikilink completion, block widgets whose three reveal defects are fixed (#101, #102, #104, #109), the file tree / backlinks / search panes with create note / create folder and a keyboard-complete context menu on every platform, the quick switcher and command palette on one registry, the PDF and image viewers with the contained range transport that restarts on a changed file, backlink frontmatter keys and the search "approximate" marker, CI budget jobs for FR-VLT-01, FR-VLT-04 and NFR-PRF-06, the report-only NFR-PRF-02 job, cargo-deny, and PRD v1.9. Since the last refresh, also on `main`: the no-network proofs in CI (#20), the detached index-writer registry (#96), rusqlite 0.41 / SQLite 3.54.1 (#112), the save-reaches-index test (#111), `tree.insertLink` as a registered command (#114), test cases for every P1 ID with the hostile-suite map and the ipc-types drift job (#27), cargo-deny (#113) and npm-deny (#119) in the CI gate, the rebuilt webview bundle (#116), the SIGBUS root cause and tmpfs fixtures (#105b, #124), the review rulings (#115: CSP `form-action`/`base-uri 'none'`, ADR-0017 item 6, `OpeningReason`), PRD v1.10 (NFR-SEC-07, US-42) and v1.11 (FR-VLT-12/13, A-09, US-43), the refused-names spec (#125), plain-UTF-8 app writes (#121), two deterministic ordering tests (#122, #129), reads and writes off the main thread (#49), the gate-protocol no-ID rule (#140), ADR-0018 online-only files (#134, P2), the `OpeningReason` publish and clear (#117 serat-app half) and refused names reported with a count (#35). Since 2026-11-13 (owner: "review with existing reviewers and merge"), also on `main`: the refused-names count button and panel (#126), the Opening-reason status strip (#117b), vitest 5 (#128) and one vitest worker (#131), the derived passthrough skip-set (#132) and one chord canonicaliser (#135), pdf.js cMaps and standard fonts with `connect-src 'self'` and the T-02 row (#127, #148), the contentless-body and stylesheet allowlist tests (#137), the conflict-marker CI check (#138), the Obsidian link corpus (#139) and the code-span fix (#145), the callout fold marker (#146), backlinks and settings reads off the workers (#133), the refusal throttle (#144), tree first screen in the cold-start trace (#141), the ADR-0018 locality amendment (#150) and its P1 pieces (#143), Windows CI preparation (#77), the remaining NFR-PRF-02 scenarios and the real-IPC bridge (#61), and the bundle-drift job with a current bundle (#136). Then: open or switch the vault from inside Serat with a native picker, `vault.open` and a "No vault open" state, reopening the last vault at launch (#151, owner decision); the fixture-side SIGBUS fix (#153); one index writer per vault enforced by `.serat/index.lock` across processes (#154) with A-02 recording its local-filesystem limit; and a held lock shown as a reasoned wait with a 2 s retry for 60 s instead of a dead end (#155). No branch on origin is unmerged. Not started: the week-in-vault report; the reference-machine NFR-PRF-02 run; #62; #147 (needs runners).

`main` == integration branch `claude/great-mccarthy-jywddn` == **7dfe677**. Every merge goes through the full gate first: `cargo test --workspace`, `cargo clippy --workspace --all-targets -D warnings`, `cargo fmt --check`, `cargo check -p serat-app --release`, doclint, ts-rs drift, `npm test` and `vue-tsc` in `app/`. Index schema version on main: **12** (`links.field_key`, #86). Toolchain: pinned Rust 1.92; Chromium 153 for the latency harness; pdfjs-dist 6.5.438.

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
- **§3.4 vault id (#79, #90, #103, #91):** `BootInfo.vault_id` at launch, `vault://opened` after every later open; the failed-open window is bounded, not absent — after a `choose_vault` Err the front end drops every event carrying the announced id; the index-status fallback is restricted to launch.
- **ADR-0017 Amendment 1 (#94) and the granularity clause (#110):** pdf.js 6 has no eval path, the CSP is the control; `PDFDataRangeTransport` over `read_asset_range` everywhere; `AssetInfo.etag` + `if_match` → `IpcError::AssetChanged` (#98, #108); the validator misses a same-size rewrite within the kernel's coarse-clock tick before Linux 6.13 and an mtime-preserving writer; the inode would close the rename cases only. cMaps/standard fonts are a recorded gap.
- **§9.3 one registry (#25, #43, #39):** the palette, switcher, tree menu and ⌘N/⌘⇧N all go through `CommandRegistry`; the tree menu's Insert-link chord is still hard-coded (follow-up owed). The native webview context menu is suppressed outside editable text on every platform (T-07); the Rust-side suppression covers Linux release builds only.
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
| 99 | blockWidgets reuse test asserts by identity, no wall time | approved |
| 25 | quick switcher + command palette on the one `CommandRegistry`; palette is FR-NAV-06 | approved after 2 rounds |
| 94/95 | ADR-0017 Amendment 1 (pdf.js 6); NFR-PRF-02 Chromium 153 readings; NFR-PRF-03 post-spacing status | approved |
| 86 (+87, 97) | `links.field_key`, INDEX_VERSION 12, `Backlink.frontmatter_keys`; serat-parse empty-frontmatter test | approved after 2 rounds (+ lead rustfmt fix-up) |
| 101 | block whose first/last character held the cursor is re-rendered when it leaves | approved |
| 26 | PDF and image viewers, contained range transport, same-origin worker, CSP unchanged | approved after 3 rounds |
| 92 | backlink frontmatter keys and the search row's "approximate" marker | approved |
| 88 (+89) | search hits carry mtime/size; serat-app stats ≤50 hits per page, 50 ms deadline, `changed_since_indexed` | approved after 2 rounds |
| 102 | range selections spanning a block re-examine only the range heads | approved |
| 98 | `AssetInfo.etag`, `if_match`, `AssetChanged` before and during a read | approved after 2 rounds |
| 108 | PDF transport sends `if_match`, restarts on `asset_changed` with the 4 s notice | approved |
| 90 | `BootInfo.vault_id`, `vault://opened`, retired publisher, bounded join on a failed open | approved after 3 rounds |
| 103 | §3.4 pre-Err window qualified: drop every event with the announced id | approved |
| 91 | front end adopts `BootInfo.vault_id` / `vault://opened`; guarded adopt actions | approved after 2 rounds |
| 110 | ADR-0017 granularity clause (Linux < 6.13 coarse clock; inode limit); #96 re-scoped | approved; lead resolved a doc conflict with #98's wording |
| 43 | create note / create folder from tree, palette, switcher; inline naming prompt; context menu | approved after 3 rounds |
| 41 | cargo-deny: 14-licence allow-list, crates.io only, nine reasoned unmaintained ignores; `deny` job not yet in `gate.needs` | approved after 2 rounds |
| 39 | tree context menu on every platform (mac Ctrl-click, Menu key, ⇧F10); native menu policy outside editable text | approved |
| 104 | document-change path re-examines both sides and the changed span; sweeps split per case | approved after 4 rounds |
| 109 | `reliableParsedTo`: partial reparse territory re-examined when the parse completes | approved after 2 rounds |
| 107 | two load-sensitive tests pinned by counts; `walk_cost.rs` deleted; clippy forbids uncounted `canonicalize` in serat-vault | approved |
| 112 | rusqlite 0.41, libsqlite3-sys 0.39.1, bundled SQLite 3.54.1; real fallback when `wal_hook` fails | approved after 2 rounds |
| 111 | save-reaches-index test holds the watcher instead of racing it | approved; re-merged after `OpenHooks` grew |
| 20 | no outbound network: `cargo tree --offline`, deny-list, bundle scans, INERT counts, pdf.js pin; `NODE_ENV=production` | approved after 3 rounds |
| 96 | second index writer refused while a detached actor holds `index.db`; actor-started latch, rescan backstop, keyed by `FileIdentity` | approved after 4 rounds |
| 114 | `tree.insertLink` registered; palette greys unavailable commands; reserved chords; global-chord collisions | approved after 3 rounds |
| 27 (+113) | test cases for every P1 ID, hostile-suite map, two real-note corpus tests, ipc-types drift job; cargo-deny in `gate.needs` | approved |
| 105b | SIGBUS root cause: page-in failure of a fixture's `index.db-shm` on the thin disk under a concurrent build in the same target dir | approved |
| 116 | committed webview bundle rebuilt in production mode (stale again since; rebuilt at integration after the vue set lands) | approved |
| 115b (+118) | CSP `form-action 'none'; base-uri 'none'` (ADR-0017 Amendment 2); item 6 corrected to `connect-src 'self'` when cMaps ship; `IndexState::Opening { reason }`; `FileIdentity` on ReFS; PRD v1.10 NFR-SEC-07 + US-42 | approved after 2 rounds |
| 125 (+follow-up) | refused-names spec: status-strip count button, anchored non-modal panel; background re-fetch failure keeps the old list | approved |
| 122 | `vault://opened` ordering test waits for B's `index://changed` | approved |
| 120b (+123) | binary-viewers.md records the cMaps/fonts gap; ADR-0007 records the ipc-types job | approved |
| 130b | ADR-0007 generated-file count; §9.3 command inventory | approved |
| 119 | npm audit + licence allow-list + registry-only sources for `app/` (NFR-SEC-07 npm half); SPDX parser, stale-exception check | approved after 4 rounds |
| 121 | every file serat-app writes itself is plain UTF-8 Obsidian accepts (NFR-CMP-02) | approved |
| 124 | serat-app fixtures on `/dev/shm` with a Drop guard; 0 SIGBUS in 15 relink-under-run rounds | approved |
| 129 | bench-flag walk-complete test awaits `Rebuilding` instead of sampling it | approved |
| 49 | vault bytes, settings and folder writers on the blocking pool; `offload::blocking()` | approved after 2 rounds |
| 140b | gate protocol: a reach mapping to no PRD ID counts as missing P1 | approved |
| 134b | ADR-0018: online-only files are listed and only an explicit open downloads one (P2 Must; P1 carries RISK-015, vault is local only by owner decision) | approved after 3 rounds |
| 142c | PRD v1.11: FR-VLT-12 (Must, P2), FR-VLT-13 (Won't), A-09, US-43; architecture accounting | doclint green |
| 117 | `OpeningReason::previous_index_still_closing` published on a deferred re-open, cleared before the open check; §3.4 amended | approved after 2 rounds |
| 35 | refused names: capped `RefusalLog` with `on_change`, count on `IndexStatus`, `vault.refused-names`, re-emit only when the count moves; `test-hooks` feature with release assert | approved after 3 rounds |
| docs | status and task list refreshed at 3ff7806 | doclint green |
| 128 | vitest 5.1.0; tinypool critical advisories gone; one jsdom `execCommand` shim | approved after 2 rounds |
| 126 | refused-names count button and anchored non-modal panel; focus never on body; leaving panel inert; global `.sr-only` | approved after 4 rounds |
| 132 | passthrough skip-set derived from the named commands' bindings in canonical chord form | approved after 2 rounds (+ lead §9.3 conflict union) |
| 135 | one chord canonicaliser for `collidesWith`, `buildKeymap`, `chordMatches`, skip-set; `fallthrough` refused outside `editor.cm*` | approved after 2 rounds |
| 150b | ADR-0018 amended to the shipped locality API; `None` on Linux ruled | doclint green |
| 143 | `serat_core::locality` predicates; a failed read is `Io`, never zero bytes | approved after 2 rounds |
| 138 | `check-conflict-markers` doclint binary + CI job; the diff3 base marker covered | approved after 2 rounds |
| 127 (+148c) | cMaps and standard fonts ship under stable names; `connect-src 'self'` pinned; T-02 row rewritten | approved after 2 rounds |
| 77 | Windows CI prep round 2: UNC assertions, NTFS names, `SERAT_WINDOWS_STRICT`, 17 runner-only assumptions | approved after 2 rounds (by reading and cross-check) |
| 117b | Opening-reason strip: snapshot announcements, `aria-describedby`, no layout shift; seeds a reasoned Opening from `VaultOpened.index` | approved after 3 rounds |
| 137 | shipped body is the mount point; stylesheet allowlist with eight standing mutants | approved after 2 rounds |
| 133 | backlinks and `get_ui_settings` on the blocking pool; latch-based tests | approved after 2 rounds |
| 144 | refusal re-emit through the throttle with a trailing flush; emit lock prevents reordering | approved after 2 rounds |
| 131 | `maxWorkers: 1` on vitest 5 (poolOptions removed in v4; RPC timeout gone) | approved |
| 139 | Obsidian link corpus pins serat-parse and serat-index; callout widget test | approved after 2 rounds |
| 145 | tags and links inside inline code spans not extracted; merged excluded ranges; prf04 unchanged within noise | approved after 2 rounds |
| 141 | `tree_first_screen` term of the trace; `parse_requiring_tree` in the budget; `root_sent` stored before the sink | approved after 2 rounds |
| 61 | five remaining NFR-PRF-02 scenarios, CRLF/BOM, real-IPC bridge example (token, Host, origin, temp-root checks; allow-listed in the no-network scan); figures informational | approved after 2 rounds |
| 146 | callout `]-`/`]+` is a fold state, not title text | approved |
| 136 | `frontend-bundle` CI job; bundle rebuilt on c20c211 with cmaps/fonts | approved |
| docs | status and task list refreshed at c20c211 | doclint green |
| 153 | fixtures close each `AppState` once its index is Ready; `TestDir::drop` retries until the path stays absent; "#105b closed" retracted with the real mechanism | approved after 2 rounds |
| 151 | `choose_vault` (native picker from Rust, no dialog grant to the webview), `vault.open`, "No vault open" state, tree reopened from the command's answer, switch refused with unsavable text, editor frozen until adopt, `last_vault` at launch | approved after 3 rounds |
| 154b | A-02: the index lock is guaranteed only on a local filesystem (architect-drafted, lead-applied; PA ratifies in #156) | doclint green |
| 154 | `IndexLock` on `.serat/index.lock` held for the writer's life; `HeldByAnotherProcess`; deterministic child-process refusal test; NFS/SMB/Windows limits documented | approved after 2 rounds |
| 155 | `OpeningReason::IndexHeldByAnotherProcess`: 2 s retry for 60 s with a cancellable open, ratified strip copy, transient plain `Opening` not announced | approved (five mutants killed) |
| docs | status and task list refreshed at ceb4048 | doclint green |
| 68 | `TreeSession::replay_to` + `close`; a reloaded webview rejoins the live session with no second walk; poisoned sessions re-walk per ADR-0015 | approved after 2 rounds (rust half by code-reviewer-35, full branch by code-reviewer-49) |
| 69 | `serat_index::quick_check` (957 ms on 20k beside a live writer); serat-app runs it once after an unclean open, writer idle 2 s, index Ready, never with an in-process rebuild; `IndexStatus.index_check_problems` (additive) and a Corrupt rebuild | approved after 2 rounds |
| docs | status and task list refreshed at 8c6d21b | doclint green |
| 157 | strip shows "Index check found problems" beside a repairing rebuild, count in the tooltip, one combined announcement, the ending Ready/Unavailable announced; never survives a switch or failed open | approved after 2 rounds (copy ratified by architect) |
| 158 (+158b) | one plain phrase per `RebuildReason` with tooltips (architect's table); one announcement rule for every rebuild; a contract member without a copy row fails vue-tsc; "1 note" pluralised | approved after 2 rounds |

## Open branches

None. Every `task/*` branch on origin is contained in `main` (37) or superseded by a re-authored or replacement branch (17); the lists for deletion are in the lead's scratchpad.

## Not started (P1 scope)

The week-in-vault report · the reference-machine NFR-PRF-02 run · #62 attested hub-save run · #147 quiet-machine NFR-PRF-02 scenarios (needs runners) · #145 parser: tags inside inline code spans (after #139) · #68, #69, #75 carried.

## Follow-ups filed this run

#61 remaining 5 PRF-02 scenarios + CRLF/BOM + real IPC · #68 TreeSession replay on reload · #69 background quick_check · #75 48 px scroll-margin token · #77 Windows CI run · #80 vault-switch id handling in the front end · #82 editor-side 400 ms destination-block flash on backlink navigate · #83 tabs-dependent notice behaviour · #84 Compare diff view (per-hunk, ↓/↑ between ranges, assertive live region, virtualise) · #85 Mod-Shift-K/O/D notice bindings · #93 ⌘R rebind editor + `set_keybinding` + conflict UI (NFR-USE-01) · #100 · #105 SIGBUS in the serat-app lib test binary under a full workspace run (not reproduced in isolation; hypothesis: page-cache pressure from the budget benches that run before it) · register `tree.insertLink` so the tree menu's chord is not hard-coded (#39 review) · a reason shown for a long `Opening` while a detached writer finishes (#96 review) · CSP `form-action`/`base-uri 'none'` and the ADR-0017 item 6 `connect-src 'self'` inconsistency (architect, #20 review) · #37 platform follow-ups: native-menu suppression on WebView2 and WKWebView · the committed `crates/serat-app/webview` build is stale (no pdf chunk) · NFR-PRF-06 keystroke-to-paint browser bench · #26 follow-ups: PDF annotation links, find, changed-on-disk restart (`PDFDataRangeTransport` + `readAssetRange` fallback), cMaps/fonts, `AssetInfo` pixel dimensions, FileTree reveal-by-path.

## Follow-ups filed since 2026-11-10

#131 vitest worker RPC timeout · #133 remaining commands off the workers · #134/#143 online-only files (ADR-0018) · #135 chord canonicaliser · #136 bundle-drift job · #137/#138/#139 salvaged P0 branches · #140 no-ID reach · #141 tree first screen in the trace · #142 PRD v1.11 · #144 refusal throttle · #145 tags in code spans · #146 callout fold marker · #147 quiet NFR-PRF-02 re-run · #148 T-02 wording · #149 third-party notices for the shipped cMaps and fonts (release) · #150 ADR-0018 locality API · #151 open or switch the vault from inside Serat (owner, 2026-11-13) · #152 collapsed-callout glyph and `aria-expanded` (frontend-designer) · #153 fixture directories leaked in /dev/shm · #154 two index writers on one `index.db` fault each other · #155 a held index lock is a wait, not a terminal Unavailable · #156 product-analyst ratifies the A-02 sentence · #157 strip renders `index_check_problems` (from the #69 review) · #158 per-reason rebuild copy (from the #157 ratification; both merged). Findings recorded without a task: table-cell and split-preview typing are the weakest NFR-PRF-02 scenarios under load; bridge-side `save_patch` p95 723 ms under load (both re-measured by #147 first).

## Budget numbers (20k vault unless stated)

| Budget | Figure | Status |
|---|---|---|
| NFR-PRF-03 hub-note save | p95 23.3 ms (attested quiet, pre-spacing — **unsourced**: the run output was never committed, the figure first appears in e6f6738); 42.0 / 44.5 ms loaded post-spacing, NOT attested (#62 open; 0 checkpoints on the save path) | PASS claim rests on an unsourced figure; #62's quiet run replaces it |
| NFR-PRF-06 switcher keystroke 3 (`Switcher::query`, 20 ms allocation of 50 ms) | 20k: default 12–13 ms, heading 16–18 ms (loaded); 50k: 46–48 ms (reported, not gated; PRD v1.9 finding) | CI gates 50 ms per mode; allocation findings printed |
| FR-VLT-04 external change visible | single note p95 190 ms, max 454 ms of 500 ms (loaded); batch-of-100 p95 723 ms reported | PASS (gated in CI at p95, PRD v1.9) |
| NFR-PRF-04 full rebuild | 7.9–10.8 s of 30 s; 14.8 s median on #112's rusqlite 0.41 build under heavy load (indicative only; quiet A/B owed for C-06) | PASS |
| NFR-PRF-02 keystrokes | Chromium 153 (CI dry run): start/middle PASS, end-of-note INDETERMINATE (97.8 / 97.6%); positive-strict control exactly at the 99% bar; 153 and 141 figures not comparable (§9.3) | OPEN (reference machine); re-baseline to architect/product-analyst |
| catch-up on unchanged index | 16–22 ms; sync-tool worst case 294 ms | reported |
| R5 attachments read | p95 2.3 ms of 20 ms | PASS |
| R2 backlinks / R3 tags (20k, #112 loaded run) | p50 8.0 ms / 8.7 ms of 20 ms expectation | within |
| search-hit staleness stat (#88) | ≤50 hits per page under a 50 ms deadline; failed stat → `Some(true)` | reported |

## Open risks

- **No Windows or macOS build in this environment.** #19's Windows paths, #72's UNC handling and #34/#35's name rules are verified by reading only. A Windows CI run is owed before any release claim.
- **NFR-PRF-02 is not gateable headless.** The reference-machine run (WebView2) must test the editor with and without scroll margins.
- **RISK-013 / F-384:** CI still produces no jobs; #60 cannot gate until it does.
- **Shared target directory:** a TaskCompleted hook or any second cargo in the integration checkout relinks test binaries under the lead's gate (SIGBUS, F-234's shape). The lead now completes tasks only while no gate runs. The three load-sensitive tests are deterministic since #67.
- **RUSTSEC-2026-0459** closed by #112 (libsqlite3-sys 0.39.1); the `deny` job is in the gate (#113) and green.
- **RISK-015 (ADR-0018):** on a cloud-synced vault a read can hydrate an online-only file silently; the P1 gate vault is local only (owner), the locality predicate ships with #143, the gate is P2 (FR-VLT-12).
- **Committed webview bundle:** current at c20c211 (#136 rebuilt it with the cmaps/fonts); the `frontend-bundle` job now fails any `app/` change without its rebuild, so #151 carries its own rebuild commit at merge.
- **SIGBUS (#105, reopened as #153/#154):** the "page-in failure on the thin disk" conclusion was wrong. The fault reproduces on tmpfs: a test opened one index-less vault in several `AppState`s without closing, so several live index actors rebuilt the same `index.db`; `rebuild` unlinks `index.db`/`-wal`/`-shm` and recreates them, two database inodes share one `-shm`, the second connection (fcntl locks are per process) takes the exclusive DMS lock and truncates the mapping the first has, and `memset` in `walIndexAppend` faults. Both sides are merged: #153 closes between opens and retries the Drop guard; #154 takes `.serat/index.lock` before any rebuild and holds it for the writer's life, refusing a second writer in another process or this one; #155 turns the refusal into a reasoned wait. Residual: the lock is only guaranteed on a local filesystem (A-02), and a deleted `.serat/` re-admits a second writer.
- **Disk:** the session allowance overflowed once with three concurrent Rust builds; `~/.cargo/config.toml` sets incremental=false and debug=0, and each agent uses a uniquely named target dir.

## Process notes

- Teammates cannot message each other or edit the task list; the lead relays and records. Agents read their spec from the brief.
- A container reset wipes worktrees, the session task list and the cargo config; branches on origin are the record. Never remove a worktree whose agent may still be resumed.
- Merged `task/p1-*` branches remain on origin because branch deletion is denied to the lead; the owner may delete them (`git push --delete origin <branch>`); all are contained in `main`. `task/p1-79-contract-gaps` and `-79b` were re-authored as `-79c` because the architect agent's own instructions set a different commit trailer; the owner should decide whether to align `.claude/agents/architect.md`.
- The gate script merges, verifies and pushes in one run; on a failed verify the merge stays on the local checkout and is dropped with `git reset --keep origin/<integration>` (the only reset the environment allows the lead). A verify-only run (`gate3.sh -`) re-checks a merge the lead resolved by hand.
- Teammates without a shell (product-analyst) hand edits back uncommitted; the lead commits them verbatim after doclint.
