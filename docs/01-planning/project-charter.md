---
project: Serat
artifact: Project charter
phase: 01-planning
derived_from: Serat Blueprint draft v0.1 (August 2026) — full text in docs/_source/serat-blueprint.html
---

# Project Charter — Serat

**Owner:** Sujanto (lead) · **Status:** draft, pre-P0 · **Baseline:** Blueprint v0.1, August 2026 · **Downstream:** Requirements Baseline 1.0 (`docs/02-requirements/serat-requirements-baseline.md`), Design Description (`docs/03-design/serat-design-description.md`)

## Vision

Serat (/ˈse.rat/ — Javanese: a written work; also, fibre) is a local-first knowledge app for people whose notes are a **system**, not a diary. Where Obsidian treats structure and AI as plugins bolted onto a Markdown editor, Serat treats the vault as a **typed, indexed knowledge base that happens to serialise as plain Markdown files on disk**, and ships the query engine, the role lenses, and the AI in the core, where they can be fast and correct.

Nobody is shipping the combination Serat targets: typed schemas over plain Markdown, a real declarative query engine, writable views, binaries extracted into the same index as notes, and AI that reads the graph rather than the current buffer — in one fast local binary.

## Problem and opportunity (the wedge)

Most Obsidian alternatives compete on being a nicer editor, which is the one place Obsidian is already good. The gaps are downstream of editing:

1. **Structured data is a bolt-on.** Past a few thousand notes a vault becomes a small, badly-typed database. Obsidian's Bases reads only frontmatter, offers table/card views, cannot be embedded mid-note; Dataview is per-render JavaScript, read-only, and slow on large vaults.
2. **Non-Markdown files are second-class by construction.** PDFs, decks, spreadsheets and screenshots cannot be searched inside, queried, or given a single field of metadata. For anyone whose work arrives as contracts, reports, exports and screenshots, most of the archive sits outside the system.

**Go-to-market strategy — zero switching cost.** Serat reads an existing Obsidian vault unmodified (wikilinks, embeds, block refs, `key:: value` inline fields; `.obsidian/` ignored but preserved). Users can point it at their own vault on day one and keep Obsidian as the fallback, which also makes dogfooding possible before the app is good.

### Competitive position

| Tool | Storage | Structured query | Non-.md files | AI posture | Where it leaves room |
|---|---|---|---|---|---|
| Obsidian | Plain .md, local | Bases (frontmatter, read-only) + Dataview plugin | Opaque attachments; PDF text needs a plugin; Office never searchable | Plugin-mediated, per-note context | No typing, no write-back, JS query perf, plugin sprawl |
| Logseq | .md/.org, outliner | Datalog queries (steep) | Assets + PDF annotation; no Office, no extraction | Plugins | Outliner-first model, DB-rewrite churn, perf |
| Anytype | Proprietary local DB | Typed objects, good | Files are objects, no text extraction | Limited | Not plain files — no git, no grep, no Claude Code on the folder |
| SilverBullet | Plain .md, server | Space-script queries | Attachments only | Minimal | Browser-hosted, small ecosystem, no native perf story |
| Notion | Cloud | Excellent databases | Uploads indexed cloud-side | Strong, cloud-only | Not local, not yours, not fast offline |
| **Serat** | Plain .md + rebuildable index | Typed schemas, SQ language, writable views | Extracted to blocks; typed via sidecar notes | Core: local embeddings, graph retrieval, MCP server | — |

## Guiding principles

Every later trade-off falls out of these. When a feature fights one of them, the feature loses.

| Principle | What it forbids |
|---|---|
| Files are truth | No state that exists only in the index. Delete `.serat/` and the app rebuilds losslessly. |
| Structure is opt-in, then enforced | An untyped note is always valid; once a note declares a type, its schema is checked and violations surface. |
| Queries are a language, not JavaScript | No arbitrary JS in the render path. Queries are parsed, type-checked, compiled to one SQL statement. |
| AI reads the graph | Retrieval is hybrid over the whole vault; every AI write lands as a reviewable diff, never a silent edit. |
| Fast is a feature with numbers | Nothing ships that pushes cold start past 1s or keystroke latency past one frame at 50k notes. Budgets live in CI. |

## Goals and success metrics

| Goal | Metric | Target |
|---|---|---|
| Performance at scale (50k notes, one laptop) | Cold start (20k notes, warm index) | < 1 s; window visible < 400 ms |
| | Keystroke → paint | < 16 ms; editing never blocks on the indexer |
| | Save → index (single-note incremental) | < 50 ms |
| | Full rebuild, 20k notes | < 30 s |
| | Resident memory | < 400 MB with embedding model; < 200 MB without |
| Obsidian compatibility | Existing vault opens unmodified; user lives in Serat for a week without Obsidian | P1 gate |
| Structured data parity and beyond | Every Dataview query in the owner's vault expressible in SQ; writable table cell survives hostile tests (concurrent edit, undo/redo, crash mid-write) | P2 gate |
| Binaries first-class | A phrase inside a PDF findable from the same search box as a phrase in a note | P2 gate |
| Lenses and lifecycle | PM / analyst / engineer dashboards live in Serat; promote-across-tiers leaves zero broken links | P3 gate |
| AI that reads the graph | A Claude Code subagent answers a vault question via MCP that grep cannot; no AI write ever lands unreviewed | P4 gate |
| Shippable v1 | A stranger installs it, opens a 30k-note vault, nothing frightening happens | P5 gate |

Numbers are targets, not measurements; P0 exists to replace them with real ones.

## Scope

**In scope for v1 (six pillars):**

