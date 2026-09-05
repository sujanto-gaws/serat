-- Serat index schema — 001_init.sql
--
-- Serves:  FR-VLT-02, FR-STR-03, FR-NAV-02, FR-NAV-04, NFR-PRF-03, NFR-PRF-04,
--          NFR-PRF-05, NFR-REL-02
-- Honours: ADR-0001 (single-writer actor), ADR-0002 (rebuild instead of migrate),
--          ADR-0003 (field pivot by LEFT JOIN, typed value columns)
-- Design:  docs/03-design/architecture.md §3.4, §4; PRD appendix "Data dictionary"
--
-- This file is the definition of the index. Where docs/03-design/architecture.md
-- and this file disagree, this file is correct (architecture.md, §Context diagram).
--
-- It runs as delivered:
--     sqlite3 <vault>/.serat/index.db < schema/001_init.sql
--
-- SCOPE. This is the P0 subset: notes, blocks, fields, links, tasks, the
-- full-text index, and the index's own stamp. Deliberately absent, with the
-- phase that adds them:
--   P1  assets(note_id, mime, bytes, page_count, extractor, extractor_ver,
--       content_hash, extracted_at, needs_ocr)      — FR-BIN-01, FR-BIN-02
--       ix_links_dst_path (re-resolving links when a missing note appears),
--       ix_blocks_ref (block-reference resolution)
--   P2  types(name, path, spec_json)                — FR-STR-01, FR-STR-02
--   P4  chunks(id, note_id, block_lo, block_hi, text) and the sqlite-vec
--       virtual table chunk_vec(chunk_id, embedding FLOAT[384])
--                                                   — FR-AIX-01, ADR-0009
-- Adding any of them is a schema change, and a schema change means "drop the
-- file and rebuild" (ADR-0002). That is why they are not stubbed here: an empty
-- table that P2 will redefine buys nothing when the upgrade path is rm.
--
-- The extraction cache is NOT in this file and never will be. It is expensive to
-- recompute, so it lives outside the index, keyed by content hash and extractor
-- version, and survives an index wipe (architecture.md §7.2, §4.3).
--
-- ============================================================================
-- Connection settings
-- ============================================================================
--
-- journal_mode is persistent (it lives in the file header); the rest are
-- per-connection and MUST be set by serat-index on every connection it opens,
-- writer and readers alike. They are repeated here so that the connection that
-- loads this file behaves like the ones that follow it.
--
--   foreign_keys = ON     -- Set it explicitly on every connection, and know
--                            why: finding F-08 established that our vendored
--                            SQLite is compiled with DEFAULT_FOREIGN_KEYS, so
--                            cascade already fires without it. It is set anyway
--                            because the stock default is OFF and depending on a
--                            vendored build flag is precisely what changes
--                            silently at a version bump. Without it every ON
--                            DELETE CASCADE below is decoration, and blocks_fts
--                            keeps hits for text that no longer exists.
--   busy_timeout = 5000   -- readers never block the single writer under WAL
--                            (ADR-0001), but the checkpointer can.

PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;

-- ----------------------------------------------------------------------------
-- Index version stamp (ADR-0002, NFR-REL-02)
-- ----------------------------------------------------------------------------
-- user_version is NOT a migration marker and there is no upgrade path to a
-- higher one. It exists so that opening the index is a two-way decision and
-- never a three-way one:
--
--     user_version == INDEX_VERSION  ->  open it
--     anything else                  ->  delete the file and rebuild
--
-- There is deliberately no migration machinery to read this value with.
--
-- IT IS NOT A SCHEMA VERSION, and the constant is deliberately not named for
-- one. Bump it in the same commit as EITHER of:
--
--   * a change to any statement in this file; or
--   * a change to what the indexer extracts from input that has not changed --
--     a parser that starts recognising an inline field it used to ignore, a
--     block-kind mapping that stops dropping HTML, a different FTS tokenizer.
--
-- The second case is the one that gets missed, and it was missed here: this
-- comment used to say "any statement in this file", which is a rule no author
-- of a parse change would think applied to them. The failure is silent -- the
-- schema matches, the vault matches, every open check passes, and the rows are
-- quietly derived by a version of the code that no longer exists (F-134).
--
-- One number rather than two. A second constant for the derivation layer would
-- carry the same discipline, the same absence of any test that can enforce it,
-- and one more thing to forget; and both cases have the identical remedy and
-- the identical cost to the user, which is one rebuild. What made a single
-- number look insufficient was its name, and a name is cheaper to fix than a
-- mechanism is to add.

