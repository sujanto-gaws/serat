---
project: Serat
artifact: User stories
phase: 02-requirements
derived_from: Serat Requirements Baseline 1.0 §04 — original in docs/_source/serat-requirements-baseline.html
companion: docs/02-requirements/prd.md
---

# User Stories — Serat

The same scope as the PRD's functional and non-functional requirements, expressed as work. Sizes are relative (S, M, L, XL), not hours. Where two phases are shown, the first is when the story becomes demonstrable and the second is when the last requirement it draws on lands.


## Epic E1 — Vault and editing core

P1 · An editor you could live in: open the real vault, write, link, find, and survive external edits.

### US-01 — Open my real vault  (serves FR-VLT-01, FR-VLT-07, NFR-CMP-01, FR-VLT-02, NFR-CMP-03, NFR-CMP-04, NFR-SEC-06)
**Phase:** P1 · **Size:** M

As a vault owner, I want to open my existing Obsidian vault directly, so that trying Serat costs me nothing and I can keep Obsidian as a fallback.

**Acceptance criteria**

- Given my Obsidian vault, when I open it in Serat, then wikilinks, embeds, block refs and inline fields all resolve.
- Given I used Serat for a week, when I reopen Obsidian, then no setting or plugin state was lost.

### US-02 — Write without seeing syntax  (serves FR-EDT-01, FR-EDT-02, FR-EDT-06, FR-EDT-07)
**Phase:** P1 → P4 · **Size:** XL

As a writer, I want formatting to render as I type, so that I read prose rather than punctuation.

**Acceptance criteria**

- Given the cursor leaves a bold span, when the selection settles, then the \*\* markers hide within one frame.
- Given the cursor enters that span, then the raw markers reappear and are editable.

### US-03 — Link and traverse  (serves FR-EDT-03, FR-NAV-04, FR-EDT-04, FR-NAV-05, FR-NAV-07)
**Phase:** P1 → P4 · **Size:** M

As a note-taker, I want to link notes and see what links back, so that structure emerges from writing rather than filing.

**Acceptance criteria**

