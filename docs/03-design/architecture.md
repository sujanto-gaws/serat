---
project: Serat
artifact: Software design description / architecture
phase: 03-design
derived_from: Serat Design Description 1.0 (design baseline, August 2026) — original with six figures in docs/_source/serat-design-description.html
---

# Architecture Overview — Serat

**Owner:** Sujanto (architect role) · **Version:** 1.0 (design baseline) · **Status:** Baselined, pre-P0 · **Upstream:** `docs/02-requirements/prd.md` (135 requirements) · **Decisions:** 10 ADRs in `adr/`

How the 135 requirements are actually built: the crate graph, the indexing pipeline, the query compiler, the write-back patcher, the extraction sandbox and the ten decisions that would be expensive to reverse. The original document has six figures; they did not survive conversion to Markdown and are referenced below by figure number — open the HTML original to see them.

> The design description names companion repo-ready files — `schema/001_init.sql`, `grammar/sq.ebnf`, `grammar/lowering.md`, `grammar/conformance.md`, `contracts/ipc.rs`, `contracts/extractor.rs`, `contracts/mcp-tools.json` — and states that where this document and those files disagree, the files are correct. `schema/001_init.sql` exists as of P0 and is the correct copy of §4; §4 and §4.4 have been updated where writing and running it contradicted them. The rest arrive with the phase that needs them: `contracts/ipc.rs` in P0 as the four-command subset, the `grammar/` files in P2, `contracts/extractor.rs` in P2 and `contracts/mcp-tools.json` in P4.

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

The ruling comes with a constraint that keeps it from becoming a licence, and it is mechanically checkable in the same way the dependency rule is: **serat-index may acquire no knowledge of Markdown, and no access to the vault.** Everything it learns about either arrives through a trait declared in serat-core. If `comrak` or `notify` ever appears in that crate, orchestration has stopped being a caller of traits and has started being a second implementation, and the placement is wrong rather than merely untidy.

The second half needs stating precisely, because the obvious phrasing — *no filesystem access* — is wrong, and was written that way here first. serat-index owns `.serat/`, and §3.4 requires it to delete `index.db`, `-wal` and `-shm` and recreate the directory when it rebuilds; a crate forbidden the filesystem could not implement the design's own recovery path. The line is not "no files", it is **whose** files: serat-index may create and delete the index directory it owns, and may not open, walk, stat or read anything under the vault root. A vault path reaches it only as a `VaultPath` inside a row, which is a string it stores, not a file it touches.

So the check has two halves and they need different instruments. A forbid-list of external dependencies — `comrak`, `notify` — catches the first at the moment someone adds the dependency, in the same file and the same shape as `CORE_ALLOWED_EXTERNAL`, which already proves serat-core does no I/O. The second is a source scan restricting `std::fs` to the module that owns the index directory's lifecycle: crude, but it fails when someone reaches for the filesystem in the writer or the apply path, which is where the mistake would actually be made.

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

The channel is not the whole interface

Anything that must observe its own write needs a barrier, so the writer handle also exposes `flush()`, which blocks until the queue has drained. The rebuild uses it before `ANALYZE`; tests use it for read-after-write. It had to exist on the first day the writer existed.

This is the design's real shape: **one writer, asynchronous, with an explicit and expensive barrier** — not an interface with no synchronisation left in it, which is how the paragraph above reads on its own. The barrier must never appear on the save path, where blocking would put the index back on the keystroke path (goal 2). A caller there reads the pre-write snapshot and is corrected by `index.changed`; a caller that cannot tolerate that is telling you it belongs on the writer's thread, not that it needs a longer lock.

The test that keeps this honest: **if a message needs a reply, the channel is a lie and you have a mutex with extra steps.** That sentence was written against `IndexOp`, whose variants need no reply, and it is the right instinct stated one word too narrowly — `flush()` was carved out of it by name on the day it was written, and I4 added three more replies for the rebuild's phases. Four exceptions to a rule that names one is a rule about to be ignored, so here is the category rather than the list.

**A message may need a reply only if it is a barrier.** The distinction is not whether a reply exists but what it is for, and one question separates them: *would this message exist if the caller did not need to wait?*

- A **barrier** exists only because of the wait. Its reply means "the state you asked for has been reached", the caller is the code that owns the sequence, and the next step is invalid until the previous one has landed. `flush()` before `ANALYZE`, and the rebuild bracketing its own bulk load, are barriers. A barrier may carry a value — a count of what it completed — as long as that value is a fact about what the barrier finished and not an answer computed on the caller's behalf.
- A **request** would exist anyway, and the reply is the point of sending it: the caller wants a value the actor holds. That is the thing the original sentence forbids, and it is still forbidden. A request answered over the channel is a mutex with extra steps, and a per-operation completion handle is the same thing wearing a smaller hat — which is why callers still get none.