PRAGMA user_version = 3;

-- ============================================================================
-- Tables
-- ============================================================================

-- ----------------------------------------------------------------------------
-- index_meta — what this index is, so that opening it is cheap (§3.4)
-- ----------------------------------------------------------------------------
-- Exactly one row. Every column here describes the INDEX, not the vault: it is
-- derived, disposable and re-derivable by a rebuild, so it does not breach C-02.
-- Nothing about the user's notes may ever be added to it.
--
-- Read on the cold-start path (NFR-PRF-01) to answer "may I use this file?"
-- without paying for PRAGMA quick_check. See architecture.md §3.4.
--
-- INVARIANT for serat-index: any transaction that changes the number of rows in
-- notes updates note_count in the SAME transaction. The cold-start check
-- compares them, so a lag between them is indistinguishable from corruption.
CREATE TABLE index_meta (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    schema_version  INTEGER NOT NULL,      -- must equal PRAGMA user_version
    indexer_version TEXT,                  -- crate version that built it. DIAGNOSTIC ONLY:
                                           -- nothing reads it and nothing should. What
                                           -- invalidates an index is user_version above;
                                           -- wiring a check to a crate version would
                                           -- rebuild 20,000 notes for a patch release that
                                           -- changed a log message (F-134).
    vault_root      TEXT,                  -- canonical vault path this index was built from
    built_at        INTEGER,               -- unix seconds, last completed full rebuild
    note_count      INTEGER NOT NULL DEFAULT 0,  -- must equal (SELECT count(*) FROM notes)
    state           TEXT NOT NULL DEFAULT 'building'
                    CHECK (state IN ('building', 'ready'))
);

-- ----------------------------------------------------------------------------
-- notes — one row per Markdown file, and one per asset (kind='asset')
-- ----------------------------------------------------------------------------
-- FR-VLT-02: every column is derived from the file. Nothing here is a place to
-- put state the Markdown does not encode.
CREATE TABLE notes (
    id           INTEGER PRIMARY KEY,
    path         TEXT    NOT NULL UNIQUE,  -- vault-relative, canonical, / separated
    -- The file name WITH its extension. This is what a bare `[[wikilink]]` or
    -- `![[embed]]` names: Obsidian resolves a target with no `/` in it by file
    -- name anywhere in the vault, not by path and not by frontmatter title
    -- (NFR-CMP-01). Stored rather than computed because resolution joins on it
    -- once per link -- 212,378 of them on the 20k vault -- and a derived
    -- basename cannot use an index (F-312).
    --
    -- With the extension rather than without, because one column then serves
    -- both halves: a note link omits `.md` and matches `file_name = target ||
    -- '.md'`, while an asset embed carries its own extension (`![[report.pdf]]`)
    -- and matches `file_name = target` directly. Assets are linkable by §7.3,
    -- so excluding them here silently drops every embed (F-313).
    file_name    TEXT,
    title        TEXT,                     -- frontmatter title, else H1, else filename
    type         TEXT,                     -- declared type name; NULL = untyped (P2)
    lens         TEXT,                     -- role lens (P3)
    folder_tier  TEXT CHECK (folder_tier IS NULL OR
                             folder_tier IN ('inbox','project','system','note','ref')),
    kind         TEXT    NOT NULL CHECK (kind IN ('note','asset')),
    asset_path   TEXT,                     -- kind='asset': the binary this row stands for
    sidecar_path TEXT,                     -- kind='asset': the .md carrying its frontmatter
    mtime        INTEGER NOT NULL,         -- filesystem mtime at last index
    hash         TEXT    NOT NULL,         -- blake3 of file bytes; the hash gate (§3.1)
                                           -- and the stale guard (§6.1) read this
    word_count   INTEGER,
    -- kind='note' rows describe a Markdown file directly; only assets carry the
    -- two indirection paths. Enforced rather than assumed (design goal 3).
    CHECK (kind = 'asset' OR (asset_path IS NULL AND sidecar_path IS NULL))
);

