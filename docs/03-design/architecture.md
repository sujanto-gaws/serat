---
project: Serat
artifact: Software design description / architecture
phase: 03-design
derived_from: Serat Design Description 1.0 (design baseline, August 2026) — original with six figures in docs/_source/serat-design-description.html
---

# Architecture Overview — Serat

**Owner:** Sujanto (architect role) · **Version:** 1.0 (design baseline) · **Status:** Baselined, pre-P0 · **Upstream:** `docs/02-requirements/prd.md` (135 requirements) · **Decisions:** 10 ADRs in `adr/`

How the 135 requirements are actually built: the crate graph, the indexing pipeline, the query compiler, the write-back patcher, the extraction sandbox and the ten decisions that would be expensive to reverse. The original document has six figures; they did not survive conversion to Markdown and are referenced below by figure number — open the HTML original to see them.

> The design description names companion repo-ready files — `schema/001_init.sql`, `grammar/sq.ebnf`, `grammar/lowering.md`, `grammar/conformance.md`, `contracts/ipc.rs`, `contracts/extractor.rs`, `contracts/mcp-tools.json` — and states that where this document and those files disagree, the files are correct. Those files are **not yet in this repository**; producing them is the architect's first implementation-phase task.

## Context diagram

This document describes how Serat is built. It takes the 135 requirements of the requirements baseline as given and records the structures, algorithms and interfaces that satisfy them, together with the alternatives that were rejected and why.

It is a companion to the repo-ready design files, which carry the parts a compiler, a database or a test harness consumes: the runnable DDL, the SQ grammar and its lowering, the IPC contract, the extractor trait and the MCP tool schemas. Where this document describes a mechanism, those files define it; where they disagree, the files are correct and this document is stale.

### 1.1 Design goals, in priority order

When two goals conflict, the higher one wins. This ordering is the tie-breaker for every decision in the ADR register.

| \#  | Goal                                           | What it forbids                                                                                                                         |
|-----|------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------------|
| 1   | The vault survives the application             | No state that only the index encodes; no format only Serat can read; no write that rewrites a file the user did not ask to change.      |
| 2   | Nothing blocks the keystroke                   | No indexing, extraction, embedding or query on the editor's path. A feature that can stall typing is redesigned, not optimised.         |
| 3   | Correctness is provable, not argued            | No mechanism whose behaviour under concurrent edit is reasoned about rather than tested; no silent coercion anywhere in the query path. |
| 4   | The cost of a query is knowable before it runs | No user-supplied code in the render path; no traversal whose depth is unbounded at plan time.                                           |
| 5   | Capability degrades; nothing breaks            | No unsupported input that stops the vault indexing. A file with no extractor is still typed, linkable and queryable.                    |

### 1.2 What P0 may invalidate

The performance figures this design is shaped around are targets, not measurements. Three structures exist specifically because of a number that has not yet been measured: the single-writer index actor assumes write contention is the bottleneck; the parse pool assumes parsing dominates rebuild time; brute-force vector search assumes 200,000 chunks scan fast enough. If P0 contradicts any of them, this document is re-baselined before P1 rather than quietly amended.

## Components & responsibilities

One Rust workspace of eight crates plus a Vue front end. The decomposition is by capability, not by layer, and the dependency rule is strict enough to be checked mechanically.

*(figure — see the original in docs/_source/serat-design-description.html)*

Fig. 1 — Crate dependency graph. Every arrow means “depends on” and every arrow points down, so a cycle would be visible as an arrow pointing up. serat-core holds types and traits and no I/O, which is what lets serat-sq typecheck against a SchemaProvider without linking serat-index.

### 2.1 The dependency rule

serat-core contains domain types and trait definitions and performs no I/O. Every other crate depends on serat-core and on nothing else internal. serat-app depends on all of them and nothing depends on serat-app. Cross-crate needs are expressed as traits declared in core and implemented wherever the capability lives, so the graph stays acyclic without anyone having to remember that it should.

This is not architectural decoration. It is what lets serat-sq typecheck a query against a SchemaProvider without linking a database, serat-mcp serve a QueryApi it does not own, and every crate be tested without a running application — which in turn is what makes the conformance suite and the property tests cheap enough to actually write.

