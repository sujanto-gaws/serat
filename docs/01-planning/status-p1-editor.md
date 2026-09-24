---
project: Serat
artifact: P1 status — paused for resume
phase: 01-planning
owner: lead
status: paused by owner, 2026-09-24 — gate NOT reached, P1 in progress
---

# P1 — An editor you could live in — Status (paused)

**Gate (charter §Milestones):** owner opens their own vault in Serat for a week without needing Obsidian. Protocol: `docs/05-testing/p1-gate-protocol.md` (merged).

**Where P1 stands:** design is complete and merged. Implementation has started on task branches, and none of it is merged. Every teammate was stopped at the owner's request. Nothing is running, and the PR watch on sujanto-gaws/serat#10 and its scheduled check-in are cancelled.

Integration branch: `claude/great-mccarthy-jywddn` (PR sujanto-gaws/serat#10), tip at the time of pausing = the commit that adds this file. The last code-bearing merge is `e179b2c`. The full Rust workspace and `app/` `npm test` (31/31) were green there, and doclint was 50/50.

## Owner decisions recorded during P1 (2026-09-24)

1. Enter P1 with P0's gate not formally closed. F-384 (no CI jobs) and #123 (a scanner-on re-run) are carried as RISK-013 and an open item.
2. The flat table view stays in P2 (FR-VIW-01). FR-NAV-02 is split: note bodies in P1, and extracted/PDF content becomes new **FR-NAV-08** (P2).
3. Gate rule: reaching for Obsidian for **any post-P1 capability** is logged and counted, not gate-failing. A missing-P1-feature, defect or "defect (trust)" reach is NO-GO and restarts the 7 days.
4. FR-VLT-01 is re-defined (PRD v1.6, ADR-0015): the complete tree within 2 s p95, timed from the window's first paint; empty folders are reported, not gated.
5. NFR-PRF-02 is re-defined (PRD v1.6): ≥95% of keystrokes painted in the first frame after the keystroke. The old "p95 < 16 ms" is unmeetable at 60 Hz.
6. Lead default (owner told, not objected to): a gate reach that maps to **no PRD ID** counts as missing P1 unless product-analyst records an ID or exclusion first.

## Merged on the integration branch

| # | What | Merge |
|---|---|---|
| 1 | FR-NAV-02 split, FR-NAV-08 (PRD v1.5) | 5485c2e |
| 2 | ADR-0015 file tree (accepted) | e624c20 |
| 3 | Quick switcher, architecture §9.2 | 487abc3 |
| 4 | P1 read paths, §4.5 | b2d2f1c |
| 5 | ADR-0016 editor saves are patches (accepted); §3.3, §9.3 | d0d5300 |
| 6 | ADR-0017 serat-asset scheme; §7.4 | 1e74f07 |
| 7 | contracts/ipc.rs P1 subset + generated TS (50 types) | be8baa7 |
| 8 | UI screen specs, tokens, components (docs/03-design/ui) | 8fe20a3 |
| 28a | Gate protocol | d118997 |
| 30 | FR-NAV-08 accounted for in architecture.md | db1d967 |
| 32 | PRD v1.6 (FR-VLT-01, NFR-PRF-02 re-definitions) | 857af57 |
| 36 | ADR-0017 correction: real controls (a) top-level nav guard, (b) sandbox CSP, (c) frame-src/object-src 'none' + sanitiser | e274d75 |

## Unmerged branches: exact state at pause

Review state: **APPROVED** = can merge. **CR** = changes required, not yet fixed. **FIXED-UNREVIEWED** = the author committed fixes that no one has re-reviewed. **WIP** = incomplete. Where a branch lives only in a local worktree, that is stated.

| # | Branch @ hash | Owner | State | Next step |
|---|---|---|---|---|
| 9 | `task/p1-09-tree-budget` @ 1098c60 | test-engineer | FIXED-UNREVIEWED (code-reviewer-2's B1 job_exit / B2 P1_BUDGETS) | code-reviewer-2 re-review |
| 10 | `task/p1-10-switcher-budget` @ e54986a | test-engineer | FIXED-UNREVIEWED; should sit on top of #9 (Cargo.toml `[[bin]]` keep-both) | re-review after #9 |
| 11 | `task/p1-11-editor-latency` @ aedb9e1 | test-engineer | CR. The §9.3 rule is unsettled: three definitions failed on recorded frames | **empirical step first**: record candidate rules (positive n=300 ≥99% on time; negative 2.2R ≥95% late; graded 1.5R stable band). Candidate: late iff frame k's rAF `run_k > D` (D = ts_j+2R), render cost from LoAF / Event Timing. If nothing clears 99% headless, escalate to the owner (a WebView2 reference-machine gate?) |
| 12 | `task/p1-12-compat-corpus` @ 1a081c7 | test-engineer | WIP (fixtures) | continue |
| 16b | `task/p1-16b-adr0016-separator-note` @ 72a50f9 | architect | in review queue (code-reviewer-2); watch F-239, since it cites a path on unmerged #21 | review |
| 21 | `task/p1-21-editor` @ b87755d | vue-frontend-dev | **CR, BLOCKING data loss**: two undos after load empty the note and autosave writes it. Also: markdown() adds its own Enter/Backspace keymap; the bench seam ships in release; boot chunk +78% gzip (lang-html pulls JS/CSS grammars). Plus NITs: StaleHash blind retry, offsetAt validation, sanitiser class/data-*, auxclick | fix (the author had just started); merge blocked on #11 and #31 |
| 28b | `task/p1-28b-no-id-reach` @ e9564c4 | test-engineer | unreviewed | review |
| 31 | `task/p1-31-tree-first-screen-mark` @ d8e2547 | test-engineer | CR + a lead-committed WIP on top (unverified) | finish the `tree:false` declaration approach (ADR-0013), then re-review |
| 33 | `task/p1-33-patch-apply` @ 3ddad26 | rust-core-dev | WIP, lead-committed; tests not yet run | finish; add the lone-`\r` table case matching app/src/editor/patches.test.ts |
| 34 | `task/p1-34-path-escape` @ 10603a1 | rust-core-dev | FIXED-UNREVIEWED (platform-conditional name rules, refused names reported per #35, device-name list, `\` on Unix, resolve_segments seam) | code-reviewer-2 re-review. **Live security defect on the integration branch until merged** (Windows drive-relative escape in read_file) |
| 37 | `task/p1-37-nav-guard` @ 4f3766f | tauri-app-dev | in security review (code-reviewer-2, interrupted) | re-run the review |
| 38 | `task/p1-38-prf02-vsync-count` @ 13c5486 | architect | CR, **on hold** until #11's data | rewrite §9.3 from the measured winner |
| 40 | `task/p1-40-block-widgets` @ b5d3424 | vue-frontend-dev | WIP, paused for the #21 fixes | resume after #21 |

Not started: #13, #14, #15, #16, #17 (rust-core-dev, each after its bench or design); #18, #19, #20 (tauri-app-dev); #22–#26 (vue-frontend-dev); #27 (test-engineer); #31's re-measure; #39 (context menu on Windows/macOS, needs platform access). Task definitions are in `tasks-p1-editor.md` and on the session task list.

## Budget numbers measured in P1

None of the P1 budgets has been measured: no subject code exists yet. Carried from P0: NFR-PRF-04 27.7 s / 30 s PASS; NFR-PRF-03 4.3 ms p95 PASS; NFR-PRF-01 Serat term 233.8 ms (a *buffer-rendered* figure, pre-tree) against 350 ms. All are `gate_eligible: false` (scanner off, RISK-012). FR-VLT-01's pre-design walk figure was 4.2 s median / 8.1 s p95 (F-193).

## Open risks at pause

- **RISK-013 / F-384: CI still produces no jobs.** All 19 checks on PR #10 fail in 2–4 s with 404 logs, and `main` behaves the same. This is an owner action (Actions billing/limits or enablement). A comment on PR #10 says so.
- **The #34 security defect is live** on the integration branch until #34 merges.
- **NFR-PRF-02 has no validated measurement rule.** Three §9.3 definitions failed against recorded frames. It may not be gateable in headless Chromium.
- **NFR-PRF-01 at risk:** the editor's boot chunk grew +78% gzip; it must be re-measured (#31) before #21 merges.
- **RISK-003 (CM6 live preview):** block widgets (#40) are needed for the gate, not just split preview.
- **Disk:** the container reached 96% twice from parallel build dirs. On resume, build targets go on `/dev/shm` or a single shared dir, deleted after use.

## Process lessons for resume

- Teammates cannot message each other or update the task list; the lead relays everything. product-analyst has no shell, so the lead commits its edits (commit messages say so).
- Run `cargo test -p doclint` on **every** docs change, the lead's own planning edits included (the lead broke the tip twice this way).
- A measurement definition must be checked against recorded data **before** it goes into the design text.
- The task branches above exist **only in this container** unless they are pushed. See the resume note below.

## To resume

1. Restore the task branches (from the pushed refs, if the owner approved pushing them).
2. Re-spawn the roles from `.claude/agents/`, each with its own worktree, and hand each the row above.
3. Order: #34 re-review → merge (security); #21 data-loss fix; #9/#10 re-review → merge; #11 empirical rule selection → #38 → #11 → #31 → #21 merge; then #33/#15/#17 → #18/#19 → #22–#26 → #27 → #28 report.