-- ----------------------------------------------------------------------------
-- blocks — the granularity Serat indexes at (§4.1)
-- ----------------------------------------------------------------------------
-- Block references, AI citations, writable views and binary extraction all fall
-- out of this one table. A block is anchored EITHER by a byte range in a
-- Markdown file OR by a locator inside a binary, never by both.
CREATE TABLE blocks (
    id           INTEGER PRIMARY KEY,
    note_id      INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    ord          INTEGER NOT NULL,         -- position within the note, 0-based
    kind         TEXT    NOT NULL CHECK (kind IN
                     ('para','heading','task','code','list','quote','table','html')),
                 -- 'html' is here and 'rule' is not, deliberately. A raw HTML
                 -- block holds text people search for — a pasted table, an
                 -- embedded callout — and dropping it silently costs full-text
                 -- hits, which design goal 5 forbids. A thematic break holds
                 -- nothing, so a row per '---' across 20,000 notes is pure cost
                 -- and serat-parse emits none. Frontmatter is likewise not a
                 -- block: its fields carry their own byte ranges, and the CHECK
                 -- on fields already says a frontmatter field has no block.
                 -- Finding F-20, raised by rust-core-dev before I3 emitted a
                 -- single row.
    block_ref    TEXT,                     -- author-supplied ^id
    heading_path TEXT,                     -- breadcrumb of enclosing headings (§8.1)
    text         TEXT,                     -- plain text; the FTS content column
    byte_start   INTEGER,                  -- NULL for extracted blocks
    byte_end     INTEGER,                  -- the write-back handle (§6.1, ADR-0008)
    locator      TEXT,                     -- inside a binary: p:12, s:7, Sales!A1:D20
    UNIQUE (note_id, ord),
    CHECK ((byte_start IS NULL) = (byte_end IS NULL)),
    CHECK (byte_start IS NULL OR byte_end >= byte_start),
    -- A block is anchored in bytes or by locator. Both would mean two truths
    -- about where it came from, and the patcher would have to choose.
    CHECK (byte_start IS NULL OR locator IS NULL)
);

-- ----------------------------------------------------------------------------
-- fields — the EAV table the query compiler pivots (ADR-0003, §4.2)
-- ----------------------------------------------------------------------------
-- The value goes in exactly ONE typed column of val_num / val_date / val_bool,
-- chosen at index time from the schema, with val_text always carrying the text
-- projection for display and for untyped fields. That is what lets the compiler
-- know at plan time which column a comparison touches, so a date range is an
-- indexed scan and not a per-row string parse (FR-STR-05, NFR-PRF-05).
--
-- id is load-bearing: it is the __fid every editable view cell carries back to
-- the write-back patcher (§5.2, §6.1). Do not make this table WITHOUT ROWID.
CREATE TABLE fields (
    id         INTEGER PRIMARY KEY,
    note_id    INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    block_id   INTEGER REFERENCES blocks(id) ON DELETE CASCADE,
    key        TEXT    NOT NULL,
    val_text   TEXT,
    val_num    REAL,
    val_date   TEXT,                       -- ISO-8601, so lexical order is chronological
    val_bool   INTEGER CHECK (val_bool IS NULL OR val_bool IN (0,1)),
    source     TEXT    NOT NULL CHECK (source IN ('frontmatter','inline')),
    byte_start INTEGER,                    -- start of the VALUE's bytes: the patch target
    byte_end   INTEGER,
    -- Frontmatter fields belong to the note, inline fields to the block they were
    -- written in. FR-STR-03 indexes both identically; their provenance differs.
    CHECK ((source = 'frontmatter') = (block_id IS NULL)),
    CHECK ((byte_start IS NULL) = (byte_end IS NULL)),
    CHECK (byte_start IS NULL OR byte_end >= byte_start)
);