| Crate         | Owns                                                                           | Notably does not own                                              |
|---------------|--------------------------------------------------------------------------------|-------------------------------------------------------------------|
| serat-core    | Domain types, trait definitions, error taxonomy                                | Any I/O whatsoever                                                |
| serat-vault   | Watching, reading, atomic writes, ignore rules, path canonicalisation          | Knowledge of Markdown                                             |
| serat-parse   | Markdown to blocks, fields, links, tasks; custom syntax extensions             | Any file access — it takes bytes and returns structures           |
| serat-index   | SQLite schema, the single-writer actor, incremental upsert, FTS, vectors, and the pipelines that fill them | Query language semantics; any knowledge of Markdown or of the filesystem |
| serat-sq      | Lexer, parser, typechecker, SQL codegen                                        | Execution — it emits a statement and a plan, nothing more         |
| serat-extract | Extractor registry, cache, child-process supervision                           | The parsing of any specific binary format                         |
| serat-ai      | Chunking, embedding, retrieval fusion, provider clients, proposal construction | Applying a proposal — only the app can, and only on a user action |
| serat-mcp     | Tool schemas and transport over the same query API the UI uses                 | A second implementation of anything                               |
| serat-app     | Tauri commands and events, wiring, the review queue                            | Domain logic of any kind                                          |

Where the rebuild lives, and why the rule is not bent to put it there

A full rebuild walks, reads, parses and writes, and the rule forbids any crate from taking two of those as dependencies. It is the first thing built that has no obvious home, and the answer decides whether the rule survives contact with the product.

It lives in **serat-index**, as `rebuild(source: &dyn NoteSource, parser: &dyn NoteParser, index_dir: &Path)`, with `serat_vault::Vault` implementing `NoteSource`. This adds no dependency edge: serat-index still depends on serat-core and nothing else internal, and it links neither a Markdown parser nor a filesystem walker. It is the escape hatch this section already describes, used for the case it was designed for.

Two arguments decided it against a `serat-pipeline` crate sitting above vault, parse and index. The first is that such a crate would need an exception to the rule — *this one crate may depend on three others* — and the rule's whole value is that it is one sentence a test can check. An exception is not a small cost; it is the cost of the rule. The second is that `rebuild` is less of a pipeline than it looks: four of its six steps are the bulk-load sequence in `schema/001_init.sql` §4.4 — drop the FTS triggers, load, rebuild the index, restore them — and all four are SQLite's business and nobody else's. The walk and the parse are inputs to it. Reading it as "the index's bulk-load entry point, taking its input through traits" describes it more accurately than "a pipeline that happens to end in SQLite", and the crate table above was simply written too narrowly.

The ruling comes with a constraint that keeps it from becoming a licence, and it is mechanically checkable in the same way the dependency rule is: **serat-index may acquire no knowledge of Markdown and no knowledge of the filesystem.** Everything it learns about either arrives through a trait declared in serat-core. If `comrak`, `notify` or a `std::fs` call ever appears in that crate, orchestration has stopped being a caller of traits and has started being a second implementation, and the placement is wrong rather than merely untidy.

The trigger to revisit is named so the decision cannot drift indefinitely: **the second orchestrator that must not link SQLite.** Extraction in P2 and embedding in P4 are the candidates. One orchestrator in the crate that owns the sink is a narrow reading of a table row; two, one of which has no business linking a database, is a `serat-pipeline` crate and an ADR amending this rule — and by then it will be written against two real callers instead of one imagined one.

### 2.2 Threading and ownership

Concurrency is designed by ownership rather than by locking. Each of the four concerns below owns its data outright, and they communicate over channels.

| Concern      | Model                                                                   | Why                                                                                                                                         |
|--------------|-------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------|
| Webview      | The main thread. Runs the UI and nothing else.                          | Anything that shares this thread can stall typing.                                                                                          |
| Index writes | One thread owning the only write connection; work arrives on a channel. | SQLite in WAL permits one writer. Making that explicit removes lock contention from the design entirely — there is nothing to reason about. |
| Index reads  | A small pool of read-only connections.                                  | WAL readers never block the writer, so views and queries proceed during indexing.                                                           |
| Parsing      | A rayon pool over cores minus one.                                      | Parsing is pure and CPU-bound. Leaving a core free is what keeps the UI responsive during a rebuild.                                        |
| Extraction   | Separate processes, supervised, capped at min(cores−1, 4).              | A security boundary and a fault boundary, both of which a thread cannot provide.                                                            |

Why one writer, not a mutex

