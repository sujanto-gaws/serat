---
project: Serat
artifact: P1 gate protocol
phase: 05-testing
owner: test-engineer
serves: NFR-CMP-01, NFR-CMP-02, NFR-CMP-04, NFR-USE-01, NFR-MNT-01, FR-VLT-08, FR-VLT-11, FR-EDT-02, FR-EDT-10, FR-BIN-03, FR-BIN-11, FR-BIN-12, FR-NAV-06, US-01, US-41, US-29, RISK-013
---

# P1 Gate Protocol

**The gate (charter §Milestones, task list `docs/01-planning/tasks-p1-editor.md`):** the owner opens their own vault in Serat for a week without needing Obsidian.

This is not a benchmark. There is no synthetic vault, no CI runner, and — while F-384/RISK-013 stands — no CI job at all: "budgets are not enforced in CI because CI produces no jobs; every P1 budget job runs locally, fails loudly, and the gate report states which budgets were CI-asserted (none, today)." The P1 gate has the same honesty obligation in a different shape. Nobody but the owner can witness "lived in it for a week", so this document turns that sentence into things a single person can actually do, log, and be held to — and states plainly, in the final report, which parts were witnessed and which were merely believed.

**Precondition to starting the week:** tasks #9–#20 (bench jobs, Obsidian corpus, fault injection, IPC types, asset serving) have landed enough of the P1 scope that a real week of use is possible — not that every ID in scope is done. Starting the week early to find out what is missing is itself useful data and is not a protocol violation; what would be a violation is *reporting* the gate met on a week that ran against a stub.

## What counts as "needing Obsidian"

A bright line, decided now rather than argued case-by-case during the week, because the case-by-case version always resolves in favour of whichever answer is more comfortable that day.

**"The real vault" is not just the one file tree on the primary machine.** Any copy, clone or synced replica of it is the real vault for this purpose — a laptop-side clone, a synced copy on another machine, a phone. **Obsidian mobile counts.** Doing vault work in any other tool that isn't Obsidian either — VS Code, a plain text editor, a terminal `cat`/`sed` — because Serat couldn't do it also counts as needing Obsidian: the gate is about not needing help, not specifically about not needing *Obsidian's app icon*. The only exemption from all of this is the scheduled external-edit test in daily-checklist item 6, which exists precisely to inject an edit from outside Serat and is not itself a "need."

**Needing Obsidian (or an equivalent workaround)** means any of:
- Opening Obsidian — desktop or mobile — to **read or edit vault content**, on any copy of the vault, because Serat could not show it, could not edit it, or the owner did not trust what Serat showed.
- Opening Obsidian to **perform an action** — install/configure a plugin's data, resolve a link, view a PDF/image, run a query — because Serat has no equivalent yet or the equivalent broke.
- Opening Obsidian to **check whether Serat corrupted or lost something**, other than the scheduled NFR-CMP-01 checks below. Reaching for Obsidian as a safety net is itself evidence the gate is not met, independent of what the check finds — this is its own disposition ("defect (trust)"), not folded into the other three; see the GO/NO-GO rule below.
- Doing real vault work in **any other tool** (VS Code, a plain text editor, a shell) because Serat could not do it. Swapping Obsidian for a different workaround does not satisfy "without needing Obsidian" — it satisfies the letter and defeats the point.
- Reaching for Obsidian, or a workaround, to use a **post-P1 capability** — anything the PRD's Phase column puts at P2 or later (Dataview-style queries, the table view, PDF search, block refs — all P2), P3 (lenses, dashboard notes), P4 (the scoped graph view, FR-NAV-07), or v2 (the PRD's four Won't-priority scope exclusions: multiple vaults, audio/video transcription, no sync engine, no plugin API) — **is a "need" by this definition too.** Per the owner's ruling (2026-09-24) it does not fail the gate on its own, but it is not exempt from being logged as a reach; see the GO/NO-GO rule below for what happens to it.

**Creating a note or folder is a P1 capability (FR-VLT-11, US-41, added 2026-10-20 at PRD v1.7).** Opening Obsidian, or any other tool, to create a new note or folder in the real vault is a **missing P1 feature** and NO-GO. Rename and move remain P3 (FR-VLT-09); a reach for those is a post-P1 reach, logged and counted.