-- ----------------------------------------------------------------------------
-- links — the graph. Backlinks read dst, rollups read src (§4.4)
-- ----------------------------------------------------------------------------
CREATE TABLE links (
    id            INTEGER PRIMARY KEY,
    src_note      INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    src_block     INTEGER REFERENCES blocks(id) ON DELETE CASCADE,
    dst_path      TEXT,                    -- raw target as written, before resolution
    dst_note      INTEGER REFERENCES notes(id) ON DELETE SET NULL,
    dst_block_ref TEXT,                    -- targeted ^id
    kind          TEXT NOT NULL CHECK (kind IN ('wiki','embed','md','tag','unresolved')),
    alias         TEXT,
    -- A tag names no note and never will, so it can never carry a dst_note.
    -- Structural rather than remembered: see the note below.
    CHECK (kind <> 'tag' OR dst_note IS NULL)
);
-- Resolution. kind records how the link was written; dst_note records whether it
-- found a target. The test for "unresolved, and worth retrying when a new note
-- appears" is therefore NOT dst_note IS NULL on its own:
--
--     dst_note IS NULL AND kind <> 'tag'
--
-- because a tag has no target by construction, and a re-resolution pass that
-- trusts the shorter predicate would try to resolve every tag in the vault on
-- every pass, forever. Tags live in this table on purpose — tag search and the
-- link graph are then one mechanism rather than two, which is the same reasoning
-- that makes a task a block — and the CHECK above is what keeps the longer
-- predicate honest instead of merely documented. Finding F-22, caught by
-- rust-core-dev reading the DDL against its own comment before any code
-- depended on it.
--
-- P1 adds the index that makes the predicate cheap and keeps it in one place,
-- which is where the re-resolution pass must read it from:
--
--     CREATE INDEX ix_links_unresolved ON links(dst_path)
--         WHERE dst_note IS NULL AND kind <> 'tag';
--
-- It is not here because P0 does not re-resolve links, and an index P0 never
-- reads would still cost P0's rebuild benchmark (NFR-PRF-04) to build.
--
-- ON DELETE SET NULL on dst_note is what makes deleting a note mark every
-- inbound link unresolved in the same statement, so the index can never hold a
-- dangling note id — and the re-resolution pass has dst_path to work from.

-- ----------------------------------------------------------------------------
-- tasks — a task is a block, not a new mechanism (§10.4)
-- ----------------------------------------------------------------------------
CREATE TABLE tasks (
    id         INTEGER PRIMARY KEY,
    note_id    INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    block_id   INTEGER NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
    status     TEXT    NOT NULL,           -- the raw marker: space, x, /, -, > ...
    done       INTEGER NOT NULL CHECK (done IN (0,1)),
    text       TEXT,                       -- task text with inline fields stripped
    due        TEXT,                       -- from due::   (ISO-8601)
    owner      TEXT,                       -- from owner::
    est        REAL,                       -- from est::   (the usual rollup target)
    byte_start INTEGER,                    -- just the marker byte: the patch target
    byte_end   INTEGER,
    CHECK ((byte_start IS NULL) = (byte_end IS NULL)),
    CHECK (byte_start IS NULL OR byte_end >= byte_start)
);
-- due, owner and est are denormalised from inline fields, and serat-index fills
-- them from the field rows it is already writing — not serat-parse. Which keys
-- are special is a schema question, and it becomes the schema registry's answer
-- in P2 (FR-STR-01); putting it in the parser would mean the same decision
-- living in two places, and would give serat-parse domain knowledge that has
-- nothing to do with Markdown. serat-index needs no Markdown knowledge for this
-- either: it reads field keys off rows, never off a document.
--
-- byte_start/byte_end are Task::marker_range — just the character between the
-- brackets, the smallest possible instance of ADR-0008 (F-237). Mirrors fields'
-- pair rather than inventing a second shape for the same idea: a recorded range
-- lets §6.1 step 3 verify the byte it is about to patch still holds what it
-- claims, the same way it does for a field, instead of the P2 patcher re-locating
-- a task marker by rescanning the block — a second definition of where it is.
--
-- Why both status and done: status keeps the author's marker, because a vault
-- that uses / for in-progress must survive a round trip (C-01). done is the
-- binary projection ix_tasks_open is partial on. The marker-to-done mapping is
-- computed by serat-parse and written here — it is not a generated column,
-- because putting Markdown semantics in SQL would split the parser across two
-- languages and serat-index owns no knowledge of Markdown (§2.1).

-- ============================================================================
-- Indexes (architecture.md §4.4)
-- ============================================================================
-- Every index below is justified by a read path that exists. None of them is
-- chosen by the planner reliably without statistics; see the ANALYZE at the end.