A shared connection behind a mutex has the same throughput and worse failure modes: lock ordering becomes a thing anyone touching the index has to think about, and a panic while holding it poisons the database handle. A channel to one owner has a single queue to reason about, natural backpressure, and a crash that takes down one thread rather than corrupting shared state. See ADR-01.

## Data model

The full DDL is schema/001_init.sql and runs as delivered. This section records why it is shaped that way.

### 4.1 Block granularity is the pivotal decision

Serat indexes at block level rather than note level. One table of blocks, each anchored either by a byte range in a Markdown file or by a locator inside a binary, is what makes block references, precise AI citations, writable views and binary extraction all fall out of the same structure. Every one of those features would need its own machinery under note-level indexing; under block-level indexing they need none.

### 4.2 Three typed value columns

A field stores its value in val_text, val_num, val_date or val_bool, chosen at index time from the schema. The query compiler therefore knows at plan time which column a comparison touches, so a date range becomes an indexed scan rather than a per-row string parse. This single choice is most of the performance gap between Serat and a Dataview-style evaluator at scale.

### 4.3 Migration strategy: there isn't one

Because the index is derived and rebuildable, a breaking schema change drops the database and rebuilds it. No migration scripts, no versioned upgrade path, no partially-migrated states to debug. This removes an entire category of machinery from the project, and it is a direct and unglamorous consequence of the first design goal.

The one exception is the extraction cache, which is expensive to recompute and therefore does need care. It is keyed by content hash and extractor version, so a better PDF extractor invalidates PDFs and nothing else, and an index rebuild does not touch it at all.

Found by running it, not by reviewing it

Executing the reference query against a fresh database showed the planner choosing ix_notes_kind — which matches nearly every row — over ix_notes_type. Without ANALYZE, SQLite cannot know that kind='note' is almost the whole table while type='project' is a small slice, so every typed query degrades into a full scan. The DDL now ends in ANALYZE and the indexer re-runs it after every rebuild and after any batch touching more than 5% of notes. This is exactly the defect that would otherwise surface as “queries are slow on my real vault but fine in the tests” three months into P2.

### 4.4 Index strategy

| Index                               | Serves                                                    | Note                                                          |
|-------------------------------------|-----------------------------------------------------------|---------------------------------------------------------------|
| ix_notes_type                       | Every typed query's outer scan                            | Requires statistics to be chosen; see above.                  |
| ix_fields_note_key                  | The pivot join, one per referenced field key              | Becomes a covering index for the common projection.           |
| ix_fields_key_date / \_num / \_text | Range and equality predicates on a field across the vault | Partial indexes — only rows where that column is non-null.    |
| ix_links_src / ix_links_dst         | Rollups and the backlinks pane                            | Backlinks read dst; rollups read src.                         |
| ix_tasks_open                       | Every open-task query                                     | Partial on done = 0, which is the only side anyone queries.   |
| blocks_fts (external content)       | Full-text over notes and extracted binaries alike         | Triggers keep it in step; verified including the delete path. |

## Key flows

### Indexing

The pipeline that turns filesystem events into index rows. Its design constraint is not throughput but isolation: it must be impossible for anything here to delay a keystroke.

*(figure — see the original in docs/_source/serat-design-description.html)*

Fig. 2 — The indexing pipeline. Backpressure lives in the bounded, path-deduplicating work queue; the hash check discards most events before any parsing happens. Nothing here sits on the keystroke path.

#### 3.1 From event to row

1.  The watcher coalesces events per path over a 150ms window. Editors that save via temp-and-rename emit several events per save; they collapse to one.
2.  The work queue deduplicates by path and is bounded. A git checkout touching 4,000 files enqueues 4,000 entries, not 4,000 parses.
3.  The hash check reads the file and compares blake3 against notes.hash. Sync tools touch mtime constantly without changing content; this discards those for the cost of a hash.
4.  Changed files go to the parse pool, which is pure: bytes in, structures out, no I/O, so it parallelises across cores without coordination.
5.  Rows go to the index writer on a channel. It batches within a transaction, and runs ANALYZE when a batch touched more than 5% of notes.
6.  The event bus emits index.changed, throttled to ten per second, and open views invalidate the rows they hold.

#### 3.2 Echo suppression

Serat's own writes produce filesystem events indistinguishable from anyone else's. Before writing, the writer records the hash it is about to produce in a short-lived set; when the watcher sees an event whose post-hash is in that set, it removes the entry and drops the event. The set is keyed by hash rather than by path and entries expire after two seconds, so a genuine external edit that happens to produce the same bytes is indistinguishable from our own — and is correctly treated as a no-op, because it is one.

