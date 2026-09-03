---
project: Serat
artifact: Product blueprint
phase: 01-planning
source: https://claude.ai/code/artifact/65ef67b3-30c5-4ccf-9b4a-324824e62920
source_html: docs/_source/serat-blueprint.html
imported: 2026-09-02
---

Product blueprint · draft v0.1 · August 2026

# Serat

/ˈse.rat/ — Javanese: a written work; also, fibre. Name confirmed.

A local-first knowledge app for people whose notes are a **system**, not a diary. Obsidian treats structure and AI as plugins bolted onto a Markdown editor. Serat treats the vault as a **typed, indexed knowledge base that happens to serialise as Markdown files on disk** — and ships the query engine, the role lenses, and the AI in the core, where they can be fast and correct.

Shell  
Tauri v2 (Rust)

Frontend  
Vue 3 · Vite · TS

Store  
Plain .md + SQLite

Target  
50k notes, 1 laptop

To v1  
~30 weeks

1.  [01The wedge](#wedge)
2.  [02Principles](#principles)
3.  [03Feature set](#features)
4.  [04Architecture](#architecture)
5.  [05Data model](#data)
6.  [06Query language](#query)
7.  [07Write-back](#writeback)
8.  [08Binaries](#binaries)
9.  [09AI layer](#ai)
10. [10Sync & history](#sync)
11. [11Roadmap](#roadmap)
12. [12Risks](#risks)
13. [13Not building](#notbuilding)
14. [14Next moves](#next)

01

## The wedge

There are a dozen Obsidian alternatives. Almost all of them compete on being a nicer editor. That is the one place Obsidian is already good.

The interesting gap is downstream of editing. A vault that has grown past a few thousand notes stops being a pile of prose and becomes a small, badly-typed database — and the tools for interrogating it are all bolt-ons. Obsidian shipped **Bases** (a core plugin storing view definitions in `.base` files) to close part of that gap, but it still reads only YAML frontmatter, offers table and card views only, and cannot be embedded mid-note. Dataview, the community answer, is more powerful but is JavaScript evaluated per-render, read-only, and slows to a crawl on a large vault.

There is a second gap, and it is arguably worse because nobody even argues about it: **everything in an Obsidian vault that is not Markdown is a second-class citizen by construction.** A PDF, a deck, a spreadsheet, a screenshot — you can link to it and preview some of them, but you cannot search inside it, cannot query it, and cannot give it a single field of metadata, because the data model has nowhere to put YAML on a binary. Core search does not read PDF text (that needs a plugin); Office documents are not searched at all, and the request has sat on the forum for years. For anyone whose actual work arrives as contracts, reports, exports and screenshots, that is most of the archive sitting outside the system.

Nobody is shipping the combination: **typed schemas over plain Markdown, a real declarative query engine, writable views, binaries extracted into the same index as notes, and AI that reads the graph rather than the current buffer** — in one fast local binary.

#### Competitive read

| Tool         | Storage                       | Structured query                                 | Non-.md files                                                        | AI posture                                          | Where it leaves room                                            |
|--------------|-------------------------------|--------------------------------------------------|----------------------------------------------------------------------|-----------------------------------------------------|-----------------------------------------------------------------|
| Obsidian     | Plain .md, local              | Bases (frontmatter, read-only) + Dataview plugin | Opaque attachments. PDF text needs a plugin; Office never searchable | Plugin-mediated, per-note context                   | No typing, no write-back, JS query perf, plugin sprawl          |
| Logseq       | .md/.org, outliner            | Datalog queries (steep)                          | Assets + PDF annotation; no Office, no extraction                    | Plugins                                             | Outliner-first model, long DB-rewrite churn, perf               |
| Anytype      | Proprietary local DB          | Typed objects, good                              | Files are objects, but no text extraction                            | Limited                                             | Not plain files — no git, no grep, no Claude Code on the folder |
| SilverBullet | Plain .md, server             | Space-script queries                             | Attachments only                                                     | Minimal                                             | Browser-hosted, small ecosystem, no native perf story           |
| Notion       | Cloud                         | Excellent databases                              | Uploads indexed — cloud-side, not yours                              | Strong, cloud-only                                  | Not local, not yours, not fast offline                          |
| Serat        | Plain .md + rebuildable index | Typed schemas, SQ language, writable views       | Extracted to blocks; typed via sidecar notes                         | Core: local embeddings, graph retrieval, MCP server | —                                                               |

Strategy: zero switching cost

Serat reads an existing Obsidian vault unmodified — wikilinks, embeds, block refs, `key:: value` inline fields, the `.obsidian/` folder ignored but preserved. Day one you can point it at your own vault and keep Obsidian installed as the fallback. That removes the single biggest reason people never try an alternative, and it makes dogfooding possible before the app is good.

02

## Five principles that decide arguments

Every later trade-off in this document falls out of these. When a feature fights one of them, the feature loses.

| Principle                              | What it forbids                                                                                                                                         |
|----------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------|
| Files are truth                        | No state that exists only in the index. Delete `.serat/` and the app rebuilds losslessly — so nothing may live there that the Markdown does not encode. |
| Structure is opt-in, then enforced     | An untyped note is always valid. But once a note declares `type: project`, its schema is checked and violations surface — no silent half-typed data.    |
| Queries are a language, not JavaScript | No arbitrary JS in the render path. Queries are parsed, type-checked against schemas, and compiled to one SQL statement. Predictable cost, no sandbox.  |
| AI reads the graph                     | No "send the current file and hope". Retrieval is hybrid over the whole vault, and every AI write lands as a reviewable diff, never a silent edit.      |
| Fast is a feature with numbers         | No feature ships that pushes cold start past 1s or keystroke latency past one frame at 50k notes. Budgets are in CI, not in the README.                 |

03

## Feature set

Six pillars. Phase chips map to the roadmap in §11 — P1 is the MVP you can live in, P4 is the last thing before v1.

### A. Vault & editing core

table stakes

Nothing here is a differentiator; all of it is disqualifying if missing. Build it once, boringly, and stop.

P1

CommonMark + GFM + wikilinks — `[[link]]`, `![[embed]]`, `^block-id`, tags, footnotes, callouts.

P1

CodeMirror 6 hybrid editor — live preview that renders everything except the line under the cursor. Not a block WYSIWYG.

P1

Filesystem reconciliation — external edits (git pull, Claude Code, Syncthing) land in an open buffer without clobbering.

P1

Backlinks & outgoing links — block-precise, with unlinked mentions.

P1

Quick switcher & command palette — fuzzy over title, path, alias, heading; recency-weighted.

P2

Panes, tabs, saved workspaces — split editing, one workspace per lens.

P2

Templates & daily notes — templates are typed: a template declares the type it instantiates.

P4

Local graph view — scoped to a note or a query result. No global hairball.

### B. Structured data

pillar 1 of 5

The answer to Dataview and Bases. A note is a row; a type is a schema; a view is a query you can edit through.

P2

Type notes — a schema is itself a Markdown note in `20-systems/types/` declaring fields, kinds, enums, required-ness, defaults. Version-controlled with everything else.

P2

Frontmatter *and* inline fields — `key:: value` anywhere in the body is indexed and queryable, which Bases cannot do.

P2

SQ query language — declarative, statically type-checked against the schema registry, compiled to one SQL statement (§06).

P2

Queries anywhere — in a fenced block mid-note, or saved as a standalone view. Same engine, same syntax.

P2

Writable views — edit a cell in a table or drag a card between board columns; the change is patched back into the source note's field (§07). This is the feature Dataview structurally cannot have.

P3

View kinds — table, board, calendar, timeline, gallery, list. Each is a rendering of the same row set.

P3

Rollups across links — `sum(outgoing.tasks.est)`: aggregate over linked notes, not just the current one.

P4

Schema migrations — rename or retype a field across 400 notes as one reviewable, revertible operation.

P4

Validation surface — a standing "schema violations" view; typed notes that drifted show up instead of rotting.

### C. Binaries as first-class citizens

pillar 2 of 5

The blind spot in every plain-file notes app. A PDF, deck, spreadsheet or screenshot gets extracted into the same block rows a note produces — so search, backlinks, queries and AI citations work on it unchanged, with no new machinery.

P1

Sidecar notes — `contract.pdf` gets `contract.pdf.md` holding its frontmatter, type, tags, links and annotations. That one move makes a binary a typed, queryable row. The file browser folds the pair into a single entry.

P2

Text extraction into blocks — PDF, DOCX, PPTX, XLSX, CSV, EPUB, EML. Every extracted block carries a locator (page, slide, cell range) so a search hit or an AI citation points *inside* the file, not just at it.

P2

Spreadsheets as data — a sheet or CSV becomes a queryable table: `from sheet:"budget.xlsx#Q3"` joins against your notes.

P2

In-app viewers — PDF, images, and read-only Office renders built from the extracted structure. "Open in Word" stays available; it stops being the only option.

P3

Annotation as blocks — highlight a PDF passage and get a real block you can `[[link]]` to and quote. Replaces a three-plugin stack.

P4

OCR and image understanding — scanned PDFs and screenshots become searchable; EXIF lands as queryable fields. Models download on demand, never at install.

P4

Extracted text is embedded — "which contracts mention arbitration" searches the PDFs themselves, not their filenames.

### D. Lenses & lifecycle

pillar 3 of 5

Built around the working model you already use: folders by artifact lifespan, roles as lenses over the same vault.

P3

Lens as a top-level mode — PM / analyst / engineer. One keystroke re-skins the sidebar, the default dashboard, quick-switcher ranking, and which templates are offered.

P3

Lens definitions are notes — a lens declares its filter, its home dashboard, its pinned views. Same pattern as types.

P3

Lifespan folders, first class — `00-inbox → 10-projects → 20-systems → 30-notes → 40-refs`. "Promote" moves the file, rewrites frontmatter, and repairs every inbound link atomically.

P3

Task model — checkboxes with inline fields (`due::`, `owner::`, `est::`) become first-class rows; tickable from any view.

P3

Dashboard notes — a note whose body is a grid of query blocks. Your dashboards stay portable Markdown.

P4

Review queues — inbox triage, orphaned notes, stale `20-systems` entries whose projects moved on.

### E. AI layer

pillar 4 of 5

Detailed in §09. The short version: retrieval is local and hybrid, generation is provider-agnostic, and the vault is exposed outward over MCP so Claude Code can operate on it as a first-class client.

P4

Local embedding index — ONNX model in-process; incremental on save. No cloud call to search your own notes.

P4

Hybrid retrieval — BM25 (FTS5) + vector + graph proximity, fused and reranked.

P4

Vault as MCP server — `search`, `read`, `query`, `propose_edit`. Your existing Claude Code subagents get a typed interface to the vault instead of grepping it.

P4

Diff-reviewed writes — every agent edit arrives as a patch you accept, edit, or reject. No silent mutation, ever.

P4

Capture agent — drop anything into the inbox; it proposes type, frontmatter, links, and destination folder.

P4

Synthesis with citations — "draft the systems note for auth from these nine project notes", every claim linked to its source block.

P4

Scoped AI — per-folder network scope; `ai: false` in frontmatter is absolute. Embeddings stay local by default.

### F. Performance & restraint

pillar 5 of 5

Speed here is not an optimisation pass at the end; it is the reason the architecture looks the way it does. Restraint is the other half: no plugin API in v1, and the ecosystem is not the product.

P0

Budgets in CI — a synthetic 50k-note vault in the test suite; a PR that regresses a budget fails.

P0

Cold start \< 1s — at 20k notes, index already warm. Window visible in \< 400ms.

P1

Keystroke → paint \< 16ms — editing never blocks on the indexer; indexing is a background job with a debounce.

P1

Save → index \< 50ms — single-note incremental reindex, not a vault sweep.

P1

Full rebuild \< 30s / 20k notes — parallel parse across cores; the index is disposable, so rebuilding must be cheap.

P4

Resident memory \< 400MB — with the embedding model loaded. Below 200MB without.

04

## Architecture

One Rust process owns the vault, the index, and the query engine. The Vue frontend owns nothing but presentation and the editor buffer. The boundary is deliberately narrow and deliberately typed.

*(embedded image)*

Fig. 1 — The read path. Everything above the vault folder is derived and disposable: delete the index and it rebuilds. The MCP server is not a wrapper around the app, it is a second front door onto the same query API, which is why an external agent can never see a staler vault than the UI does.

#### Layer responsibilities

| Layer           | Owns                                                                                     | Candidate crates / libs                                                 |
|-----------------|------------------------------------------------------------------------------------------|-------------------------------------------------------------------------|
| Vault IO        | Atomic writes, path canonicalisation, debounced watching, echo suppression               | `notify`, `tempfile` + rename, `camino`                                 |
| Parser          | Markdown → AST; extract headings, blocks, links, tasks, tags, inline fields, frontmatter | `comrak` (GFM), `serde_yaml`, custom extensions for `[[ ]]` and `key::` |
| Index           | The schema in §05, incremental upsert, FTS5, vector table                                | `rusqlite` (bundled SQLite), `sqlite-vec`, FTS5                         |
| Schema registry | Load type notes, validate, cache, expose field kinds to the typechecker                  | Plain Rust + `serde`                                                    |
| Query engine    | SQ lexer/parser → typed AST → SQL codegen → execute → row stream                         | `chumsky` or `pest` for the grammar                                     |
| AI service      | Chunking, embedding, hybrid retrieval, provider calls, diff proposals                    | `fastembed`/`ort` (ONNX), `reqwest`, `rmcp`                             |
| IPC             | Typed commands; large result sets streamed as events, never one giant JSON blob          | Tauri v2 commands + channels; `ts-rs` to generate the TS types          |
| Frontend        | Editor buffer, view rendering, lens state, agent panel                                   | Vue 3, Pinia, CodeMirror 6, TanStack Table, Tailwind, shadcn-vue        |

On Rust, given a Go-shaped brain

Tauri v2 (currently `2.11.x`) is the right shell, but it means the core is Rust rather than Go. Keep the Rust surface deliberately small and boring: no async framework gymnastics, no trait-heavy abstractions, one background thread pool, and as much product logic as possible pushed into SQL and into Vue. A Go sidecar binary is technically possible and is the wrong call — it doubles the packaging, signing, and notarisation work for every platform, and puts a process boundary in the hot path. Roughly six focused files of Rust carry this design.

05

## Data model

The one decision everything else hangs from: index at **block** granularity, not note granularity. Block rows are what make block references, precise AI citations, and writable views all possible from a single table.

    -- lives in .serat/index.db — rebuildable, gitignored, never synced

    CREATE TABLE notes (
      id          INTEGER PRIMARY KEY,
      path        TEXT UNIQUE NOT NULL,   -- vault-relative, canonical
      title       TEXT,
      type        TEXT,                    -- FK-ish → types.name, nullable
      lens        TEXT,                    -- pm | analyst | engineer | ...
      folder_tier TEXT,                    -- inbox|project|system|note|ref
      mtime       INTEGER,
      hash        TEXT,                    -- blake3 of bytes; skip reparse if equal
      word_count  INTEGER
    );

    CREATE TABLE blocks (
      id         INTEGER PRIMARY KEY,
      note_id    INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
      ord        INTEGER NOT NULL,
      kind       TEXT NOT NULL,           -- para|heading|task|code|list|quote|table
      block_ref  TEXT,                    -- the ^id, if the author gave one
      heading_path TEXT,                  -- 'Design / Storage' — cheap breadcrumbs
      text       TEXT,
      byte_start INTEGER, byte_end INTEGER  -- the write-back handle
    );

    CREATE TABLE fields (             -- frontmatter AND inline key:: value
      note_id    INTEGER NOT NULL,
      block_id   INTEGER,                -- null ⇒ frontmatter
      key        TEXT NOT NULL,
      val_text   TEXT, val_num REAL, val_date TEXT,  -- typed at index time
      source     TEXT NOT NULL,           -- frontmatter | inline
      byte_start INTEGER, byte_end INTEGER
    );

    CREATE TABLE links (
      src_note   INTEGER NOT NULL, src_block INTEGER,
      dst_path   TEXT, dst_note INTEGER, dst_block_ref TEXT,
      kind       TEXT NOT NULL,           -- wiki | embed | md | tag | unresolved
      alias      TEXT
    );

    CREATE TABLE tasks (
      id INTEGER PRIMARY KEY, note_id INTEGER, block_id INTEGER,
      status TEXT, text TEXT, due TEXT, owner TEXT, est REAL
    );

    CREATE TABLE types (name TEXT PRIMARY KEY, path TEXT, spec_json TEXT);

    -- binaries are notes too (§08)
    --   notes.kind         'note' | 'asset'
    --   notes.asset_path   the binary this row stands for
    --   notes.sidecar_path the .md holding its frontmatter
    --   blocks.locator     'p:12' | 's:7' | 'Sales!A1:D20' | 't:00:14:22'
    CREATE TABLE assets (
      note_id     INTEGER PRIMARY KEY REFERENCES notes(id) ON DELETE CASCADE,
      mime        TEXT, bytes INTEGER, page_count INTEGER,
      extractor   TEXT, extractor_ver INTEGER,
      content_hash TEXT,             -- blake3 of the binary itself
      extracted_at INTEGER, needs_ocr INTEGER
    );

    CREATE VIRTUAL TABLE notes_fts USING fts5(title, body, content='');
    CREATE VIRTUAL TABLE chunk_vec USING vec0(
      chunk_id INTEGER PRIMARY KEY, embedding FLOAT[384]
    );
    CREATE TABLE chunks (id INTEGER PRIMARY KEY, note_id INTEGER,
                        block_lo INTEGER, block_hi INTEGER, text TEXT);

- **Why `byte_start`/`byte_end` on both blocks and fields.** They are the handle that makes a table cell edit a byte-range patch rather than a document rewrite. Without them, writable views mean re-serialising Markdown and losing the author's formatting.
- **Why `hash`.** A watcher event on an unchanged file (very common with sync tools) costs one hash, not one parse.
- **Why `val_text` / `val_num` / `val_date` as three columns.** Typing happens once at index time, so `where due < today()` is an indexed range scan rather than a per-row string parse — this is most of the gap between Serat and Dataview at scale.
- **Why `blocks.locator`.** A Markdown block is anchored by byte range; an extracted block cannot be, because you cannot patch bytes inside a PDF. The locator is the same idea in the binary's own coordinate system — page, slide, cell range, timestamp — so "point back at the source" stays one concept across every file type.
- **Why 384 dimensions.** Sized for a small local model (bge-small / MiniLM class). At 50k notes ≈ 200k chunks that is roughly 300MB of vectors — acceptable on disk, and `sqlite-vec` brute-forces it fast enough at this scale that no ANN index is needed in v1.

06

## SQ — the query language

Familiar enough to guess, restrictive enough to compile. Every query is parsed, checked against the schema registry, and lowered to exactly one SQL statement — so its cost is knowable before it runs.

    ```sq
    from type:project
    where status != "done" and lens has "pm"
    let remaining = rollup(out.tasks[status != "done"].est, sum)
    let slipping  = due < today() + 7d and remaining > 8
    select title, status, owner, due, remaining, slipping
    sort slipping desc, due asc
    as board group by status
    ```

- **`from`** takes a type, a tag, a folder, or a link expression — never a table name. Users never see the schema in §05.
- **`let`** defines a computed column in the query, not in the note. Nothing is written to disk to make a derived value queryable.
- **`rollup`** walks the link graph one hop and aggregates — the thing Dataview needs hand-written JS for.
- **`as`** chooses the view kind. The row set is identical across table, board, calendar and gallery; only the renderer differs.
- **Static typechecking.** Because `types` knows `project.due` is a date, `due < today() + 7d` is validated at parse time and a typo in a field name is an editor squiggle, not an empty table at render time.

Deliberate omission

SQ has no user-defined functions, no loops, and no escape hatch to JavaScript. That is what buys the performance guarantee and the writable views — the engine can always answer "which note and which byte range produced this cell", which is impossible once arbitrary code is in the path. If someone needs a computation SQ cannot express, the answer is to compute it in a note or a script and store it as a field, not to widen the language.

07

## Write-back: the risky mechanism

Editing a cell in a query result and having it land correctly in a Markdown file is the hardest correctness problem in this design, and the one most likely to burn a user's data. It gets designed explicitly rather than discovered.

*(embedded image)*

Fig. 2 — The write-back round trip. The two rules that keep it safe: a patch may only replace the bytes of a single field or task marker, never re-serialise the document; and the writer records the hash it just wrote, so the watcher recognises its own echo and does not fight the editor.

#### Constraints on write-back

- **Field-scoped patches only.** A view can change a frontmatter value, an inline `key:: value`, or a task checkbox. It can never move a block, reflow text, or reorder frontmatter keys. Round-trip fidelity is not a goal — *not round-tripping at all* is the goal.
- **Stale-range guard.** If the file's hash changed since the row was read, the patch is rejected and the view refreshes instead of writing. Same discipline as an optimistic lock.
- **Creating a missing field.** Writing to a field a note does not have appends it to frontmatter in schema order. That is the one structural edit permitted, and it is why type notes declare field order.
- **Undo crosses the boundary.** A view edit pushes onto the same undo stack as an editor edit, and undo re-patches the file. Anything less makes writable views feel dangerous, and users will stop trusting them.

08

## Binaries as first-class citizens

Most of a working archive is not Markdown. Obsidian's model has nowhere to put metadata on a file that isn't a note, so PDFs, decks, spreadsheets and screenshots are opaque attachments — unsearchable, unqueryable, untypeable. Two mechanisms fix that, and neither adds a new subsystem.

### Mechanism 1 — the sidecar note

A binary gets an optional companion note: `contract.pdf` plus `contract.pdf.md`. The sidecar holds frontmatter, type, tags, links and your own annotations. Because it is an ordinary note, a PDF instantly becomes a typed row that `from type:contract where expires < today() + 30d` can return — with no change to the schema registry, the query engine, or write-back.

The alternatives all lose to the first principle. Storing this metadata in the index only would make it un-portable and un-rebuildable; extended attributes do not survive git or sync; one manifest per folder invites merge conflicts. A sidecar is portable, greppable, diffable, and already understood by every part of the system. Its one real cost is folder clutter, which the file browser hides by folding the pair into a single entry.

### Mechanism 2 — the extraction pipeline

On index, a binary is routed by MIME type to an extractor that emits the *same block rows* a Markdown parser emits. That is the whole trick, and it is only cheap because §05 already indexes at block granularity: once a PDF's pages are blocks, full-text search, backlinks, embeddings, hybrid retrieval and block-precise AI citations all work on it with no new code.

*(embedded image)*

Fig. 3 — Why block-granular indexing pays for itself twice. A PDF and a note take different front ends but converge on the same row shape, so every capability built for notes applies to binaries without a parallel code path.

#### Extractors

| Format        | Candidate crate                                                   | Blocks produced                                | Locator      |
|---------------|-------------------------------------------------------------------|------------------------------------------------|--------------|
| PDF (text)    | `pdfium-render`, or a pure-Rust extractor if layout quality holds | Text runs merged into paragraphs by layout     | p:12         |
| PDF (scanned) | \+ ONNX OCR, downloaded on demand                                 | Same, flagged `needs_ocr`                      | p:12         |
| DOCX          | `zip` + `quick-xml` (OOXML text is simple), or `office_oxide`     | Paragraphs with heading level; tables          | ¶:214        |
| PPTX          | Same                                                              | One block per slide, plus speaker notes        | s:7          |
| XLSX / ODS    | `calamine`                                                        | Header row as field names; rows as data        | Sales!A1:D20 |
| CSV / TSV     | `csv`                                                             | Rows as queryable data                         | row:88       |
| Images        | `kamadak-exif` + optional caption/OCR model                       | EXIF as fields; caption and OCR text as blocks | —            |
| EPUB / EML    | `epub`, `mail-parser`                                             | Chapters; message parts and headers            | ch:3         |
| Audio / video | Whisper via ONNX — **v2**                                         | Transcript segments                            | t:00:14:22   |

#### Four rules that keep this from wrecking the app

- **Extraction runs out of process.** Parsing untrusted binaries in the main process is how a notes app becomes a CVE. A short-lived child with a wall-clock limit, a memory cap and no network; a crash marks that file unextractable and the indexer moves on.
- **Two caches, deliberately separated.** The index is disposable by principle — but re-OCRing 400 scanned PDFs is hours, not seconds. Extraction output lives in `.serat/extract/<blake3>.json`, keyed by content hash *and* extractor version, and survives an index wipe. Deleting the index stays cheap; the expensive thing is kept separately and can be synced if you want it to be.
- **Extraction never blocks.** A file appears in the index immediately with metadata only; its text arrives later and views update in place. A 900-page PDF dropped into the vault must not stall a keystroke.
- **A format with no extractor still works.** It indexes as an asset row with a sidecar, so it is still typed, linkable and queryable — just not full-text searchable. Nothing breaks; capability degrades.

Where the line is

Serat reads, extracts, annotates and queries binaries. It does not edit them. Round-tripping a DOCX is a product in itself and would put the same corruption risk as §07 onto files whose format you do not control. Editing a Word document opens Word — and the extracted blocks reindex when it comes back.

09

## AI layer

Three separable pieces: retrieval (always local), generation (provider-agnostic), and exposure (the vault as an MCP server). Only the middle one ever touches the network.

### Retrieval

Chunk on block boundaries with heading-path context prepended, embed with a small ONNX model in-process (`fastembed` or `ort` directly, bge-small / MiniLM class, 384 dims), store in `sqlite-vec`. Re-embed only chunks whose text hash changed on save. Retrieval fuses three signals:

| Signal     | Source                                    | Catches                                                                |
|------------|-------------------------------------------|------------------------------------------------------------------------|
| Lexical    | FTS5 / BM25                               | Exact identifiers, error strings, names — where embeddings are weakest |
| Semantic   | `sqlite-vec` cosine over chunk embeddings | Paraphrase, "the note where I argued against X"                        |
| Structural | `links` table, 1–2 hops from seed notes   | The note that is *about* this, which neither text search finds         |

Fuse with reciprocal-rank fusion, then optionally rerank the top ~40 with the model. Structural boost is what makes this feel different from a semantic-search plugin: a project note two hops from three strong hits outranks a lexically similar note with no graph relationship.

### Generation and trust

- **Provider-agnostic.** Anthropic API by key, or a local model over Ollama. The retrieval half never depends on which.
- **Every write is a diff.** The agent panel proposes patches; you see a per-file diff and accept, edit, or reject. There is no configuration that turns this off — it is the reason the feature is safe to build at all.
- **Citations are block-precise.** Because chunks map to block ranges, every generated claim links to the block that produced it, not just the note.
- **Scope is explicit.** Network scope is set per folder; `ai: false` in frontmatter excludes a note from retrieval and from context, absolutely.

### The vault as an MCP server

This is the highest-leverage feature in the document and among the cheapest to build, because it is a thin second front door onto the query API that already exists. Serat exposes `search`, `read_note`, `run_query`, `list_types` and `propose_edit` over MCP. Your existing Claude Code subagents stop grepping a folder and start asking typed questions — *"which systems notes have no project referencing them"* is one SQ query, not a shell pipeline. Writes from an external agent land in the same diff-review queue as writes from the in-app panel.

10

## Sync and history

Build no sync engine. Build a very good history view.

Because files are truth and the index is disposable, sync is somebody else's problem: git, Syncthing, iCloud, Dropbox all work on the folder as-is, and `.serat/` is gitignored so two machines never fight over an index. Writing a CRDT sync layer would be months of the hardest work in the project, competing with Obsidian Sync on its strongest ground.

What is worth building instead — and cheaply, with the `gix` crate — is **git-backed history**: auto-commit on a debounce, a per-note timeline, diff any two points, restore a version, and a conflict view that renders both sides as notes rather than as merge markers in a text file. That covers the actual user need (undo across days, "what did this note say in June") without owning a distributed systems problem.

11

## Roadmap

Ordered so that the riskiest unknowns are answered first and so that you are dogfooding on your real vault by the end of P1. Weeks assume solo, part-time; each phase ends on a gate, not a date.

P02 weeks

### Spike — prove the numbers

Tauri v2 shell, CodeMirror 6 mounted, read/write one file, `notify` watcher, `comrak` parse and index a generated 20k-note vault into SQLite. No product features whatsoever.

Gate: full index \< 30s, incremental save→index \< 50ms, cold start \< 1s. If these miss badly, the architecture changes before anything is built on it.

P18–10 weeks

### An editor you could actually live in

Hybrid live-preview editing, wikilinks and embeds, backlinks pane, quick switcher, command palette, full-text search, Obsidian vault compatibility, external-edit reconciliation. One flat table view over frontmatter — no query language yet. Also the sidecar/asset model and PDF + image viewers: the data-model half is cheap now and painful to retrofit later.

Gate: you open your own vault in Serat for a week without needing Obsidian.

P26–8 weeks

### Structure — types, SQ, writable tables

Schema registry from type notes, inline field indexing, the SQ parser and typechecker, SQL codegen, embedded query blocks, table view with cell write-back (§07) and its undo integration. Plus the extraction pipeline (§08) for PDF, Office and CSV, and sheets as queryable data.

Gate: every Dataview query in your current vault is expressible in SQ; a table cell edit survives a hostile test — concurrent external edit, undo, redo, crash mid-write; and a phrase inside a PDF is findable from the same search box as a phrase in a note.

P34–5 weeks

### Lenses, lifecycle, tasks

Lens definitions and the switcher, dashboard notes, promote-across-tiers with link repair, task model and board/calendar views, rollups.

Gate: your PM, analyst and engineer dashboards all live in Serat, and promoting an inbox note to a project leaves zero broken links.

P46–8 weeks

### AI — retrieval, agent panel, MCP

Local embedding pipeline over notes *and* extracted blocks, hybrid retrieval, agent panel with diff review, capture agent, synthesis with citations, MCP server. OCR and image captioning land here too, behind on-demand model downloads. Ship MCP early in the phase: it is the cheapest piece and the one that changes your daily workflow most.

Gate: a Claude Code subagent can answer a question about the vault through MCP that grep cannot, and no AI write has ever landed unreviewed.

P54 weeks

### Harden and ship v1

Performance pass against the 50k budget, schema migration tooling, validation view, git history view, crash recovery, packaging, code signing and notarisation, auto-update.

Gate: a stranger installs it, opens a 30k-note vault, and nothing frightening happens.

#### Held for v2 — deliberately

WASM plugin API with capability scoping · mobile client · audio and video transcription · publish/share · canvas · multi-vault · collaboration. Each of these is a product in its own right, and any of them started before v1 will eat it.

12

## Risks, honestly

| Risk                                      | Severity | Mitigation                                                                                                                                                                                                                      |
|-------------------------------------------|----------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Write-back corrupts notes                 | Critical | Byte-range patches only; hash guard; atomic rename; git auto-commit before any view-originated write in the first releases. Property-test the patcher against a corpus of real notes.                                           |
| Rust ramp-up eats the schedule            | High     | Small, boring Rust surface; product logic in SQL and Vue. P0 exists partly to find out how much friction this actually is before committing 30 weeks.                                                                           |
| CodeMirror 6 hybrid preview is deep water | High     | It is the single largest frontend cost. Start from an existing CM6 Markdown live-preview implementation rather than from zero; time-box it inside P1 and fall back to split preview if it overruns.                             |
| Obsidian's plugin ecosystem               | Medium   | Do not compete on breadth — it is unwinnable. Compete on the four pillars plus vault compatibility, so leaving is never all-or-nothing.                                                                                         |
| Scope creep into "second Obsidian"        | Medium   | §13 is the contract. Anything on that list needs an explicit decision to reopen, not a quiet afternoon.                                                                                                                         |
| Obsidian closes the gap                   | Medium   | Bases is improving and will keep improving. The parts hardest for them to copy are writable views and a core AI layer — lead with those, not with query syntax.                                                                 |
| Binary parsing is an attack surface       | High     | Every extractor runs in a short-lived child process with a wall-clock limit, a memory cap and no network. A crash marks the file unextractable rather than taking the app down. Fuzz the MIME dispatcher, not just the parsers. |
| Extractors are a long tail                | Medium   | One format per iteration behind a trait — PDF first, Office next, the rest optional. A format with no extractor still indexes as a typed asset row, so coverage gaps degrade capability instead of breaking the vault.          |
| Embedding model ships as a fat binary     | Low      | Download the model on first AI use, not at install. The app must be fully useful with the AI layer never enabled.                                                                                                               |
| Solo project loses momentum               | High     | The gates are chosen so each phase ends with something you personally use daily. Dogfooding is the momentum plan.                                                                                                               |

13

## What this is deliberately not

A blueprint is as much about the closed doors as the open ones. Each of these is a reasonable product; none of them is this one.

- **Not a block-based WYSIWYG.** The editor is Markdown with live preview. Notion's editing model and plain-file truth are in tension, and truth wins.
- **Not a real-time collaboration tool.** Single-user, local-first. Collaboration means git.
- **Not a mobile app in v1.** Tauri v2 supports mobile; the editor and view surface do not survive the shrink without a separate design pass.
- **Not a plugin marketplace.** No plugin API until v1 ships, and then WASM-sandboxed with scoped capabilities — not "arbitrary JS with filesystem access".
- **Not an Office editor.** Binaries are read, extracted, annotated and queried — never rewritten. Round-tripping a DOCX carries §07-scale corruption risk on a format you do not control.
- **Not a sync service.** §10.
- **Not a canvas or whiteboard.** Large, visual, and orthogonal to every pillar here.
- **Not a publishing platform.** Static export exists; hosting does not.

14

## Next five moves

Concrete enough to start this week. None of them require committing to the whole thing.

1.  **Generate the benchmark vault.** A script that emits 20k realistic notes — frontmatter, wikilinks, inline fields, tasks, a plausible link distribution. Everything downstream is measured against it, so it exists before any app code.
2.  **Run the P0 spike.** Tauri v2 + `comrak` + `rusqlite`, no UI beyond a text box. Index the benchmark vault and record the four numbers. This is two weeks that can save thirty.
3.  **Write the SQ grammar as a document, then as a parser.** Take ten real Dataview queries from your current vault and write each one in SQ by hand. If any is awkward, the language is wrong — and that is far cheaper to learn on paper.
4.  **Prototype the write-back patcher standalone.** A Rust function: given a file, a field key, and a new value, produce new bytes. Property-test it against your real vault with the assertion that nothing outside the field's byte range changes. This is the riskiest mechanism in the design and it can be de-risked with no app around it.
5.  **Spike one extractor against your real archive.** PDF text to blocks, out of process, cached by hash — pointed at whatever folder of contracts, specs or papers you already have. It tells you fast whether extraction quality is good enough to build a pillar on, and it is independent of everything else in P0.
6.  **Ship the MCP server first, against Obsidian.** Point the P0 indexer at your existing vault and expose `search` and `run_query` over MCP. You get real value in your Claude Code workflow within weeks, and it validates the query API long before there is a UI to render it.

Draft v0.1 — the numbers in §03 are targets, not measurements, and P0 exists to replace them with real ones. Tool versions current as of August 2026 (Tauri 2.11.x). Name confirmed: *Serat*, for the double sense of a written work and a fibre. Known collision to watch if this ever goes public — Serato, the DJ software, is phonetically one letter away and owns the search results.