Two limits keep the category from swallowing the rule. No barrier may appear on the save path or on anything a keystroke waits for, because blocking there puts the index back on the editor's path whatever the message is called (goal 2, §3.1.1). And a barrier is a phase boundary in a sequence, so the number of them is small and each one is legible as such; a barrier that appears in the middle of ordinary work is a request that has been renamed.

The second limit needs a countable form, because the first real case put it in conflict with the category question. A proposed `apply_and_wait(path, bytes)` for the save-to-index harness is a barrier by the category — it is `submit` plus a wait and would not exist without the wait — and is called once per note in a thousand-iteration loop, which is the middle of ordinary work. So: **count the calls per unit of work at the call site.** A barrier appears a fixed number of times per phase; one that appears once per item is the signal to look harder, and it resolves one of two ways.

Either it is on a production path, and the first limit already forbids it — which is the case that matters and the reason this is a rule at all. Or it is a harness deliberately serialising in order to measure a latency, which is legitimate, because a benchmark is neither the save path nor anything a keystroke waits for. Note what this means: `flush()` called once per note *in a harness* is fine, so counting alone does not settle it and the first limit is the one doing the work.

What the harness case does require is that **the barrier stays visible where it is called**. `submit()` followed by `flush()` shows a wait; `apply_and_wait()` is a verb that reads like a write and hides one, and it is a single copy-paste from the save path, where the wait is forbidden. So the ruling on that command is that it should not exist — not because a harness may not block, but because a name that conceals a barrier is how a barrier reaches a path that forbids it. Composing the two existing primitives costs the harness one line and keeps the actor's command set at the barriers it already has, each of which is a thing ADR-01 obliges someone to reason about.

The rule earned its keep by catching its own exceptions: it is why the three new replies were examined at all, and the answer was that they are correct and the wording was too narrow. A test sharp enough to fire on the cases it should permit is worth more than one loose enough never to fire.

The queue's other under-specified half — what happens when it is full — is ADR-11. The short version is that the answer is different for each producer: the watcher may be dropped because it cannot be blocked without losing OS events silently, and the bulk producer may not be dropped because it can be blocked at no cost. Routing both through one policy is a measured defect, not a theoretical one.

## Data model

The full DDL is schema/001_init.sql and runs as delivered. This section records why it is shaped that way.

The file in the repository is the P0 subset — notes, blocks, fields, links, tasks, the full-text index and the index's own stamp — and it names, in a header comment, the phase that adds each of the tables it omits: assets in P1, types in P2, chunks and the sqlite-vec table in P4. Under ADR-02 there is nothing to gain from stubbing them now, because the upgrade path for a schema change is `rm`.

One table is in the DDL that is not in the requirements data dictionary. `index_meta` is a single row describing the index itself — schema version, indexer version, the vault root it was built from, a note count and whether the last build committed. It is what makes the open check of §3.4 cost two milliseconds instead of a second. Every column of it describes the index rather than the vault and is re-derived by a rebuild, so it does not breach C-02; nothing about the user's notes may ever be added to it. The data dictionary also names the full-text table `notes_fts`, which the DDL does not have: it is `blocks_fts`, over blocks, per §4.4 — a note-level FTS table cannot answer "which block" and so cannot serve a citation or a locator.

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
| ix_notes_type                       | Every typed query's outer scan                            | Partial on type IS NOT NULL, and requires statistics; see above and below. |
| ix_notes_kind                       | Listing assets; separating notes from assets              | The index the planner wrongly preferred before ANALYZE existed. |
| ix_fields_note_key                  | The pivot join, one per referenced field key              | Becomes a covering index for the common projection.           |
| ix_fields_key_date / \_num / \_text | Range and equality predicates on a field across the vault | Partial indexes — only rows where that column is non-null.    |
| ix_links_src / ix_links_dst         | Rollups and the backlinks pane                            | Backlinks read dst; rollups read src.                         |
| ix_tasks_open                       | Every open-task query                                     | Partial on done = 0, which is the only side anyone queries.   |
| blocks_fts (external content)       | Full-text over notes and extracted binaries alike         | Triggers keep it in step; verified including the delete path. |

Two of these were changed by writing the DDL and running it, and the file is now the correct copy. `ix_notes_type` is partial on `type IS NOT NULL`, because an untyped note is always valid and most of a real vault is untyped: on a 20,000-note fixture with 500 typed notes, the full index holds 20,000 entries and `ANALYZE` estimates 10,000 rows for `type = 'project'`, while the partial index holds 500 and estimates 500. Both give the same plan; only one gives the planner an estimate that is not wrong by a factor of twenty, which is the same failure §4.3 is about. The cost accepted is that `GROUP BY type` becomes a scan and a temp b-tree.

Running the DDL also settled how a full rebuild must load it. `blocks_fts` is kept in step by triggers, and for an incremental save that is exactly right — a note is a dozen blocks and the trigger is what makes the search index correct with no second code path. For a rebuild it is the wrong shape by a wide margin. Loading 26,000 blocks with the triggers live took 2.1 seconds in one transaction and 6.8 to 10.3 seconds spread across two to twenty-six; dropping the three triggers, loading, and then issuing the FTS5 `'rebuild'` command took 0.32 seconds, and produced a byte-identical search index. The same shape loaded 259,971 blocks in 5.6 seconds.