#### 3.3 Reconciling an external change into an open buffer

This is FR-VLT-05, and the naive implementation — replace the document — destroys cursor position and the undo stack, which is the difference between a tool you can leave open beside Claude Code and one you cannot.

On an external change to an open file, the core diffs old bytes against new and emits a minimal edit script as ExternalChange.edits. CodeMirror applies it as a transaction, which maps the selection and the undo history through the change automatically. The whole-document path exists only as a fallback when the diff exceeds half the file, where preserving position is meaningless anyway.

The case that decides the design

You are editing line 400. Claude Code rewrites the frontmatter at line 3. With a document replacement you lose your place and your undo history; with an edit script you do not notice. Every reconciliation decision follows from wanting the second outcome.

### Query compilation

SQ is specified in grammar/sq.ebnf, its lowering in grammar/lowering.md, and its behaviour in grammar/conformance.md. This section is the rationale.

#### 5.1 Two properties enforced by shape, not discipline

The grammar admits no user-defined functions, no loops and no host-language escape, and it fixes rollup traversal depth at exactly one hop. Neither property is a rule someone has to remember: both are structural, so violating them is a parse error rather than a review comment.

They buy two things. Because no query can execute arbitrary code, the engine can always answer “which note and which byte range produced this cell” — which is precisely what writable views require. And because no traversal is unbounded, cost is knowable at plan time, so a dashboard cannot silently become the reason the application is slow.

#### 5.2 Lowering

Seven passes, none of which can loop: resolve the source, collect referenced field keys, plan one LEFT JOIN per key, lower rollups to correlated scalar subqueries, bind let-expressions inside a CTE, choose the value column from the schema kind, and emit. The output is one statement. The worked example in lowering.md was executed against a fixture during design and returns the rows it should.

    -- fragment of the emitted statement for the reference query
    SELECT note_id, path, base_hash, title, status, owner, due, remaining,
           (due < date('now','+7 day') AND remaining > 8) AS slipping,
           status__fid, owner__fid, due__fid
    FROM base
    WHERE status IS NOT 'done'
      AND EXISTS (SELECT 1 FROM fields fl
                  WHERE fl.note_id = base.note_id AND fl.key='lens' AND fl.val_text='pm')
    ORDER BY slipping DESC, due ASC NULLS LAST;

- IS NOT rather than != — a missing field pivots to NULL, and NULL != 'done' drops the row. A project with no status set is exactly the row a status filter should surface.
- \_\_fid columns — every editable cell ships the fields.id that produced it. With base_hash this is the cell anchor the write-back patcher needs; computed columns have none, which is why the UI renders them read-only.
- EXISTS for list membership — a list-valued field is many rows, and joining it would multiply the result set.
- NULLS LAST — an undated project must not float to the top of a due-date sort.

#### 5.3 The typechecker is load-bearing

Lowering is only sound because types are known. If due were Text, pass six would pivot it to val_text and the date comparison would compare strings, returning plausible and wrong rows in silence. The type rules are not ergonomics; they are what makes the emitted SQL correct. This is also why unknown fields are an error on a typed source rather than a warning.

### Write-back

Editing a source note by changing a value in a view is the highest-risk mechanism in the product: it is the only path where a user action can corrupt a file they did not have open. It is designed by constraint rather than by care.

#### 6.1 The algorithm

1.  The cell carries an anchor: note id, field id, and the file hash at the time the row was read.
2.  The core re-reads the file and verifies the hash. A mismatch refuses the write and returns Refused{StaleHash} with the current hash — an expected outcome, not an error.
3.  It verifies that the recorded byte range still contains the field it claims to. A mismatch returns Refused{RangeMoved}.
4.  It replaces exactly those bytes with the serialised new value. Nothing else in the file is touched — no reflow, no key reordering, no re-serialisation.
5.  It writes to a temporary file and renames, recording the new hash for echo suppression first.
6.  The watcher recognises the echo; the note is reindexed alone; the affected row refreshes without re-running the query.

#### 6.2 Invariants