- Given I type \[\[, when two characters follow, then ranked completions appear.
- Given a note linked from three paragraphs, when I open it, then three block-precise backlinks are listed.

### US-04 — Reach any note instantly  (serves FR-NAV-01, FR-NAV-06, NFR-PRF-06, NFR-USE-01)
**Phase:** P1 · **Size:** M

As a daily user, I want to jump to any note in two keystrokes, so that navigation never interrupts thought.

**Acceptance criteria**

- Given 20,000 notes, when I type three characters, then ranked results render in under 50ms.

### US-05 — Search everything  (serves FR-NAV-02, FR-NAV-03, FR-NAV-08)
**Phase:** P1 → P2 · **Size:** M

As a researcher, I want one search box that covers the whole vault, so that I never wonder whether something was missed.

**Acceptance criteria**

- Given a phrase present in a note, when I search it (P1, FR-NAV-02), then it is returned with a snippet and a block-precise location.
- Given a phrase present only in a PDF, when I search it from the same search box (P2, FR-NAV-08), then the PDF is returned with a snippet and a locator naming its page.

### US-06 — Survive external edits  (serves FR-VLT-04, FR-VLT-05)
**Phase:** P1 · **Size:** L

As an engineer, I want Claude Code and git to edit my files while I work, so that my tools do not have to take turns.

**Acceptance criteria**

- Given an open note, when an external process edits an earlier line, then my cursor and undo history survive.
- Given a file is modified by git or another editor, when 500ms have elapsed at p95 over 100 single-note external changes on the 20,000-note vault, then the index and any open view reflect the change.

### US-07 — Never lose work  (serves FR-VLT-03, FR-PLT-04, NFR-REL-01, NFR-REL-02)
**Phase:** P1 → P5 · **Size:** M

As a vault owner, I want a crash to cost me nothing, so that I can trust the app with my only copy.

**Acceptance criteria**

- Given a forced kill mid-write, when I inspect the file, then it is either wholly old or wholly new.
- Given unsaved changes and a crash, when I restart, then recovery is offered.

### US-08 — Fall back when preview misbehaves  (serves FR-EDT-10)
**Phase:** P1 · **Size:** S

As a pragmatist, I want a split source/preview mode available, so that a live-preview bug never blocks my writing.

**Acceptance criteria**

- Given live preview is disabled, when I open a note, then split mode offers full editing.

### US-41 — Start a new note  (serves FR-VLT-11)
**Phase:** P1 · **Size:** S

As a daily user, I want to create a note or folder without leaving Serat, so that capturing a new thought never sends me to another tool.

**Acceptance criteria**

- Given a folder selected in the file tree, when I choose New note from the tree or the command palette, then a plain-Markdown file with a unique name appears in that folder, opens in the editor, and is listed in the tree without a rebuild.
- Given a folder selected in the file tree, when I choose New folder, then the folder appears in the tree and I can create notes in it.
- Given a note I created, when I open it in a plain text editor outside Serat, then it is ordinary Markdown with no template or proprietary syntax.

### US-38 — Arrange my workspace  (serves FR-EDT-09, NFR-USE-03)
**Phase:** P2 · **Size:** M

As a daily user, I want panes, tabs and themes I can arrange and keep, so that the tool matches how I actually work rather than resetting each session.

**Acceptance criteria**

- Given two panes and three tabs, when I restart, then the arrangement is restored.
- Given the OS theme changes, when the app is in system mode, then it follows without restart.


## Epic E2 — Structured knowledge

P2–P5 · Types, the SQ language, and views you can edit through — the answer to Dataview and Bases.

### US-09 — Declare what a project note is  (serves FR-STR-01, FR-STR-02, FR-STR-03)
**Phase:** P2 · **Size:** L

As a systems analyst, I want to define the shape of a note type, so that structure is enforced instead of remembered.

**Acceptance criteria**

- Given a type note declaring required fields, when a note of that type omits one, then it appears in the violations view.

### US-10 — Put data where the prose is  (serves FR-STR-04, FR-EDT-05)
**Phase:** P2 · **Size:** M

As a note-taker, I want fields inline in the body, so that I do not have to hoist every fact into frontmatter.

**Acceptance criteria**

- Given key:: value mid-paragraph, when I query that key, then the note is returned.
- Given the same text inside a code fence, then no field is created.

### US-11 — Query in a language I can trust  (serves FR-STR-06, FR-STR-08, FR-STR-09, NFR-PRF-05, FR-STR-05)
**Phase:** P2 · **Size:** XL

As an engineer, I want a declarative query language with predictable cost, so that a dashboard never makes the app slow.

**Acceptance criteria**

- Given any conformance query, when compiled, then exactly one SQL statement is emitted.
- Given a 500-row result at 20k notes, when it runs, then it completes within 100ms at p95.

### US-12 — Catch my typos before the table is empty  (serves FR-STR-07)
**Phase:** P2 · **Size:** M

As a query author, I want field names checked against the schema, so that a mistake is a squiggle rather than a silent empty result.

**Acceptance criteria**

- Given a query naming a field the type lacks, when it is parsed, then a diagnostic marks that token and nothing executes.

### US-13 — Put a live table in a note  (serves FR-STR-10, FR-VIW-01, FR-STR-11, FR-VIW-03, FR-VIW-04)
**Phase:** P2 → P3 · **Size:** M

As a project manager, I want query results embedded where I am writing, so that my planning note is also my status view.

**Acceptance criteria**

- Given an sq fence, when the note renders, then results appear inline and update as source notes change.

### US-14 — Edit through the table  (serves FR-VIW-05, FR-VIW-06, FR-VIW-07, FR-EDT-08, FR-VLT-06, FR-VIW-08, NFR-MNT-03)
**Phase:** P2 · **Size:** XL

As a project manager, I want to change a status directly in the table, so that keeping data current is not a chore.

**Acceptance criteria**

- Given I change a cell, when I read the source file, then only that field's bytes changed.
- Given the file changed externally since I read the row, then my write is refused and the view refreshes.
- Given I press undo, then the file returns to its previous bytes.

### US-15 — Roll up from linked notes  (serves FR-STR-12)
**Phase:** P3 · **Size:** M

As a project manager, I want totals computed across linked notes, so that a project row shows real remaining effort.

**Acceptance criteria**

- Given a project linking five estimated tasks, when I sum the rollup, then it equals those five estimates.

### US-16 — See what has drifted  (serves FR-STR-13, FR-LNS-10)
**Phase:** P4 · **Size:** M

As a systems analyst, I want a standing list of notes that violate their type, so that structure decays visibly rather than silently.

**Acceptance criteria**

- Given a typed note missing a required field, when I open the violations view, then the note and field are named.

### US-17 — Rename a field everywhere  (serves FR-STR-14)
**Phase:** P5 · **Size:** L

As a systems analyst, I want to rename a field across every note of a type, so that my schema can evolve without a scripting session.

**Acceptance criteria**

- Given 400 notes carrying the field, when I rename it, then a diff preview precedes the write and one undo reverses all of it.


## Epic E3 — Binaries as citizens

P1–P4 · PDFs, Office files, spreadsheets and images extracted into the same index as notes.

### US-18 — Give a PDF metadata  (serves FR-BIN-01, FR-BIN-02, FR-BIN-03, FR-VLT-08, FR-BIN-19)
**Phase:** P1 → P2 · **Size:** M

As a vault owner, I want to tag and type a PDF, so that my contracts are data rather than attachments.

**Acceptance criteria**

- Given contract.pdf with a sidecar declaring type: contract, when I look contracts up by type, then the PDF is returned as a typed row.
- Given a file with a sidecar, when I browse the folder, then I see one entry, not two.

### US-19 — Read documents without leaving  (serves FR-BIN-11, FR-BIN-12, FR-BIN-13)
**Phase:** P1 → P2 · **Size:** M

As a daily user, I want PDFs and images to open in-app, so that reading a source does not mean losing my place.

**Acceptance criteria**

- Given a PDF, when I open it, then I can navigate pages and select text in-app.

### US-20 — Search inside my documents  (serves FR-BIN-05, FR-BIN-06)
**Phase:** P2 · **Size:** XL

As a researcher, I want the search box to look inside PDFs and decks, so that most of my archive stops being invisible.

**Acceptance criteria**

- Given a phrase on page 12 of a PDF, when I search it, then the result names the page and opens there.

### US-21 — Query a spreadsheet beside my notes  (serves FR-BIN-14, FR-BIN-04)
**Phase:** P2 · **Size:** L

As an analyst, I want a sheet to be queryable data, so that a budget export can join against my project notes.

**Acceptance criteria**

- Given an XLSX with a header row, when I query that sheet, then rows are returned as typed records.

### US-22 — Not be taken down by a bad file  (serves FR-BIN-07, FR-BIN-10, NFR-REL-03, NFR-SEC-03, FR-BIN-09, NFR-PRF-09)
**Phase:** P2 · **Size:** L

As a vault owner, I want one corrupt PDF to be a logged failure rather than a crash, so that the app is safe to point at a real archive.

**Acceptance criteria**

- Given a corpus of malformed binaries, when indexing runs, then each failure is isolated and the rest of the vault indexes.

### US-23 — Keep expensive work after a rebuild  (serves FR-BIN-08)
**Phase:** P2 · **Size:** M

As a vault owner, I want extraction results to survive an index wipe, so that rebuilding costs seconds rather than hours.

**Acceptance criteria**

- Given 100 extracted PDFs, when I delete the index and reopen, then no re-extraction occurs.

### US-24 — Quote a passage from a PDF  (serves FR-BIN-15)
**Phase:** P3 · **Size:** L

As a researcher, I want to highlight a passage and link to it, so that source material joins the graph instead of sitting beside it.

**Acceptance criteria**

- Given a highlighted passage, when another note links it, then the backlink shows the passage and its page.

### US-25 — Search a scan  (serves FR-BIN-16, FR-BIN-17)
**Phase:** P4 · **Size:** L

As a vault owner, I want scanned documents to become searchable, so that the oldest part of my archive is not the least usable.

**Acceptance criteria**

- Given a scanned PDF with OCR enabled, when indexing completes, then its text is searchable and flagged as OCR-derived.


## Epic E4 — Lenses, lifecycle and tasks

P3 · The multi-role working model: lenses, tier promotion, tasks, dashboards.

### US-26 — Switch to my PM head  (serves FR-LNS-01, FR-LNS-02, FR-LNS-08)
**Phase:** P3 · **Size:** L

As someone working three roles, I want one keystroke to re-frame the whole workspace, so that context switching is a decision rather than a chore.

**Acceptance criteria**

- Given the PM lens, when I switch to it, then sidebar, dashboard, switcher ranking and templates all change together.

### US-27 — Promote a note through its lifespan  (serves FR-LNS-04, FR-VLT-09, NFR-USE-04, FR-LNS-03)
**Phase:** P3 · **Size:** L

As a systems analyst, I want to promote an inbox note into a project, so that the folder model is an action rather than a manual convention.

**Acceptance criteria**

- Given an inbox note with inbound links, when I promote it, then the file moves, frontmatter updates, every link is repaired, and one undo reverses it all.

### US-28 — See every task in one place  (serves FR-LNS-05, FR-LNS-06, FR-VIW-10, FR-VIW-02, FR-VIW-09)
**Phase:** P3 · **Size:** M

As a project manager, I want my checkboxes aggregated across the vault, so that my tasks live where I write instead of in another app.

**Acceptance criteria**

- Given tasks in 30 notes, when I query overdue, then all are returned and tickable from the board.

### US-29 — Build a dashboard that stays a file  (serves FR-LNS-07, FR-LNS-09, FR-PLT-05, NFR-CMP-02)
**Phase:** P3 · **Size:** M

As a vault owner, I want my dashboard to be a Markdown note, so that it survives this app.

**Acceptance criteria**

- Given a dashboard note, when I open it in a plain text editor, then it is readable Markdown containing sq fences.


## Epic E5 — AI and MCP

P4 · Local retrieval, diff-reviewed writes, and the vault exposed to external agents.

### US-30 — Ask my vault a question  (serves FR-AIX-01, FR-AIX-03, FR-AIX-04, FR-AIX-02)
**Phase:** P4 · **Size:** XL

As a daily user, I want to ask a question and get an answer with sources, so that the vault becomes something I can interrogate.

**Acceptance criteria**

- Given a question, when it is answered, then every claim links to the block that produced it.
- Given a graph-connected note two hops from strong hits, then it outranks an unrelated lexical match.

### US-31 — Never be edited behind my back  (serves FR-AIX-05)
**Phase:** P4 · **Size:** M

As a vault owner, I want every AI change to arrive as a diff I accept, so that I can use agents without auditing my own notes.

**Acceptance criteria**

- Given a proposal I reject, when I inspect the vault, then every file is byte-identical.

### US-32 — Keep some folders out of it entirely  (serves FR-AIX-09, FR-AIX-10, NFR-SEC-04)
**Phase:** P4 · **Size:** M

As a vault owner, I want to exclude folders and individual notes from AI, so that sensitive material never leaves the machine.

**Acceptance criteria**

- Given a note with ai: false containing a unique token, when I ask about that token, then it is neither retrieved nor cited.

### US-33 — Let Claude Code query properly  (serves FR-MCP-01, FR-MCP-02, FR-MCP-03, FR-MCP-04, NFR-SEC-05)
**Phase:** P4 · **Size:** M

As an engineer, I want my subagents to ask the vault typed questions, so that they stop grepping a folder.

**Acceptance criteria**

- Given an MCP client, when it runs a query, then results are identical to the in-app result for the same query.
- Given it calls propose_edit, then nothing changes until I accept the diff.

### US-34 — Work with no key at all  (serves FR-AIX-11, FR-AIX-12, NFR-SEC-02, FR-AIX-06, NFR-SEC-01)
**Phase:** P4 · **Size:** S

As a privacy-minded user, I want the app fully usable with no provider configured, so that AI is an addition rather than a dependency.

**Acceptance criteria**

- Given no key and no network, when I use the app, then editing, search, queries, views and extraction all work.

### US-39 — Capture something in one motion  (serves FR-AIX-07)
**Phase:** P4 · **Size:** M

As a busy user, I want to drop content into the inbox and have its type, fields and destination proposed, so that capture never costs me a filing decision.

**Acceptance criteria**

- Given pasted text in the inbox, when capture runs, then a proposal is shown as an editable diff before anything is written.

### US-40 — Draft a systems note from its sources  (serves FR-AIX-08)
**Phase:** P4 · **Size:** L

As a systems analyst, I want a durable note drafted from scattered project notes, so that knowledge is consolidated instead of re-derived.

**Acceptance criteria**

- Given nine selected notes, when synthesis runs, then the draft is presented as a diff and every paragraph carries at least one citation.


## Epic E6 — Platform and performance

P0/P5 · The budgets, the packaging, the history, and everything that makes it shippable.

### US-35 — Stay fast as the vault grows  (serves NFR-PRF-01, NFR-PRF-02, NFR-PRF-03, NFR-PRF-04, NFR-PRF-08, NFR-PRF-10, NFR-PRF-07, NFR-MNT-01, NFR-MNT-02)
**Phase:** P0 → P5 · **Size:** L

As a long-term user, I want the app to feel the same at 50,000 notes as at 500, so that success does not degrade the tool.

**Acceptance criteria**

- Given the synthetic vault, when CI runs, then every performance budget is asserted and a regression fails the build.

### US-36 — Install it like a normal app  (serves FR-PLT-01, FR-PLT-02, FR-PLT-03, NFR-USE-02)
**Phase:** P5 · **Size:** M

As a new user, I want a signed installer and quiet updates, so that nothing about installation makes me distrust it.

**Acceptance criteria**

- Given a fresh macOS machine, when I open the app, then no Gatekeeper warning appears.

### US-37 — Get an earlier version back  (serves FR-HST-01, FR-HST-02, FR-HST-03, FR-HST-04)
**Phase:** P5 · **Size:** L

As a vault owner, I want to see and restore what a note said last month, so that undo works across days rather than sessions.

**Acceptance criteria**

- Given a note edited over three days, when I open its timeline, then I can diff and restore any version as a new change.

05