Those figures are calibration, not results: a development machine, through a Python harness, against SQLite 3.50.4 — the same caveat §3.4 carries for its own numbers, repeated here because these are the ones `serat-index` will be quoted at and they sit a long way from it. The ratio is what the decision rests on, and it reproduces: an independent run through rusqlite on 3.53.2 measured 5.8x against the 6.6x here, in-memory against on-disk, with an identical resulting search index. T2's numbers on the benchmark vault replace both.

So a full rebuild runs: recreate the schema, read the trigger definitions out of `sqlite_master` and drop them, bulk-load in batched transactions, `INSERT INTO blocks_fts(blocks_fts) VALUES('rebuild')`, re-execute the saved trigger text, then stamp `index_meta` and `ANALYZE`.

That last step is also what makes the sequence crash-safe, and the property is worth stating because the next person to touch it will not derive it. If the process dies anywhere in the middle — including in the window where the triggers are dropped and `blocks_fts` is therefore not being maintained — `index_meta.state` is still `'building'`, so the next open discards the file and rebuilds (§3.4). **That holds only because the final step is the sole place `state` becomes `'ready'`.** A second write of `'ready'` added anywhere earlier, for any reason, silently removes crash recovery from the rebuild path and leaves a half-built index that opens cleanly. Reading the triggers back out of the database rather than keeping a second copy of the DDL is what makes the recreated triggers identical to the delivered ones by construction, instead of by a test that has to notice a copy going stale. The incremental path never drops anything: at 100 blocks per transaction the trigger path cost 15ms, which is inside NFR-PRF-03 with room to spare.

The three `ix_fields_key_*` partial indexes were expected to need help from the query compiler — a redundant `val_date IS NOT NULL` emitted beside every predicate — on the assumption that SQLite's partial-index prover is too limited to infer it. It is not: `=`, `<`, `BETWEEN`, `IN` and a `LIKE` prefix on the typed column all select the partial index unaided. The lowering emits no redundant term. The one shape that cannot use them is a bare `key = ?` with no predicate on the value, which is why a merely *projected* field must be reached through the pivot join on `ix_fields_note_key`, never by selecting on `key` alone.

## Key flows

### Indexing

The pipeline that turns filesystem events into index rows. Its design constraint is not throughput but isolation: it must be impossible for anything here to delay a keystroke.

*(figure — see the original in docs/_source/serat-design-description.html)*

Fig. 2 — The indexing pipeline. Backpressure lives in the bounded, path-deduplicating work queue; the hash check discards most events before any parsing happens. Nothing here sits on the keystroke path.

#### 3.1 Two paths into the index, and two budgets

There are two ways a note reaches the index, and conflating them is what made NFR-PRF-03 look unsatisfiable.

| Path                      | Trigger                                       | Budget                                   | Debounced?                       |
|---------------------------|-----------------------------------------------|------------------------------------------|----------------------------------|
| Save path (§3.1.1)        | Serat writes a file the user is editing.      | 50ms p95 — NFR-PRF-03                  | No. The watcher is not involved. |
| Watcher path (§3.1.2)     | Any other process writes a file in the vault. | 500ms — FR-VLT-04's acceptance criterion | Yes, 150ms coalescing.         |

Finding F-03 read the 150ms coalescing window as a design that misses a 50ms budget by construction, and the arithmetic was right: 150 > 50. The mistake was in taking both requirements to describe one path. They do not, and they never conflicted — FR-VLT-04, the requirement that actually governs an external change, allows 500ms precisely because it has a debounce to absorb.

Found by reading the design against itself

The save path is not a new mechanism invented to close a gap; it was already required by §3.2, and this document simply never said so. Echo suppression **drops** the watcher event for Serat's own write. If the index were fed only by the watcher, a note saved in Serat would therefore never be indexed at all, and the mechanism that exists to prevent duplicated work would instead prevent all of it. Echo suppression is only coherent if the write path has already submitted the rows itself. F-03 did not find a missing 100ms; it found a missing paragraph.

##### 3.1.1 The save path

Serat knows what it wrote, so it does not wait to be told.

1.  The buffer is written by temp-file-and-rename (§6.1 step 5), recording the hash it is about to produce for echo suppression **before** the rename.
2.  The bytes just written — the ones already in memory, not a re-read of the file — go to the parse pool.
3.  The rows go to the index writer on the same channel everything else uses.
4.  `index.changed` is emitted and open views refresh.

The watcher sees the event and drops it (§3.2), which is the whole of the interaction between the two paths. Nothing is coalesced because there is nothing to coalesce: one save is one write is one set of rows.