**Not needing Obsidian** (does not break the week):
- The two scheduled NFR-CMP-01 checks (day 0 baseline, day 7 reopen) and item 6's scheduled external-edit test — those exist to exercise compatibility and reconciliation, not to use Obsidian (or another editor) as a tool for getting real work done.
- Opening Obsidian on a **different** vault, or out of idle curiosity with zero vault interaction.
- Consulting Obsidian's online documentation without opening the vault in it. (Obsidian's settings screen is per-vault state, not documentation — opening it against the real vault is a vault interaction and is logged like any other.)

Every reach for Obsidian (or an equivalent workaround tool) against the real vault — justified or not — is logged the same way (see daily log below): what was being attempted, why Serat did not serve it, and one of four dispositions — a **missing P1 feature**, a **post-P1 capability correctly out of P1 scope**, a **defect**, or **"defect (trust)"** for the specific case of opening Obsidian as a safety net to check for corruption or loss. **"Missing P1 feature" vs. "post-P1 capability" is not a judgement call at logging time — it is read straight off the PRD's Phase column for the ID the reach maps to.** If the capability's ID is P1 in `docs/02-requirements/prd.md`, the reach is a missing P1 feature; if its ID is P2, P3, P4, v2, or it is one of the four Won't-priority scope exclusions, the reach is post-P1. A reach that does not obviously map to any single ID is logged with the candidate ID(s) considered and which one the disposition rests on, so the report can be checked against the PRD rather than taken on trust. A week with zero log entries is not evidence of nothing happening; it is evidence the owner did not use the vault hard enough to find out, and the gate report says so if session logs are thin.

**GO/NO-GO rule.** This is not a judgement call made once at the end of the week:
- **Any reach logged as "missing P1 feature", "defect" or "defect (trust)" is NO-GO**, full stop — not weighed against how many good days surrounded it. A safety-net reach — opening Obsidian (or a workaround) only to check whether Serat corrupted or lost something — is NO-GO **even when the check finds nothing wrong**: needing to look is itself the failure the gate cares about, independent of the answer.
- **A gate-stopping breach restarts the 7-day count.** The week is 7 consecutive days with no NO-GO event in it, not "7 good days spread across a longer window." A NO-GO on day 5 means a new day 1 starts once the cause is fixed, not a resumption at day 6.
- **Reaches logged as "post-P1 capability, correctly out of P1 scope" do not fail the gate — owner ruling, 2026-09-24, extended same day.** The owner's first ruling covered P2-scope capability by name; extended the same day to **any post-P1 capability** per the PRD's Phase column — P2 (Dataview-style queries, the flat table view, search inside PDFs, block refs), P3 (lenses, dashboard notes), P4 (the scoped graph view, FR-NAV-07), or v2 (the PRD's four Won't-priority scope exclusions) — is **logged and reported, but does not make the week NO-GO**. Only a reach disposed as "missing P1 feature", "defect" or "defect (trust)" is a NO-GO event. This replaces the earlier placeholder; the owner decided it directly rather than leaving it to inference from the charter's gate sentence.
  - **Every post-P1 reach is still counted, not waved through.** The gate report lists them with per-capability counts (how many times Dataview-equivalent queries were reached for, how many times the table view, how many times PDF search, how many times block refs, how many times graph view or lenses, and any other post-P1 capability that came up) — because a P1 gate that passes while the owner reached for a later-phase capability twenty times in a week is telling later phases something the pass/fail line alone would hide, and those counts are exactly what feed prioritisation for whichever phase each capability belongs to.

## Daily checklist

Each day of the week the vault is the owner's actual note-taking tool for real work, not a demo performed on the side. Minimum per session:

1. **Open the vault in Serat** (not a copy, not the benchmark vault — the owner's real vault, `docs/01-planning/tasks-p1-editor.md`'s "owner's own vault").
2. **Write** at least one real note (created in Serat, FR-VLT-11) or edit an existing one with live preview on (FR-EDT-02) — read it back without dropping to source view to confirm formatting rendered as typed.
3. **Link** — create or follow at least one `[[wikilink]]`/embed, and check its **backlinks** pane (FR-NAV-04) shows the block that referenced it.
4. **Search or switch** — use the quick switcher (FR-NAV-01) or full-text search (FR-NAV-02, notes only in P1 scope) to reach a note that was not already open, and use the **command palette** (FR-NAV-06) for at least one action instead of a menu.
5. **Touch a non-Markdown file** if the day's real work has one: open a PDF or image through Serat's viewer (FR-BIN-11/12), confirm a sidecar/asset shows as one entry in the file tree (FR-BIN-03).
6. **Let an external edit happen** at least twice across the week, not necessarily daily — edit a note with git (a pull, a stash pop, a checkout) or another editor while Serat is open, and confirm the change appears in Serat without losing cursor position, selection or undo history in any note that was open at the time (FR-VLT-04/05, out of this protocol's Demo/Inspect list but the same week is the only chance to see it for real).
7. **Log the day** — a dated entry appended to `docs/05-testing/reports/p1-gate-log.md` (created on day 0), recording: session start/end, what was done against items 2–6, any reach for Obsidian (with the reason, per the definition above), any crash, hang, data-loss scare or visibly wrong render, and its severity. A day with nothing to log still gets a one-line entry saying so — an absent day is not the same claim as an uneventful one.

**A day only counts toward the week if items 2–4 were actually done** (write, link + check backlinks, switch/search + palette). Items 5 and 6 are conditional on the day's real work having a binary file or an external edit to exercise, and their absence on a given day does not disqualify it — but a day with no real writing, no link exercised and no navigation is not a day of "living in it," whatever else got logged.

A defect found during the week is filed the same way test-plan.md requires elsewhere in this project: **a failure becomes a task**, assigned to the owning crate/role, with reproduction steps and the exact vault state — not left as a line in the log for someone to rediscover.

## The NFR-CMP-01 procedure (Obsidian reopens with no configuration loss)

NFR-CMP-01's acceptance criterion is exact: *"Given a real Obsidian vault, when opened and used for a session, then Obsidian opens it afterwards with no configuration loss."* Architecture §10.3/line 940 gives the mechanism this checks: `.obsidian/`, `.trash/` and `.serat/` are in serat-vault's ignore list — descended into by nothing, read by nothing, written by nothing. This procedure checks that promise held, not just that it was made.

**Day 0, before Serat ever opens the vault:**
1. **From inside Obsidian, before closing it**, record by hand: enabled community plugins, hotkey overrides, workspace layout (panes open), and any plugin-specific state the owner cares about (e.g. Dataview query cache, daily-notes settings). This is the part a checksum can't tell you was lost, because it can't tell you it *mattered* — it has to be captured while Obsidian can still show it.
2. Close Obsidian cleanly.
3. Snapshot `.obsidian/` — `tar` it (or equivalent) to a location **outside** the vault, and record its size and a recursive checksum that includes symlinks as well as regular files (e.g. `find .obsidian \( -type f -o -type l \) -exec sha256sum {} \; | sort` — a plain `-type f` search silently skips any symlinked config Obsidian itself follows).
4. Note the vault's git status if it is a git repo (clean/dirty, current branch) — NFR-CMP-04 is checked separately below but starts from the same baseline. **If it is a git repo, also run `git ls-files .obsidian` and record whether `.obsidian/` itself is tracked.** If it is tracked, either keep item 6's external-edit git operations (pulls, stash pops, checkouts) to paths outside `.obsidian/` for the rest of the week, or run the step-3 checksum **both immediately before and immediately after every such git operation**, logging both results as a numbered, unbroken chain — the day-0 snapshot is before₁, the checksum taken right after that first git operation is after₁ and must equal the before₂ taken right before the *next* git operation, and so on, so that afterₙ (after the last git operation of the week) is the only value day 7 is allowed to compare against. A tracked `.obsidian/` means an ordinary git command touches the same tree this check is trying to hold still, and only an unbroken chain — not merely "some before/after pair exists somewhere in the log" — rules out a Serat write landing in the gap between two logged checksums and being explained away by a git operation that happened nowhere near it.
5. Record `.trash/`'s existence and contents (a file listing, or "absent" if Obsidian hasn't created it yet), and note the moment as a timestamp marker for the day-7 comparison.

**Through the week:** Obsidian stays closed on this vault except for the two scheduled checks. Serat is the only thing with the vault open.