| Invariant                                            | How it is enforced                                                                                             |
|------------------------------------------------------|----------------------------------------------------------------------------------------------------------------|
| No byte outside the field's range changes            | Property test over a corpus of real notes; this is NFR-MNT-03 and the single most important test in the suite. |
| A concurrent external edit never loses data          | Optimistic lock on the file hash; the write is refused and the view refreshes.                                 |
| The document is never re-serialised                  | The patcher's only operation is byte-range replacement. It has no serialiser to call.                          |
| Creating an absent field is the only structural edit | Appended to frontmatter at the position the type declares — which is why FieldSpec carries an order.           |
| Undo crosses the boundary                            | A view edit enters the editor's undo stack; undo re-patches the file.                                          |

Why this is not solved by parsing and re-emitting

An AST round-trip would be simpler to write and would lose the author's formatting on every save: list markers normalised, table padding rebuilt, YAML key order rearranged. Every save would become a diff, which destroys git history and fights every external tool. Not round-tripping at all is the design, not an approximation of it. See ADR-08.

### Binary extraction

The full contract is contracts/extractor.rs. An extractor turns a binary into the same block rows a Markdown parser emits, which is why search, backlinks, embeddings and citations work on a PDF with no parallel code path.

#### 7.1 Isolation

Extraction runs in a short-lived child process with a wall-clock limit, a memory cap and no network. Parsing untrusted binaries in the main process is how a notes application becomes a CVE, and a thread cannot provide that boundary. The supervisor communicates over newline-delimited JSON on stdout, so a partial read is never a partial block.

A malformed file is never retried automatically. It will fail again, and a retry loop on a broken PDF is an indexer that spins forever on a vault the user cannot fix. The failure is recorded in assets.extract_error and the pipeline moves on.

#### 7.2 Two caches, deliberately separated

The index is disposable by principle. Re-OCRing four hundred scanned PDFs is not. Extraction output therefore lives outside the index, keyed by content hash and extractor version, and survives an index wipe. Deleting the index stays a seconds-long operation; the expensive artefact is kept separately and is invalidated per format when an extractor improves.

#### 7.3 Degradation is a supported state

When no extractor claims a file, the asset still indexes with its sidecar and stays typed, linkable and queryable — only full-text search over its contents is unavailable. ExtractionState::Unavailable is a normal value in the IPC contract, not an error, and the UI shows it as a fact about the file rather than as a failure.

### AI retrieval and MCP

Three separable pieces: retrieval, which is always local; generation, which is provider-agnostic; and exposure, which is the MCP server. Only the middle one ever touches the network.

*(figure — see the original in docs/_source/serat-design-description.html)*

Fig. 3 — Hybrid retrieval. Three signals computed independently, fused by reciprocal rank. The structural signal is the one competitors lack and the reason a graph-connected note can outrank a lexically closer one.

#### 8.1 Chunking and embedding

Chunks span block boundaries with the heading path prepended as context, so an embedded paragraph carries the section it came from. Chunks are keyed by a text hash and only changed chunks are re-embedded, which is what keeps a one-word edit from re-embedding a long note. Embedding runs in-process against an ONNX model downloaded on first use — never bundled, so the installer stays small and the application is fully usable with the AI layer never enabled.

#### 8.2 Fusion

Lexical, semantic and structural rankings are computed independently and fused by reciprocal rank, with fixed per-signal weights and no tuning knob exposed. The structural signal — proximity in the link graph to the strongest seeds — is what makes retrieval feel different from a semantic-search plugin, and it is available only because the link table exists for other reasons.

Exclusions are applied before retrieval, not after. A note carrying ai: false, or living outside the configured scope, is never a candidate, so its text cannot reach a request payload even as a discarded low-ranked result.

#### 8.3 The proposal lifecycle

Every AI-originated change becomes a Proposal: a summary, a set of unified diffs each carrying the base hash of its target, and the citations that justify it. Proposals from the agent panel, the capture agent, synthesis and external MCP clients all enter the same queue. There is exactly one function that writes a proposal to disk, it is reachable only from a user action, and adding a second path would violate a constraint rather than merely a preference.

The MCP server is a second front door onto the same query API, not a wrapper around the application. That is why an external agent can never see a staler vault than the window does, and why propose_edit returns queued and never applied. apply_edit, write_file, delete_note and shell are recorded in the schema file as deliberately absent, so their omission reads as a decision.

### User interface

Three screens carry the design's risk: the editor, because it is where the product is judged; the table view, because it is where a user can change a file they do not have open; and the review queue, because it is where trust in the AI layer is won or lost.

*(figure — see the original in docs/_source/serat-design-description.html)*

