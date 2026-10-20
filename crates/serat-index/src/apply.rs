//! Turning an [`IndexOp`] into rows.
//!
//! Every function here runs inside a transaction owned by the writer thread. None of
//! them opens a connection, and none is public: the only way to change the index is
//! to send a message to the one writer (ADR-0001).
//!
//! # Why every statement goes through `prepare_cached`
//!
//! `Transaction::execute` prepares its statement afresh on every call, and these
//! calls sit in per-row loops over blocks, fields, links and tasks. A 20,000-note
//! rebuild re-parsed the same eight SQL strings roughly a million times: measured at
//! **16.0 s of a ~21 s writer**, which was the whole of NFR-PRF-04's bottleneck
//! (F-324). `prepare_cached` keys a per-connection cache on the SQL text, so each is
//! parsed once; the same rebuild's apply phase fell to 6.6 s.
//!
//! So `tx.execute(SQL, params)` reads more simply and is the wrong call here. It is
//! also the exact edit someone makes while tidying, which is why the reason lives at
//! the top of the file rather than beside one of the eight. The cache capacity is
//! pinned in `schema::STATEMENT_CACHE` for the same reason: outgrowing it is silent.

use rusqlite::{OptionalExtension, Transaction, params};
use serat_core::{
    Block, BlockKind, Field, FieldValue, IndexError, IndexOp, Link, LinkKind, NoteMeta, ParsedNote,
    Task, VaultPath, fold_case,
};

use crate::resolve;
use crate::schema::sqlite;

/// How many rows a batch touched, so the caller can decide about `ANALYZE`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Applied {
    /// Notes inserted or replaced.
    pub notes: usize,
    /// Notes removed.
    pub removed: usize,
}

/// Apply one operation.
///
/// `bulk_load` is `true` only while a rebuild is loading (writer.rs's `commit`, driven
/// by `begin_bulk_load`/`end_bulk_load`). It exists solely to gate the per-row link
/// resolve in `upsert_note`/`upsert_asset` — see F-371 on `upsert_note` for why a
/// rebuild must not pay that cost per row.
pub(crate) fn apply(
    tx: &Transaction<'_>,
    op: &IndexOp,
    bulk_load: bool,
) -> Result<Applied, IndexError> {
    match op {
        IndexOp::UpsertNote { meta, parsed } => {
            upsert_note(tx, meta, parsed, bulk_load)?;
            Ok(Applied {
                notes: 1,
                removed: 0,
            })
        }
        IndexOp::UpsertAsset { meta } => {
            upsert_asset(tx, meta, bulk_load)?;
            Ok(Applied {
                notes: 1,
                removed: 0,
            })
        }
        IndexOp::Remove { path } => {
            let gone = remove(tx, path)?;
            // B1 (architecture §4.5): `ON DELETE SET NULL` unresolved every inbound link;
            // those whose written target also names a remaining note go back to it.
            // Without this the index differs from a rebuild's, which C-02 forbids.
            if gone && !bulk_load {
                resolve_inbound(tx, path)?;
            }
            Ok(Applied {
                notes: 0,
                removed: usize::from(gone),
            })
        }
        IndexOp::Rename { from, to } => {
            rename(tx, from, to, bulk_load)?;
            Ok(Applied::default())
        }
    }
}