-- Every typed query's outer scan (FR-STR-05). Partial, because an untyped note
-- is always valid and most of a real vault is untyped, so the NULLs are the bulk
-- of the index and nobody searches them.
--
-- Measured on a 20,000-note fixture, 500 of them typed (SQLite 3.50.4):
--   full index    ANALYZE says "20000 10000" — two distinct values, so the
--                 planner's estimate for type='project' is 10,000 rows.
--   partial index ANALYZE says "500 500", the truth, and the index holds 500
--                 entries instead of 20,000.
-- Both produce the same plan for type='project' and for type IS NOT NULL
-- (SEARCH ... USING COVERING INDEX ix_notes_type); the partial one gives the
-- planner an estimate that is not off by 20x, which is what §4.3 is about.
-- Cost accepted: GROUP BY type (a P2 validation view) becomes a scan plus a
-- temp b-tree rather than an ordered index walk. That is 20,000 rows, once.
CREATE INDEX ix_notes_type ON notes(type) WHERE type IS NOT NULL;

-- Listing assets, and separating notes from assets in every other scan. This is
-- the index the planner wrongly preferred over ix_notes_type before ANALYZE
-- existed (§4.3): kind='note' matches nearly every row, type='project' a slice.
CREATE INDEX ix_notes_kind ON notes(kind);
-- Link resolution matches a bare `[[wikilink]]` against this. Without it the
-- join is a full scan of notes per link and the rebuild does not finish inside
-- NFR-PRF-04's budget (F-312).
CREATE INDEX ix_notes_file_name ON notes(file_name) WHERE file_name IS NOT NULL;

-- The pivot join, one lookup per referenced field key per row (ADR-0003).
-- val_text is carried so the common untyped projection is answered from the
-- index alone; fields.id rides along as the rowid, which is the __fid anchor.
CREATE INDEX ix_fields_note_key ON fields(note_id, key, val_text);

-- Range and equality predicates on one field across the whole vault. Partial,
-- because only the rows whose value landed in that column can ever match, and a
-- vault of 20,000 notes has far more text fields than date fields.
--
-- NOTE FOR THE SQ LOWERING (grammar/lowering.md). SQLite's partial-index prover
-- was measured, not assumed, against these three indexes on 3.50.4. Every
-- predicate shape the lowering emits proves val_x IS NOT NULL on its own:
--     key=? AND val_text=?                 -> SEARCH ... ix_fields_key_text
--     key=? AND val_date<?                 -> SEARCH ... ix_fields_key_date
--     key=? AND val_date BETWEEN ? AND ?   -> SEARCH ... ix_fields_key_date
--     key=? AND val_date IN (?,?)          -> SEARCH ... ix_fields_key_date
--     key=? AND val_date LIKE '2026-03%'   -> SEARCH ... ix_fields_key_date
--     key=? AND val_num>?                  -> SEARCH ... ix_fields_key_num
-- so no redundant IS NOT NULL term is needed. The one shape that cannot use
-- them is a bare `key=?` with no predicate on the value — a projection-only
-- reference — which is a full scan of fields. The lowering must reach a
-- projected field through the pivot join on ix_fields_note_key(note_id, key),
-- never by selecting on key alone.
CREATE INDEX ix_fields_key_date ON fields(key, val_date) WHERE val_date IS NOT NULL;
CREATE INDEX ix_fields_key_num  ON fields(key, val_num)  WHERE val_num  IS NOT NULL;
CREATE INDEX ix_fields_key_text ON fields(key, val_text) WHERE val_text IS NOT NULL;

-- Rollups read src; the backlinks pane reads dst (FR-NAV-04).
CREATE INDEX ix_links_src ON links(src_note);
CREATE INDEX ix_links_dst ON links(dst_note);

-- Every open-task query, and it is always ordered by due date. Partial on
-- done = 0, which is the only side anyone queries: a vault accumulates done
-- tasks forever and never reads them in bulk.
CREATE INDEX ix_tasks_open ON tasks(due, note_id) WHERE done = 0;

