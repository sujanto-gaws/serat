---
project: Serat
artifact: SRS / requirements baseline 1.0
phase: 02-requirements
source: https://claude.ai/code/artifact/2fd192d7-40cc-4090-8fc1-8d4ad5213627
source_html: docs/_source/serat-requirements-baseline.html
imported: 2026-09-02
---

Requirements phase · baseline 1.0 (requirements baseline) · August 2026

# Serat Requirements Baseline

135 requirements and 40 user stories derived from the product blueprint, each one traceable to a delivery phase, a phase gate and a way of proving it was met.

Document  
SRS — hybrid

Version  
1.0 (requirements baseline)

Status  
Baselined, pre-P0

Derived from  
Blueprint v0.2

Owner  
Sujanto

1.  [01Scope](#scope)
2.  [02Functional](#fr)
3.  [03Non-functional](#nfr)
4.  [04Stories](#stories)
5.  [05Traceability](#trace)
6.  [06Glossary](#glossary)
7.  [07Data dictionary](#dict)
8.  [08Assumptions](#assumptions)

01

## Scope and reading guide

This document is the requirements baseline for Serat. It restates the product blueprint as verifiable obligations: every requirement carries a priority, a delivery phase, an acceptance criterion and a verification method, and every one of them traces to a user story or is recorded as a deliberate exclusion.

135Requirements

40User stories

86Must-have

4Scope exclusions

Must · 86Should · 37Could · 8Won't · 4

P0

7

P1

29

P2

41

P3

17

P4

26

P5

11

v2

4

Requirements per delivery phase, segmented by MoSCoW priority. Priority is ordinal, so the scale is one hue stepped by lightness rather than four unrelated colours. P2 and P4 carry the heaviest load — that is where scope should be cut first if the schedule slips. Exact counts are in the traceability table below, which is the table view of this chart.

### Identifier scheme

`FR-AREA-nn` for functional requirements, `NFR-AREA-nn` for non-functional ones, `US-nn` for stories, `A-nn` for assumptions and `C-nn` for constraints. Identifiers are permanent: a dropped requirement is marked dropped, never deleted and never reused.

### Priority and verification

| Code    | Meaning                                                                     |
|---------|-----------------------------------------------------------------------------|
| M       | Must — v1 does not ship without it                                          |
| S       | Should — v1 is materially weaker without it; may slip one phase             |
| C       | Could — Included if the phase has room                                      |
| W       | Won't — Explicitly out of scope for v1 — recorded so it is not rediscovered |
| Test    | Automated test in CI                                                        |
| Bench   | Benchmark against the 20k/50k synthetic vault, enforced in CI               |
| Demo    | Manual demonstration against the phase gate                                 |
| Inspect | Code, config or document review                                             |

What this baseline does not decide

The performance numbers are targets carried over from the blueprint, not measurements. Phase P0 exists to replace them with real figures; if it does, this document is re-baselined before P1 begins rather than quietly edited.

02

## Functional requirements

107 requirements across 11 areas. Each states an obligation of the system, not a design; where the blueprint fixed a mechanism, the mechanism appears in the acceptance criterion rather than in the requirement itself.

### FR-VLTVault & files

How the application treats the folder on disk: reading, watching, writing, moving.

| ID        | Requirement                                                                                                                                                                                       | Pri    | Phase | Acceptance criteria                                                                                                                                            | Verify  |
|-----------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|--------|-------|----------------------------------------------------------------------------------------------------------------------------------------------------------------|---------|
| FR-VLT-01 | **Open a vault folder**The system shall open any folder of Markdown files as a vault without requiring import, conversion or a manifest.                                                          | Must   | P1    | Given a folder of 20,000 .md files, when it is opened, then the file tree is populated and navigable within 2 seconds.                                         | Test    |
| FR-VLT-02 | **Files are the only source of truth**The system shall store no note content, metadata or user state anywhere other than the user's own files in the vault.                                       | Must   | P0    | Given a fully indexed vault, when .serat/ is deleted and the app restarted, then every query returns identical results after rebuild.                          | Test    |
| FR-VLT-03 | **Atomic writes**The system shall write every file via a temporary file and rename, never by truncating in place.                                                                                 | Must   | P1    | Given a write is interrupted by process kill, when the vault is inspected, then the file is either fully the old version or fully the new one — never partial. | Test    |
| FR-VLT-04 | **Detect external changes**The system shall detect changes made to vault files by other processes and reflect them in the index.                                                                  | Must   | P1    | Given a file is modified by git or another editor, when 500ms have elapsed, then the index and any open view reflect the change.                               | Test    |
| FR-VLT-05 | **Non-destructive reconciliation**When a file open in the editor changes externally, the system shall apply the change as a minimal edit, preserving cursor position, selection and undo history. | Must   | P1    | Given an open note with the cursor at line 40, when an external process edits line 5, then the cursor remains on line 40 and undo still reaches prior states.  | Test    |
| FR-VLT-06 | **Suppress write echo**The system shall recognise filesystem events caused by its own writes and shall not treat them as external changes.                                                        | Must   | P2    | Given a view-originated field write, when the watcher fires, then exactly one incremental reindex occurs and no editor reload is triggered.                    | Test    |
| FR-VLT-07 | **Ignore rules**The system shall exclude .serat/, .git/ and .obsidian/ from indexing, and shall honour a user-editable ignore file using gitignore syntax.                                        | Should | P1    | Given an ignore entry for /archive, when the vault is indexed, then no note under /archive appears in search or query results.                                 | Test    |
| FR-VLT-08 | **Attachments are visible**The system shall list and permit linking to non-Markdown files in the vault.                                                                                           | Must   | P1    | Given a folder containing a .pdf, when the file tree is opened, then the PDF is listed and can be inserted as a link into a note.                              | Demo    |
| FR-VLT-09 | **Rename with link repair**When a note is renamed or moved, the system shall rewrite every inbound link in one reviewable operation.                                                              | Must   | P3    | Given a note with 50 inbound links, when it is moved to another folder, then zero unresolved links remain and one undo reverses the whole operation.           | Test    |
| FR-VLT-10 | **Multiple vaults**The system shall not support opening more than one vault per window in v1.                                                                                                     | Won't  | v2    | Not applicable — recorded to prevent scope re-entry.                                                                                                           | Inspect |

### FR-EDTEditor

The writing surface itself — Markdown editing, live preview, undo.

| ID        | Requirement                                                                                                                                                | Pri    | Phase | Acceptance criteria                                                                                                                                                 | Verify |
|-----------|------------------------------------------------------------------------------------------------------------------------------------------------------------|--------|-------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------|--------|
| FR-EDT-01 | **Markdown editing**The system shall provide a text editor supporting CommonMark and GitHub Flavored Markdown, including tables, task lists and footnotes. | Must   | P1    | Given a GFM table typed into a note, when the file is read from disk, then it is valid GFM and renders identically in Obsidian.                                     | Test   |
| FR-EDT-02 | **Hybrid live preview**The system shall render Markdown formatting inline, revealing raw syntax only on lines intersecting the selection.                  | Must   | P1    | Given the cursor enters a bold span, when the selection updates, then the \*\* delimiters become visible within one frame; when the cursor leaves, they hide again. | Demo   |
| FR-EDT-03 | **Wikilinks and embeds**The system shall support \[\[note\]\], \[\[note#heading\]\], \[\[note^block\]\] and \![\[embed\]\] syntax, with autocompletion.    | Must   | P1    | Given typing \[\[, when two characters follow, then a ranked completion list appears and selecting an entry inserts a resolving link.                               | Test   |
| FR-EDT-04 | **Block references**The system shall support ^block-id anchors and resolve links that target them.                                                         | Should | P2    | Given a paragraph with ^abc, when another note links \[\[note^abc\]\], then the backlink pane shows that paragraph, not the whole note.                             | Test   |
| FR-EDT-05 | **Inline field assistance**The editor shall highlight key:: value fields and offer completion of field names from the note's declared type.                | Must   | P2    | Given a note with type: project, when the user types a field name in the body, then completions are drawn from the project schema.                                  | Demo   |
| FR-EDT-06 | **Slash commands**The editor shall provide a slash-command menu that inserts Markdown constructs.                                                          | Should | P2    | Given / is typed at the start of an empty line, when a command is chosen, then valid Markdown is inserted and the file remains plain Markdown.                      | Demo   |
| FR-EDT-07 | **Table editing aid**The editor should provide assistance for editing Markdown tables, either as a widget or as column alignment.                          | Could  | P4    | Given a Markdown table, when a cell is edited through the aid, then only that cell's text changes and the table remains valid GFM.                                  | Test   |
| FR-EDT-08 | **Unified undo**Edits originating in a view shall enter the same undo stack as edits made in the editor.                                                   | Must   | P2    | Given a cell edited in a table view, when undo is invoked, then the source file returns to its previous bytes.                                                      | Test   |
| FR-EDT-09 | **Split panes and tabs**The system shall support multiple tabs and side-by-side panes, restorable as named workspaces.                                     | Should | P2    | Given two panes and three tabs, when the app is restarted, then the arrangement is restored.                                                                        | Demo   |
| FR-EDT-10 | **Split preview fallback**The system shall provide a source/preview split mode usable when hybrid live preview is disabled or unavailable.                 | Should | P1    | Given live preview is turned off in settings, when a note is opened, then a split source and rendered view is available with no loss of editing capability.         | Demo   |

### FR-NAVNavigation & search

Finding things: switcher, full-text search, backlinks, palette.

| ID        | Requirement                                                                                                                                 | Pri    | Phase | Acceptance criteria                                                                                                                                               | Verify  |
|-----------|---------------------------------------------------------------------------------------------------------------------------------------------|--------|-------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------|---------|
| FR-NAV-01 | **Quick switcher**The system shall provide fuzzy navigation over note titles, paths, aliases and headings, ranked by recency and frequency. | Must   | P1    | Given a 20,000-note vault, when three characters are typed, then ranked results appear in under 50ms.                                                             | Bench   |
| FR-NAV-02 | **Full-text search**The system shall provide full-text search over note bodies and extracted binary content using BM25 ranking.             | Must   | P1    | Given a phrase present in one note and one PDF, when it is searched, then both are returned with a snippet and a precise location.                                | Test    |
| FR-NAV-03 | **Search scoping**Search shall be filterable by folder, type, tag and lens.                                                                 | Should | P2    | Given a search scoped to type:project, when results return, then no note of another type is included.                                                             | Test    |
| FR-NAV-04 | **Backlinks**The system shall show inbound links to the current note at block precision.                                                    | Must   | P1    | Given a note linked from three paragraphs across two notes, when it is opened, then three entries appear, each showing its source block.                          | Test    |
| FR-NAV-05 | **Unlinked mentions**The system shall surface occurrences of a note's title or aliases that are not yet links.                              | Should | P2    | Given a note titled 'Auth' and another containing the word Auth unlinked, when the first is opened, then the occurrence is listed and can be converted to a link. | Demo    |
| FR-NAV-06 | **Command palette**Every user-invocable action shall be reachable from a searchable command palette.                                        | Must   | P1    | Given the palette is open, when an action name is typed, then the action is listed with its keyboard shortcut.                                                    | Inspect |
| FR-NAV-07 | **Scoped graph view**The system may provide a graph view scoped to a note or a query result. A global graph view is out of scope.           | Could  | P4    | Given a note is open, when the graph is shown, then only notes within two link hops are drawn.                                                                    | Demo    |

### FR-STRStructured data

Types, schemas, fields and the SQ query language.

| ID        | Requirement                                                                                                                                                           | Pri    | Phase | Acceptance criteria                                                                                                                                  | Verify  |
|-----------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------|--------|-------|------------------------------------------------------------------------------------------------------------------------------------------------------|---------|
| FR-STR-01 | **Type notes**A type shall be declared by an ordinary Markdown note specifying field names, kinds, required-ness, enumerations, defaults and field order.             | Must   | P2    | Given a type note declaring due as a date, when a note of that type sets due to text, then a validation warning is raised naming the note and field. | Test    |
| FR-STR-02 | **Schema registry**The system shall load, validate and cache all type notes, and shall reload a type within one second of its note changing.                          | Must   | P2    | Given a type note gains a field, when it is saved, then completions and queries reflect the new field without restart.                               | Test    |
| FR-STR-03 | **Frontmatter indexing**The system shall index YAML frontmatter keys and values as queryable fields.                                                                  | Must   | P1    | Given a note with status: done, when queried by status, then the note is returned.                                                                   | Test    |
| FR-STR-04 | **Inline field indexing**The system shall index key:: value fields occurring anywhere in a note body, excluding occurrences inside code spans and fenced code blocks. | Must   | P2    | Given key:: value inside a fenced code block, when the vault is queried, then no field is created from it.                                           | Test    |
| FR-STR-05 | **Typed values at index time**Field values shall be coerced to text, number or date at index time according to the declared schema.                                   | Must   | P2    | Given 10,000 notes with a due date, when a range query is run, then it executes as an indexed range scan and returns in under 100ms.                 | Bench   |
| FR-STR-06 | **SQ query language**The system shall provide a declarative query language supporting from, where, let, select, sort, group by and view selection.                    | Must   | P2    | Given the reference query in §06, when it is executed, then it returns the expected rows grouped as a board.                                         | Test    |
| FR-STR-07 | **Static type checking**Queries shall be validated against the schema registry before execution, with errors reported at the offending token.                         | Must   | P2    | Given a query referencing a field absent from the type, when it is parsed, then a diagnostic names the field and the query does not execute.         | Test    |
| FR-STR-08 | **Single-statement compilation**Every valid query shall compile to exactly one SQL statement.                                                                         | Must   | P2    | Given any query in the conformance suite, when compiled, then the emitted statement count is exactly one.                                            | Test    |
| FR-STR-09 | **No code execution in queries**The query language shall provide no user-defined functions, loops or host-language escape.                                            | Must   | P2    | Grammar review confirms no production admits arbitrary code; no evaluator is linked into the query path.                                             | Inspect |
| FR-STR-10 | **Embedded query blocks**A fenced sq block inside a note shall render its result set in place.                                                                        | Must   | P2    | Given a note containing an sq block, when it is viewed, then results render inline and update when underlying notes change.                          | Test    |
| FR-STR-11 | **Saved views**A query shall be storable as a standalone view addressable from the sidebar.                                                                           | Should | P3    | Given a saved view, when it is opened from the sidebar, then it renders the same rows as the equivalent embedded block.                              | Demo    |
| FR-STR-12 | **Rollups across links**Queries shall aggregate values from notes one link hop from each result row.                                                                  | Should | P3    | Given a project linking five task notes with estimates, when a sum rollup is queried, then the returned value equals the sum of those five.          | Test    |
| FR-STR-13 | **Schema violation surface**The system shall provide a standing view of typed notes that violate their schema.                                                        | Should | P4    | Given a note whose required field is missing, when the violations view is opened, then the note and the missing field are listed.                    | Test    |
| FR-STR-14 | **Schema migration**The system shall rename or retype a field across all notes of a type as a single reviewable and revertible operation.                             | Should | P5    | Given 400 notes carrying a field, when it is renamed, then a diff preview precedes the write and one undo reverses all 400.                          | Test    |

### FR-VIWViews & write-back

Rendering result sets, and editing source notes through them.

| ID        | Requirement                                                                                                                                                        | Pri    | Phase | Acceptance criteria                                                                                                                                      | Verify |
|-----------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------|--------|-------|----------------------------------------------------------------------------------------------------------------------------------------------------------|--------|
| FR-VIW-01 | **Table view**Query results shall render as a sortable table with typed columns.                                                                                   | Must   | P2    | Given a result set of 500 rows, when a column header is clicked, then rows sort by that column's declared kind, not lexically.                           | Test   |
| FR-VIW-02 | **Board view**Query results shall render as a board grouped by a chosen field.                                                                                     | Should | P3    | Given results grouped by status, when the board renders, then one column exists per enum value declared in the schema.                                   | Demo   |
| FR-VIW-03 | **Calendar view**Query results may render on a calendar keyed by a date field.                                                                                     | Could  | P3    | Given results with a due field, when the calendar renders, then each row appears on its due date.                                                        | Demo   |
| FR-VIW-04 | **List and gallery views**Query results may render as a list or as a gallery of cards.                                                                             | Could  | P3    | Given the same query, when the view kind is changed, then the row set is unchanged and only presentation differs.                                        | Demo   |
| FR-VIW-05 | **Editable cells**A cell in a view shall be editable, writing the new value back to the source note's field.                                                       | Must   | P2    | Given a status cell is changed to done, when the source file is read, then its status field reads done and nothing else changed.                         | Test   |
| FR-VIW-06 | **Field-scoped patching**A write originating in a view shall replace only the bytes of the target field or task marker, and shall never re-serialise the document. | Must   | P2    | Property test over a corpus of real notes: for every field write, every byte outside the field's recorded range is unchanged.                            | Test   |
| FR-VIW-07 | **Stale range guard**A view-originated write shall be rejected if the source file's hash changed since the row was read, and the view shall refresh instead.       | Must   | P2    | Given a file edited externally after a row was read, when a cell write is attempted, then the write is refused and the view refreshes without data loss. | Test   |
| FR-VIW-08 | **Field creation order**Writing to a field a note does not yet have shall append it to frontmatter in the order declared by the type.                              | Should | P2    | Given a typed note missing an optional field, when it is set from a view, then it appears in frontmatter at its schema position.                         | Test   |
| FR-VIW-09 | **Board drag writes status**Dragging a card between board columns shall write the corresponding field value.                                                       | Should | P3    | Given a card dragged from doing to done, when the source note is read, then its status field reads done.                                                 | Test   |
| FR-VIW-10 | **Task interaction from views**Task rows shall be checkable from any view, writing the checkbox state to the source block.                                         | Should | P3    | Given a task ticked in a board, when the source note is read, then the checkbox is marked and no other text changed.                                     | Test   |

### FR-BINBinaries & extraction

Non-Markdown files as first-class, queryable, searchable citizens.

| ID        | Requirement                                                                                                                                       | Pri    | Phase | Acceptance criteria                                                                                                                                              | Verify  |
|-----------|---------------------------------------------------------------------------------------------------------------------------------------------------|--------|-------|------------------------------------------------------------------------------------------------------------------------------------------------------------------|---------|
| FR-BIN-01 | **Assets are indexed rows**Every non-Markdown file in the vault shall be represented by an indexed asset row carrying path, MIME type and size.   | Must   | P1    | Given a vault containing 200 PDFs, when it is indexed, then 200 asset rows exist and are returnable by query.                                                    | Test    |
| FR-BIN-02 | **Sidecar notes**A binary shall be able to carry frontmatter, type, tags, links and annotations through a companion Markdown note named after it. | Must   | P1    | Given contract.pdf and contract.pdf.md declaring type: contract, when contracts are queried, then the PDF is returned as a typed row.                            | Test    |
| FR-BIN-03 | **Paired presentation**The file browser shall present a binary and its sidecar as a single entry.                                                 | Should | P1    | Given a file with a sidecar, when the folder is listed, then one entry appears, not two.                                                                         | Demo    |
| FR-BIN-04 | **Binaries are queryable**Assets shall be returnable by the same query language as notes, including by fields from their sidecar.                 | Must   | P2    | Given typed PDFs with an expires field, when a date-range query is run, then matching PDFs are returned.                                                         | Test    |
| FR-BIN-05 | **Text extraction**The system shall extract text from PDF, DOCX, PPTX, XLSX, CSV, EPUB and EML files into block rows.                             | Must   | P2    | Given a text PDF containing a known phrase, when the vault is searched for it, then the PDF is returned with the page shown.                                     | Test    |
| FR-BIN-06 | **Locators**Every extracted block shall carry a locator identifying its position within the source file — page, slide, cell range or timestamp.   | Must   | P2    | Given a search hit in a PDF, when it is opened, then the viewer opens at the page named by the locator.                                                          | Test    |
| FR-BIN-07 | **Out-of-process extraction**Extraction shall run in a separate short-lived process with a wall-clock limit, a memory cap and no network access.  | Must   | P2    | Given a corpus of malformed and adversarial binaries, when they are indexed, then no crash reaches the main process and each failure is isolated.                | Test    |
| FR-BIN-08 | **Extraction cache**Extraction output shall be cached by content hash and extractor version, and shall survive deletion of the index.             | Must   | P2    | Given 100 OCR'd PDFs, when the index is deleted and rebuilt, then no re-extraction occurs and rebuild completes within the markdown budget plus cache read time. | Test    |
| FR-BIN-09 | **Extraction never blocks**Indexing a binary shall not delay editor responsiveness; the asset row shall appear before extraction completes.       | Must   | P2    | Given a 900-page PDF added to the vault, when typing continues, then keystroke latency stays within NFR-PRF-02.                                                  | Bench   |
| FR-BIN-10 | **Graceful absence of an extractor**A file type with no extractor shall still index as a typed asset row and remain linkable and queryable.       | Must   | P2    | Given a .sketch file, when the vault is indexed, then it appears as an asset with a sidecar and is returned by an asset query.                                   | Test    |
| FR-BIN-11 | **PDF viewer**The system shall display PDFs in-app with page navigation and text selection.                                                       | Must   | P1    | Given a PDF is opened, when a page number is entered, then the viewer navigates to that page.                                                                    | Demo    |
| FR-BIN-12 | **Image viewer**The system shall display common image formats in-app with zoom.                                                                   | Must   | P1    | Given a PNG is opened, when zoom is applied, then the image scales without leaving the app.                                                                      | Demo    |
| FR-BIN-13 | **Office document rendering**The system shall render DOCX and PPTX read-only from extracted structure.                                            | Should | P2    | Given a DOCX is opened, when it renders, then headings and paragraphs are legible and an 'open externally' action is offered.                                    | Demo    |
| FR-BIN-14 | **Sheet view**XLSX and CSV files shall render as a sortable sheet view.                                                                           | Should | P2    | Given a CSV of 5,000 rows, when it is opened, then it renders paginated and sortable within one second.                                                          | Demo    |
| FR-BIN-15 | **Annotation as blocks**Highlighting a passage in a PDF shall create a linkable, quotable block recorded in the sidecar.                          | Should | P3    | Given a highlighted PDF passage, when another note links it, then the backlink pane shows the passage and its page.                                              | Test    |
| FR-BIN-16 | **Optical character recognition**The system shall extract text from scanned PDFs and images using a model downloaded on demand.                   | Should | P4    | Given a scanned PDF and OCR enabled, when indexing completes, then its text is searchable and the asset is flagged as OCR-derived.                               | Test    |
| FR-BIN-17 | **Image metadata as fields**EXIF metadata may be indexed as queryable fields on the image's asset row.                                            | Could  | P4    | Given a photo with a capture date, when images are queried by date, then it is returned.                                                                         | Test    |
| FR-BIN-18 | **Audio and video transcription**The system shall not transcribe audio or video in v1.                                                            | Won't  | v2    | Not applicable — recorded to prevent scope re-entry.                                                                                                             | Inspect |
| FR-BIN-19 | **Binaries are never rewritten**The system shall never write back into a binary file. All user metadata lives in the sidecar.                     | Must   | P2    | Code review confirms no write path targets a non-Markdown file; content hashes of source binaries are unchanged after a full session.                            | Inspect |

### FR-LNSLenses, lifecycle & tasks

Role lenses, the folder lifespan model, task handling, dashboards.

| ID        | Requirement                                                                                                                                             | Pri    | Phase | Acceptance criteria                                                                                                               | Verify |
|-----------|---------------------------------------------------------------------------------------------------------------------------------------------------------|--------|-------|-----------------------------------------------------------------------------------------------------------------------------------|--------|
| FR-LNS-01 | **Lens definitions**A lens shall be declared by an ordinary note specifying its filter, home dashboard and pinned views.                                | Must   | P3    | Given a lens note, when it is saved, then the lens appears in the switcher without restart.                                       | Test   |
| FR-LNS-02 | **Lens switching**Switching lens shall change the sidebar contents, the default dashboard, quick-switcher ranking and offered templates, in one action. | Must   | P3    | Given the PM lens is selected, when the switcher is opened, then results are ranked by that lens's filter.                        | Demo   |
| FR-LNS-03 | **Lifespan tiers**The system shall recognise the inbox, projects, systems, notes and refs folder tiers and expose the tier as a queryable field.        | Should | P3    | Given a note in 10-projects, when tier is queried, then it reports project.                                                       | Test   |
| FR-LNS-04 | **Promote across tiers**Promoting a note shall move the file, rewrite its frontmatter and repair every inbound link as one atomic, undoable operation.  | Must   | P3    | Given an inbox note with inbound links, when it is promoted to a project, then zero links break and one undo reverses everything. | Test   |
| FR-LNS-05 | **Task parsing**Markdown checkboxes with inline fields shall be indexed as task rows carrying status, text, due, owner and estimate.                    | Must   | P3    | Given a checkbox with due:: and owner::, when tasks are queried, then those values populate the task row.                         | Test   |
| FR-LNS-06 | **Task aggregation**Tasks shall be queryable across the whole vault, not only within one note.                                                          | Should | P3    | Given tasks in 30 notes, when an overdue query runs, then all matching tasks are returned with their source note.                 | Test   |
| FR-LNS-07 | **Dashboard notes**A note containing multiple query blocks shall render as a dashboard while remaining a portable Markdown file.                        | Must   | P3    | Given a dashboard note, when opened in a plain text editor, then it is readable Markdown containing sq fences.                    | Demo   |
| FR-LNS-08 | **Typed templates**A template shall be able to declare the type it instantiates, pre-filling required fields.                                           | Should | P2    | Given a project template, when a note is created from it, then all required project fields are present with defaults.             | Test   |
| FR-LNS-09 | **Daily notes**The system may create and open a dated note on a configurable path and template.                                                         | Could  | P2    | Given daily notes are enabled, when today's note is opened, then it is created from the configured template if absent.            | Demo   |
| FR-LNS-10 | **Review queues**The system may provide standing views for inbox triage, orphaned notes and stale systems notes.                                        | Could  | P4    | Given a note with no inbound or outbound links, when the orphan queue is opened, then it is listed.                               | Test   |

### FR-AIXAI layer

Local retrieval, generation, and the trust rules around AI writes.

| ID        | Requirement                                                                                                                                         | Pri    | Phase | Acceptance criteria                                                                                                                                                                         | Verify  |
|-----------|-----------------------------------------------------------------------------------------------------------------------------------------------------|--------|-------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|---------|
| FR-AIX-01 | **Local embedding**The system shall compute embeddings locally, in-process, without any network call.                                               | Must   | P4    | Given the network is disabled, when a note is saved, then its embedding is computed and stored.                                                                                             | Test    |
| FR-AIX-02 | **Incremental embedding**Only chunks whose text hash changed shall be re-embedded on save.                                                          | Must   | P4    | Given a one-word edit in a long note, when it is saved, then exactly one chunk is re-embedded.                                                                                              | Test    |
| FR-AIX-03 | **Hybrid retrieval**Retrieval shall combine lexical, semantic and link-graph signals into a single fused ranking.                                   | Must   | P4    | Given a query matching one note lexically and another semantically two hops away, when retrieval runs, then both appear and the graph-connected result outranks an unrelated lexical match. | Test    |
| FR-AIX-04 | **Block-precise citations**Every AI-generated claim shall cite the block that produced it, including a locator for extracted binary content.        | Must   | P4    | Given a generated summary, when a citation is clicked, then the source block is opened and highlighted.                                                                                     | Demo    |
| FR-AIX-05 | **No unreviewed AI writes**No AI-originated change shall be applied to any file without explicit user acceptance of a displayed diff.               | Must   | P4    | Code review confirms no path applies an AI patch without a user action; a test asserts that rejecting a proposal leaves all files byte-identical.                                           | Inspect |
| FR-AIX-06 | **Provider abstraction**Generation shall work against a hosted API or a local model, selected in settings, with retrieval unaffected by the choice. | Must   | P4    | Given the provider is switched, when the same question is asked, then retrieval results are identical and only generation differs.                                                          | Test    |
| FR-AIX-07 | **Capture agent**The system shall propose a type, frontmatter, links and destination folder for content dropped into the inbox.                     | Should | P4    | Given pasted text in the inbox, when capture runs, then a proposal is shown as an editable diff before anything is written.                                                                 | Demo    |
| FR-AIX-08 | **Synthesis with sources**The system shall draft a note from a set of source notes, with every section citing its sources.                          | Should | P4    | Given nine selected notes, when synthesis runs, then the draft is presented as a diff and every paragraph carries at least one citation.                                                    | Demo    |
| FR-AIX-09 | **Per-folder AI scope**Network scope for AI shall be configurable per folder.                                                                       | Must   | P4    | Given a folder excluded from AI, when a question is asked, then no content from that folder appears in the request payload.                                                                 | Test    |
| FR-AIX-10 | **Frontmatter AI opt-out**A note declaring ai: false shall be excluded from retrieval and from any generation context.                              | Must   | P4    | Given a note with ai: false containing a unique token, when that token is asked about, then the note is neither retrieved nor cited.                                                        | Test    |
| FR-AIX-11 | **Fully usable without AI**Every non-AI feature shall function with no API key configured and no network available.                                 | Must   | P4    | Given no key and no network, when the app is used, then editing, search, queries, views and extraction all work.                                                                            | Test    |
| FR-AIX-12 | **Models download on demand**No embedding or OCR model shall be bundled in the installer.                                                           | Must   | P4    | Installer inspection confirms no model weights are present; first AI use prompts for download.                                                                                              | Inspect |

### FR-MCPMCP server

Exposing the vault to external agents through a typed interface.

| ID        | Requirement                                                                                                                                  | Pri    | Phase | Acceptance criteria                                                                                                             | Verify |
|-----------|----------------------------------------------------------------------------------------------------------------------------------------------|--------|-------|---------------------------------------------------------------------------------------------------------------------------------|--------|
| FR-MCP-01 | **MCP server**The system shall expose the vault over MCP with search, read_note, run_query, list_types and propose_edit operations.          | Must   | P4    | Given an MCP client connects, when it lists tools, then those five operations are advertised with typed schemas.                | Test   |
| FR-MCP-02 | **Single query API**MCP query results shall be identical to in-app results for the same query against the same vault state.                  | Must   | P4    | Given the same query run in-app and over MCP, when results are compared, then they are byte-identical.                          | Test   |
| FR-MCP-03 | **External writes are reviewed**propose_edit shall place changes in the same diff-review queue as in-app AI writes, never applying directly. | Must   | P4    | Given an external agent calls propose_edit, when the vault is inspected, then no file changed until the user accepted the diff. | Test   |
| FR-MCP-04 | **Local-only binding**The MCP server shall be startable and stoppable by the user and shall bind only to a local transport.                  | Should | P4    | Given the server is running, when a remote host attempts to connect, then the connection is refused.                            | Test   |

### FR-HSTHistory

Git-backed versioning, diff and restore. Explicitly not sync.

| ID        | Requirement                                                                                                         | Pri    | Phase | Acceptance criteria                                                                                           | Verify  |
|-----------|---------------------------------------------------------------------------------------------------------------------|--------|-------|---------------------------------------------------------------------------------------------------------------|---------|
| FR-HST-01 | **Git-backed history**The system shall commit vault changes to a git repository on a debounce, excluding the index. | Should | P5    | Given ten edits within a minute, when the debounce elapses, then one commit exists and .serat/ is not in it.  | Test    |
| FR-HST-02 | **Per-note timeline**The system shall show a chronological list of versions for a single note.                      | Should | P5    | Given a note edited on three days, when its timeline is opened, then three entries appear with timestamps.    | Demo    |
| FR-HST-03 | **Diff and restore**The system shall diff any two versions of a note and restore an earlier one as a new change.    | Should | P5    | Given a restore from an earlier version, when it completes, then the change is itself committed and undoable. | Test    |
| FR-HST-04 | **Conflict presentation**Sync conflicts may be presented as two readable notes rather than inline merge markers.    | Could  | P5    | Given a conflicted file, when it is opened, then both sides render as notes and a choice is offered.          | Demo    |
| FR-HST-05 | **No sync engine**The system shall not implement file synchronisation between devices.                              | Won't  | v2    | Not applicable — the vault folder is synced by the user's own tool.                                           | Inspect |

### FR-PLTPlatform

Packaging, distribution, updates, recovery, settings.

| ID        | Requirement                                                                                                                | Pri    | Phase | Acceptance criteria                                                                                                     | Verify  |
|-----------|----------------------------------------------------------------------------------------------------------------------------|--------|-------|-------------------------------------------------------------------------------------------------------------------------|---------|
| FR-PLT-01 | **Cross-platform builds**The system shall ship signed builds for Windows, macOS and Linux from one codebase.               | Must   | P5    | Given a release, when artifacts are listed, then all three platforms are present and install without warnings.          | Demo    |
| FR-PLT-02 | **Code signing and notarisation**macOS builds shall be notarised and Windows builds signed.                                | Must   | P5    | Given a fresh macOS machine, when the app is opened, then no Gatekeeper warning appears.                                | Demo    |
| FR-PLT-03 | **Automatic updates**The system shall check for and apply updates with user consent.                                       | Should | P5    | Given an update is available, when the user accepts, then the app updates and restarts preserving open tabs.            | Demo    |
| FR-PLT-04 | **Crash recovery**Unsaved editor buffers shall be recoverable after an abnormal termination.                               | Must   | P5    | Given unsaved changes and a forced kill, when the app restarts, then the buffer is offered for recovery.                | Test    |
| FR-PLT-05 | **Settings as readable files**Application settings shall be stored as human-readable files that can be version-controlled. | Should | P2    | Given settings are changed, when the settings file is opened in a text editor, then the change is legible and editable. | Inspect |
| FR-PLT-06 | **No plugin API in v1**The system shall not expose a plugin API before v1 ships.                                           | Won't  | v2    | Not applicable — recorded to prevent scope re-entry.                                                                    | Inspect |

03

## Non-functional requirements

28 quality obligations. The performance group is unusual in being numeric and CI-enforced: in this product a budget breach is a build failure, not a backlog item.

### NFR-PRFPerformance

The numeric budgets that constrain every design decision.

| ID         | Requirement                                                                                                                                                  | Pri    | Phase | Acceptance criteria                                                           | Verify  |
|------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------|--------|-------|-------------------------------------------------------------------------------|---------|
| NFR-PRF-01 | **Cold start**The application shall present an interactive window within 400ms and a fully usable vault within 1 second, at 20,000 notes with a warm index.  | Must   | P0    | Measured on the reference machine over 20 runs; p95 within budget.            | Bench   |
| NFR-PRF-02 | **Editor latency**Keystroke to paint shall not exceed 16ms at p95 in a 5,000-line note.                                                                      | Must   | P1    | Benchmark harness reports p95 under 16ms.                                     | Bench   |
| NFR-PRF-03 | **Incremental index**A single note save shall be reflected in the index within 50ms at p95.                                                                  | Must   | P0    | Benchmark over 1,000 saves; p95 within budget.                                | Bench   |
| NFR-PRF-04 | **Full rebuild**A full index rebuild of 20,000 Markdown notes shall complete within 30 seconds.                                                              | Must   | P0    | Benchmark on the synthetic vault; wall-clock within budget.                   | Bench   |
| NFR-PRF-05 | **Query latency**A query returning 500 rows over a 20,000-note vault shall complete within 100ms at p95.                                                     | Must   | P2    | Conformance query suite benchmarked; p95 within budget.                       | Bench   |
| NFR-PRF-06 | **Switcher latency**Quick-switcher results shall render within 50ms of a keystroke at 20,000 notes.                                                          | Must   | P1    | Benchmarked; p95 within budget.                                               | Bench   |
| NFR-PRF-07 | **Memory ceiling**Resident memory shall stay below 400MB with the embedding model loaded and below 200MB without it.                                         | Should | P4    | Measured after a one-hour session on the synthetic vault.                     | Bench   |
| NFR-PRF-08 | **Budgets enforced in CI**Every performance budget shall be asserted in continuous integration; a regression shall fail the build.                           | Must   | P0    | CI configuration review confirms each budget has an asserting job.            | Inspect |
| NFR-PRF-09 | **Extraction budget**Binary extraction shall have its own throughput budget, separate from the Markdown rebuild budget, and shall not be counted against it. | Should | P2    | Benchmark reports extraction throughput independently with its own threshold. | Bench   |
| NFR-PRF-10 | **Scale ceiling**All budgets above shall degrade no worse than linearly to 50,000 notes.                                                                     | Should | P5    | Benchmarks run at 20k and 50k; measured growth is sub-quadratic.              | Bench   |

### NFR-SECSecurity & privacy

What may leave the machine, and what may run untrusted input.

| ID         | Requirement                                                                                                                                                               | Pri  | Phase | Acceptance criteria                                                                                      | Verify  |
|------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------|------|-------|----------------------------------------------------------------------------------------------------------|---------|
| NFR-SEC-01 | **No telemetry**The application shall collect and transmit no usage data.                                                                                                 | Must | P1    | Network capture during a full session shows no outbound request absent explicit AI use.                  | Test    |
| NFR-SEC-02 | **No unconfigured network access**With no AI provider configured, the application shall make no outbound network request other than an explicitly requested update check. | Must | P4    | Network capture confirms silence; update check is user-initiated or consented.                           | Test    |
| NFR-SEC-03 | **Sandboxed extraction**Extractor processes shall have no network access and shall be terminated on exceeding time or memory limits.                                      | Must | P2    | Fault-injection suite confirms termination and isolation; network syscalls are unavailable to the child. | Test    |
| NFR-SEC-04 | **Credential storage**API keys shall be stored in the operating system credential store, never in the vault or in plain configuration.                                    | Must | P4    | Filesystem inspection after configuring a key finds no key material outside the OS store.                | Inspect |
| NFR-SEC-05 | **Local-only services**Any service the application listens on shall bind to a local transport only.                                                                       | Must | P4    | Port scan from another host finds no reachable service.                                                  | Test    |

### NFR-RELReliability

Behaviour under crash, corruption and malformed input.

| ID         | Requirement                                                                                                                        | Pri  | Phase | Acceptance criteria                                                                                                                          | Verify |
|------------|------------------------------------------------------------------------------------------------------------------------------------|------|-------|----------------------------------------------------------------------------------------------------------------------------------------------|--------|
| NFR-REL-01 | **No data loss**No user-authored content shall be lost as a result of a crash, a failed write or a concurrent external edit.       | Must | P1    | Fault-injection suite across write, crash and concurrent-edit scenarios reports zero content loss.                                           | Test   |
| NFR-REL-02 | **Index is recoverable**A corrupted or deleted index shall be fully recoverable by rebuild, with no user action beyond restarting. | Must | P0    | Given a truncated index file, when the app starts, then it detects the corruption and rebuilds automatically.                                | Test   |
| NFR-REL-03 | **Failure isolation**A failure in any one file's parsing or extraction shall not prevent the rest of the vault from indexing.      | Must | P2    | Given a corpus containing malformed files, when indexing completes, then all valid files are indexed and failures are individually reported. | Test   |

### NFR-CMPCompatibility & portability

Interoperation with Obsidian and with plain-text tooling.

| ID         | Requirement                                                                                                                             | Pri  | Phase | Acceptance criteria                                                                                                            | Verify  |
|------------|-----------------------------------------------------------------------------------------------------------------------------------------|------|-------|--------------------------------------------------------------------------------------------------------------------------------|---------|
| NFR-CMP-01 | **Obsidian vault compatibility**The system shall open an existing Obsidian vault without modification, preserving .obsidian/ untouched. | Must | P1    | Given a real Obsidian vault, when opened and used for a session, then Obsidian opens it afterwards with no configuration loss. | Demo    |
| NFR-CMP-02 | **No proprietary content format**All user content shall remain readable and editable with a plain text editor.                          | Must | P1    | Inspection of every file the app writes confirms plain Markdown, YAML or documented plain text.                                | Inspect |
| NFR-CMP-03 | **Index excluded from version control**The index directory shall be gitignored by default and shall never be required for correctness.  | Must | P1    | A fresh clone without .serat/ builds a correct index on first open.                                                            | Test    |

### NFR-USEUsability & accessibility

Keyboard, contrast, themes, destructive-action safety.

| ID         | Requirement                                                                                                                               | Pri    | Phase | Acceptance criteria                                                                                       | Verify  |
|------------|-------------------------------------------------------------------------------------------------------------------------------------------|--------|-------|-----------------------------------------------------------------------------------------------------------|---------|
| NFR-USE-01 | **Keyboard-first operation**Every primary action shall be reachable without a pointing device.                                            | Must   | P1    | Command palette review confirms every primary action is listed and bindable.                              | Inspect |
| NFR-USE-02 | **Accessible presentation**Text shall meet WCAG AA contrast in both themes and all interactive elements shall have a visible focus state. | Should | P5    | Automated contrast audit passes in both themes; keyboard traversal shows focus at every stop.             | Test    |
| NFR-USE-03 | **Light and dark themes**The interface shall support light and dark themes and follow the system setting by default.                      | Should | P2    | Given the OS theme changes, when the app is in system mode, then it follows without restart.              | Demo    |
| NFR-USE-04 | **Destructive actions preview**Any operation that modifies more than one file shall present a preview before writing.                     | Must   | P3    | Given a rename affecting 50 notes, when it is invoked, then a diff preview is shown and can be cancelled. | Demo    |

### NFR-MNTMaintainability

What keeps the codebase verifiable as it grows.

| ID         | Requirement                                                                                                                                 | Pri    | Phase | Acceptance criteria                                                     | Verify  |
|------------|---------------------------------------------------------------------------------------------------------------------------------------------|--------|-------|-------------------------------------------------------------------------|---------|
| NFR-MNT-01 | **Typed IPC boundary**TypeScript types for every command and event shall be generated from the Rust definitions, not hand-written.          | Should | P1    | Build fails if generated types are stale relative to the Rust source.   | Inspect |
| NFR-MNT-02 | **Benchmark vault in repository**A generator for the synthetic benchmark vault shall live in the repository and be runnable by one command. | Must   | P0    | Repository review confirms the generator and its documented invocation. | Inspect |
| NFR-MNT-03 | **Property-tested patcher**The field patcher shall be covered by property tests asserting that no byte outside the target range changes.    | Must   | P2    | Property test suite present and passing over a corpus of real notes.    | Test    |

04

## User stories

The same scope as sections 02 and 03, expressed as work. Sizes are relative (S, M, L, XL), not hours. Where two phases are shown, the first is when the story becomes demonstrable and the second is when the last requirement it draws on lands — a story that keeps improving across phases is recorded as spanning them rather than pretending to finish early.

### E1Vault and editing core

P1 · An editor you could live in: open the real vault, write, link, find, and survive external edits.

US-01

#### Open my real vault

P1M

As a vault owner, I want to open my existing Obsidian vault directly, so that trying Serat costs me nothing and I can keep Obsidian as a fallback.

- Given my Obsidian vault, when I open it in Serat, then wikilinks, embeds, block refs and inline fields all resolve.
- Given I used Serat for a week, when I reopen Obsidian, then no setting or plugin state was lost.

Satisfies FR-VLT-01, FR-VLT-07, NFR-CMP-01, FR-VLT-02, NFR-CMP-03

US-02

#### Write without seeing syntax

P1 → P4XL

As a writer, I want formatting to render as I type, so that I read prose rather than punctuation.

- Given the cursor leaves a bold span, when the selection settles, then the \*\* markers hide within one frame.
- Given the cursor enters that span, then the raw markers reappear and are editable.

Satisfies FR-EDT-01, FR-EDT-02, FR-EDT-06, FR-EDT-07

US-03

#### Link and traverse

P1 → P4M

As a note-taker, I want to link notes and see what links back, so that structure emerges from writing rather than filing.

- Given I type \[\[, when two characters follow, then ranked completions appear.
- Given a note linked from three paragraphs, when I open it, then three block-precise backlinks are listed.

Satisfies FR-EDT-03, FR-NAV-04, FR-EDT-04, FR-NAV-05, FR-NAV-07

US-04

#### Reach any note instantly

P1M

As a daily user, I want to jump to any note in two keystrokes, so that navigation never interrupts thought.

- Given 20,000 notes, when I type three characters, then ranked results render in under 50ms.

Satisfies FR-NAV-01, FR-NAV-06, NFR-PRF-06, NFR-USE-01

US-05

#### Search everything

P1 → P2M

As a researcher, I want one search box that covers the whole vault, so that I never wonder whether something was missed.

- Given a phrase in a note and in a PDF, when I search it, then both are returned with snippets.

Satisfies FR-NAV-02, FR-NAV-03

US-06

#### Survive external edits

P1L

As an engineer, I want Claude Code and git to edit my files while I work, so that my tools do not have to take turns.

- Given an open note, when an external process edits an earlier line, then my cursor and undo history survive.

Satisfies FR-VLT-04, FR-VLT-05

US-07

#### Never lose work

P1 → P5M

As a vault owner, I want a crash to cost me nothing, so that I can trust the app with my only copy.

- Given a forced kill mid-write, when I inspect the file, then it is either wholly old or wholly new.
- Given unsaved changes and a crash, when I restart, then recovery is offered.

Satisfies FR-VLT-03, FR-PLT-04, NFR-REL-01, NFR-REL-02

US-08

#### Fall back when preview misbehaves

P1S

As a pragmatist, I want a split source/preview mode available, so that a live-preview bug never blocks my writing.

- Given live preview is disabled, when I open a note, then split mode offers full editing.

Satisfies FR-EDT-10

US-38

#### Arrange my workspace

P2M

As a daily user, I want panes, tabs and themes I can arrange and keep, so that the tool matches how I actually work rather than resetting each session.

- Given two panes and three tabs, when I restart, then the arrangement is restored.
- Given the OS theme changes, when the app is in system mode, then it follows without restart.

Satisfies FR-EDT-09, NFR-USE-03

### E2Structured knowledge

P2–P5 · Types, the SQ language, and views you can edit through — the answer to Dataview and Bases.

US-09

#### Declare what a project note is

P2L

As a systems analyst, I want to define the shape of a note type, so that structure is enforced instead of remembered.

- Given a type note declaring required fields, when a note of that type omits one, then it appears in the violations view.

Satisfies FR-STR-01, FR-STR-02, FR-STR-03

US-10

#### Put data where the prose is

P2M

As a note-taker, I want fields inline in the body, so that I do not have to hoist every fact into frontmatter.

- Given key:: value mid-paragraph, when I query that key, then the note is returned.
- Given the same text inside a code fence, then no field is created.

Satisfies FR-STR-04, FR-EDT-05

US-11

#### Query in a language I can trust

P2XL

As an engineer, I want a declarative query language with predictable cost, so that a dashboard never makes the app slow.

- Given any conformance query, when compiled, then exactly one SQL statement is emitted.
- Given a 500-row result at 20k notes, when it runs, then it completes within 100ms at p95.

Satisfies FR-STR-06, FR-STR-08, FR-STR-09, NFR-PRF-05, FR-STR-05

US-12

#### Catch my typos before the table is empty

P2M

As a query author, I want field names checked against the schema, so that a mistake is a squiggle rather than a silent empty result.

- Given a query naming a field the type lacks, when it is parsed, then a diagnostic marks that token and nothing executes.

Satisfies FR-STR-07

US-13

#### Put a live table in a note

P2 → P3M

As a project manager, I want query results embedded where I am writing, so that my planning note is also my status view.

- Given an sq fence, when the note renders, then results appear inline and update as source notes change.

Satisfies FR-STR-10, FR-VIW-01, FR-STR-11, FR-VIW-03, FR-VIW-04

US-14

#### Edit through the table

P2XL

As a project manager, I want to change a status directly in the table, so that keeping data current is not a chore.

- Given I change a cell, when I read the source file, then only that field's bytes changed.
- Given the file changed externally since I read the row, then my write is refused and the view refreshes.
- Given I press undo, then the file returns to its previous bytes.

Satisfies FR-VIW-05, FR-VIW-06, FR-VIW-07, FR-EDT-08, FR-VLT-06, FR-VIW-08, NFR-MNT-03

US-15

#### Roll up from linked notes

P3M

As a project manager, I want totals computed across linked notes, so that a project row shows real remaining effort.

- Given a project linking five estimated tasks, when I sum the rollup, then it equals those five estimates.

Satisfies FR-STR-12

US-16

#### See what has drifted

P4M

As a systems analyst, I want a standing list of notes that violate their type, so that structure decays visibly rather than silently.

- Given a typed note missing a required field, when I open the violations view, then the note and field are named.

Satisfies FR-STR-13, FR-LNS-10

US-17

#### Rename a field everywhere

P5L

As a systems analyst, I want to rename a field across every note of a type, so that my schema can evolve without a scripting session.

- Given 400 notes carrying the field, when I rename it, then a diff preview precedes the write and one undo reverses all of it.

Satisfies FR-STR-14

### E3Binaries as citizens

P1–P4 · PDFs, Office files, spreadsheets and images extracted into the same index as notes.

US-18

#### Give a PDF metadata

P1 → P2M

As a vault owner, I want to tag and type a PDF, so that my contracts are data rather than attachments.

- Given contract.pdf with a sidecar declaring type: contract, when I query contracts, then the PDF is returned as a typed row.
- Given a file with a sidecar, when I browse the folder, then I see one entry, not two.

Satisfies FR-BIN-01, FR-BIN-02, FR-BIN-03, FR-VLT-08, FR-BIN-19

US-19

#### Read documents without leaving

P1 → P2M

As a daily user, I want PDFs and images to open in-app, so that reading a source does not mean losing my place.

- Given a PDF, when I open it, then I can navigate pages and select text in-app.

Satisfies FR-BIN-11, FR-BIN-12, FR-BIN-13

US-20

#### Search inside my documents

P2XL

As a researcher, I want the search box to look inside PDFs and decks, so that most of my archive stops being invisible.

- Given a phrase on page 12 of a PDF, when I search it, then the result names the page and opens there.

Satisfies FR-BIN-05, FR-BIN-06

US-21

#### Query a spreadsheet beside my notes

P2L

As an analyst, I want a sheet to be queryable data, so that a budget export can join against my project notes.

- Given an XLSX with a header row, when I query that sheet, then rows are returned as typed records.

Satisfies FR-BIN-14, FR-BIN-04

US-22

#### Not be taken down by a bad file

P2L

As a vault owner, I want one corrupt PDF to be a logged failure rather than a crash, so that the app is safe to point at a real archive.

- Given a corpus of malformed binaries, when indexing runs, then each failure is isolated and the rest of the vault indexes.

Satisfies FR-BIN-07, FR-BIN-10, NFR-REL-03, NFR-SEC-03, FR-BIN-09, NFR-PRF-09

US-23

#### Keep expensive work after a rebuild

P2M

As a vault owner, I want extraction results to survive an index wipe, so that rebuilding costs seconds rather than hours.

- Given 100 extracted PDFs, when I delete the index and reopen, then no re-extraction occurs.

Satisfies FR-BIN-08

US-24

#### Quote a passage from a PDF

P3L

As a researcher, I want to highlight a passage and link to it, so that source material joins the graph instead of sitting beside it.

- Given a highlighted passage, when another note links it, then the backlink shows the passage and its page.

Satisfies FR-BIN-15

US-25

#### Search a scan

P4L

As a vault owner, I want scanned documents to become searchable, so that the oldest part of my archive is not the least usable.

- Given a scanned PDF with OCR enabled, when indexing completes, then its text is searchable and flagged as OCR-derived.

Satisfies FR-BIN-16, FR-BIN-17

### E4Lenses, lifecycle and tasks

P3 · The multi-role working model: lenses, tier promotion, tasks, dashboards.

US-26

#### Switch to my PM head

P3L

As someone working three roles, I want one keystroke to re-frame the whole workspace, so that context switching is a decision rather than a chore.

- Given the PM lens, when I switch to it, then sidebar, dashboard, switcher ranking and templates all change together.

Satisfies FR-LNS-01, FR-LNS-02, FR-LNS-08

US-27

#### Promote a note through its lifespan

P3L

As a systems analyst, I want to promote an inbox note into a project, so that the folder model is an action rather than a manual convention.

- Given an inbox note with inbound links, when I promote it, then the file moves, frontmatter updates, every link is repaired, and one undo reverses it all.

Satisfies FR-LNS-04, FR-VLT-09, NFR-USE-04, FR-LNS-03

US-28

#### See every task in one place

P3M

As a project manager, I want my checkboxes aggregated across the vault, so that my tasks live where I write instead of in another app.

- Given tasks in 30 notes, when I query overdue, then all are returned and tickable from the board.

Satisfies FR-LNS-05, FR-LNS-06, FR-VIW-10, FR-VIW-02, FR-VIW-09

US-29

#### Build a dashboard that stays a file

P3M

As a vault owner, I want my dashboard to be a Markdown note, so that it survives this app.

- Given a dashboard note, when I open it in a plain text editor, then it is readable Markdown containing sq fences.

Satisfies FR-LNS-07, FR-LNS-09, FR-PLT-05, NFR-CMP-02

### E5AI and MCP

P4 · Local retrieval, diff-reviewed writes, and the vault exposed to external agents.

US-30

#### Ask my vault a question

P4XL

As a daily user, I want to ask a question and get an answer with sources, so that the vault becomes something I can interrogate.

- Given a question, when it is answered, then every claim links to the block that produced it.
- Given a graph-connected note two hops from strong hits, then it outranks an unrelated lexical match.

Satisfies FR-AIX-01, FR-AIX-03, FR-AIX-04, FR-AIX-02

US-31

#### Never be edited behind my back

P4M

As a vault owner, I want every AI change to arrive as a diff I accept, so that I can use agents without auditing my own notes.

- Given a proposal I reject, when I inspect the vault, then every file is byte-identical.

Satisfies FR-AIX-05

US-32

#### Keep some folders out of it entirely

P4M

As a vault owner, I want to exclude folders and individual notes from AI, so that sensitive material never leaves the machine.

- Given a note with ai: false containing a unique token, when I ask about that token, then it is neither retrieved nor cited.

Satisfies FR-AIX-09, FR-AIX-10, NFR-SEC-04

US-33

#### Let Claude Code query properly

P4M

As an engineer, I want my subagents to ask the vault typed questions, so that they stop grepping a folder.

- Given an MCP client, when it runs a query, then results are identical to the in-app result for the same query.
- Given it calls propose_edit, then nothing changes until I accept the diff.

Satisfies FR-MCP-01, FR-MCP-02, FR-MCP-03, FR-MCP-04, NFR-SEC-05

US-34

#### Work with no key at all

P4S

As a privacy-minded user, I want the app fully usable with no provider configured, so that AI is an addition rather than a dependency.

- Given no key and no network, when I use the app, then editing, search, queries, views and extraction all work.

Satisfies FR-AIX-11, FR-AIX-12, NFR-SEC-02, FR-AIX-06, NFR-SEC-01

US-39

#### Capture something in one motion

P4M

As a busy user, I want to drop content into the inbox and have its type, fields and destination proposed, so that capture never costs me a filing decision.

- Given pasted text in the inbox, when capture runs, then a proposal is shown as an editable diff before anything is written.

Satisfies FR-AIX-07

US-40

#### Draft a systems note from its sources

P4L

As a systems analyst, I want a durable note drafted from scattered project notes, so that knowledge is consolidated instead of re-derived.

- Given nine selected notes, when synthesis runs, then the draft is presented as a diff and every paragraph carries at least one citation.

Satisfies FR-AIX-08

### E6Platform and performance

P0/P5 · The budgets, the packaging, the history, and everything that makes it shippable.

US-35

#### Stay fast as the vault grows

P0 → P5L

As a long-term user, I want the app to feel the same at 50,000 notes as at 500, so that success does not degrade the tool.

- Given the synthetic vault, when CI runs, then every performance budget is asserted and a regression fails the build.

Satisfies NFR-PRF-01, NFR-PRF-02, NFR-PRF-03, NFR-PRF-04, NFR-PRF-08, NFR-PRF-10, NFR-PRF-07, NFR-MNT-01, NFR-MNT-02

US-36

#### Install it like a normal app

P5M

As a new user, I want a signed installer and quiet updates, so that nothing about installation makes me distrust it.

- Given a fresh macOS machine, when I open the app, then no Gatekeeper warning appears.

Satisfies FR-PLT-01, FR-PLT-02, FR-PLT-03, NFR-USE-02

US-37

#### Get an earlier version back

P5L

As a vault owner, I want to see and restore what a note said last month, so that undo works across days rather than sessions.

- Given a note edited over three days, when I open its timeline, then I can diff and restore any version as a new change.

Satisfies FR-HST-01, FR-HST-02, FR-HST-03, FR-HST-04

05

## Traceability

Requirement to story to phase to gate to verification method. Four requirements carry no story by design — they are scope exclusions, recorded so the decision is not rediscovered as an idea in six months. Nothing else is uncovered.

| ID         | Requirement                             | Pri    | Phase | Stories                           | Verify  | Gate it must satisfy                                                                                                          |
|------------|-----------------------------------------|--------|-------|-----------------------------------|---------|-------------------------------------------------------------------------------------------------------------------------------|
| FR-VLT-01  | **Open a vault folder**                 | Must   | P1    | US-01                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-VLT-02  | **Files are the only source of truth**  | Must   | P0    | US-01                             | Test    | Index under 30s, save-to-index under 50ms, cold start under 1s                                                                |
| FR-VLT-03  | **Atomic writes**                       | Must   | P1    | US-07                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-VLT-04  | **Detect external changes**             | Must   | P1    | US-06                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-VLT-05  | **Non-destructive reconciliation**      | Must   | P1    | US-06                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-VLT-06  | **Suppress write echo**                 | Must   | P2    | US-14                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-VLT-07  | **Ignore rules**                        | Should | P1    | US-01                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-VLT-08  | **Attachments are visible**             | Must   | P1    | US-18                             | Demo    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-VLT-09  | **Rename with link repair**             | Must   | P3    | US-27                             | Test    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-VLT-10  | **Multiple vaults**                     | Won't  | v2    | Scope exclusion — inspection only | Inspect | Out of scope for v1 — no gate                                                                                                 |
| FR-EDT-01  | **Markdown editing**                    | Must   | P1    | US-02                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-EDT-02  | **Hybrid live preview**                 | Must   | P1    | US-02                             | Demo    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-EDT-03  | **Wikilinks and embeds**                | Must   | P1    | US-03                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-EDT-04  | **Block references**                    | Should | P2    | US-03                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-EDT-05  | **Inline field assistance**             | Must   | P2    | US-10                             | Demo    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-EDT-06  | **Slash commands**                      | Should | P2    | US-02                             | Demo    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-EDT-07  | **Table editing aid**                   | Could  | P4    | US-02                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-EDT-08  | **Unified undo**                        | Must   | P2    | US-14                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-EDT-09  | **Split panes and tabs**                | Should | P2    | US-38                             | Demo    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-EDT-10  | **Split preview fallback**              | Should | P1    | US-08                             | Demo    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-NAV-01  | **Quick switcher**                      | Must   | P1    | US-04                             | Bench   | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-NAV-02  | **Full-text search**                    | Must   | P1    | US-05                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-NAV-03  | **Search scoping**                      | Should | P2    | US-05                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-NAV-04  | **Backlinks**                           | Must   | P1    | US-03                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-NAV-05  | **Unlinked mentions**                   | Should | P2    | US-03                             | Demo    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-NAV-06  | **Command palette**                     | Must   | P1    | US-04                             | Inspect | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-NAV-07  | **Scoped graph view**                   | Could  | P4    | US-03                             | Demo    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-STR-01  | **Type notes**                          | Must   | P2    | US-09                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-STR-02  | **Schema registry**                     | Must   | P2    | US-09                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-STR-03  | **Frontmatter indexing**                | Must   | P1    | US-09                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-STR-04  | **Inline field indexing**               | Must   | P2    | US-10                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-STR-05  | **Typed values at index time**          | Must   | P2    | US-11                             | Bench   | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-STR-06  | **SQ query language**                   | Must   | P2    | US-11                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-STR-07  | **Static type checking**                | Must   | P2    | US-12                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-STR-08  | **Single-statement compilation**        | Must   | P2    | US-11                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-STR-09  | **No code execution in queries**        | Must   | P2    | US-11                             | Inspect | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-STR-10  | **Embedded query blocks**               | Must   | P2    | US-13                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-STR-11  | **Saved views**                         | Should | P3    | US-13                             | Demo    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-STR-12  | **Rollups across links**                | Should | P3    | US-15                             | Test    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-STR-13  | **Schema violation surface**            | Should | P4    | US-16                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-STR-14  | **Schema migration**                    | Should | P5    | US-17                             | Test    | A stranger opens a 30k vault and nothing frightening happens                                                                  |
| FR-VIW-01  | **Table view**                          | Must   | P2    | US-13                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-VIW-02  | **Board view**                          | Should | P3    | US-28                             | Demo    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-VIW-03  | **Calendar view**                       | Could  | P3    | US-13                             | Demo    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-VIW-04  | **List and gallery views**              | Could  | P3    | US-13                             | Demo    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-VIW-05  | **Editable cells**                      | Must   | P2    | US-14                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-VIW-06  | **Field-scoped patching**               | Must   | P2    | US-14                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-VIW-07  | **Stale range guard**                   | Must   | P2    | US-14                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-VIW-08  | **Field creation order**                | Should | P2    | US-14                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-VIW-09  | **Board drag writes status**            | Should | P3    | US-28                             | Test    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-VIW-10  | **Task interaction from views**         | Should | P3    | US-28                             | Test    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-BIN-01  | **Assets are indexed rows**             | Must   | P1    | US-18                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-BIN-02  | **Sidecar notes**                       | Must   | P1    | US-18                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-BIN-03  | **Paired presentation**                 | Should | P1    | US-18                             | Demo    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-BIN-04  | **Binaries are queryable**              | Must   | P2    | US-21                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-BIN-05  | **Text extraction**                     | Must   | P2    | US-20                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-BIN-06  | **Locators**                            | Must   | P2    | US-20                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-BIN-07  | **Out-of-process extraction**           | Must   | P2    | US-22                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-BIN-08  | **Extraction cache**                    | Must   | P2    | US-23                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-BIN-09  | **Extraction never blocks**             | Must   | P2    | US-22                             | Bench   | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-BIN-10  | **Graceful absence of an extractor**    | Must   | P2    | US-22                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-BIN-11  | **PDF viewer**                          | Must   | P1    | US-19                             | Demo    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-BIN-12  | **Image viewer**                        | Must   | P1    | US-19                             | Demo    | A week in Serat on the real vault without needing Obsidian                                                                    |
| FR-BIN-13  | **Office document rendering**           | Should | P2    | US-19                             | Demo    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-BIN-14  | **Sheet view**                          | Should | P2    | US-21                             | Demo    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-BIN-15  | **Annotation as blocks**                | Should | P3    | US-24                             | Test    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-BIN-16  | **Optical character recognition**       | Should | P4    | US-25                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-BIN-17  | **Image metadata as fields**            | Could  | P4    | US-25                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-BIN-18  | **Audio and video transcription**       | Won't  | v2    | Scope exclusion — inspection only | Inspect | Out of scope for v1 — no gate                                                                                                 |
| FR-BIN-19  | **Binaries are never rewritten**        | Must   | P2    | US-18                             | Inspect | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-LNS-01  | **Lens definitions**                    | Must   | P3    | US-26                             | Test    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-LNS-02  | **Lens switching**                      | Must   | P3    | US-26                             | Demo    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-LNS-03  | **Lifespan tiers**                      | Should | P3    | US-27                             | Test    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-LNS-04  | **Promote across tiers**                | Must   | P3    | US-27                             | Test    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-LNS-05  | **Task parsing**                        | Must   | P3    | US-28                             | Test    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-LNS-06  | **Task aggregation**                    | Should | P3    | US-28                             | Test    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-LNS-07  | **Dashboard notes**                     | Must   | P3    | US-29                             | Demo    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| FR-LNS-08  | **Typed templates**                     | Should | P2    | US-26                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-LNS-09  | **Daily notes**                         | Could  | P2    | US-29                             | Demo    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-LNS-10  | **Review queues**                       | Could  | P4    | US-16                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-AIX-01  | **Local embedding**                     | Must   | P4    | US-30                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-AIX-02  | **Incremental embedding**               | Must   | P4    | US-30                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-AIX-03  | **Hybrid retrieval**                    | Must   | P4    | US-30                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-AIX-04  | **Block-precise citations**             | Must   | P4    | US-30                             | Demo    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-AIX-05  | **No unreviewed AI writes**             | Must   | P4    | US-31                             | Inspect | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-AIX-06  | **Provider abstraction**                | Must   | P4    | US-34                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-AIX-07  | **Capture agent**                       | Should | P4    | US-39                             | Demo    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-AIX-08  | **Synthesis with sources**              | Should | P4    | US-40                             | Demo    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-AIX-09  | **Per-folder AI scope**                 | Must   | P4    | US-32                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-AIX-10  | **Frontmatter AI opt-out**              | Must   | P4    | US-32                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-AIX-11  | **Fully usable without AI**             | Must   | P4    | US-34                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-AIX-12  | **Models download on demand**           | Must   | P4    | US-34                             | Inspect | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-MCP-01  | **MCP server**                          | Must   | P4    | US-33                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-MCP-02  | **Single query API**                    | Must   | P4    | US-33                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-MCP-03  | **External writes are reviewed**        | Must   | P4    | US-33                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-MCP-04  | **Local-only binding**                  | Should | P4    | US-33                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| FR-HST-01  | **Git-backed history**                  | Should | P5    | US-37                             | Test    | A stranger opens a 30k vault and nothing frightening happens                                                                  |
| FR-HST-02  | **Per-note timeline**                   | Should | P5    | US-37                             | Demo    | A stranger opens a 30k vault and nothing frightening happens                                                                  |
| FR-HST-03  | **Diff and restore**                    | Should | P5    | US-37                             | Test    | A stranger opens a 30k vault and nothing frightening happens                                                                  |
| FR-HST-04  | **Conflict presentation**               | Could  | P5    | US-37                             | Demo    | A stranger opens a 30k vault and nothing frightening happens                                                                  |
| FR-HST-05  | **No sync engine**                      | Won't  | v2    | Scope exclusion — inspection only | Inspect | Out of scope for v1 — no gate                                                                                                 |
| FR-PLT-01  | **Cross-platform builds**               | Must   | P5    | US-36                             | Demo    | A stranger opens a 30k vault and nothing frightening happens                                                                  |
| FR-PLT-02  | **Code signing and notarisation**       | Must   | P5    | US-36                             | Demo    | A stranger opens a 30k vault and nothing frightening happens                                                                  |
| FR-PLT-03  | **Automatic updates**                   | Should | P5    | US-36                             | Demo    | A stranger opens a 30k vault and nothing frightening happens                                                                  |
| FR-PLT-04  | **Crash recovery**                      | Must   | P5    | US-07                             | Test    | A stranger opens a 30k vault and nothing frightening happens                                                                  |
| FR-PLT-05  | **Settings as readable files**          | Should | P2    | US-29                             | Inspect | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| FR-PLT-06  | **No plugin API in v1**                 | Won't  | v2    | Scope exclusion — inspection only | Inspect | Out of scope for v1 — no gate                                                                                                 |
| NFR-PRF-01 | **Cold start**                          | Must   | P0    | US-35                             | Bench   | Index under 30s, save-to-index under 50ms, cold start under 1s                                                                |
| NFR-PRF-02 | **Editor latency**                      | Must   | P1    | US-35                             | Bench   | A week in Serat on the real vault without needing Obsidian                                                                    |
| NFR-PRF-03 | **Incremental index**                   | Must   | P0    | US-35                             | Bench   | Index under 30s, save-to-index under 50ms, cold start under 1s                                                                |
| NFR-PRF-04 | **Full rebuild**                        | Must   | P0    | US-35                             | Bench   | Index under 30s, save-to-index under 50ms, cold start under 1s                                                                |
| NFR-PRF-05 | **Query latency**                       | Must   | P2    | US-11                             | Bench   | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| NFR-PRF-06 | **Switcher latency**                    | Must   | P1    | US-04                             | Bench   | A week in Serat on the real vault without needing Obsidian                                                                    |
| NFR-PRF-07 | **Memory ceiling**                      | Should | P4    | US-35                             | Bench   | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| NFR-PRF-08 | **Budgets enforced in CI**              | Must   | P0    | US-35                             | Inspect | Index under 30s, save-to-index under 50ms, cold start under 1s                                                                |
| NFR-PRF-09 | **Extraction budget**                   | Should | P2    | US-22                             | Bench   | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| NFR-PRF-10 | **Scale ceiling**                       | Should | P5    | US-35                             | Bench   | A stranger opens a 30k vault and nothing frightening happens                                                                  |
| NFR-SEC-01 | **No telemetry**                        | Must   | P1    | US-34                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| NFR-SEC-02 | **No unconfigured network access**      | Must   | P4    | US-34                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| NFR-SEC-03 | **Sandboxed extraction**                | Must   | P2    | US-22                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| NFR-SEC-04 | **Credential storage**                  | Must   | P4    | US-32                             | Inspect | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| NFR-SEC-05 | **Local-only services**                 | Must   | P4    | US-33                             | Test    | A subagent answers via MCP what grep cannot; no unreviewed AI write                                                           |
| NFR-REL-01 | **No data loss**                        | Must   | P1    | US-07                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| NFR-REL-02 | **Index is recoverable**                | Must   | P0    | US-07                             | Test    | Index under 30s, save-to-index under 50ms, cold start under 1s                                                                |
| NFR-REL-03 | **Failure isolation**                   | Must   | P2    | US-22                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| NFR-CMP-01 | **Obsidian vault compatibility**        | Must   | P1    | US-01                             | Demo    | A week in Serat on the real vault without needing Obsidian                                                                    |
| NFR-CMP-02 | **No proprietary content format**       | Must   | P1    | US-29                             | Inspect | A week in Serat on the real vault without needing Obsidian                                                                    |
| NFR-CMP-03 | **Index excluded from version control** | Must   | P1    | US-01                             | Test    | A week in Serat on the real vault without needing Obsidian                                                                    |
| NFR-USE-01 | **Keyboard-first operation**            | Must   | P1    | US-04                             | Inspect | A week in Serat on the real vault without needing Obsidian                                                                    |
| NFR-USE-02 | **Accessible presentation**             | Should | P5    | US-36                             | Test    | A stranger opens a 30k vault and nothing frightening happens                                                                  |
| NFR-USE-03 | **Light and dark themes**               | Should | P2    | US-38                             | Demo    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |
| NFR-USE-04 | **Destructive actions preview**         | Must   | P3    | US-27                             | Demo    | All three role dashboards live in Serat; promote leaves zero broken links                                                     |
| NFR-MNT-01 | **Typed IPC boundary**                  | Should | P1    | US-35                             | Inspect | A week in Serat on the real vault without needing Obsidian                                                                    |
| NFR-MNT-02 | **Benchmark vault in repository**       | Must   | P0    | US-35                             | Inspect | Index under 30s, save-to-index under 50ms, cold start under 1s                                                                |
| NFR-MNT-03 | **Property-tested patcher**             | Must   | P2    | US-14                             | Test    | Every current Dataview query expressible in SQ; write-back survives hostile tests; PDF text findable from the same search box |

06

## Glossary

Canonical vocabulary. Where this document, the blueprint, the code and the UI disagree on a term, this section is what the others are wrong about.

| Term                   | Definition                                                                                                                               | Source   |
|------------------------|------------------------------------------------------------------------------------------------------------------------------------------|----------|
| **Vault**              | A folder of Markdown files and assets that Serat opens. The vault is the product's unit of scope; there is exactly one per window.       | §04      |
| **Note**               | A single Markdown file in the vault. Also the row representing it in the index.                                                          | §05      |
| **Block**              | A parsed structural unit within a note — paragraph, heading, task, list item, code fence, table. The granularity at which Serat indexes. | §05      |
| **Field**              | A typed key/value pair attached to a note, from YAML frontmatter or from a key:: value occurrence in the body.                           | §05      |
| **Frontmatter**        | The YAML block at the top of a note. One of the two sources of fields.                                                                   | §05      |
| **Inline field**       | A key:: value pair written in the note body. Indexed identically to frontmatter, unlike in Obsidian Bases.                               | §03 B    |
| **Wikilink**           | A link written \[\[target\]\], optionally with \#heading, ^block or \|alias.                                                             | §03 A    |
| **Block reference**    | A ^identifier anchor that makes a specific block linkable from elsewhere.                                                                | §05      |
| **Type**               | A declared shape for a note: named fields with kinds, required-ness, enumerations, defaults and order.                                   | §03 B    |
| **Type note**          | The Markdown note that declares a type. Types are content, not configuration.                                                            | §03 B    |
| **Schema registry**    | The in-memory, hot-reloaded set of all types, used to typecheck queries and validate notes.                                              | §04      |
| **SQ**                 | Serat Query — the declarative query language. Compiles to exactly one SQL statement; contains no host-language escape.                   | §06      |
| **Query block**        | A fenced sq code block inside a note whose results render in place.                                                                      | §03 B    |
| **View**               | A rendering of a query's row set: table, board, calendar, gallery or list. The row set is identical across kinds.                        | §03 B    |
| **Rollup**             | An aggregate computed over notes one link hop from a result row.                                                                         | §06      |
| **Write-back**         | Editing a source note by changing a value in a view. Implemented as a byte-range patch, never a document rewrite.                        | §07      |
| **Field-scoped patch** | A write that replaces only the recorded byte range of one field or task marker.                                                          | §07      |
| **Stale guard**        | The check that refuses a view-originated write when the file's hash changed since the row was read.                                      | §07      |
| **Locator**            | The position of an extracted block inside a binary: page, slide, cell range or timestamp. The binary's equivalent of a byte range.       | §05, §08 |
| **Asset**              | A non-Markdown file in the vault, represented in the index by a row of its own.                                                          | §08      |
| **Sidecar note**       | The companion Markdown file that carries an asset's frontmatter, type, tags, links and annotations.                                      | §08      |
| **Extractor**          | A per-format component that converts a binary into block rows. Runs out of process, capped, with no network.                             | §08      |
| **Extraction cache**   | Extraction output stored by content hash and extractor version, surviving index deletion because it is expensive to recompute.           | §08      |
| **Chunk**              | A span of blocks embedded as one vector for semantic retrieval, carrying its heading path as context.                                    | §09      |
| **Hybrid retrieval**   | Fusing lexical (BM25), semantic (vector) and structural (link graph) signals into one ranking.                                           | §09      |
| **Diff queue**         | The review surface where every AI-originated change waits for explicit acceptance. In-app and MCP writes share it.                       | §09      |
| **MCP server**         | The typed external interface onto the same query API the UI uses, so agents see exactly what the app sees.                               | §09      |
| **Lens**               | A role-scoped configuration — filter, dashboard, pinned views — declared as a note and switchable in one keystroke.                      | §03 C    |
| **Lifespan tier**      | The folder level expressing how long an artifact is expected to live: inbox, project, system, note, ref.                                 | §03 C    |
| **Promote**            | Moving a note to a longer-lived tier, rewriting its frontmatter and repairing inbound links atomically.                                  | §03 C    |
| **Index**              | The SQLite database in .serat/. Entirely derived, never synced, and rebuildable from the vault alone.                                    | §05      |
| **Budget**             | A numeric performance limit asserted in CI. A change that breaches one fails the build.                                                  | §03 E    |
| **Gate**               | The exit condition for a delivery phase, stated as an observable outcome rather than a date.                                             | §11      |
| **Benchmark vault**    | A generated synthetic vault of 20,000 notes used for every performance measurement.                                                      | §14      |

07

## Data dictionary

Field-level definition of the index. Everything here is derived from the vault and rebuildable; nothing here is a place to put state the Markdown does not encode.

| Table     | Column                                | Type              | Null | Description                                                                              | Source   |
|-----------|---------------------------------------|-------------------|------|------------------------------------------------------------------------------------------|----------|
| notes     | id                                    | INTEGER           | No   | Surrogate key.                                                                           | §05      |
| notes     | path                                  | TEXT              | No   | Vault-relative canonical path. Unique.                                                   | §05      |
| notes     | title                                 | TEXT              | Yes  | Display title: frontmatter title, else H1, else filename.                                | §05      |
| notes     | type                                  | TEXT              | Yes  | Declared type name; null for untyped notes.                                              | §05      |
| notes     | lens                                  | TEXT              | Yes  | Role lens this note belongs to.                                                          | §03 C    |
| notes     | folder_tier                           | TEXT              | Yes  | inbox \| project \| system \| note \| ref.                                               | §03 C    |
| notes     | kind                                  | TEXT              | No   | note \| asset. Distinguishes Markdown notes from binaries.                               | §08      |
| notes     | asset_path                            | TEXT              | Yes  | For kind=asset, the binary this row represents.                                          | §08      |
| notes     | sidecar_path                          | TEXT              | Yes  | For kind=asset, the .md carrying its frontmatter.                                        | §08      |
| notes     | mtime                                 | INTEGER           | No   | Filesystem modification time at last index.                                              | §05      |
| notes     | hash                                  | TEXT              | No   | blake3 of file bytes. Skips reparse when unchanged; backs the stale guard.               | §05, §07 |
| notes     | word_count                            | INTEGER           | Yes  | Cached for display and ranking.                                                          | §05      |
| blocks    | id                                    | INTEGER           | No   | Surrogate key.                                                                           | §05      |
| blocks    | note_id                               | INTEGER           | No   | Owning note. Cascade delete.                                                             | §05      |
| blocks    | ord                                   | INTEGER           | No   | Position within the note.                                                                | §05      |
| blocks    | kind                                  | TEXT              | No   | para \| heading \| task \| code \| list \| quote \| table.                               | §05      |
| blocks    | block_ref                             | TEXT              | Yes  | Author-supplied ^id, if present.                                                         | §05      |
| blocks    | heading_path                          | TEXT              | Yes  | Breadcrumb of enclosing headings, used as embedding context.                             | §05, §09 |
| blocks    | text                                  | TEXT              | Yes  | Plain text content of the block.                                                         | §05      |
| blocks    | byte_start                            | INTEGER           | Yes  | Start offset in the source file. Null for extracted blocks.                              | §05      |
| blocks    | byte_end                              | INTEGER           | Yes  | End offset in the source file. The write-back handle.                                    | §05, §07 |
| blocks    | locator                               | TEXT              | Yes  | Position inside a binary: p:12, s:7, Sales!A1:D20, t:00:14:22.                           | §08      |
| fields    | note_id                               | INTEGER           | No   | Owning note.                                                                             | §05      |
| fields    | block_id                              | INTEGER           | Yes  | Null means the field came from frontmatter.                                              | §05      |
| fields    | key                                   | TEXT              | No   | Field name.                                                                              | §05      |
| fields    | val_text                              | TEXT              | Yes  | Text projection of the value.                                                            | §05      |
| fields    | val_num                               | REAL              | Yes  | Numeric projection, when the schema declares a number.                                   | §05      |
| fields    | val_date                              | TEXT              | Yes  | ISO-8601 projection, when the schema declares a date. Makes range queries indexed scans. | §05      |
| fields    | source                                | TEXT              | No   | frontmatter \| inline.                                                                   | §05      |
| fields    | byte_start                            | INTEGER           | Yes  | Start of the value's bytes — the patch target.                                           | §07      |
| fields    | byte_end                              | INTEGER           | Yes  | End of the value's bytes.                                                                | §07      |
| links     | src_note                              | INTEGER           | No   | Linking note.                                                                            | §05      |
| links     | src_block                             | INTEGER           | Yes  | Block the link occurs in, for block-precise backlinks.                                   | §05      |
| links     | dst_path                              | TEXT              | Yes  | Raw link target as written.                                                              | §05      |
| links     | dst_note                              | INTEGER           | Yes  | Resolved target; null when unresolved.                                                   | §05      |
| links     | dst_block_ref                         | TEXT              | Yes  | Targeted ^id, when the link addresses a block.                                           | §05      |
| links     | kind                                  | TEXT              | No   | wiki \| embed \| md \| tag \| unresolved.                                                | §05      |
| links     | alias                                 | TEXT              | Yes  | Display text when the link was aliased.                                                  | §05      |
| tasks     | id                                    | INTEGER           | No   | Surrogate key.                                                                           | §05      |
| tasks     | note_id                               | INTEGER           | No   | Owning note.                                                                             | §05      |
| tasks     | block_id                              | INTEGER           | No   | The checkbox block; the write target when ticked from a view.                            | §05      |
| tasks     | status                                | TEXT              | No   | Checkbox state, supporting non-binary markers.                                           | §05      |
| tasks     | text                                  | TEXT              | Yes  | Task text with inline fields stripped.                                                   | §05      |
| tasks     | due                                   | TEXT              | Yes  | From due:: inline field.                                                                 | §05      |
| tasks     | owner                                 | TEXT              | Yes  | From owner:: inline field.                                                               | §05      |
| tasks     | est                                   | REAL              | Yes  | From est:: inline field; the usual rollup target.                                        | §05      |
| types     | name                                  | TEXT              | No   | Type name as referenced by notes.                                                        | §05      |
| types     | path                                  | TEXT              | No   | Path of the type note that declares it.                                                  | §05      |
| types     | spec_json                             | TEXT              | No   | Compiled schema: fields, kinds, required, enums, defaults, order.                        | §05      |
| assets    | note_id                               | INTEGER           | No   | The asset's row in notes. Cascade delete.                                                | §08      |
| assets    | mime                                  | TEXT              | Yes  | Detected MIME type; selects the extractor.                                               | §08      |
| assets    | bytes                                 | INTEGER           | Yes  | File size.                                                                               | §08      |
| assets    | page_count                            | INTEGER           | Yes  | Pages, slides or sheets, where meaningful.                                               | §08      |
| assets    | extractor                             | TEXT              | Yes  | Extractor that produced the blocks.                                                      | §08      |
| assets    | extractor_ver                         | INTEGER           | Yes  | Version; a bump invalidates the cache entry.                                             | §08      |
| assets    | content_hash                          | TEXT              | Yes  | blake3 of the binary. The extraction cache key.                                          | §08      |
| assets    | extracted_at                          | INTEGER           | Yes  | Timestamp of last successful extraction.                                                 | §08      |
| assets    | needs_ocr                             | INTEGER           | Yes  | Set when a PDF yielded no text layer.                                                    | §08      |
| notes_fts | title, body                           | FTS5              | No   | External-content FTS5 index over notes and extracted blocks.                             | §05      |
| chunks    | id, note_id, block_lo, block_hi, text | mixed             | No   | Embedding units spanning a block range, with heading-path context.                       | §05      |
| chunk_vec | chunk_id, embedding                   | vec0 FLOAT\[384\] | No   | Vector index sized for a small local model.                                              | §05      |

08

## Assumptions and constraints

An assumption is something believed true that was not verified; a constraint is a decision already made that requirements may not reopen. Each assumption records what breaks if it turns out to be wrong, because that is the part people forget to write down.

### Assumptions

| ID   | Assumption                                                                                                                             | If it is wrong                                                                                     |
|------|----------------------------------------------------------------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------|
| A-01 | **Single user, single machine**There is one author per vault. No concurrent multi-user editing is designed for.                        | If collaboration becomes a requirement, the write-back and history design both change materially.  |
| A-02 | **Vault fits on local disk**Vaults are local folders, not network shares or object stores.                                             | Network filesystems may break the watcher and atomic-rename assumptions.                           |
| A-03 | **Sync is external**The user runs git, Syncthing or a cloud folder for multi-device sync.                                              | Serat must tolerate files changing underneath it at any moment — hence FR-VLT-04 and FR-VLT-05.    |
| A-04 | **Obsidian remains the reference dialect**Wikilink, embed and inline-field syntax follow Obsidian and Dataview conventions.            | A divergence in Obsidian's syntax would require a parser update to preserve NFR-CMP-01.            |
| A-05 | **One developer, part-time**Effort estimates assume solo part-time delivery of roughly 30 weeks to v1.                                 | Any phase gate slipping compounds; scope is cut before gates are weakened.                         |
| A-06 | **Rust is a learnable cost**The Tauri decision commits the core to Rust rather than Go.                                                | P0 exists in part to measure this friction before the remaining 28 weeks are committed.            |
| A-07 | **Local embedding quality suffices**A small local embedding model gives adequate semantic recall over personal notes.                  | If not, retrieval leans harder on lexical and graph signals rather than adding a cloud dependency. |
| A-08 | **Extraction quality is good enough**Open-source PDF and Office extractors produce text of usable fidelity on the user's real archive. | Move 5 in the blueprint exists to test this before the pillar is committed to.                     |

### Constraints

| ID   | Constraint                                                                                                        | Source               |
|------|-------------------------------------------------------------------------------------------------------------------|----------------------|
| C-01 | **Files must stay plain Markdown**No proprietary or non-standard syntax may be required for any feature.          | §02 Principle 1      |
| C-02 | **The index must be disposable**No feature may depend on state that only exists in .serat/.                       | §02 Principle 1      |
| C-03 | **No JavaScript in the query path**Queries must be statically analysable, which forbids a scripting escape hatch. | §02 Principle 3, §06 |
| C-04 | **No document re-serialisation**Writes patch byte ranges; the app must never rewrite a note wholesale.            | §07                  |
| C-05 | **No AI write without review**There must be no configuration that applies an AI change silently.                  | §02 Principle 4      |
| C-06 | **Performance budgets are binding**A feature that breaches a budget does not ship until it stops breaching it.    | §02 Principle 5      |
| C-07 | **Tauri v2 and Vue 3**The shell, the language and the frontend framework are fixed decisions, not open questions. | §04                  |

Generated from a single requirements dataset, so this page and the companion workbook cannot drift apart. Priority counts: 86 Must, 37 Should, 8 Could, 4 Won't. Coverage: 131 of 135 requirements carry at least one user story; the remaining 4 are scope exclusions verified by inspection.