/// Replace every row for a note.
///
/// Delete-then-insert rather than a diff. Blocks are identified by position, so a
/// partial update would have to work out which block moved where — and re-inserting a
/// note's rows inside a transaction that is open anyway is cheaper than being clever.
/// `ON DELETE CASCADE` takes the blocks, fields, links and tasks with the note row,
/// and the FTS triggers follow the blocks.
///
/// # F-371: `bulk_load` gates the per-row resolve below
///
/// Task #13's `resolve_inbound`/`resolve_outbound` calls are correct and necessary for
/// the path they were written for — one save, one note, an `IN (...)` lookup served by
/// `ix_links_unresolved`. They are also shared code (this function is the rebuild's
/// only way to write a note too — architecture.md §3.1's "two paths", corrected by
/// F-371 to name the third), so a cost added here is paid once by a save and roughly
/// forty thousand times by a rebuild over the 20k vault. Measured: `prf04-full-rebuild`
/// burned over 1,269 CPU-seconds without finishing, against a 30 s budget and the
/// ~21–27 s the same job took before this call existed (F-324's baseline).
///
/// The rebuild already runs one full `resolve_links` pass over the whole table after
/// every note is loaded (`rebuild.rs`, after `end_bulk_load`), so the per-row work is
/// not merely expensive there — it is redundant. `bulk_load` skips it for exactly that
/// caller; the save and watcher paths (`bulk_load == false`) are unaffected and still
/// resolve as task #13 intended.
fn upsert_note(
    tx: &Transaction<'_>,
    meta: &NoteMeta,
    parsed: &ParsedNote,
    bulk_load: bool,
) -> Result<(), IndexError> {
    remove(tx, &meta.path)?;

    let word_count: i64 = parsed
        .blocks
        .iter()
        .map(|block| block.text.split_whitespace().count() as i64)
        .sum();

    tx.prepare_cached(
        "INSERT INTO notes (path, file_name, path_fold, file_name_fold,
                            title, type, kind, mtime, hash, word_count)
         VALUES (?1, ?2, ?8, ?9, ?3, ?4, 'note', ?5, ?6, ?7)",
    )
    .map_err(sqlite)?
    .execute(params![
        meta.path.as_str(),
        // What a bare `[[wikilink]]` or `![[embed]]` names (F-312).
        // With the extension, so one column serves note links (which omit
        // `.md`) and asset embeds (which carry their own) alike.
        meta.path.file_name(),
        parsed.title.as_deref(),
        parsed.note_type.as_deref(),
        meta.mtime_ms,
        meta.hash.to_hex(),
        word_count,
        // The folded keys link resolution compares by (architecture §4.5).
        fold_case(meta.path.as_str()),
        fold_case(meta.path.file_name()),
    ])
    .map_err(sqlite)?;
    let note_id = tx.last_insert_rowid();

    // Block ord to block id, so fields, links and tasks can name the block they were
    // found in. The parser refers to blocks by position because it has no ids to use.
    let mut block_ids = Vec::with_capacity(parsed.blocks.len());
    for block in &parsed.blocks {
        block_ids.push(insert_block(tx, note_id, block)?);
    }
    let id_of = |ord: Option<u32>| -> Option<i64> {
        ord.and_then(|ord| block_ids.get(ord as usize).copied().flatten())
    };

    for field in &parsed.fields {
        insert_field(tx, note_id, id_of(field.block_ord), field)?;
    }
    for link in &parsed.links {
        insert_link(tx, note_id, id_of(link.block_ord), link)?;
    }
    for task in &parsed.tasks {
        if let Some(block_id) = id_of(Some(task.block_ord)) {
            insert_task(tx, note_id, block_id, task, &parsed.fields)?;
        }
    }

    // task #13 (FR-NAV-04). The `remove` above deleted the previous row under this
    // path, if there was one, and `ON DELETE SET NULL` nulled every inbound link the
    // cascade could see -- including links this very save is about to restore, since
    // the note comes back under a new rowid. Without this, only a full rebuild ever
    // repairs a backlink to a note that was saved incrementally even once.
    //
    // Skipped during a bulk load (F-371): see this function's doc comment. The
    // rebuild's own full resolve_links pass at the end covers what this would have
    // done, at a fraction of the cost.
    if !bulk_load {
        resolve_inbound(tx, &meta.path)?;
        resolve_rivals(tx, &meta.path)?;
        // The note's own outbound links, inserted unresolved just above: point them at
        // whatever they name that already exists. Symmetric with resolve_inbound, and
        // both are needed -- one fixes what this save broke, the other finishes what it
        // could not have done at insert time (a link's own rows do not yet have a
        // dst_note to compare against until they are in the table).
        resolve_outbound(tx, note_id)?;
    }

    Ok(())
}

