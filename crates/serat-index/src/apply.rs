//! Turning an [`IndexOp`] into rows.
//!
//! Every function here runs inside a transaction owned by the writer thread. None of
//! them opens a connection, and none is public: the only way to change the index is
//! to send a message to the one writer (ADR-0001).

use rusqlite::{Transaction, params};
use serat_core::{
    Block, BlockKind, Field, FieldValue, IndexError, IndexOp, Link, LinkKind, NoteMeta, ParsedNote,
    Task, VaultPath,
};

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
pub(crate) fn apply(tx: &Transaction<'_>, op: &IndexOp) -> Result<Applied, IndexError> {
    match op {
        IndexOp::UpsertNote { meta, parsed } => {
            upsert_note(tx, meta, parsed)?;
            Ok(Applied {
                notes: 1,
                removed: 0,
            })
        }
        IndexOp::UpsertAsset { meta } => {
            upsert_asset(tx, meta)?;
            Ok(Applied {
                notes: 1,
                removed: 0,
            })
        }
        IndexOp::Remove { path } => {
            let gone = remove(tx, path)?;
            Ok(Applied {
                notes: 0,
                removed: usize::from(gone),
            })
        }
        IndexOp::Rename { from, to } => {
            rename(tx, from, to)?;
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
fn upsert_note(
    tx: &Transaction<'_>,
    meta: &NoteMeta,
    parsed: &ParsedNote,
) -> Result<(), IndexError> {
    remove(tx, &meta.path)?;

    let word_count: i64 = parsed
        .blocks
        .iter()
        .map(|block| block.text.split_whitespace().count() as i64)
        .sum();

    tx.execute(
        "INSERT INTO notes (path, title, type, kind, mtime, hash, word_count)
         VALUES (?1, ?2, ?3, 'note', ?4, ?5, ?6)",
        params![
            meta.path.as_str(),
            parsed.title.as_deref(),
            parsed.note_type.as_deref(),
            meta.mtime_ms,
            meta.hash.to_hex(),
            word_count,
        ],
    )
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
            insert_task(tx, note_id, block_id, task)?;
        }
    }

    Ok(())
}

/// A file that is not Markdown: recorded, not parsed.
///
/// Design goal 5. Until an extractor claims it (P2, architecture §7) it has a row, a
/// path and a hash, so it is linkable and queryable even though nothing can search
/// inside it.
fn upsert_asset(tx: &Transaction<'_>, meta: &NoteMeta) -> Result<(), IndexError> {
    remove(tx, &meta.path)?;
    tx.execute(
        "INSERT INTO notes (path, kind, asset_path, mtime, hash)
         VALUES (?1, 'asset', ?1, ?2, ?3)",
        params![meta.path.as_str(), meta.mtime_ms, meta.hash.to_hex()],
    )
    .map_err(sqlite)?;
    Ok(())
}

/// Delete a note and everything hanging off it. Returns whether a row was there.
fn remove(tx: &Transaction<'_>, path: &VaultPath) -> Result<bool, IndexError> {
    let removed = tx
        .execute("DELETE FROM notes WHERE path = ?1", params![path.as_str()])
        .map_err(sqlite)?;
    Ok(removed > 0)
}

/// Move a note without disturbing its inbound links.
///
/// Distinct from remove-then-insert because deleting the row would set every inbound
/// `links.dst_note` to NULL, and the backlinks would have to be rediscovered. A
/// rename changes where a file is, not what points at it.
fn rename(tx: &Transaction<'_>, from: &VaultPath, to: &VaultPath) -> Result<(), IndexError> {
    tx.execute(
        "UPDATE notes SET path = ?2 WHERE path = ?1",
        params![from.as_str(), to.as_str()],
    )
    .map_err(sqlite)?;
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

    tx.execute(
        "INSERT INTO blocks (note_id, ord, kind, block_ref, heading_path, text, byte_start, byte_end)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
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
        tx.execute(
            "INSERT INTO fields
                 (note_id, block_id, key, val_text, val_num, val_date, val_bool, source,
                  byte_start, byte_end)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
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
            ],
        )
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
    };
    tx.execute(
        "INSERT INTO links (src_note, src_block, dst_path, dst_block_ref, kind, alias)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            note_id,
            block_id,
            link.target.note,
            link.target.block_ref.as_deref(),
            kind,
            link.target.alias.as_deref(),
        ],
    )
    .map_err(sqlite)?;
    Ok(())
}

/// Insert a task.
///
/// `status` keeps the author's marker so a vault using `/` for in-progress survives a
/// round trip (C-01); `done` is the binary projection `ix_tasks_open` is partial on.
/// The mapping between them is the parser's, not SQL's.
fn insert_task(
    tx: &Transaction<'_>,
    note_id: i64,
    block_id: i64,
    task: &Task,
) -> Result<(), IndexError> {
    tx.execute(
        "INSERT INTO tasks (note_id, block_id, status, done, text)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            note_id,
            block_id,
            task.marker.to_string(),
            i64::from(task.done),
            task.text,
        ],
    )
    .map_err(sqlite)?;
    Ok(())
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
