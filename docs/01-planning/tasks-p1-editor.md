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
| 9 | `[P1/test] Budget jobs for FR-VLT-01 tree ≤2 s and FR-VLT-04 ≤500 ms` | test-engineer | 2 | **merged** 9ce1c6c (reconciled as task/p1-09-budget-jobs: p1-tree-path, p1-external-change, p1-switcher jobs with --assert at 20k; old branch obsolete)4's p1-external-change bench |
| 10 | `[P1/test] Budget jobs for the quick switcher (FR-NAV-01, NFR-PRF-06)` | test-engineer | 3 | **merged** 9ce1c6c (with #9; switcher gated at the binding 50 ms per mode, 20 ms allocation printed as a finding; old branch obsolete)'s p1-switcher bench |
| 11 | `[P1/test] NFR-PRF-02 editor keystroke-latency harness` | test-engineer | 5 | **merged** 6521013 (3/8 scenarios; #60 CI, #61 scenarios follow) |
| 12 | `[P1/test] Obsidian-compatibility corpus and write fault-injection suite` | test-engineer | 5 | **merged** as #139 (28907eb); old branch superseded |
| 13 | `[P1/impl] FR-VLT-01 tree path + F-12 walk fix (+F-33/F-34)` | rust-core-dev | 2, 9 | **merged** 1e15a3d |
| 14 | `[P1/impl] Ignore file and external-change indexing` | rust-core-dev | 9 | **merged** db877e5 (+#58; INDEX_VERSION 10) |
| 15 | `[P1/impl] Search, backlinks and frontmatter read paths` | rust-core-dev | 4 | **merged** |
| 16 | `[P1/impl] Quick-switcher index` | rust-core-dev | 3, 10 | **merged** 81d7ef9 |
| 17 | `[P1/impl] Index assets and associate sidecars` | rust-core-dev | 6 | **merged** (approved at dbfa2b5) |
| 18 | `[P1/impl] P1 IPC commands/events and generated TS types` | tauri-app-dev | 7, 13–17 | **merged** rounds 1–3; last 61c3054 (24b4073). `choose_vault` landed with #151; `resolve_links` and `note_anchors` landed with #181 (no longer blocked) |
| 19 | `[P1/impl] Contained asset serving + .serat/ gitignore offer` | tauri-app-dev | 6, 7 | **merged** fd7f9c6 (Windows by reading only) |
| 20 | `[P1/test] Verify no outbound network requests` | tauri-app-dev | 18 | **merged** 01d46a3 (3 rounds) |
| 21 | `[P1/impl] Hybrid live-preview editor + split fallback` | vue-frontend-dev | 8, 11 | **merged** (via #42 lineage) |
| 22 | `[P1/impl] Wikilink autocompletion and embeds` | vue-frontend-dev | 18, 21 | **merged** d6f32da (completion guards, app://closing → savePatch → flush_ack; follow-ups #82–#85) |
| 23 | `[P1/impl] Reconcile external edits into the open note` | vue-frontend-dev | 12, 18, 21 | **merged** d6f32da (reconcile, Compare view, notice states; follow-ups #83/#84/#85) exist |
| 24 | `[P1/impl] File tree, backlinks pane, search panel` | vue-frontend-dev | 8, 18 | **merged** 5695394 (three rounds; #43 enables create/rename; #79 fields pending) |
| 25 | `[P1/impl] Quick switcher and command palette` | vue-frontend-dev | 8, 18 | **merged** 6a608b8 (one registry per §9.3; palette is FR-NAV-06) |
| 26 | `[P1/impl] PDF and image viewers` | vue-frontend-dev | 8, 19 | **merged** c41d6cc (+#98 validator, #108 transport restart; cMaps/fonts gap recorded) |
| 27 | `[P1/test] Test cases for every P1 ID + hostile suite` | test-engineer | 12–17, 22–26 | **merged** a3f4b5f (carries #113) |
| 28 | `[P1/test] Gate protocol (early) and GO/NO-GO report (last)` | test-engineer | 20, 27 | protocol merged; report status half **merged** 06f2867 as #163; week-in-vault half blank |
| 30 | `[P1/design] Account for FR-NAV-08 in architecture.md so doclint passes` | architect | 1 | **merged** db1d967 |
| 31 | `[P1/test] Update and re-run the NFR-PRF-01 cold-start bench for ADR-0015's tree mark` | test-engineer | 2 | **merged** as #141; old branch superseded |
| 75 | `[P1/design] Ratify or re-token the editor's 48 px bottom scroll margin` (NFR-PRF-02, FR-EDT-01, US-35) | frontend-designer | 42 | **merged** 28ab56a (kept as named token exception) |
| 32 | `[P1/req] Amend FR-VLT-01 and NFR-PRF-02 to the owner-approved definitions` (PRD v1.6) | product-analyst | 2, 5 | **merged** 857af57 |
| 33 | `[P1/impl] Implement ADR-0016 patch-set application and edit-script in serat-vault` | rust-core-dev | 5 | **merged** (with #55) |
| 34 | `[P1/impl] Close the Windows drive-relative path escape in VaultPath and Vault::resolve` — **live security defect** (NFR-SEC-06, T-07, T-10). Scope: reject `:` in any segment; reject Windows device names and trailing dot/space; `starts_with(root)` after resolve; case-fold the ignored-dir check. ADR-0017 relies on all four. | rust-core-dev | — | **merged** |
| 35 | `[P1/impl] Report, never drop, vault files whose names Serat refuses` — Windows-only name rules apply on Windows only; refused names are reported with a count (FR-VLT-01, NFR-SEC-06) | rust-core-dev | 34 | **merged** 3ff7806 (3 rounds; throttle follow-up #144) |
| 36 | `[P1/design] Correct ADR-0017's false remote-origin control claim` — real controls: (a) top-level navigation guard, (b) sandbox CSP on scheme responses, (c) app CSP `frame-src 'none'; object-src 'none'` + sanitiser | architect | 6 | **merged** |
| 37 | `[P1/impl] Install the webview navigation guard, scope the opener to http/https/mailto, pin frame-src/object-src 'none', refuse downloads, disable release context menu` — implements controls (a) and (c) with tests (NFR-SEC-01, NFR-SEC-06, T-09, T-10) | tauri-app-dev | 5, 36 | **merged** (context-menu suppression is Linux-release only; WebView2/WKWebView follow-ups owed) |
| 39 | `[P1/impl] Context-menu platform conventions (Windows / macOS / Linux)` (FR-NAV-06, NFR-USE-01, T-07) | vue-frontend-dev | 43 | **merged** b42de75 |
| 41 | `[P1/test] cargo-deny: licences, advisories, duplicate crates, sources — in CI` (NFR-SEC-01, NFR-MNT-01) | test-engineer | — | **merged** bedf295; #113 adds the job to the gate after #112 |
| 43 | `[P1/impl] Tree operations UI: create note/folder, context menu` (FR-VLT-11, US-41; rename/move are P3 FR-VLT-09) | vue-frontend-dev | 24 | **merged** 9db3d06 |
| 49 | `[P1/impl] Read/write off the main thread in serat-app` (NFR-PRF-02, goal 2) | tauri-app-dev | 18 | **merged** c4590d9 (2 rounds; #133 carries the rest) |
| 96 | `[P1/impl] Refuse a second index writer while a detached actor holds the same index.db` (FR-VLT-04, NFR-REL-01, ADR-0015 §2) | tauri-app-dev | 90 | **merged** a05418d (4 rounds) |
| 112 | `[P1/impl] rusqlite 0.41 so libsqlite3-sys ≥ 0.39 (RUSTSEC-2026-0459, NFR-SEC-01)` | rust-core-dev | — | **merged** a126f7f |
| 113 | `[P1/test] Add the cargo-deny job to the CI gate` (NFR-SEC-01, NFR-MNT-01) | test-engineer | 41, 112 | **merged** a3f4b5f (with #27) |
| 77 | `[P1/test] Windows CI preparation: UNC/mapped-drive roots, NTFS names, asset scheme, OneDrive placeholders` (NFR-SEC-06, NFR-CMP-03) | test-engineer | 72 | **merged** 57c0790 (by reading and cross-check; runner owed, F-384) |
| 61 | `[P1/test] Remaining NFR-PRF-02 scenarios, CRLF/BOM, real-IPC bridge` | test-engineer | 11 | **merged** 1d646b5 (figures informational; #147) |
| 114 | `[P1/impl] Register tree.insertLink in the CommandRegistry` (FR-NAV-06, NFR-USE-01, §9.3) | vue-frontend-dev | 25, 43 | **merged** ff48c99 |
| 115 | `[P1/design] Review rulings: CSP form-action/base-uri, ADR-0017 item 6, Opening reason, FileIdentity on ReFS` | architect | 20, 96 | **merged** fdde549 (115b) |
| 116 | `[P1/impl] Rebuild the committed webview bundle` | tauri-app-dev | — | **merged** 7654393; drifted again, rebuilt at integration after the vue set |
| 117 | `[P1/impl] OpeningReason::previous_index_still_closing on a deferred re-open` (NFR-REL-01/02, FR-VLT-04) | tauri-app-dev + vue-frontend-dev | 115 | **merged** both halves (3f51229, faceb4f) |
| 118 | `[P1/req] NFR-SEC-07 dependency hygiene + US-42` (PRD v1.10) | product-analyst | 41 | **merged** fdde549 |
| 119 | `[P1/test] npm audit and licence job for app/` (NFR-SEC-07) | test-engineer | 118 | **merged** f4db5ec (4 rounds) |
| 120 | `[P1/design] Record the cMaps/fonts gap; ADR-0017 Amendment 2 review` | architect | 94 | **merged** e87a944 (120b, with #123) |
| 121 | `[P1/test] Every file serat-app writes itself is plain UTF-8` (NFR-CMP-02) | test-engineer | 19 | **merged** 092e897 |
| 122 | `[P1/test] vault://opened ordering test waits on index://changed` | test-engineer | 90 | **merged** 9635974 |
| 123 | `[P1/design] ADR-0007 records the ipc-types job` | architect | 27 | **merged** e87a944 |
| 124 | `[P1/test] serat-app fixtures on tmpfs with a Drop guard` (SIGBUS, #105b) | test-engineer | 105 | **merged** 0dc8574 |
| 125 | `[P1/design] Spec the refused-names count and list` | frontend-designer | 35 | **merged** 9b71715 + 3520e1d |
| 126 | `[P1/impl] Refused-names count button and panel in app/` (FR-VLT-01, NFR-SEC-06, NFR-USE-01) | vue-frontend-dev | 125, 35 | **merged** b840b5b (4 rounds) |
| 127 | `[P1/impl] Ship pdf.js cMaps and standard fonts; connect-src 'self'` (FR-BIN-02, US-19, NFR-SEC-01) | vue-frontend-dev + tauri-app-dev | 120 | **merged** 2b97a8e (with #148) |
| 128 | `[P1/impl] vitest 5 (tinypool advisories out of the dev tree)` (NFR-SEC-07, US-42) | vue-frontend-dev | 119 | **merged** 9a945fd |
| 129 | `[P1/test] Bench-flag walk-complete test awaits Rebuilding` | test-engineer | — | **merged** 6f0bb60 |
| 130 | `[P1/design] ADR-0007 count; §9.3 command inventory` | architect | 114 | **merged** bc3f01a (130b) |
| 131 | `[P1/test] vitest worker RPC timeout under load` | test-engineer | 128 | **merged** 982d0f1 (maxWorkers 1 on vitest 5) |
| 132 | `[P1/impl] seenKeys derived from the named commands' bindings` (NFR-USE-01) | vue-frontend-dev | 114 | **merged** (gate132) |
| 133 | `[P1/impl] Remaining serat-app commands off the Tokio workers` (NFR-PRF-02, goal 2) | tauri-app-dev | 49 | **merged** (gate133) |
| 134 | `[P1/design] ADR-0018 online-only (cloud placeholder) files` | architect | — | **merged** 8d934b2 (134b); P2 Must, P1 RISK-015 |
| 135 | `[P1/impl] One chord canonicaliser for the four call sites` (NFR-USE-01) | vue-frontend-dev | 132 | **merged** aba3e44 |
| 136 | `[P1/test] Bundle-drift CI job` (salvages task/110) | test-engineer | 116 | **merged** c20c211 (bundle current) |
| 137 | `[P1/test] Contentless page body tests` (salvages task/128) | test-engineer | 116 | **merged** 1e8cd47 |
| 138 | `[P1/test] Fail CI on a committed merge-conflict marker` (NFR-MNT-01) | test-engineer | — | **merged** (gate138) |
| 139 | `[P1/test] Obsidian link-syntax corpus` (NFR-CMP-01; resumes #12) | test-engineer | 12 | **merged** 28907eb |
| 140 | `[P1/test] Gate protocol: a no-ID reach counts as missing P1` (resumes #28b) | lead | 28 | **merged** 7ad1ac0 (140b) |
| 141 | `[P1/test] tree_first_screen in the NFR-PRF-01 trace` (resumes #31) | tauri-app-dev + test-engineer | 31 | **merged** (gate141) |
| 142 | `[P1/req] PRD v1.11: FR-VLT-12/13, A-09, US-43` | product-analyst + architect | 134 | **merged** 44d1a61 (142c) |
| 143 | `[P1/impl] serat_core::locality::is_local; a failed read is an error, never zero bytes` (ADR-0018 P1 pieces) | rust-core-dev | 134 | **merged** (gate143; ADR amended by #150) |
| 144 | `[P1/impl] Refused-names re_emit through the throttle with a trailing flush` (NFR-PRF-04) | tauri-app-dev | 35 | **merged** 4e72e08 |
| 145 | `[P1/impl] No Tag links for #word inside inline code spans` (NFR-CMP-01) | rust-core-dev | 139 | **merged** 20b2562 |
| 146 | `[P1/impl] Callout fold marker is a fold state, not title text` (NFR-CMP-01, C-04) | vue-frontend-dev | 139 | **merged** f88331f (glyph: #152) |
| 147 | `[P1/test] Quiet-machine NFR-PRF-02 scenarios re-run; triage table-cell/split-preview and save_patch` | test-engineer | 61, runners | merged (container re-run done 2026-11-24; reference-machine run owed) |
| 149 | `[P5/release] Third-party notices for every shipped asset: pdf.js, Adobe cMaps, Foxit, Liberation (GPL-2.0 with font exception), three OFL-1.1 families` (NFR-SEC-07) | release-engineer | 127 | **merged** 6fd6f4e |
| 150 | `[P1/design] ADR-0018 describes the shipped locality API` | architect | 143 | **merged** d1356b4 (150b) |
| 151 | `[P1/impl] Open or switch the vault from inside Serat: choose_vault, vault.open, last_vault at launch` (US-01, FR-VLT-01, FR-NAV-06; ADR-0012) | tauri-app-dev + vue-frontend-dev | — | **merged** 1ab859e (3 rounds) |
| 152 | `[P1/design] Collapsed-callout glyph and aria-expanded` | frontend-designer | 146 | **merged** 485c368 (+152b a7b1be6) |
| 159 | `[P1/impl] Build the collapsed-callout fold marker per the #152 spec` (FR-EDT-01/02, NFR-USE-01) | vue-frontend-dev | 152 | **merged** 0b50a7c |
| 160 | `[P1/design] Specify live-preview typography and export --serat-* tokens as CSS` (FR-EDT-02, US-02; C-01, C-07; ADR-0006) | frontend-designer | 159 | **merged** c32b48e (+ rulings in 1d7e69d) |
| 161 | `[P1/impl] Style the hybrid live preview (HighlightStyle, mono-source, --serat-* tokens)` (FR-EDT-01/02, US-02, NFR-PRF-01/02; C-04) | vue-frontend-dev | 160 | **merged** c32b48e |
| 162 | `[P1/impl] Bundle the three OFL font families locally and re-measure measure.editor` (FR-EDT-02, NFR-SEC-01, NFR-SEC-07, NFR-USE-01) | vue-frontend-dev + tauri-app-dev | 161 | **merged** 1d7e69d |
| 62 | `[P1/test] Attested post-spacing hub-save run` (NFR-PRF-03, NFR-PRF-08; ADR-0011) | test-engineer | 56 | **merged** e82eb01 (17.01 ms p95, attested 0) |
| 163 | `[P1/test] P1 GO/NO-GO report, status half` (charter gate, NFR-PRF-01..09, NFR-CMP-01/02/04) | test-engineer | 27, 28 | **merged** 06f2867 (3 rounds) |
| 164 | `[P1/test] budgets.yml builds serat-app before the NFR-PRF-01/03 jobs measure` (NFR-PRF-01/03/08; ADR-0011) | test-engineer | 60 | **merged** 5ac4dae |
| 165 | `[P1/impl] FR-VLT-04 Windows CI tail triage; report-only on hosted Windows` (FR-VLT-04, NFR-PRF-08; ADR-0019, ADR-0015) | rust-core-dev | 164 | **merged** 5e27541 (3 rounds) |
| 166 | `[P1/test] NFR-PRF-04 CI runner class / gate-eligibility rule` (NFR-PRF-04/08, C-06; ADR-0019) | test-engineer + architect | 164 | **merged** 13eec23 (with ADR-0019 as 166b) |
| 167 | `[P1/test] Windows re-runs of the report-only budget jobs when a runner is available` (NFR-PRF-04, FR-VLT-04; ADR-0019) | test-engineer | 165, 166, runners | **merged** 3e35ce2 (2 rounds) |
| 168 | `[P1/test] ubuntu-latest asserting jobs for NFR-PRF-04 and FR-VLT-04; two-column C-06 position in the gate report` (NFR-PRF-08; ADR-0019) | test-engineer | 166 | **merged** 410b378 (2 rounds) |
| 169 | `[P1/req] PRD v1.12: NFR-PRF-08, FR-VLT-04, NFR-PRF-03 note, open question and US-35 restated for ADR-0019` | product-analyst | 166 | **merged** 13eec23 |
| 170 | `[P1/impl] Contain the split-preview pane so typing does not pay for 10,000 preview elements` (NFR-PRF-02, US-01; ADR-0019, C-06) | vue-frontend-dev | 147 | **merged** a27de35 |
| 171 | `[P1/impl] Do not flatten the whole note on every keystroke for a preview that is not showing` (NFR-PRF-02, US-01; ADR-0019, C-06) | vue-frontend-dev | 147 | **merged** a27de35 |
| 172 | `[P1/test] Make the IPC bridge buildable in release so save_patch can be timed as shipped` (NFR-PRF-03, NFR-PRF-02, US-01; ADR-0019, ADR-0008) | tauri-app-dev + test-engineer | 147 | **merged** 70bec0a (2 rounds) |
| 173 | `[P1/design] Rule whether save_patch parses the note after the save returns, off the command thread` → ADR-0020 (NFR-PRF-03, FR-VLT-04, US-01; ADR-0008, ADR-0015, ADR-0016, ADR-0019) | architect | 172 | **merged** ecef8ec (2 rounds; re-authored branch) |
| 174 | `[P1/req] PRD v1.13: NFR-PRF-03 names the note class it binds; 5,000-line scenario reported, not gating` (NFR-PRF-03, US-01; ADR-0019) | product-analyst | 172 | **merged** 08f193f |
| 175 | `[P1/impl] Report each non-bulk committed batch from the index writer to an installed observer` (NFR-PRF-03, FR-VLT-04, US-01; ADR-0001, ADR-0020) | rust-core-dev | 173 | **merged** 9ba7cb5 (2 rounds) |
| 176 | `[P1/impl] Announce index://changed from the writer's commit observer only, and drop the slot lock before the parse` (NFR-PRF-03, FR-VLT-04, US-01; ADR-0016, ADR-0011, ADR-0020) | tauri-app-dev | 175 | **merged** e9b6576 (2 rounds) |
| 177 | `[P1/test] Linux desktop reference run (gaws-dev-05): RM-2 runbook row, report, gate-report columns` (NFR-PRF-01/03/04/08, US-35, C-06; ADR-0019) | test-engineer | owner run | **merged** 12542f1 (2 rounds) |
| 178 | `[P1/impl] Startup timeout prints the marks snapshot and every rejected paint evidence` (NFR-PRF-01/08; ADR-0014) | tauri-app-dev | 167 | **merged** fe85178 (2 rounds) |
| 179 | `[P1/test] WebView2 diagnostic dispatch of the NFR-PRF-01 jobs on windows-latest` (NFR-PRF-01/08, US-35, C-06; ADR-0019, ADR-0014) | test-engineer | 178 | **merged** 2409ca0 (1 round) |
| 180 | `[P1/test] Re-read FR-VLT-04 on windows-latest after the #167 harness diagnostics` (FR-VLT-04, NFR-PRF-08, US-35, C-02, C-06; ADR-0019, ADR-0015) | test-engineer | 167 | **merged** 96476ec (2 rounds) |
| 181 | `[P1/impl] Register the resolve_links and note_anchors commands the editor already invokes` (FR-EDT-03, NFR-CMP-01, US-03; ADR-0007, ADR-0010, ADR-0011, ADR-0020) | tauri-app-dev | 176 | **merged** 4003717 (2 rounds + ts-drift fixup) |
| 182 | `[P1/test] Hunt two one-off test failures seen 2026-11-24` (NFR-REL-01, NFR-MNT-01, US-07) | test-engineer | 175, 176 | **merged** a2d6884 (1 round) |
| 183 | `[P1/impl] External-change processing under a WAL checkpoint overlap` (FR-VLT-04, NFR-PRF-03; ADR-0001) | rust-core-dev | 180 | **merged** 2342453 |
| 184 | `[P1/impl] Nested heading subpaths: [[Note#A#B]] must require B under A` (FR-EDT-03, NFR-CMP-01, US-03; ADR-0010) | tauri-app-dev + rust-core-dev | 181 | **merged** 8bc0417 (INDEX_VERSION 13; 1 round) |
| 185 | `[P1/impl] Cache the tree-fallback link resolver on the tree generation` (FR-EDT-03, NFR-PRF-02, US-03; ADR-0011, ADR-0015) | tauri-app-dev | 181 | **merged** df543ba (1 round) |
| 186 | `[P1/impl] Retry the paint mark when the frame gate refuses equal requestAnimationFrame timestamps` (NFR-PRF-01/08, US-35; ADR-0014) | vue-frontend-dev | 179 | **merged** 83b616d (1 round; acceptance 1 of 3 hang-free runs) |
| 187 | `[P1/test] prf01-cold-start documents --skip-scale but its parser rejects the flag` (NFR-PRF-01/08, US-35; ADR-0014, ADR-0019) | test-engineer | 179 | **merged** 2337a32 (1 round) |
| 188 | `[P1/impl] Watcher: a delete event must not record a refused name` (FR-VLT-01, NFR-REL-01, US-07; ADR-0015) | rust-core-dev + architect | 182 | **merged** a0cbf0a (ruling 6e70650; 1 round each after a BLOCKING on the ruling) |
| 189 | `[P1/test] asset tests spawn mkfifo inside the serat-app lib test binary (fork-exec lock hazard)` (NFR-REL-01, NFR-MNT-01) | test-engineer | 182 | **merged** 697c457 (2 rounds) |
| 190 | `[P1/test] NFR-PRF-01 Serat term on windows-latest after #186: INDETERMINATE; decide the CI ceiling` → report-only on hosted Windows, ADR-0019 amendment (NFR-PRF-01/08, US-35, C-06; ADR-0014, ADR-0019) | test-engineer + architect | 186 | **merged** 5de8807 (2 rounds) |
| 191 | `[P1/test] ci.yml red on main: Windows lib tests do not compile, cargo-deny cookie, windows-gnu target` (NFR-REL-01, NFR-SEC-07, NFR-MNT-01, NFR-CMP-01) | test-engineer | — | **merged** 001b1ef (1 round) |
| 192 | `[P1/impl] Startup launches exit 2 after a frame-gate refusal (runs 431/432); 55 s no-window launch; prf03 INDETERMINATE` (NFR-PRF-01/03/08, US-35; ADR-0014, ADR-0019, ADR-0015) | tauri-app-dev | 186, 190 | **merged** 7204e4e (1 round) |
| 193 | `[P1/test] ci run 460: Windows-only search-slot test defect, url advisory, windows-gnu target step` (NFR-REL-01, NFR-SEC-07, NFR-MNT-01, NFR-CMP-01) | test-engineer | 191 | **merged** 0a4b3fe |
| 194 | `[P1/test] Promote the windows-gnu compile job to gate.needs in ci.yml` (NFR-CMP-01, NFR-MNT-01; ADR-0019; TC-172, TC-176) | test-engineer | 191, 193 | **merged** 405f39e |
| 195 | `[P1/test] NFR-PRF-01 scale term FAIL on run 437 was a mislabelled Serat-term reading; message fixed; spread finding recorded` (NFR-PRF-01, US-35; ADR-0019, ADR-0014; TC-178) | test-engineer | 187, 190 | **merged** 89d8a50 |
| 196 | `[P1/test] prf03 per-sample flags: checkpoint overlap, FTS5 merge, slow writer turn` (NFR-PRF-03, US-14; ADR-0019, ADR-0001, ADR-0008, ADR-0017; TC-175) | test-engineer | 183, 192 | **merged** b9b289b |
| 197 | `[P1/test] clippy --all-targets: compile-time PHASE_TIMING check, per-package serat-vault clippy step` (NFR-MNT-01; TC-177) | test-engineer | — | **merged** 894cfb1 |
| 198 | `[P1/design] Hosted run 439 closed #183's question with NO; Frame floor ruling (IDLE_FLOOR_FRAMES = 1,000)` (FR-VLT-04, NFR-PRF-03, NFR-REL-01; ADR-0001, ADR-0019, C-02; architecture §3.1.1 N8) | architect | 183, 196 | **merged** cd54a2e |
| 199 | `[P1/impl] Idle checkpoint frame floor in serat-index (IDLE_FLOOR_FRAMES = 1,000)` (FR-VLT-04, NFR-PRF-03, NFR-REL-01, C-02, C-06; ADR-0001, ADR-0019; architecture §3.1.1 N8 *Frame floor*, #198). A `Bounds` field; a below-floor idle wake disarms until the next commit (no timer spin). Unit tests: none below the floor, one at it, size bound and `close()` unaffected. Bench: the checkpointer line adds frames per idle run and floor skips; overlap tags say idle or size-bound. **Acceptance:** one hosted `budgets.yml` run with the FR-VLT-04 single overlap ratio ≤ 2x and no prf03 over-limit sample carrying the idle-checkpoint flag (one carrying a size-bound flag is reported to architect); `p1-save-tail` p95 on Linux within +10% of main's, read in the same session (and within that series' own batch-to-batch spread if wider). | rust-core-dev (+ test-engineer, bench tags) | 183, 196, 198 | **merged** 5cc377c (acceptance: run 447 failed the 2x criterion, withdrawn by #201) |
| 200 | `[P1/test] Windows analyze-stats test race: pin the 39+1 batch cut with CommitGate` (NFR-REL-01, NFR-MNT-01; ADR-0001, architecture §4.3; TC-180) | test-engineer | — | **merged** 28d302e |
| 201 | `[P1/design] Floor kept after run 447 (per-frame cost); ADR-0019 amendment 2026-11-26 excuses one checkpoint-overlapped prf03 outlier; Known gap 12; RISK-017 proposed` (FR-VLT-04, NFR-PRF-03, NFR-PRF-08, US-14; ADR-0001, ADR-0019, C-02, C-06) | architect | 199 | **merged** a4faf5e |
| 202 | `[P1/test] NFR-PRF-02 middle-caret placement waits on stable scroll geometry and retries to a deadline` (NFR-PRF-02, NFR-MNT-01; ADR-0019, architecture §9.3; TC-181) | test-engineer | — | **merged** 038ec03 |
| 203 | `[P1/test] Catch-up window test pins the window it asserts on; boundary test at caught_up_from_ms - WINDOW_MS` (NFR-REL-01, NFR-MNT-01, FR-VLT-04; ADR-0015 §2; TC-182) | test-engineer | — | **merged** 1db2230 |
| 205 | `[P1/test] prf03 determinate(): excuse one checkpoint-overlapped over-limit save` (NFR-PRF-03, NFR-PRF-08, C-06; ADR-0019 amendment 2026-11-26, ADR-0013; budget-harness.md, #201; TC-183). In `bench/budget`, prf03 only. Unit test on synthetic 1,000-sample series: one overlapped save over, class p95 within: PASS; unflagged, two over, or class p95 over: INDETERMINATE; overlapped class n=19: INDETERMINATE (its p95 is the outlier), n=20: PASS; no tags or another budget: plain rule. | test-engineer | 196, 199, 201 | **merged** 37975ce |
| 206 | `[P1/design] ADR-0019 amendment restated: n ≥ 20 is the operative condition, class p95 the guaranteed bound; windows_vault.rs no longer marked planned` (NFR-MNT-01, NFR-PRF-03, FR-VLT-12; ADR-0018, ADR-0019) | architect | 205 | **merged** 5252788 |
| 207 | `[P1/test] prf03 per-save phase timings on the bench exit line; runtime-armed recorder; TC-184; report item (a-5)` (NFR-PRF-03, NFR-PRF-08, C-06; ADR-0008, ADR-0017, ADR-0019, ADR-0020) | test-engineer | 205 | **merged** e1c25dc |
| 208 | `[P1/docs] Ratify RISK-017 (checkpoint flush contention on Windows); PRD v1.14 cross-references` (NFR-PRF-03, NFR-PRF-08; ADR-0019; RISK-013, RISK-017) | product-analyst | 201 | in review, branch task/p1-208-risk-017-ratify |
| 209 | `[P1/test] arm() source-scan test records forward-slash paths so it passes on windows-latest (ci run 478 red on e1c25dc); also makes the webview-reach guard check on Windows` (NFR-PRF-03, NFR-PRF-08; ADR-0017; TC-184) | test-engineer | 207 | **merged** 192af09 |
| 210 | `[P1/test] run_save_benchmark fixture waits for Ready under the 60 s liveness ceiling, not a 2 s deadline (ci run 480)` (NFR-PRF-03, NFR-MNT-01; ADR-0019) | test-engineer | 207 | **merged** c356d8a |
| 211 | `[P1/design] Architect rules report item (a-5) on hosted phase evidence (budgets 453/454/455): attributed class for the unflagged prf03 outlier; ADR-0019 amendment` (NFR-PRF-03, NFR-PRF-08, C-06; ADR-0008, ADR-0019) | architect | 207 | **merged** f39b490 |
| 212 | `[P1/req] Record the owner's RISK-016 decision: NFR-PRF-01 binds Windows/WebView2 in P1; Linux term reported, attribution owed; PRD v1.15` (NFR-PRF-01, NFR-PRF-08, US-35; ADR-0019, C-06) | product-analyst | 177 | **merged** fb2ecb3 |
| 213 | `[P1/test] prf03 vault-I/O excusal (Excusal::VaultIo), per-save fsync-count pin and flush source scan in serat-vault, saturated-tag note; TC-185` (NFR-PRF-03, NFR-PRF-08, C-06; ADR-0008, ADR-0017, ADR-0019, ADR-0020) | test-engineer | 211 | **merged** a96811e |
| 214 | `[P1/lead] Status refresh 15 and task list at a96811e` (NFR-MNT-01) | lead | 213 | in review, branch task/p1-214-status-15 |
| 215 | `[P1/req] RISK-017 wording after #213: the unflagged class is attributed to the vault's disk I/O, excused by the second amendment` (NFR-PRF-03, NFR-PRF-08; ADR-0019) | product-analyst | 213 | **merged** 93f4493 |
| 216 | `[P1/test] Report item (a-5) evidence after #213: runs 457 (would-be excused, near-boundary), 458, 459 (PASS, attribution unexercised)` (NFR-PRF-03, NFR-PRF-08, C-06; ADR-0019) | test-engineer | 213 | **merged** 5b5ce49 |
| 217 | `[P1/test] Hosted runs 460-463 recorded: #205 checkpoint excusal verified on windows-latest (461, 463); saturated-tag note seen; vault-I/O excusal still unexercised` (NFR-PRF-03, NFR-PRF-08, C-06; ADR-0019) | test-engineer | 216 | in review, branch task/p1-217-hosted-excusal-evidence |
| 218 | `[P1/test] gaws-dev-05 per-mark cold-start trace: Linux Serat term attributed to the webkit2gtk front-end shell (s_shell ≈ 885 ms median) and settle; Rust-side terms < 1 ms` (NFR-PRF-01, NFR-PRF-08, US-35; ADR-0014, ADR-0019) | test-engineer | 212 | in review, branch task/p1-218-prf01-terms-gaws-dev-05 |
| 219 | `[P2/design] Linux cold start: split the webkit2gtk front-end shell term (s_shell) and decide the RISK-016 P2 fix` (NFR-PRF-01, NFR-PRF-08, US-35; ADR-0014, ADR-0019; RISK-016, from #218) | architect | 218 | pending (P2) |
| 220 | `[P1/impl] External-change consumer stall: find the wait cycle or say none exists; stall line reports the consumer round phase, the writer turn and the checkpoint in flight` (FR-VLT-04, NFR-REL-01, NFR-PRF-08, US-35; ADR-0001, ADR-0015, ADR-0019; TC-159 (c); from runs 464/465) | rust-core-dev | 167 | **merged** e5b1c94 (b06e811, 45f46fc, d597fae, ba204f3; no wait cycle in the product) |
| 221 | `[P1/test] NFR-PRF-02 headless job: per-scenario timeout and partial artifact upload on cancel (if: always()); run 465 step 7 hung, cancelled at 60 min` (NFR-PRF-02, NFR-PRF-08, NFR-MNT-01; ADR-0019; next to #202) | test-engineer | 202 | in review (not gating): code supports every-scenario-timed-out (7 x 420 s + controls = 56 min); globalTimeout 40 min, scenario timeout 300 s, step timeout 45 min, progress lines; branch task/p1-221-headless-scenarios-hang; TC-181 (b) |
| 222 | `[P1/test] Record #220 in the docs: TC-159 (c), Triage 1 hypothesis-3 evidence for runs 464/465, corrected checkpointer claims, #221 note next to #202` (FR-VLT-04, NFR-PRF-03, NFR-PRF-08, C-06; ADR-0019; TC-159) | test-engineer | 220 | **merged** b1a2573 |
| 223 | `[P1/test] Record hosted runs 466/467: first hosted exercise of the #213 vault-I/O excusal, FR-VLT-04 run 466 tail, no stall line yet` (FR-VLT-04, NFR-PRF-03, NFR-PRF-08, C-06; ADR-0019; TC-159, TC-185) | test-engineer | 222 | in review, branch task/p1-223-hosted-466-467 |
| 153 | `[P1/test] Fixture directories leaked in /dev/shm; SIGBUS reproduces on tmpfs` | test-engineer | 124 | **merged** 6c98e69 (2 rounds) |
| 154 | `[P1/impl] Cross-process single-writer guard in serat-index (two writers on one index.db fault each other)` (NFR-REL-01, ADR-0001) | rust-core-dev | 153 | **merged** 57273cf (2 rounds; A-02 sentence ec3bf71) |
| 155 | `[P1/impl] A held index lock is a wait with bounded retry: OpeningReason::IndexHeldByAnotherProcess` (NFR-REL-01/02, US-07) | tauri-app-dev + vue-frontend-dev | 154 | **merged** ceb4048 |
| 156 | `[P1/req] product-analyst ratifies the A-02 consequence sentence applied by the lead` | product-analyst | 154 | **merged** 3db6644 |
| 68 | `[P1/impl] TreeSession replay to a new channel on webview reload` (FR-VLT-01, NFR-PRF-01; ADR-0015) | rust-core-dev + tauri-app-dev | 18 | **merged** (gate68, 2026-11-23) |
| 69 | `[P1/impl] Background quick_check after an unclean open` (NFR-REL-01/02, C-02; ADR-0002, ADR-0011) | rust-core-dev + tauri-app-dev | 18 | **merged** 8c6d21b |
| 157 | `[P1/impl] Strip renders index_check_problems` (NFR-REL-01/02, US-07) | vue-frontend-dev | 69 | **merged** 8145f26 |
| 158 | `[P1/design+impl] Per-RebuildReason copy in the strip` (NFR-USE-01, US-07) | architect + vue-frontend-dev | 157 | **merged** 7dfe677 (158b) |
| 148 | `[P1/design] T-02 row states the CSP directive set once` | architect | 127 | **merged** with #127 (148c) |

Task 29 was folded into 15. Every branch is reviewed by code-reviewer before merge.

**Owner rulings recorded during P1 (2026-09-24):** gate reaches for any post-P1 capability are logged and counted, not gate-failing; FR-VLT-01 is re-defined as the complete tree timed from window paint (ADR-0015); NFR-PRF-02 is re-defined as ≥95% of keystrokes painted in the first frame (architecture §9.3), because the old "p95 < 16 ms" is unmeetable at 60 Hz (a bare contenteditable measures 17.1 ms).