/// The four folded spellings under which a link can name the note at `path`, then
/// each with `.md` appended: the written targets that can reach it
/// (`resolve::RESOLVE_RIVALS`).
///
/// The first four are `resolve`'s matching rule read in reverse: a link's `dst_fold`
/// names a note `n` when `n.path_fold` equals it, or equals it plus `.md`, or -- when
/// the written target has no `/`, i.e. names a file rather than a path (F-312) --
/// `n.file_name_fold` equals it either way. So what could name *this* note is its
/// path, its path without a trailing `.md`, its file name, and its file name without a
/// trailing `.md`. Always four slots, with a duplicate where a spelling does not apply
/// (an asset's path never ends in `.md`), so the SQL text never varies
/// (`schema::STATEMENT_CACHE`'s whole point). Folded here, in Rust, by the function
/// that folded the stored columns: the statements contain no folding of their own.
fn spellings(path: &VaultPath) -> [String; 8] {
    let whole = path.as_str();
    let name = path.file_name();
    let four = [
        whole,
        whole.strip_suffix(".md").unwrap_or(whole),
        name,
        name.strip_suffix(".md").unwrap_or(name),
    ]
    .map(fold_case);
    let [a, b, c, d] = four;
    let [e, f, g, h] = [&a, &b, &c, &d].map(|s| format!("{s}.md"));
    [a, b, c, d, e, f, g, h]
}

/// Point every currently-unresolved link that could name `path` at the note it now
/// identifies. Architecture §4.5 **A1** (a note appears) and **B1** (a note
/// disappears: its orphaned inbound links, nulled by `ON DELETE SET NULL`, are
/// re-pointed at a remaining same-named note or left unresolved).
///
/// Scoped by `dst_fold`, not a pass over the whole `links` table: an incremental save
/// touches one note, and paying for a full scan on every save is what left this path
/// silently skipping FR-NAV-04 in the first place. `ix_links_unresolved`
/// (schema/001_init.sql) keeps the `IN` lookup cheap rather than a table scan wearing
/// an index's name.
fn resolve_inbound(tx: &Transaction<'_>, path: &VaultPath) -> Result<(), IndexError> {
    let s = spellings(path);
    tx.prepare_cached(resolve::RESOLVE_INBOUND)
        .map_err(sqlite)?
        .execute(params![s[0], s[1], s[2], s[3]])
        .map_err(sqlite)?;
    Ok(())
}