Fig. 4 — Main window. The lens switcher sits in the title bar because switching role is a frequent, deliberate act; the proposal count sits in the context panel because an unreviewed AI edit must never be invisible.

The lens switcher sits in the title bar because switching role is frequent and deliberate, and burying it would make the multi-role model a setting rather than a workflow. The proposal count sits in the context panel because a pending AI edit must never be invisible; the one thing worse than an agent that edits your notes is an agent that edited them where you could not see.

*(figure — see the original in docs/_source/serat-design-description.html)*

Fig. 5 — Table view. The editable cell carries its field anchor; computed columns are recessive and inert because they have no anchor to write back through. The stale-guard notice is an explanation with two next actions, not an error.

Two interaction rules carry most of the trust in this screen. Editable cells look editable and carry an anchor; computed cells are recessive and inert, because there is no source field to write back through and pretending otherwise would invite an edit that silently does nothing. And the stale guard is presented as an explanation with two next actions rather than as an error, because a concurrent external edit is a normal event in a vault that git and Claude Code also write to.

*(figure — see the original in docs/_source/serat-design-description.html)*

Fig. 6 — Review queue. One queue for every origin, including external agents over MCP. Citations sit beside the diff because a proposal you cannot check is a proposal you should not accept.

#### 9.1 Interaction rules that are design decisions

| Rule                                                                   | Why                                                                                      |
|------------------------------------------------------------------------|------------------------------------------------------------------------------------------|
| Every primary action has a keyboard binding and appears in the palette | NFR-USE-01. A vault tool that requires the mouse is a vault tool used less.              |
| Anything touching more than one file previews first                    | NFR-USE-04. Rename, promote and schema migration all share one preview component.        |
| Query results stream; the view never blocks on completeness            | A 5,000-row result must render its first page immediately.                               |
| Extraction state is shown on the file, not in a global progress bar    | The user cares whether this PDF is searchable yet, not about queue depth.                |
| No modal dialogs on the write path                                     | A refused write resolves in place; a modal would make a routine event feel like a fault. |

## Cross-cutting concerns (packaging, security, verification)

Three areas that carry Must-priority requirements and no interesting mechanism, which is exactly why they are easy to leave undesigned until they become a crisis. This section exists because a coverage check against the requirements baseline found them missing.

### 10.1 Distribution

Tauri's bundler produces the three platform artefacts from one workspace. macOS builds are signed with a Developer ID and notarised in CI, so a first launch shows no Gatekeeper warning; Windows builds are signed with an Authenticode certificate. Linux ships an AppImage and a .deb. Updates use signed manifests: the updater verifies a detached signature against a public key compiled into the binary, so a compromised update host cannot ship a payload.

Crash recovery is a journal, not a heuristic. Unsaved editor buffers are written to .serat/recovery/ on a two-second debounce, keyed by path and base hash; on startup any journal entry whose target file still matches its base hash is offered for recovery, and any whose target has changed underneath is offered as a diff instead. The journal is cleared on clean shutdown, so its presence at startup is itself the signal that the last session ended badly.

| Requirement | Design                                                                                                             |
|-------------|--------------------------------------------------------------------------------------------------------------------|
| FR-PLT-01   | One workspace, three bundler targets, one CI matrix.                                                               |
| FR-PLT-02   | Developer ID signing and notarisation on macOS; Authenticode on Windows. Both in CI, never on a developer machine. |
| FR-PLT-03   | Signed update manifests verified against a compiled-in public key.                                                 |
| FR-PLT-04   | Debounced buffer journal in .serat/recovery/ keyed by path and base hash.                                          |
| FR-PLT-05   | Settings as a readable TOML file in the platform config directory, version-controllable.                           |

### 10.2 Security posture

The security requirements are mostly negative — statements about what must not happen — and a negative is only credible if something enforces it. Each therefore has a mechanism rather than an intention.

| Requirement                          | Enforced by                                                                                                                                                                          |
|--------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| NFR-SEC-01 · no telemetry            | There is no analytics dependency in the tree, and CI fails on a deny-list of known telemetry crates. A network capture over a full session is part of the release checklist.         |
| NFR-SEC-02 · no unconfigured network | All outbound calls go through one client in serat-ai. With no provider configured it is never constructed, and a test asserts the process opens no socket during a scripted session. |
| NFR-SEC-03 · sandboxed extraction    | Child process with time and memory caps; the worker binary links no HTTP client at all, so a network call is a link error rather than a policy violation.                            |
| NFR-SEC-04 · credentials             | OS keychain via the keyring crate. A filesystem scan after configuring a key is part of the release checklist and must find no key material.                                         |
| NFR-SEC-05 · local-only services     | The MCP server binds stdio or a unix socket. There is no TCP listener in the codebase.                                                                                               |
| FR-BIN-19 · binaries never rewritten | No write path accepts a non-Markdown target. Verified by comparing content hashes of every asset before and after a scripted session.                                                |