-- ============================================================================
-- Full-text search (FR-NAV-02)
-- ============================================================================
-- External content: the text lives once, in blocks. An external-content FTS5
-- table over blocks is what makes a phrase inside a PDF and a phrase inside a
-- note come out of the same search box with no second code path — an extracted
-- block is a block (§7).
--
-- rowid = blocks.id, so a hit joins straight back to its note, byte range or
-- locator, which is what "a snippet and a precise location" in FR-NAV-02 means.
--
-- Not here on purpose: prefix='2 3'. Search-as-you-type (NFR-PRF-06) wants it and
-- it costs rebuild time and file size, both of which P0 is measuring. P1 adds it
-- with the NFR-PRF-04 and NFR-PRF-06 numbers side by side.
CREATE VIRTUAL TABLE blocks_fts USING fts5(
    text,
    content       = 'blocks',
    content_rowid = 'id',
    tokenize      = "unicode61 remove_diacritics 2"
);

-- The triggers that keep it in step. The delete path is the one that is usually
-- wrong and is verified: an external-content FTS5 table cannot see the old row,
-- so a delete command must replay the OLD text exactly as it was indexed. Get
-- this wrong and the index rots silently — no error, just missing hits.
--
-- A FULL REBUILD MUST NOT LOAD THROUGH THESE TRIGGERS. Measured: 26,000 blocks
-- took 2.1 s in one transaction and 6.8-10.3 s across several with the triggers
-- live, against 0.32 s when they were dropped and blocks_fts was built once with
-- the FTS5 'rebuild' command; the resulting search index is identical. The
-- rebuild sequence in serat-index is therefore:
--
--   1. run this file
--   2. SELECT sql FROM sqlite_master WHERE type='trigger' AND tbl_name='blocks'
--      -- keep the strings, then DROP the three triggers
--   3. bulk-load notes, blocks, fields, links, tasks in batched transactions
--   4. INSERT INTO blocks_fts(blocks_fts) VALUES('rebuild')
--   5. re-execute the strings from step 2
--   6. UPDATE index_meta ... state='ready'; ANALYZE; wal_checkpoint(TRUNCATE)
--
-- Step 2 reads the triggers out of the database rather than keeping a second
-- copy of this DDL anywhere, so what is restored is identical to what was
-- delivered by construction and not by a test that has to catch a stale copy.
--
-- The INCREMENTAL path keeps the triggers. One save is a dozen blocks, and at
-- 100 blocks per transaction the trigger path cost 15 ms — inside NFR-PRF-03.
--
-- These fire on cascaded deletes as well as direct ones — verified by running
-- it on SQLite 3.42.0 and 3.50.4 here and, per finding F-02, through rusqlite on
-- the 3.53.2 the application actually links, with foreign_keys = ON and recursive_triggers
-- left at its default of 0. That is what makes "delete the note, delete its
-- blocks, delete its search rows" one DELETE FROM notes, and it is the reason
-- foreign_keys = ON is not optional on any connection: with it off, the blocks
-- survive, the triggers never run, and the FTS index silently keeps hits for
-- text that no longer exists.
CREATE TRIGGER blocks_fts_ai AFTER INSERT ON blocks BEGIN
    INSERT INTO blocks_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TRIGGER blocks_fts_ad AFTER DELETE ON blocks BEGIN
    INSERT INTO blocks_fts(blocks_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;

CREATE TRIGGER blocks_fts_au AFTER UPDATE ON blocks BEGIN
    INSERT INTO blocks_fts(blocks_fts, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO blocks_fts(rowid, text) VALUES (new.id, new.text);
END;

-- ============================================================================
-- Seed
-- ============================================================================
-- state='building' until the first full rebuild commits. An index opened in this
-- state was interrupted mid-build and is discarded (§3.4, NFR-REL-02).
INSERT INTO index_meta (id, schema_version, state) VALUES (1, 2, 'building');

-- ============================================================================
-- ANALYZE (architecture.md §4.3)
-- ============================================================================
-- This one produces no statistics — the tables are empty — but it creates
-- sqlite_stat1, so the planner has somewhere to read from and every later
-- ANALYZE is a data change rather than a schema change.
--
-- The statistics that matter are collected by serat-index, which MUST re-run
-- ANALYZE after every full rebuild and after any batch touching more than 5% of
-- notes (§3.1 step 5). Without them the planner picks ix_notes_kind over
-- ix_notes_type and every typed query degrades into a full scan (§4.3).
ANALYZE;