/// Architecture §4.5 **A2**: a note appeared at `path`, so links that already resolve
/// to *another* note under the same spelling may now prefer this one as the shorter
/// path. Re-runs the resolver over exactly those links.
fn resolve_rivals(tx: &Transaction<'_>, path: &VaultPath) -> Result<(), IndexError> {
    let s = spellings(path);
    tx.prepare_cached(resolve::RESOLVE_RIVALS)
        .map_err(sqlite)?
        .execute(params![s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
        .map_err(sqlite)?;
    Ok(())
}

/// Point `note_id`'s own unresolved outbound links at whatever they name that already
/// exists.
///
/// The mirror of `resolve_inbound`, scoped by `src_note` instead of `dst_path` for
/// the same reason: this note's rows are the only ones this save could have made
/// resolvable, so the general pass over every unresolved link in the vault
/// (`writer::resolve_links`, the rebuild's one full pass) is not needed here.
fn resolve_outbound(tx: &Transaction<'_>, note_id: i64) -> Result<(), IndexError> {
    tx.prepare_cached(resolve::RESOLVE_OUTBOUND)
        .map_err(sqlite)?
        .execute(params![note_id])
        .map_err(sqlite)?;
    Ok(())
}

/// A file that is not Markdown: recorded, not parsed.
///
/// Design goal 5. Until an extractor claims it (P2, architecture §7) it has a row, a
/// path and a hash, so it is linkable and queryable even though nothing can search
/// inside it.
fn upsert_asset(tx: &Transaction<'_>, meta: &NoteMeta, bulk_load: bool) -> Result<(), IndexError> {
    remove(tx, &meta.path)?;
    // Assets carry a file_name too: `![[report.pdf]]` names one, and §7.3
    // says an asset stays typed, linkable and queryable. Omitting it here
    // silently drops every embed (F-313).
    tx.prepare_cached(
        "INSERT INTO notes (path, file_name, kind, asset_path, mtime, hash,
                            path_fold, file_name_fold)
         VALUES (?1, ?2, 'asset', ?1, ?3, ?4, ?5, ?6)",
    )
    .map_err(sqlite)?
    .execute(params![
        meta.path.as_str(),
        meta.path.file_name(),
        meta.mtime_ms,
        meta.hash.to_hex(),
        fold_case(meta.path.as_str()),
        fold_case(meta.path.file_name()),
    ])
    .map_err(sqlite)?;

    // task #13. The remove() above nulled every inbound link the cascade could see,
    // same as upsert_note -- and F-313 is exactly why an asset needs this call too:
    // an embed can name an asset, so an asset's own inbound backlinks are as real a
    // gap as a note's. Assets have no outbound links, so there is no
    // resolve_outbound counterpart here.
    //
    // Skipped during a bulk load (F-371) -- see upsert_note's doc comment.
    if !bulk_load {
        resolve_inbound(tx, &meta.path)?;
        resolve_rivals(tx, &meta.path)?;
    }

    Ok(())
}

/// Delete a note and everything hanging off it. Returns whether a row was there.
fn remove(tx: &Transaction<'_>, path: &VaultPath) -> Result<bool, IndexError> {
    let removed = tx
        .prepare_cached("DELETE FROM notes WHERE path = ?1")
        .map_err(sqlite)?
        .execute(params![path.as_str()])
        .map_err(sqlite)?;
    Ok(removed > 0)
}

/// Move a note, keeping its rows and re-resolving the links the move changes.
///
/// The row keeps its id, so its blocks, fields and outbound links stay. What a move
/// changes is which *written targets* name it, so (architecture §4.5):
///
/// 1. set `path`, `file_name` and their folds;
/// 2. unresolve its inbound links -- a link written `[[old/P]]` must stop pointing at
///    the note now at `Q`;
/// 3. **B1** for the old path: those links go to another same-named note, or stay
///    unresolved;
/// 4. **A1 and A2** for the new path: links whose written target names `Q` resolve
///    back to it, and rivals that now lose to the shorter path are re-pointed.
///
/// `title` is left alone: the op carries no parse, and a title taken from the file
/// name is only corrected by the next `UpsertNote` for the new path.
fn rename(
    tx: &Transaction<'_>,
    from: &VaultPath,
    to: &VaultPath,
    bulk_load: bool,
) -> Result<(), IndexError> {
    let id: Option<i64> = tx
        .prepare_cached("SELECT id FROM notes WHERE path = ?1")
        .map_err(sqlite)?
        .query_row(params![from.as_str()], |row| row.get(0))
        .optional()
        .map_err(sqlite)?;
    let Some(id) = id else {
        return Ok(());
    };
    tx.prepare_cached(
        "UPDATE notes SET path = ?2, file_name = ?3, path_fold = ?4, file_name_fold = ?5
          WHERE id = ?1",
    )
    .map_err(sqlite)?
    .execute(params![
        id,
        to.as_str(),
        to.file_name(),
        fold_case(to.as_str()),
        fold_case(to.file_name())
    ])
    .map_err(sqlite)?;
    if bulk_load {
        return Ok(());
    }
    tx.prepare_cached("UPDATE links SET dst_note = NULL WHERE dst_note = ?1 AND kind <> 'tag'")
        .map_err(sqlite)?
        .execute(params![id])
        .map_err(sqlite)?;
    resolve_inbound(tx, from)?;
    resolve_inbound(tx, to)?;
    resolve_rivals(tx, to)?;
    Ok(())
}

fn insert_block(
    tx: &Transaction<'_>,
    note_id: i64,
    block: &Block,
) -> Result<Option<i64>, IndexError> {
    let Some(kind) = block_kind(&block.kind) else {
        // No column value exists for this kind yet, so the row is not written. See
        // block_kind for what that currently costs and why it is not silent.
        return Ok(None);
    };

    let heading_path = if block.heading_path.is_empty() {
        None
    } else {
        Some(block.heading_path.join(" › "))
    };

    tx.prepare_cached(
        "INSERT INTO blocks (note_id, ord, kind, block_ref, heading_path, text, byte_start, byte_end)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .map_err(sqlite)?
    .execute(params![
            note_id,
            block.ord,
            kind,
            block.block_ref.as_deref(),
            heading_path,
            block.text,
            block.range.start as i64,
            block.range.end as i64,
        ],
    )
    .map_err(sqlite)?;
    Ok(Some(tx.last_insert_rowid()))
}

/// The `blocks.kind` value for a parsed block, if the schema has one.
///
/// `blocks.kind` is a CHECK constraint over seven values and the parser can produce
/// three more: frontmatter, thematic breaks and raw HTML. Frontmatter and rules are
/// correctly absent — frontmatter's content is indexed as fields, and a rule has no
/// text anyone can search for.
///
/// **HTML is a real gap.** A raw HTML block holds text people search for — a pasted
/// table, an embedded callout — and dropping it costs those full-text hits, which
/// design goal 5 says should degrade rather than disappear. It is dropped rather than
/// mislabelled as a paragraph because `blocks.kind` meaning "roughly what this is"
/// would be the more expensive lie. Architect has the question of widening the CHECK;
/// the test below pins the current behaviour so the gap is visible rather than
/// discovered later as missing search results.
fn block_kind(kind: &BlockKind) -> Option<&'static str> {
    match kind {
        BlockKind::Heading { .. } => Some("heading"),
        BlockKind::Paragraph => Some("para"),
        BlockKind::ListItem { .. } => Some("list"),
        BlockKind::Task { .. } => Some("task"),
        BlockKind::Code { .. } => Some("code"),
        BlockKind::Quote => Some("quote"),
        BlockKind::Table => Some("table"),
        BlockKind::Frontmatter | BlockKind::Rule | BlockKind::Html => None,
    }
}

/// Write a field into exactly one typed column.
///
/// `val_text` always carries the text projection for display and for untyped
/// comparison; the typed column is what the query compiler plans against
/// (architecture §4.2). A list is many rows with the same key, which is why a
/// list-valued field is filtered with EXISTS rather than joined (§5.2).
fn insert_field(
    tx: &Transaction<'_>,
    note_id: i64,
    block_id: Option<i64>,
    field: &Field,
) -> Result<(), IndexError> {
    for value in flatten(&field.value) {
        let (text, num, date, boolean) = columns(value);
        tx.prepare_cached(
            "INSERT INTO fields
                 (note_id, block_id, key, val_text, val_num, val_date, val_bool, source,
                  byte_start, byte_end)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )
        .map_err(sqlite)?
        .execute(params![
            note_id,
            block_id,
            field.key,
            text,
            num,
            date,
            boolean,
            source_name(field.source),
            field.value_range.start as i64,
            field.value_range.end as i64,
        ])
        .map_err(sqlite)?;
    }
    Ok(())
}

/// One row per scalar. A nested list flattens: the schema has no place for depth and
/// nobody queries for one.
fn flatten(value: &FieldValue) -> Vec<&FieldValue> {
    match value {
        FieldValue::List(values) => values.iter().flat_map(flatten).collect(),
        scalar => vec![scalar],
    }
}

/// The four value columns for one scalar.
fn columns(value: &FieldValue) -> (Option<String>, Option<f64>, Option<String>, Option<i64>) {
    match value {
        FieldValue::Text(text) => (Some(text.clone()), None, None, None),
        FieldValue::Num(number) => (Some(number.to_string()), Some(*number), None, None),
        FieldValue::Date(date) => (Some(date.clone()), None, Some(date.clone()), None),
        FieldValue::Bool(flag) => (Some(flag.to_string()), None, None, Some(i64::from(*flag))),
        // Already flattened; a list cannot reach here.
        FieldValue::List(_) => (None, None, None, None),
    }
}

fn source_name(source: serat_core::FieldSource) -> &'static str {
    match source {
        serat_core::FieldSource::Frontmatter => "frontmatter",
        serat_core::FieldSource::Inline => "inline",
    }
}