Two consequences, stated because each looks like a bug otherwise. If another process modifies the file between our write and our parse, the index briefly reflects our bytes rather than the file's; the watcher then sees an event whose post-hash is *not* in the suppression set and the note is reindexed from disk, so the race is self-correcting and needs no mechanism of its own. And the save path must never call `flush()`: waiting for the writer to drain would put the index back on the keystroke path, which design goal 2 forbids. The UI reports "saved" when the rename completes and learns "indexed" from `index.changed`. Those are two different facts and the interface should not pretend otherwise (§2.2).

##### 3.1.2 The watcher path

1.  The watcher coalesces events per path over a 150ms window. Editors that save via temp-and-rename emit several events per save; they collapse to one.
2.  The work queue deduplicates by path and is bounded, and when it is full the watcher's push is refused, a rescan flag latches, and the consumer reconciles by walking. A git checkout touching 4,000 files enqueues 4,000 entries, not 4,000 parses. The bulk producer is never dropped this way — see ADR-11, which is about which producer may be dropped rather than about whether dropping is allowed.
3.  The hash check reads the file and compares blake3 against notes.hash. Sync tools touch mtime constantly without changing content; this discards those for the cost of a hash.
4.  Changed files go to the parse pool, which is pure: bytes in, structures out, no I/O, so it parallelises across cores without coordination. A file the parser cannot read is **indexed as a bare row, never skipped** — see below.
5.  Rows go to the index writer on a channel. It batches within a transaction, and runs ANALYZE when a batch touched more than 5% of notes. A full rebuild takes the bulk path instead — triggers dropped, FTS index built once at the end, triggers restored from their own definitions; see §4.4.
6.  The event bus emits index.changed, throttled to ten per second, and open views invalidate the rows they hold.

##### A note that cannot be parsed is still a note

F-67, and the answer is design goal 5 applied to Markdown rather than to binaries. When the parser cannot produce structures — in practice a `.md` file that is not valid UTF-8, since CommonMark has no parse failures and broken frontmatter already indexes the note and records the problem — the note is indexed as a **bare row**: path, hash, mtime, size and a title from the filename, with no blocks, no fields and no links.

It is not skipped. Skipping is the option that looks safe and is not: the file exists, so an application that indexes nothing for it is one where the note is missing from the tree and from path search, links to it read as unresolved, and there is no surface anywhere that says why. The vault would contain a file the product denies. §7.3 already settles the identical question for a binary with no extractor — it "still indexes with its sidecar and stays typed, linkable and queryable", and only full-text over its contents is unavailable — and there is no reason for a Markdown file to be treated worse than a PDF nobody can read.

Nothing records the failure in the index, deliberately. It needs no column, because it is not a fact about the note that the index has to remember: anyone can re-derive it in the time it takes to read the file. P1's validation surface lists such notes by computing them, and a column would be a schema change — a rebuild, under ADR-02 — bought for a consumer that does not exist.

#### 3.2 Echo suppression

Serat's own writes produce filesystem events indistinguishable from anyone else's. Before writing, the writer records the hash it is about to produce in a short-lived set; when the watcher sees an event whose post-hash is in that set, it removes the entry and drops the event. The set is keyed by hash rather than by path and entries expire after two seconds, so a genuine external edit that happens to produce the same bytes is indistinguishable from our own — and is correctly treated as a no-op, because it is one.

#### 3.3 Reconciling an external change into an open buffer

This is FR-VLT-05, and the naive implementation — replace the document — destroys cursor position and the undo stack, which is the difference between a tool you can leave open beside Claude Code and one you cannot.

On an external change to an open file, the core diffs old bytes against new and emits a minimal edit script as ExternalChange.edits. CodeMirror applies it as a transaction, which maps the selection and the undo history through the change automatically. The whole-document path exists only as a fallback when the diff exceeds half the file, where preserving position is meaningless anyway.

The case that decides the design

You are editing line 400. Claude Code rewrites the frontmatter at line 3. With a document replacement you lose your place and your undo history; with an edit script you do not notice. Every reconciliation decision follows from wanting the second outcome.

#### 3.4 Cold start, and how a broken index is noticed

Two requirements meet on this path and pull in opposite directions. NFR-PRF-01 wants an interactive window in 400ms and a usable vault in one second at 20,000 notes. NFR-REL-02 wants a corrupt index detected and rebuilt with no user action beyond restarting. Every thorough way of detecting corruption costs more than the entire window budget, so the two are reconciled by deciding what is checked *when*, rather than by checking less.

##### The critical path does not scale with the vault