- **A. Vault & editing core** (table stakes): CommonMark + GFM + wikilinks, CodeMirror 6 hybrid live-preview editor, filesystem reconciliation with external edits, backlinks, quick switcher and command palette, panes/tabs/workspaces, typed templates and daily notes, local graph view.
- **B. Structured data**: type notes as schemas, frontmatter and inline `key:: value` fields indexed, the SQ query language (declarative, statically type-checked, compiled to one SQL statement), queries anywhere, writable views, view kinds (table/board/calendar/timeline/gallery/list), rollups across links, schema migrations, validation surface.
- **C. Binaries as first-class citizens**: sidecar notes (`file.pdf.md`), out-of-process text extraction into block rows (PDF, DOCX, PPTX, XLSX, CSV, EPUB, EML) with locators, spreadsheets as queryable data, in-app viewers, annotation as blocks, OCR and image understanding, extracted text embedded.
- **D. Lenses & lifecycle**: lens as top-level mode (PM / analyst / engineer), lens definitions as notes, lifespan folders (`00-inbox → 10-projects → 20-systems → 30-notes → 40-refs`) with atomic promote and link repair, task model, dashboard notes, review queues.
- **E. AI layer**: local ONNX embedding index, hybrid retrieval (BM25 + vector + graph proximity), vault as MCP server (`search`, `read_note`, `run_query`, `list_types`, `propose_edit`), diff-reviewed writes, capture agent, synthesis with citations, per-folder AI scope.
- **F. Performance & restraint**: budgets in CI against a synthetic 50k-note vault; no plugin API in v1.
- **Sync & history**: no sync engine; git-backed history view (auto-commit on debounce, per-note timeline, diff/restore, conflict view) via `gix`.

**Out of scope for v1 (deliberate — reopening any of these needs an explicit decision):**

- Not a block-based WYSIWYG editor; not a real-time collaboration tool; not a mobile app; not a plugin marketplace (no plugin API until after v1, then WASM-sandboxed); not an Office editor (binaries are read, extracted, annotated, queried — never rewritten); not a sync service; not a canvas/whiteboard; not a publishing platform.
- Held for v2: WASM plugin API with capability scoping, mobile client, audio/video transcription, publish/share, canvas, multi-vault, collaboration.

## Technical baseline

Tauri v2 (Rust core) shell · Vue 3 / Vite / TypeScript frontend · plain `.md` files + rebuildable SQLite index (`.serat/index.db`, block-granular, FTS5 + `sqlite-vec`) · single-user, local-first. Architecture, data model, SQ grammar, write-back rules and extraction pipeline are specified in the Design Description; the original blueprint text is preserved in `docs/_source/serat-blueprint.html`.

## Milestones

Solo, part-time; each phase ends on a gate, not a date. ~30 weeks to v1.

| Milestone | Duration | Deliverable | Gate |
|---|---|---|---|
| P0 — Spike: prove the numbers | 2 weeks | Tauri v2 shell, CodeMirror 6 mounted, read/write one file, `notify` watcher, `comrak` parse, 20k-note benchmark vault indexed into SQLite. No product features. | Full index < 30 s, save→index < 50 ms, cold start < 1 s. If missed badly, architecture changes before anything is built on it. |
| P1 — An editor you could live in | 8–10 weeks | Hybrid live-preview editing, wikilinks/embeds, backlinks, quick switcher, command palette, full-text search over notes, Obsidian vault compatibility, external-edit reconciliation, sidecar/asset model, PDF + image viewers. *(Owner ruling 2026-09-24: the flat table view stays in P2 with FR-VIW-01; search inside PDFs moves to P2 with extraction — FR-NAV-02 split.)* | Owner opens their own vault in Serat for a week without needing Obsidian. |
| P2 — Structure: types, SQ, writable tables | 6–8 weeks | Schema registry, inline field indexing, SQ parser/typechecker/codegen, embedded query blocks, table view with cell write-back and undo, extraction pipeline (PDF, Office, CSV), sheets as data. | Every Dataview query expressible in SQ; cell edit survives hostile test; phrase inside a PDF findable from the note search box. |
| P3 — Lenses, lifecycle, tasks | 4–5 weeks | Lens definitions and switcher, dashboard notes, promote-across-tiers with link repair, task model, board/calendar views, rollups. | PM, analyst and engineer dashboards live in Serat; promotion leaves zero broken links. |
| P4 — AI: retrieval, agent panel, MCP | 6–8 weeks | Local embedding pipeline over notes and extracted blocks, hybrid retrieval, agent panel with diff review, capture agent, synthesis with citations, MCP server (ship early in the phase), OCR/captioning behind on-demand downloads. | A Claude Code subagent answers a vault question via MCP that grep cannot; no AI write has landed unreviewed. |
| P5 — Harden and ship v1 | 4 weeks | Performance pass against 50k budget, schema migration tooling, validation view, git history view, crash recovery, packaging, signing/notarisation, auto-update. | A stranger installs it, opens a 30k-note vault, nothing frightening happens. |

## Immediate next moves

1. Generate the 20k-note benchmark vault (frontmatter, wikilinks, inline fields, tasks, plausible link distribution) — before any app code.
2. Run the P0 spike (Tauri v2 + `comrak` + `rusqlite`, no UI beyond a text box) and record the four numbers.
3. Write the SQ grammar as a document first: hand-translate ten real Dataview queries from the current vault.
4. Prototype the write-back patcher standalone and property-test it against the real vault (nothing outside the field's byte range changes).
5. Spike one extractor (PDF → blocks, out of process, cached by hash) against a real folder of documents.
6. Ship the MCP server first, against the existing Obsidian vault, exposing `search` and `run_query`.

## Notes

Name confirmed: *Serat*. Known collision to watch if this goes public: Serato (DJ software) is phonetically one letter away and owns the search results. Tool versions current as of August 2026 (Tauri 2.11.x).