/// Insert a link, unresolved.
///
/// `dst_note` is left NULL: resolving "which file does `[[Meeting]]` mean" needs the
/// whole vault, and during a rebuild the target may not be indexed yet. Resolution is
/// a pass over `links` after every note exists.
fn insert_link(
    tx: &Transaction<'_>,
    note_id: i64,
    block_id: Option<i64>,
    link: &Link,
) -> Result<(), IndexError> {
    let kind = match link.kind {
        LinkKind::Wikilink => "wiki",
        LinkKind::Embed => "embed",
        LinkKind::Markdown => "md",
        LinkKind::Tag => "tag",
    };
    tx.prepare_cached(
        "INSERT INTO links (src_note, src_block, dst_path, dst_fold, dst_block_ref, kind, alias)
         VALUES (?1, ?2, ?3, ?7, ?4, ?5, ?6)",
    )
    .map_err(sqlite)?
    .execute(params![
        note_id,
        block_id,
        link.target.note,
        link.target.block_ref.as_deref(),
        kind,
        link.target.alias.as_deref(),
        // Stored folded, in Rust, so resolution is a binary `=` (architecture §4.5).
        fold_case(&link.target.note),
    ])
    .map_err(sqlite)?;
    Ok(())
}

/// Insert a task.
///
/// `status` keeps the author's marker so a vault using `/` for in-progress survives a
/// round trip (C-01); `done` is the binary projection `ix_tasks_open` is partial on.
/// The mapping between them is the parser's, not SQL's.
///
/// `byte_start`/`byte_end` are `task.marker_range` — just the character between the
/// brackets, the smallest possible instance of ADR-0008 (F-237). `due`/`owner`/`est`
/// are rolled up from `fields` here rather than parsed by `serat-parse` (F-253,
/// schema/001_init.sql's comment above `CREATE TABLE tasks`): they are ordinary inline
/// fields on the same block, so serat-index reads them off the rows it is already
/// writing in this transaction and needs no Markdown knowledge to do it.
fn insert_task(
    tx: &Transaction<'_>,
    note_id: i64,
    block_id: i64,
    task: &Task,
    fields: &[Field],
) -> Result<(), IndexError> {
    let (due, owner, est) = task_rollup(fields, task.block_ord);
    tx.prepare_cached(
        "INSERT INTO tasks (note_id, block_id, status, done, text, due, owner, est,
                             byte_start, byte_end)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .map_err(sqlite)?
    .execute(params![
        note_id,
        block_id,
        task.marker.to_string(),
        i64::from(task.done),
        task.text,
        due,
        owner,
        est,
        task.marker_range.start as i64,
        task.marker_range.end as i64,
    ])
    .map_err(sqlite)?;
    Ok(())
}