### 10.3 The verification harness

Every performance requirement in the baseline is measured against one artefact: a generated benchmark vault. It is deterministic from a fixed seed, lives in the repository, and is built by one command, because a benchmark nobody can reproduce is a number nobody can argue with.

- Shape: 20,000 notes by default and 50,000 for the scale run, with a realistic long-tailed link distribution rather than a uniform one — a uniform graph makes rollups and backlinks look far cheaper than they are.
- Content: a mix of typed and untyped notes, frontmatter and inline fields, tasks with due dates and estimates, and roughly 200 assets across PDF, DOCX, XLSX and image formats.
- Determinism: one seed produces byte-identical output, so a regression is attributable to a code change rather than to a different vault.
- CI: one job per budget in NFR-PRF. A pull request that breaches one fails, which is what NFR-PRF-08 means in practice. The generator itself lives in the repository and runs from one documented command, which is NFR-MNT-02.
- Property tests: the write-back patcher runs against a corpus of real notes, asserting that no byte outside the target field's range changes — NFR-MNT-03, and the single most important test in the suite.

### 10.4 Requirements whose design is conventional

The remaining Must-priority requirements have designs that are the obvious ones, and recording them at length would add pages without adding information. They are listed here so their absence from the preceding sections reads as a judgement rather than an oversight.

| Requirements                               | Design                                                                                                                                                                                                       |
|--------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| FR-EDT-01, FR-EDT-03, FR-EDT-05, FR-EDT-08 | CodeMirror 6 with custom Lezer nodes for wikilinks, embeds, block refs and inline fields; per ADR-06 and §3.3 for the reconciliation half. Undo integration is the transaction path already described in §6. |
| FR-NAV-06, FR-VIW-01                       | Command palette over the same registry the keybinding table uses; TanStack Table over the streamed rows and anchors defined in contracts/ipc.rs.                                                             |
| FR-STR-01, FR-STR-02                       | The schema registry described in §2.1 as a trait in serat-core, loaded from type notes and invalidated by the same index.changed events views use.                                                           |
| FR-LNS-01, FR-LNS-05, FR-LNS-07            | Lenses, tasks and dashboards are all ordinary notes parsed by ordinary rules; a lens is an SQ predicate, a task is a block, a dashboard is a note with query blocks. No new mechanism.                       |
| FR-BIN-11, FR-BIN-12                       | Bundled PDF and image viewers in the webview, opened at the locator a search hit carries.                                                                                                                    |
| FR-VLT-03, FR-VLT-08                       | Temp-file-and-rename in serat-vault, described in §6.1 step 5; assets are listed by the same walk that finds notes.                                                                                          |
| FR-AIX-06, FR-AIX-11, FR-AIX-12            | One provider trait with two implementations; retrieval never depends on which. Absence of a provider disables generation and nothing else, and no model ships in the installer.                              |
| NFR-CMP-02                                 | Falls out of the first design goal: everything Serat writes is Markdown, YAML or documented plain text.                                                                                                      |

Security enforcement mechanisms are also recorded in `threat-model.md`.

## Related ADRs

- ADR-0001 — Single-writer actor for the index (serves NFR-PRF-03, NFR-REL-01)
- ADR-0002 — Rebuild instead of migrate (serves FR-VLT-02, NFR-REL-02, NFR-PRF-04)
- ADR-0003 — Field pivot by LEFT JOIN (serves FR-STR-05, NFR-PRF-05)
- ADR-0004 — Extraction in a child process (serves FR-BIN-07, NFR-SEC-03, NFR-REL-03)
- ADR-0005 — Sidecar notes for asset metadata (serves FR-BIN-02, FR-VLT-02)
- ADR-0006 — CodeMirror 6 with decorations (serves FR-EDT-02, FR-EDT-10, FR-VLT-05)
- ADR-0007 — Generated TypeScript from Rust (serves NFR-MNT-01)
- ADR-0008 — Byte-range patching, never round-tripping (serves FR-VIW-06, FR-VIW-08, NFR-MNT-03)
- ADR-0009 — Brute-force vector search in v1 (serves FR-AIX-03, NFR-PRF-07)
- ADR-0010 — One SQL statement per query (serves FR-STR-08, FR-STR-09, NFR-PRF-05)