**Day 7, after the week's use:**
1. Close Serat cleanly.
2. Re-run the checksum from day-0 step 3 and diff it against the day-0 snapshot. **Expected: byte-identical, zero diff.** If day-0 step 4's tracked-`.obsidian/` carve-out applied, a diff counts as **explained** only if the day-7 checksum equals the chain's final logged value, afterₙ, **and every consecutive pair in the chain matches** (beforeₖ₊₁ = afterₖ for every git operation logged that week, with the day-0 snapshot as before₁). A day-7 value that equals *some* logged checksum out of order, or a chain with even one broken link, is not explained — it means a change happened somewhere the chain does not account for, which is exactly what an unbroken chain exists to catch. Any unexplained diff, and any broken link in the chain itself, is not a soft finding — it is a breach of a Must requirement, stops the gate, and is a NO-GO.
3. Open the vault in Obsidian. Confirm: it opens without a "vault needs repair" or re-index prompt, the plugins/hotkeys/layout recorded in day-0 step 1 are still enabled with the same settings, and the workspace layout restores (or fails to restore for a reason Obsidian itself would give with Obsidian alone — not a Serat-caused loss).
4. Confirm `.trash/` was not created or written to by Serat: compare against the day-0 listing, and run `find .trash -newer <day-0 marker>` (empty result expected) to catch anything the listing diff alone would miss.
5. Record all of the above — pass/fail per item, not a single "no config loss" line — in the gate report's NFR-CMP-01 section.

**NFR-CMP-04** (`.serat/` never enters version control silently) has its own three-step procedure, run **before the vault is opened in Serat for the first time**:
1. Hash `.gitignore` (`sha256sum .gitignore`, or record its absence) and run `git check-ignore .serat` to establish, factually, whether `.serat/` is already ignored.
2. Open the vault in Serat. If prompted to add an ignore entry, **decline the offer.**
3. Re-hash `.gitignore` and confirm it is unchanged, and run `git status` and confirm it is clean with respect to `.gitignore` (no untracked-but-should-be-ignored `.serat/` sitting there, no modified `.gitignore`).

**Two situations mean the real vault has nothing to offer, and both fall back to a throwaway clone** — a clone of the vault with no `.serat/` directory and no existing ignore entry, so the "offer, decline, confirm untouched" path is actually exercised once, with the same three steps run against it and recorded as having run against a clone rather than the real vault:
- `.serat/` is already ignored in `.gitignore`, so there is nothing left to offer.
- `.serat/` **already exists on disk** — from an earlier P0 run, an earlier day of this same P1 week, or any prior open — even if it is not yet ignored, because the acceptance criterion's offer is a first-open behaviour ("when it is first opened") and a vault Serat has already indexed once does not retrigger it. A vault in this state is not a fresh test of the offer even though `.gitignore` looks untouched.

If the vault is not a git repository at all, this ID is not exercised this week and the gate report says so rather than marking it witnessed.

## How each P1 Demo/Inspect ID is witnessed

Test and Bench IDs in P1 scope are witnessed by the local budget jobs and integration tests in `bench/` and `crates/*/tests/` per `docs/05-testing/test-plan.md` — not by this protocol. What follows is only the P1 requirements whose PRD `Verify` column reads **Demo** or **Inspect** (`docs/02-requirements/prd.md` traceability appendix), because those two methods have no automated pass/fail and only mean something if someone states what they watched.

| ID | Verify | What "witnessed" means here | Evidence recorded |
|---|---|---|---|
| FR-VLT-08 — Attachments are visible | Demo | During the week, a non-Markdown file (PDF/image already in the real vault) is listed in the file tree and inserted as a link into a note, from Serat's own UI | Log entry (item 5) + screenshot/recording, dated |
| FR-EDT-02 — Hybrid live preview | Demo | Daily item 2: cursor leaves a bold/italic/link span, the raw markers hide within one visible frame; cursor re-enters, markers reappear editable. Watched at least once per day, not just once for the week | Log entries across all 7 days, not a single day |
| FR-EDT-10 — Split preview fallback | Demo | With live preview turned off in settings once during the week, a note is opened and edited fully in split source/preview mode with no capability lost (can still type, save, link) | One dated log entry with the setting toggle noted |
| FR-BIN-03 — Paired presentation | Demo | A file with a sidecar note appears as one entry in the file browser, not two, observed while doing item 5 | Log entry + screenshot of the file tree row |
| FR-BIN-11 — PDF viewer | Demo | A real PDF from the vault opened in-app; a page number entered navigates to that page; text in the PDF is selectable | Log entry + screenshot/recording |
| FR-BIN-12 — Image viewer | Demo | A real image opened in-app; zoom applied without leaving Serat | Log entry + screenshot |
| NFR-CMP-01 — Obsidian vault compatibility | Demo | The day-0/day-7 procedure above, in full, including the item Demo alone would miss: the *week of actual use* in between, not just the two endpoints | The NFR-CMP-01 section of the gate report, all five day-7 checks itemised pass/fail |
| NFR-CMP-04 — `.serat/` never enters version control silently | Demo | The git-repo check above; if the vault is not a git repo, recorded as not exercised, not as passed | Gate report note; git status snippets |
| FR-NAV-06 — Command palette | Inspect | Every user-invocable action reachable in the week (open, save, switch, search, link, toggle preview, plus whatever the owner used) is checked against the palette's own listing, with its keyboard shortcut shown | A checklist of actions used during the week cross-referenced against the palette |
| NFR-CMP-02 — No proprietary content format | Inspect | Every file Serat wrote during the week (not just the ones edited through the UI — anything with a changed mtime under the vault root) opened in a plain text editor outside Serat and confirmed to be Markdown, YAML frontmatter, or documented plain text | A file listing (path, mtime) cross-checked at day 7, same pass as the NFR-CMP-01 checksum sweep |
| NFR-USE-01 — Keyboard-first operation | Inspect | Every primary action used during the week performed once via keyboard alone (no pointing device) and confirmed reachable/bindable from the command palette | Log entries noting which actions were keyboard-only tested |
| NFR-MNT-01 — Typed IPC boundary | Inspect | Not owner-observable during a week of vault use — this is a build-time property. Witnessed by running the generated-types-staleness check locally (`npm run <check>` / equivalent in `app/`) immediately before the week starts and again at day 7, since CI cannot run it while F-384/RISK-013 stands | Two dated local build logs, explicitly marked "not CI-asserted" per RISK-013's wording |