/// `due`, `owner` and `est` for one task's block, read off the fields already parsed
/// for it — not queried back from the database, since this runs before the fields
/// inserted above are visible to anything but this same transaction's own writes.
///
/// Which three keys are special is a schema question and becomes the schema
/// registry's answer in P2 (FR-STR-01); fixed here for P0, matching the DDL comment.
/// A list-valued `due`/`owner`/`est` does not roll up — none of the three is a key
/// anyone writes more than once per task, and inventing a first-wins rule for a case
/// nobody has produced would be a decision made up rather than one asked for.
fn task_rollup(fields: &[Field], block_ord: u32) -> (Option<String>, Option<String>, Option<f64>) {
    let mut due = None;
    let mut owner = None;
    let mut est = None;
    for field in fields {
        if field.block_ord != Some(block_ord) {
            continue;
        }
        let (text, num, date, _) = columns(&field.value);
        match field.key.as_str() {
            "due" => due = date.or(text),
            "owner" => owner = text,
            "est" => est = num,
            _ => {}
        }
    }
    (due, owner, est)
}

/// Keep `index_meta.note_count` in step, in the same transaction as the change.
///
/// The DDL delegates this invariant to this crate, and the cold-start check compares
/// the two: a lag between them is indistinguishable from a torn transaction, so it
/// must never be updated in a later transaction than the rows it counts.
pub(crate) fn sync_note_count(tx: &Transaction<'_>) -> Result<(), IndexError> {
    tx.execute(
        "UPDATE index_meta SET note_count = (SELECT count(*) FROM notes) WHERE id = 1",
        [],
    )
    .map_err(sqlite)?;
    Ok(())
}