## Traceability

Each design element and the requirements it satisfies. Every requirement of priority Must in the baseline has a design element here or in the repo-ready files; the four Won't-priority exclusions have none by construction.

| Design element                          | Where                      | Satisfies                                                                     |
|-----------------------------------------|----------------------------|-------------------------------------------------------------------------------|
| Crate decomposition and dependency rule | §2, Fig. 1                 | NFR-MNT-01, and the testability every verification method assumes             |
| Single-writer index actor               | §2.2, ADR-01               | NFR-PRF-03, NFR-REL-01                                                        |
| Indexing pipeline                       | §3, Fig. 2                 | FR-VLT-04, FR-VLT-07, NFR-PRF-02, NFR-PRF-03, NFR-PRF-04                      |
| Echo suppression                        | §3.2                       | FR-VLT-06                                                                     |
| Diff-based reconciliation               | §3.3                       | FR-VLT-05                                                                     |
| Index schema and indexes                | §4, schema/001_init.sql    | FR-STR-03, FR-STR-05, FR-BIN-01, FR-BIN-06, FR-NAV-02, FR-NAV-04, NFR-PRF-05  |
| Rebuild-not-migrate                     | §4.3, ADR-02               | FR-VLT-02, NFR-REL-02                                                         |
| SQ grammar and type rules               | §5, grammar/sq.ebnf        | FR-STR-06, FR-STR-07, FR-STR-09                                               |
| Lowering algorithm                      | §5.2, grammar/lowering.md  | FR-STR-08, FR-STR-12, NFR-PRF-05                                              |
| Conformance suite                       | grammar/conformance.md     | FR-STR-06, FR-STR-07, FR-STR-10, P2 gate                                      |
| Write-back patcher                      | §6, ADR-08                 | FR-VIW-05, FR-VIW-06, FR-VIW-07, FR-VIW-08, NFR-MNT-03                        |
| Extractor trait and worker protocol     | §7, contracts/extractor.rs | FR-BIN-05..10, FR-BIN-16, NFR-SEC-03, NFR-REL-03                              |
| Extraction cache                        | §7.2                       | FR-BIN-08                                                                     |
| Retrieval fusion                        | §8.2, Fig. 3               | FR-AIX-01, FR-AIX-03, FR-AIX-09, FR-AIX-10                                    |
| Proposal lifecycle                      | §8.3, Fig. 6               | FR-AIX-05, FR-AIX-04, FR-MCP-03                                               |
| MCP tool schemas                        | contracts/mcp-tools.json   | FR-MCP-01, FR-MCP-02, FR-MCP-04, NFR-SEC-05                                   |
| IPC contract                            | contracts/ipc.rs           | FR-VLT-01, FR-VLT-09, FR-LNS-02, FR-LNS-04, NFR-USE-04                        |
| Screen designs and interaction rules    | §9, Figs. 4–6              | FR-EDT-02, FR-STR-10, FR-NAV-04, FR-LNS-02, FR-VIW-05, NFR-USE-01, NFR-USE-04 |

## Open design questions

Recorded rather than resolved, because resolving them now would mean guessing. Each names what would settle it.

| Question                                                              | Settled by                                                                                                                     |
|-----------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------|
| Does the parse pool or the index writer dominate rebuild time?        | P0 benchmark. If the writer dominates, batching strategy changes; if parsing dominates, the pool sizing does.                  |
| Is brute-force vector search fast enough at 200,000 chunks?           | A P4 benchmark on the synthetic vault. ADR-09 is written to be reversed cheaply.                                               |
| Which PDF extractor gives usable layout fidelity on the real archive? | Blueprint move 5 — spike one extractor against actual contracts and papers before committing to the pillar.                    |
| How much of Markdown table editing needs a widget?                    | Dogfooding during P1. FR-EDT-07 is deliberately Could-priority until there is evidence.                                        |
| Does the 150ms watcher debounce hold for every sync tool?             | Observation during P1 against git, Syncthing and iCloud, which behave differently.                                             |
| Should saved views live in .serat or in the vault as notes?           | Follows from whether a user wants views to sync with their vault. Leaning toward notes, for consistency with types and lenses. |