| Phase                            | On the main thread                                                                                                                | In parallel                                                                                                                              | Budget                       |
|----------------------------------|-------------------------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------|------------------------------|
| Process start → window painted   | Read the settings TOML, create the Tauri window, load the embedded webview bundle, paint the shell with `IndexState::Opening`.        | Index actor opens the index and runs the open check below (2.4ms). Vault thread registers the watcher — and stops there.                    | 400ms (NFR-PRF-01)         |
| Window painted → vault usable    | Render the file tree and restore the last-open buffer as they arrive; the editor accepts keystrokes from the first paint.             | The walk starts and streams paths; the index answers the first queries; the last-open file is read from disk.                                 | 1s total (NFR-PRF-01)      |
| After usable                     | Nothing.                                                                                                                             | Catch-up reconciliation, `ANALYZE` if a rebuild ran, background `quick_check` if the last session ended badly, extraction and embedding (P2/P4). | Unbudgeted; must not stall the UI. |

The load-bearing property is in the first row: **nothing on the critical path is proportional to the size of the vault**. The main thread never opens the index — the index actor does, on its own thread (ADR-01) — and no IPC command blocks on it. The shell paints with whatever state the actor has published, which is why 400ms is achievable at 20,000 notes and still achievable at 50,000.

Two ordering rules that are correctness, not optimisation:

- **The watcher is registered before the walk starts.** A file changed during the walk then produces an event rather than being silently missed, and the walk reconciles anything that changed before the watcher existed. The gap between the two is now larger, because the walk waits for the first paint — which costs nothing, precisely because the watcher is already up and covering it.
- **The walk does not start until the window has painted.** It is the largest single cost at startup, and nothing before the first paint needs it. Leaving it running against the paint made `t_visible` depend on the size of the vault through CPU and I/O contention rather than through anything the main thread did — which is not a budget anyone can hold. This is safe only once the walk is not itself absurd: F-12 measured it at about 2.0 seconds on the 20,000-note vault because it canonicalises every entry, against about 50ms with the prefix stripped lexically. On the no-index path, where the tree comes from the walk, deferring it by one paint eats into FR-VLT-01's two seconds, so this ordering and that fix are one decision and not two.
- **The file tree is rendered from the index when it is `Ready`, and from the streaming walk when it is not.** One query over `notes(path)` returns 20,000 paths in about ten milliseconds; a cold directory walk takes far longer. This is design goal 5 in miniature — with a warm index the tree is instant, without one it degrades to streaming, and neither case blocks.

Deferred deliberately, because none of it is needed to type: the catch-up scan, `ANALYZE`, the FTS integrity check, `quick_check`, extraction and the embedding model, which is never loaded at start.

##### The three marks, defined so they can be measured

A budget that is not defined as two observable events is a number that flatters whoever reports it. NFR-PRF-01 is therefore three marks on one timeline.

| Mark        | Definition                                                                                                                                                  | Why not the cheaper thing                                                                                                                                        |
|-------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `t0`        | Process spawn, taken by the harness from outside the process.                                                                                                | Not `Instant::now()` in `main`. The loader, the DLLs and the webview runtime are real cost the user waits through, and a t0 inside `main` hides exactly the part we do not control. The app also records its own `main` entry, so the pre-`main` gap is visible rather than absorbed. |
| `t_visible` | The first frame the webview actually paints, signalled by the front end from the first `requestAnimationFrame` after the shell mounts.                        | Not window creation. Window creation is much cheaper and would pass 400ms while shipping a blank rectangle. This is test-engineer's trap and they are right about it.                                                                                                              |
| `t_usable`  | The **latest** of: the open check having finished, so the index state is known; the file tree having been delivered to the front end; the watcher having settled; and the window having painted. | Not "the index is open". A vault that is usable but unwatched silently misses every edit made in that window, which is a correctness bug dressed as a faster number.                                                                                                              |

The watcher clause is load-bearing and is why it is in the definition rather than in a note. It also fixes an ordering: the watcher is registered before the walk, so nothing that changes during startup is lost. "Settled" rather than "registered", because a watcher that failed to start is a supported outcome — a vault with no watcher is stale, not broken (goal 5) — and the trace records which it was, since a usable figure taken without a watcher is not comparable to one taken with.

Taking the *latest* of the four rather than the last one to be reported is not pedantry, and S1 found out why on its first run. Both marks arrive as IPC round trips, so a fast disk can finish the buffer read before the paint mark's round trip completes; the process then exited with the paint still in flight and wrote no trace at all. Defining usable to include the paint term makes that structural instead of a rule the front end has to remember, and the same reasoning is why a trace whose watcher never settled does not exist rather than existing with a gap.

`t_usable` as defined is the warm-index case NFR-PRF-01 names. With no index the file tree comes from the streaming walk instead, and that case is bounded by FR-VLT-01's own two-second criterion, not by this one — a cold vault and a warm vault are different measurements and the gate report must not average them.

The mechanism is one environment variable, so the harness measures the application rather than itself: with `SERAT_STARTUP_TRACE=<file>` set, `serat-app` writes one line of JSON — `t0_to_main_ms`, `t0_to_visible_ms`, `t0_to_usable_ms` and the index state at `t_usable` — and exits. It exits because a harness that has to kill the process adds its own timing noise to the number it is trying to measure, and `t0` is handed in by the harness so that the pre-`main` cost is inside the number rather than beside it. The same marks are available in a normal session through `IndexStatus.since_process_start_ms` in `contracts/ipc.rs`, reading the same monotonic clock. The trace file is the gate number for P0, because a process that exits at `t_usable` leaves no window in which to observe an event stream; if the two ever disagree, that disagreement is a finding rather than a rounding difference.