/// Mark the index as usable. Only a completed full rebuild may call this.
///
/// Two of the columns written here are not read by anything (F-134), and the
/// difference between them matters:
///
/// * `built_at` is diagnostic. Nothing claims to depend on it and nothing should.
/// * `indexer_version` is **not** diagnostic, and this is the gap. The DDL describes
///   it as "crate version that built it; a bump rebuilds", and no such check exists —
///   §3.4's open check reads `user_version`, `state`, `vault_root` and `note_count`
///   and never this. So a change to what the indexer *extracts*, without a schema
///   change, leaves every stamp step passing over an index whose content is stale:
///   the vault and the schema agree and only the derivation is out of date.
///
/// Raised as a design question rather than fixed here, because `CARGO_PKG_VERSION` is
/// the wrong granularity for it — see the note in `crate::open`.
pub(crate) fn mark_ready(tx: &Transaction<'_>, vault_root: &str) -> Result<(), IndexError> {
    tx.execute(
        "UPDATE index_meta
            SET state = 'ready', vault_root = ?1, built_at = unixepoch(),
                indexer_version = ?2, schema_version = ?3
          WHERE id = 1",
        params![vault_root, env!("CARGO_PKG_VERSION"), crate::INDEX_VERSION],
    )
    .map_err(sqlite)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{bootstrap, open_in_memory};
    use serat_core::{ByteRange, ContentHash, FieldSource, NoteKind};

    /// A note with `block_count` blocks (ord 0 the task's, any others plain
    /// paragraphs), so a field can legitimately attach to a block other than the
    /// task's own.
    fn note_with_blocks(
        block_count: u32,
        fields: Vec<Field>,
        tasks: Vec<Task>,
    ) -> (NoteMeta, ParsedNote) {
        let meta = NoteMeta {
            path: VaultPath::new("note.md").expect("path"),
            hash: ContentHash::from_bytes([1u8; 32]),
            kind: NoteKind::Note,
            size: 0,
            mtime_ms: 0,
        };
        let blocks = (0..block_count)
            .map(|ord| Block {
                ord,
                kind: if ord == 0 {
                    BlockKind::Task { depth: 0 }
                } else {
                    BlockKind::Paragraph
                },
                range: ByteRange::new(0, 20),
                text: if ord == 0 {
                    "- [ ] ship it".to_owned()
                } else {
                    "elsewhere".to_owned()
                },
                heading_path: Vec::new(),
                block_ref: None,
            })
            .collect();
        let parsed = ParsedNote {
            blocks,
            fields,
            tasks,
            ..ParsedNote::default()
        };
        (meta, parsed)
    }

    fn note(fields: Vec<Field>, tasks: Vec<Task>) -> (NoteMeta, ParsedNote) {
        note_with_blocks(1, fields, tasks)
    }

    fn field(key: &str, value: FieldValue, block_ord: u32) -> Field {
        Field {
            key: key.to_owned(),
            value,
            source: FieldSource::Inline,
            range: ByteRange::new(0, 1),
            value_range: ByteRange::new(0, 1),
            block_ord: Some(block_ord),
        }
    }

    fn task(block_ord: u32, marker_range: ByteRange) -> Task {
        Task {
            block_ord,
            done: false,
            marker: ' ',
            text: "ship it".to_owned(),
            range: ByteRange::new(0, 13),
            marker_range,
        }
    }

    fn note_named(path: &str, links: Vec<Link>) -> (NoteMeta, ParsedNote) {
        let meta = NoteMeta {
            path: VaultPath::new(path).expect("path"),
            hash: ContentHash::from_bytes([1u8; 32]),
            kind: NoteKind::Note,
            size: 0,
            mtime_ms: 0,
        };
        let parsed = ParsedNote {
            links,
            ..ParsedNote::default()
        };
        (meta, parsed)
    }

    fn link_to(target: &str) -> Link {
        Link {
            kind: LinkKind::Wikilink,
            target: serat_core::LinkTarget {
                note: target.to_owned(),
                heading: None,
                block_ref: None,
                alias: None,
            },
            range: ByteRange::new(0, 1),
            block_ord: None,
        }
    }

    fn unresolved_link_count(tx: &Transaction<'_>) -> i64 {
        tx.query_row(
            "SELECT count(*) FROM links WHERE dst_note IS NULL AND kind <> 'tag'",
            [],
            |row| row.get(0),
        )
        .expect("count")
    }

    #[test]
    fn a_bulk_load_leaves_the_per_row_resolve_to_the_rebuilds_final_pass() {
        // F-371 / task #77. upsert_note's resolve_inbound/resolve_outbound calls are a
        // per-row cost, and this crate has exactly one caller for whom "per row" means
        // "roughly forty thousand times" (a full rebuild). During a bulk load
        // (bulk_load == true) they must not run at all -- correctness there is the
        // rebuild's own full resolve_links pass at the end, not this function. This
        // test is the guard against silently dropping the `if !bulk_load` gate: with
        // it removed, both links below would resolve immediately and the two
        // assertions after the bulk-load inserts would fail.
        let conn = open_in_memory().expect("open");
        bootstrap(&conn).expect("bootstrap");
        let tx = conn.unchecked_transaction().expect("tx");

        let (meta_a, parsed_a) = note_named("a.md", vec![link_to("b")]);
        let (meta_b, parsed_b) = note_named("b.md", vec![link_to("a")]);

        upsert_note(&tx, &meta_a, &parsed_a, true).expect("upsert a (bulk)");
        upsert_note(&tx, &meta_b, &parsed_b, true).expect("upsert b (bulk)");
        assert_eq!(
            unresolved_link_count(&tx),
            2,
            "a bulk load resolved links per row instead of leaving that to the \
             rebuild's own full pass"
        );

        // Off bulk load, the same shape resolves immediately, as task #13 intended.
        let (meta_c, parsed_c) = note_named("c.md", vec![link_to("d")]);
        let (meta_d, parsed_d) = note_named("d.md", vec![link_to("c")]);
        upsert_note(&tx, &meta_c, &parsed_c, false).expect("upsert c (incremental)");
        upsert_note(&tx, &meta_d, &parsed_d, false).expect("upsert d (incremental)");
        assert_eq!(
            unresolved_link_count(&tx),
            2,
            "expected exactly the two bulk-load links still unresolved; the \
             incremental pair above should have resolved each other"
        );
    }

    #[test]
    fn a_task_records_its_marker_range() {
        // F-237. Task::marker_range exists and is written by nobody until this;
        // tasks had no column to be written to at all.
        let conn = open_in_memory().expect("open");
        bootstrap(&conn).expect("bootstrap");
        let tx = conn.unchecked_transaction().expect("tx");

        let (meta, parsed) = note(Vec::new(), vec![task(0, ByteRange::new(6, 7))]);
        upsert_note(&tx, &meta, &parsed, false).expect("upsert");

        let (start, end): (i64, i64) = tx
            .query_row(
                "SELECT byte_start, byte_end FROM tasks WHERE note_id = (SELECT id FROM notes WHERE path = 'note.md')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("row");
        assert_eq!((start, end), (6, 7));
    }

    #[test]
    fn a_task_rolls_up_due_owner_and_est_from_its_own_block() {
        // F-253. due/owner/est are ordinary inline fields on the task's block,
        // denormalised onto the task row by serat-index rather than parsed as task
        // properties (schema/001_init.sql's comment above CREATE TABLE tasks).
        let conn = open_in_memory().expect("open");
        bootstrap(&conn).expect("bootstrap");
        let tx = conn.unchecked_transaction().expect("tx");

        let fields = vec![
            field("due", FieldValue::Date("2024-01-01".to_owned()), 0),
            field("owner", FieldValue::Text("alex".to_owned()), 0),
            field("est", FieldValue::Num(2.5), 0),
        ];
        let (meta, parsed) = note(fields, vec![task(0, ByteRange::new(6, 7))]);
        upsert_note(&tx, &meta, &parsed, false).expect("upsert");

        let (due, owner, est): (String, String, f64) = tx
            .query_row(
                "SELECT due, owner, est FROM tasks WHERE note_id = (SELECT id FROM notes WHERE path = 'note.md')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("row");
        assert_eq!(due, "2024-01-01");
        assert_eq!(owner, "alex");
        assert!((est - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn a_field_on_a_different_block_does_not_roll_up() {
        // The rollup must key on the task's own block_ord — a due:: on some other
        // block in the note is not this task's due date.
        let conn = open_in_memory().expect("open");
        bootstrap(&conn).expect("bootstrap");
        let tx = conn.unchecked_transaction().expect("tx");

        let fields = vec![field("due", FieldValue::Date("2024-01-01".to_owned()), 1)];
        let (meta, parsed) = note_with_blocks(2, fields, vec![task(0, ByteRange::new(6, 7))]);
        upsert_note(&tx, &meta, &parsed, false).expect("upsert");

        let due: Option<String> = tx
            .query_row(
                "SELECT due FROM tasks WHERE note_id = (SELECT id FROM notes WHERE path = 'note.md')",
                [],
                |row| row.get(0),
            )
            .expect("row");
        assert_eq!(due, None, "a field on a different block rolled up anyway");
    }

    #[test]
    fn a_task_with_no_matching_fields_leaves_due_owner_est_null() {
        let conn = open_in_memory().expect("open");
        bootstrap(&conn).expect("bootstrap");
        let tx = conn.unchecked_transaction().expect("tx");

        let (meta, parsed) = note(Vec::new(), vec![task(0, ByteRange::new(6, 7))]);
        upsert_note(&tx, &meta, &parsed, false).expect("upsert");

        let (due, owner, est): (Option<String>, Option<String>, Option<f64>) = tx
            .query_row(
                "SELECT due, owner, est FROM tasks WHERE note_id = (SELECT id FROM notes WHERE path = 'note.md')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("row");
        assert_eq!((due, owner, est), (None, None, None));
    }
}