Two things this table is deliberately honest about, matching the standard the rest of this suite holds:

1. **A Demo/Inspect ID with one log entry for the whole week is weaker evidence than one with seven.** The table says which IDs need repeated observation (FR-EDT-02, day-to-day use) versus a single clean demonstration (FR-BIN-11/12). The gate report counts entries, not just presence.
2. **NFR-MNT-01 is not witnessed by the week at all** — it is a build check riding along on the same calendar because there is no other natural cadence for it while CI is down. Filing it as "witnessed by the gate week" would be the same mistake F-384/RISK-013 already names: reporting something as checked because the checking happened to be nearby, not because the mechanism that should have checked it actually ran.

## What the gate report will contain (task #28, second half — not this document)

This document is the protocol; the report that applies it is written after the week and after task #27's test cases land, per the task list. The report will carry, per `test-plan.md`'s reporting rule: the daily log in full, the NFR-CMP-01/CMP-04 day-7 checklist with pass/fail per item, the Demo/Inspect table above filled in with actual evidence references, every Obsidian-reach logged during the week with its disposition (missing P1 feature / post-P1 capability correctly out of P1 scope / defect / defect (trust), with the defect's task number), **a per-capability count of every post-P1 reach** (Dataview-equivalent queries, table view, PDF search, block refs, graph view, lenses, any other) per the owner's 2026-09-24 ruling above, and an explicit GO / NO-GO applying the rule above exactly — not a qualified one, because a gate that can be met "mostly" is not a gate. If any day in the reported week was restarted by a gate-stopping breach, the report shows the full history of attempts, not just the final clean run.

## Traceability

| ID | How this document serves it |
|---|---|
| NFR-CMP-01 | Turns "no configuration loss" into a day-0/day-7 checksum-and-checklist procedure instead of an impression. |
| NFR-CMP-02 | Gives Inspect a concrete artefact (every file Serat wrote that week) rather than a spot check. |
| NFR-CMP-04 | Its own hash-and-decline procedure, with a throwaway-clone fallback when the real vault has nothing to offer. |
| NFR-USE-01, FR-NAV-06 | Give Inspect a cross-reference (actions actually used vs. palette listing) instead of a read-through of the palette alone. |
| NFR-MNT-01 | States plainly that this ID is not witnessed by vault use and names what does witness it, so it is not silently marked done by proximity. |
| FR-VLT-08, FR-EDT-02, FR-EDT-10, FR-BIN-03, FR-BIN-11, FR-BIN-12 | Each Demo ID gets a specific, repeatable in-week action and the evidence it leaves behind. |
| US-01 | The whole document is US-01's acceptance criteria made checkable, narrowed to what P1 scope actually delivers: wikilinks and embeds (FR-EDT-03) resolving, and "no setting or plugin state was lost." Block refs (`[[note^block]]`) are FR-EDT-04, which is **P2** scope, not P1 — this document does not claim to witness them. |
| US-29 | Plain-text guarantee → NFR-CMP-02 row. |
| RISK-013 | The report will state which of this protocol's checks were CI-asserted (none) with the same honesty RISK-013 requires of the budget jobs. |