##### Most of the window budget is not ours to spend

S1's first measurement of the real shell, twenty spawns, release build, scanner disabled: `window_visible` p95 **1088ms against a 400ms budget**, and `vault_usable` p95 **1094ms against 1000ms**. The median splits into 57ms of loader, 23ms of Tauri runtime, **619ms inside `WebviewWindowBuilder::build`** — WebView2 environment and controller creation — 100ms to the first painted frame, and 5ms to usable.

The largest single term is a platform cost that no decision in this document moves. That is worth stating plainly rather than leaving as an inference, because the table budgets the phase as though all of it were ours, and a reader optimising against it would be tuning the wrong end of the split.

**"Serat's own share" is not one number, and the section deliberately does not name one.** An earlier draft here said "roughly 130ms of the 400"; it matched no definition of the five terms and is withdrawn. Three defensible quantities can be read off the same run and they differ by a factor of six:

| Quantity | Median | What it answers |
|---|---|---|
| Main-thread work outside the loader and the webview build | 28ms | Is Serat itself slow? No. |
| Everything except WebView2 environment creation | 185ms | What is left if the platform term were free? |
| The whole phase | 804ms | What does the user wait for? |

None of the three is wrong; quoting one without saying which is. The middle figure is the one that misleads most easily, because the 57ms of loader inside it is a function of our binary size and dependency count — ours to influence, not ours to execute — and the 100ms to first paint is our HTML and CSS being rendered by a webview we did not write. So the split is what this section carries, and an aggregate quoted anywhere else must name its basis in the same sentence as the number.

The first row is deliberately *not* labelled "code we wrote", which is what an earlier draft called it. The five medians offer only one way to reach 28ms — the 23ms of Tauri runtime plus the 5ms to usable — and on that reading most of it is somebody else's runtime rather than ours. Whether that is how the figure is actually composed is not established by this run, and the answer to the question the row asks does not depend on it: 28ms is not slow however it divides. But a table whose entire purpose is that a figure without its basis is the error is the last place to assert a basis nobody has checked.

This does not re-baseline NFR-PRF-01, which is product-analyst's at Q1 and needs more than one platform's number: WebView2 on Windows is one of three webviews, and WKWebView and WebKitGTK have their own startup costs that may differ by a lot. What P0 establishes is the shape of the answer — if all three platforms carry a webview cost of this order, the requirement is wrong rather than the design, and the honest fix is a budget that separates the part we control from the part we inherit. What it does *not* establish is that the budget is unreachable, because the figure above was taken before F-12's walk fix and on one platform.

The number is also a best case in the one way that matters for the gate: it was taken with the scanner disabled, so it is marked `gate_eligible: false` and a stock machine is slower.

##### Which mark is constant in the size of the vault, and which is not

This distinction decides what a budget job may assert, so it is stated rather than left to be inferred.

`t_visible` is **constant in the work the main thread performs**, which is the claim the design can actually keep. Nothing the main thread does before the first paint is a function of note count: it reads a settings file of a few kilobytes, creates a window, loads a bundle compiled into the binary, and paints with `IndexState::Opening`. It does not wait on the index actor, enumerate anything, or read a directory.

It is not constant in *wall clock*, and the difference matters to whoever writes the budget job. `t_visible` is measured from `t0`, so it includes contention from whatever runs in parallel — and on a two-to-four core CI runner with a virus scanner enabled, scheduler and I/O contention move that number on their own. Deferring the walk past the paint removes the largest such source deliberately, but it does not remove them all.

So the assertion a budget job may make is that `t_visible` does not grow **beyond a stated tolerance** between a 20,000-note vault and a 50,000-note one; 20% is the agreed figure. A path that had become proportional to vault size would show around 2.5x, so the tolerance has better than a factor of ten of headroom against what it exists to catch, and buying sensitivity below that only buys a job that goes red from contention and gets disabled by the next person in a hurry.

`t_usable` is **not** constant, and asserting that it is would eventually fail for a legitimate reason. Two of its three preconditions grow with the vault. The file tree is one query returning every path, so it grows linearly in note count — cheaply, but it grows. And watcher registration grows in the number of *directories* on Linux, where inotify takes one watch per directory, while it is a single call on Windows and macOS. So `t_usable` is bounded and sub-linear rather than flat, and the way to hold it is the absolute budget NFR-PRF-01 states, measured at 20,000 notes with the 50,000 figure recorded beside it — not a growth ratio.

The recovery journal of §10.1 is read after `t_usable`, not before. Its cost is proportional to the number of unsaved buffers rather than to the size of the vault, so it would not breach either budget; it is deferred because nothing about it is needed in order to type.

##### Validating the index on open

Four candidates, measured against the schema on a 20,000-note fixture (259,971 blocks, 41,647 fields, 90,744 links; `index.db` 124.2MB), warm OS cache, SQLite 3.50.4:

| Candidate                                             | Cost      | Catches                                                              | Verdict                                        |
|-------------------------------------------------------|-----------|----------------------------------------------------------------------|------------------------------------------------|
| `PRAGMA integrity_check`                              | 1310ms  | Everything, including index-versus-table disagreement.               | Rejected — 3× the whole window budget.         |
| `PRAGMA quick_check`                                  | 1150ms  | Page-level damage anywhere in the file.                              | Rejected on the open path; kept as a background and on-demand check. |
| FTS5 `integrity-check`                                | 390ms   | `blocks_fts` out of step with `blocks`.                              | Rejected on the open path; runs in CI and on demand. |
| Stamp: `user_version`, `index_meta`, `count(*) notes` | 2.4ms   | Wrong schema, interrupted build, wrong vault, truncation, row loss.  | **Chosen.**                                    |

The stamp is the open check. In order, and stopping at the first failure:

1. `.serat/index.db` absent → rebuild, reason `NoIndex`.
2. Opening it fails, or any statement below returns `SQLITE_CORRUPT` or `SQLITE_NOTADB` → rebuild, reason `Corrupt`.
3. `PRAGMA user_version` differs from the version this build writes → rebuild, reason `SchemaChanged`. This is the whole of ADR-02's upgrade path.
4. `index_meta` is missing or `state <> 'ready'` → rebuild, reason `Interrupted`; the previous build did not commit.
5. `index_meta.vault_root` is not the canonical root being opened → rebuild, reason `VaultMoved`; the folder moved, or a `.serat/` directory was copied between vaults.
6. `count(*) FROM notes` differs from `index_meta.note_count` → rebuild, reason `Corrupt`. This is why the schema requires the count to be updated in the same transaction as any change to the row count: a lag between them is indistinguishable from damage, and must be.

Measured before it was decided

The acceptance criterion for NFR-REL-02 is a *truncated* index file, so the file was truncated — by 0.1%, by half, to 5%, and to 100 bytes — and every statement failed instantly, including `PRAGMA user_version`, with "database disk image is malformed". SQLite derives the page count from the length of the file, so truncation is caught by step 2 at no cost at all, and the expensive checks buy nothing for this case.

With one boundary that has to be stated, because the sentence above was first written as though it were general and it is not. **The threshold is one page, not a percentage.** SQLite derives the page count by integer division, so a truncation that removes *less than a whole page* leaves that count unchanged and is invisible to the stamp: at 4KB pages, cutting 100 bytes or 2,048 bytes passed all six steps, while cutting one page was caught immediately by every statement, as was everything larger. That boundary reproduces on databases of 1.2MB, 1.4MB, 15.6MB and 124MB, which is the point — it is a property of the cut, not of the file. A percentage misleads only because 0.1% of a large index is thirty pages and 0.1% of a small one is half of one.

Whether the *expensive* check catches a sub-page truncation is undetermined, and the honest answer is that it depends on what the removed bytes held. On the 124MB fixture `quick_check` returned a single row, `ok`, for both sub-page cuts; on a 1.2MB and a 15.6MB database it reported real corruption for the same 2,048-byte cut. Free space at the tail of a file goes unnoticed; live cell data does not. Neither result generalises, so neither belongs in an argument.

The lazy `IndexFault` path is mandatory regardless, and rests on firmer evidence than this: a 4KB overwrite in the *middle* of the file passes all six stamp steps, passes `count(*)` over notes and blocks, and passes a full-text match, and only `quick_check` catches it — at 1,150ms, which is why it cannot be on the open path whatever it can detect. That argument is made above and needs no help from the truncation case.

One calling convention the background check must get right, because each half of it was found the hard way and neither run showed both. **`PRAGMA quick_check` signals corruption in two different ways.** Sometimes it raises: the one-page truncation failed the statement outright with "database disk image is malformed". Sometimes it succeeds and returns its findings as rows, where a healthy database is the single row `ok`. An implementation that runs it, sees no error and carries on is wrong for every case of the second kind. Treat a raised error and a first row that is not `ok` as the same outcome.

##### What the stamp cannot see

Page-level damage deeper in the file survives the open check and is detected when a statement reads the damaged page. Therefore **exactly one function classifies index failures**, and it is reachable from every index read and write: any `SQLITE_CORRUPT`, `SQLITE_NOTADB`, "malformed" error, missing table, or failed FTS integrity check — at open or in the middle of a session — becomes one `IndexFault`, and `IndexFault` has exactly one handler.

That handler closes every connection, deletes `index.db`, `index.db-wal` and `index.db-shm`, recreates the schema from `schema/001_init.sql` and rebuilds. There is no repair path and no code that attempts one (ADR-02), and the rebuild is bounded by NFR-PRF-04.

`quick_check` runs in two places, never on the open path: in the background after the window is interactive when the previous session did not shut down cleanly, and on demand from the command palette. Clean shutdown checkpoints with `TRUNCATE` and closes every connection, which removes `-wal` and `-shm`; a `-wal` file present at open is therefore the unclean-shutdown signal, and it costs a `stat` to read.

Crash safety needs no mechanism of its own. WAL recovery replays the journal on first open, and `synchronous = NORMAL` is safe *here* — not in general — precisely because the index is derived: the worst case of a power cut is losing the last committed transactions, and those are re-derived from files that were never at risk. C-02 is what pays for the pragma.

##### What the user sees

A rebuild is a status line, not a modal and not an error: *"Rebuilding index — search and queries are incomplete until this finishes"*, with a count. Editing works throughout, because editing reads and writes files and never the index (goals 1 and 2). Views and search render the partial index and refresh as `index.changed` arrives.

A rebuild is never retried in a loop. If a rebuild fails to complete twice in one session the state becomes `IndexState::Unavailable` with the path and the operating-system error, and the application runs on as an editor with no search — for the same reason a malformed binary is never re-extracted automatically (§7.1). An indexer that spins forever is worse than an index that is missing, and the vault is unharmed either way.

The states and reasons in this section are the enum the front end renders; they are defined once, in `contracts/ipc.rs`, as `IndexState` and `RebuildReason`.

##### Three things this path deliberately does not depend on

**Per-file work.** Nothing on the critical path opens, stats or hashes a note. This matters more than it reads: the reference machine measures with Windows Defender real-time protection disabled, so every per-file operation is cheaper there than on a stock machine, and a startup path that touched 20,000 files would pass the gate and fail the user. The design's answer is not that per-file work is fast; it is that there is none. The walk, which does touch every file, is after `t_usable` on the warm path.

**Git.** Whether the vault is a git repository is the user's business and not ours. `.serat/` is ignored by C-02 and NFR-CMP-03, the recovery journal is a directory of files rather than a commit, and no part of the cold-start path or the index-open check looks for a repository. A vault under git, under Syncthing, or under nothing at all takes the same path.

**An FTS index that is present and valid.** The open check does not touch `blocks_fts` — an FTS integrity check costs 390ms, which is most of the window budget, and rebuilding the whole index because the search index is stale would be a disproportionate response to a recoverable state. Full-text search degrades to unavailable and the FTS index is rebuilt in the background; the vault stays usable meanwhile, which is design goal 5.

##### Why no ADR

Recorded because an absent ADR should read as a judgement rather than an oversight. An ADR is for a decision that is expensive to reverse. Reversing "check cheaply at open, thoroughly in the background" means adding one call at one site, and nothing is built on top of it. Deferring the index off the main thread is not a new decision at all — it is ADR-01's actor. And `index_meta` is a table, so under ADR-02 changing a column costs a rebuild, which is the sanctioned move rather than a cost. Nothing here has the property that makes an ADR worth its weight.

> The costs above were measured on a development machine, not on the reference machine, and through a Python harness rather than the real pipeline. They are three orders of magnitude apart, so the decision is safe; the numbers that go into the baseline are T2's, on the benchmark vault.

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
- ADR-0011 — Which producer the index queue may drop (serves FR-VLT-04, NFR-PRF-03, NFR-REL-01)

## Traceability

Each design element and the requirements it satisfies. Every requirement of priority Must in the baseline has a design element here or in the repo-ready files; the four Won't-priority exclusions have none by construction.

| Design element                          | Where                      | Satisfies                                                                     |
|-----------------------------------------|----------------------------|-------------------------------------------------------------------------------|
| Crate decomposition and dependency rule | §2, Fig. 1                 | NFR-MNT-01, and the testability every verification method assumes             |
| Single-writer index actor               | §2.2, ADR-01               | NFR-PRF-03, NFR-REL-01                                                        |
| Queue overflow policy                   | §3.1.2 step 2, ADR-11      | FR-VLT-04, NFR-REL-01                                                         |
| Indexing pipeline                       | §3, Fig. 2                 | FR-VLT-04, FR-VLT-07, NFR-PRF-02, NFR-PRF-03, NFR-PRF-04                      |
| Echo suppression                        | §3.2                       | FR-VLT-06                                                                     |
| Diff-based reconciliation               | §3.3                       | FR-VLT-05                                                                     |
| Cold-start path and phase budgets       | §3.4                       | NFR-PRF-01, FR-VLT-01                                                         |
| Index open check and rebuild trigger    | §3.4, schema/001_init.sql (index_meta), ADR-02 | NFR-REL-02                                                |
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
| Benchmark vault generator               | §10.3, bench/              | NFR-MNT-02                                                                    |
| Budget jobs in CI, one per NFR-PRF      | §10.3, bench/, CI workflow | NFR-PRF-08                                                                    |

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
