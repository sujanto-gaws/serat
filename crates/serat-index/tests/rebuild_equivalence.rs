//! T1 (task #97): delete `.serat/`, restart, rebuild, assert identical results.
//!
//! FR-VLT-02, NFR-REL-02, US-01, US-07 · ADR-0002 · F-136.
//!
//! # What this closes, and what the existing rebuild tests did not
//!
//! `rebuild.rs`'s `a_rebuild_over_an_existing_index_replaces_it_rather_than_merging`
//! is not T1 and does not become it, for two reasons named in the lead's ruling on
//! task #97 and restated here so the gap is visible in the test that closes it:
//!
//! 1. **Counts are not equivalence.** Two indexes can agree on how many notes,
//!    blocks, fields and links they hold and disagree on every row. This file
//!    compares the *rows a query would return* — see [`snapshot`] for exactly which
//!    columns, and what is deliberately out of scope.
//! 2. **Same-process is not the recovery path.** NFR-REL-02's criterion is a user
//!    restarting the application: delete or truncate the index, start, detect,
//!    rebuild, recover the same vault. Calling `rebuild()` twice in one process
//!    never calls `open_or_rebuild`, never exercises detection, and never opens a
//!    second time. I6's own tests (`open_or_rebuild.rs`) prove detection and
//!    ordering against `FakeVault` — an in-memory `Vec`, not a file — so what a
//!    *real* 124 MB-class index on a real disk does after being deleted and
//!    rebuilt was, until this file, unexercised (F-136).
//!
//! So every test here writes real files to a real `tempfile::tempdir()`, builds the
//! index into a second real directory via [`open_or_rebuild`] exactly as
//! `serat-app` would on cold start, closes the handle (dropping the writer thread
//! and every open connection), deletes the index directory from disk, and calls
//! `open_or_rebuild` again as a second, independent open — the shape §3.4 and
//! NFR-REL-02 actually describe.

mod support;

use std::fs;
use std::path::Path;

use rusqlite::types::Value;

use serat_index::{RebuildReason, index_path, open_or_rebuild, schema};
use support::{DiskVault, LineParser};

/// The rows a query over this index would return, independent of any internal
/// integer id.
///
/// # Scope, stated because F-136's whole point was an unstated one
///
/// This snapshots every table FR-VLT-02 puts in scope for P0 — `notes`, `blocks`,
/// `fields`, `links`, `tasks` — plus one `blocks_fts` query, projected through the
/// columns a real caller reads (architecture §4, schema/001_init.sql) and joined
/// back onto the note **path** rather than the note **id**. That substitution is
/// deliberate and load-bearing: a second, independent rebuild has no reason to
/// assign the same `notes.id` to the same path (nothing promises it, and nothing
/// should have to), so comparing raw ids would fail for a reason that has nothing
/// to do with FR-VLT-02 — the two builds would disagree on numbering while
/// answering every real question identically. Every row is therefore keyed by path
/// wherever the schema would otherwise expose an id, and both snapshots are sorted
/// so row order cannot hide a difference or manufacture one.
///
/// What this does **not** prove: it is not a byte-identical database comparison
/// (`sqlite_stat1`, rowids, page layout and vacuum state are free to differ, and
/// do), and it is not exhaustive over every column combination a real SQ query
/// could project — `word_count`, `built_at` and `indexer_version` are diagnostic
/// (`apply::mark_ready`'s doc comment) and are not asserted here for the same
/// reason the open check does not read them. It proves what NFR-REL-02 is actually
/// about: a vault rebuilt from scratch answers the same questions with the same
/// answers.
struct Snapshot {
    notes: Vec<String>,
    blocks: Vec<String>,
    fields: Vec<String>,
    links: Vec<String>,
    tasks: Vec<String>,
    fts_hits: Vec<String>,
}

impl Snapshot {
    fn take(index_path: &Path) -> Self {
        let conn = schema::open(index_path).expect("open index for snapshot");
        Self {
            notes: rows(
                &conn,
                "SELECT path, file_name, title, type, kind, hash, size, mtime
                   FROM notes
                  ORDER BY path",
            ),
            blocks: rows(
                &conn,
                "SELECT n.path, b.ord, b.kind, b.text, b.heading_path,
                        b.byte_start, b.byte_end
                   FROM blocks b JOIN notes n ON n.id = b.note_id
                  ORDER BY n.path, b.ord",
            ),
            fields: rows(
                &conn,
                "SELECT n.path, f.key, f.val_text, f.val_num, f.val_date, f.val_bool,
                        f.source, f.byte_start, f.byte_end
                   FROM fields f JOIN notes n ON n.id = f.note_id
                  ORDER BY n.path, f.key, f.byte_start",
            ),
            // dst_note is deliberately re-expressed as the destination's PATH
            // (via the LEFT JOIN), not compared as an id -- see the struct doc.
            // An unresolved link's destination path stays NULL both times, which
            // is the row this table exists to get right (schema's CHECK on kind
            // <> 'tag').
            links: rows(
                &conn,
                "SELECT src.path, l.kind, l.dst_path, dst.path, l.alias, l.field_key
                   FROM links l
                   JOIN notes src ON src.id = l.src_note
                   LEFT JOIN notes dst ON dst.id = l.dst_note
                  ORDER BY src.path, l.kind, l.dst_path, l.alias",
            ),
            tasks: rows(
                &conn,
                "SELECT n.path, t.status, t.done, t.text, t.due, t.owner, t.est,
                        t.byte_start, t.byte_end
                   FROM tasks t JOIN notes n ON n.id = t.note_id
                  ORDER BY n.path, t.byte_start",
            ),
            // The search index is a query path too (FR-NAV-02), and the FTS
            // trigger dance around a bulk load (schema/001_init.sql's sequence,
            // F-32/F-38) is exactly the kind of thing that could rebuild fine on
            // notes/blocks/fields/links/tasks and still leave blocks_fts stale or
            // empty. A hit list keyed by (note path, block ord) is exactly what a
            // caller of search sees.
            fts_hits: rows(
                &conn,
                "SELECT n.path, b.ord
                   FROM blocks_fts
                   JOIN blocks b ON b.id = blocks_fts.rowid
                   JOIN notes n ON n.id = b.note_id
                  WHERE blocks_fts MATCH 'ship OR doc OR Indexing OR Loose'
                  ORDER BY n.path, b.ord",
            ),
        }
    }
}

/// Every row `sql` returns, each rendered as one `|`-joined string so a mismatch
/// prints as a readable diff rather than an opaque struct.
fn rows(conn: &rusqlite::Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let columns = stmt.column_count();
    stmt.query_map([], |row| {
        let mut parts = Vec::with_capacity(columns);
        for i in 0..columns {
            let value: Value = row.get(i)?;
            parts.push(format!("{value:?}"));
        }
        Ok(parts.join("|"))
    })
    .expect("query")
    .collect::<Result<Vec<_>, _>>()
    .expect("collect rows")
}

/// Real files, written to a real directory, with enough shape to exercise every
/// table `Snapshot` reads: two mutually-linked notes, one note with a link to a
/// note that does not exist (an unresolved link must stay unresolved identically
/// on both builds), and one asset.
fn seed(vault_root: &Path) {
    write(
        vault_root,
        "10-projects/serat.md",
        "# Serat\n\
         type:: project\n\
         status:: active\n\
         [[20-systems/indexing]]\n\
         - [ ] ship P0\n",
    );
    write(
        vault_root,
        "20-systems/indexing.md",
        "# Indexing\n\
         type:: system\n\
         [[10-projects/serat]]\n\
         - [ ] write the doc\n",
    );
    write(
        vault_root,
        "30-notes/loose.md",
        "# Loose\n\
         [[20-systems/does-not-exist]]\n",
    );
    write(vault_root, "40-refs/contract.pdf", "%PDF-1.7\n");
}

fn write(vault_root: &Path, relative: &str, body: &str) {
    let full = vault_root.join(relative);
    fs::create_dir_all(full.parent().expect("has a parent")).expect("mkdir");
    fs::write(full, body).expect("write");
}

/// One "application start": open (or rebuild) the index at `index_dir` for the
/// vault at `vault_root`, exactly as `serat-app`'s cold-start path would, and hand
/// back the reason it rebuilt (if it did) so a test can assert on *why* as well as
/// on the result.
fn start(vault_root: &Path, index_dir: &Path) -> Option<RebuildReason> {
    let vault = DiskVault::new(vault_root);
    let parser = LineParser::default();
    let reason = std::cell::Cell::new(None);
    let (handle, _report) = open_or_rebuild(vault_root, index_dir, &vault, &parser, &|status| {
        if let serat_index::IndexStatus::Rebuilding { reason: why } = status {
            reason.set(Some(why));
        }
    })
    .expect("open_or_rebuild");
    // Closing is the other half of "a real restart": it stops the writer thread and
    // drops every connection this process holds, so the next `start()` opens the
    // file cold rather than reusing a live handle -- the distinction task #97's
    // ruling draws between a same-process rebuild and an actual recovery path.
    handle.close().expect("close");
    reason.get()
}

#[test]
fn a_vault_recovered_after_deleting_serat_answers_every_query_the_same_way() {
    let vault_dir = tempfile::tempdir().expect("vault tempdir");
    let index_dir = tempfile::tempdir().expect("index tempdir");
    seed(vault_dir.path());

    // First start: no `.serat/` exists yet.
    let first_reason = start(vault_dir.path(), index_dir.path());
    assert_eq!(
        first_reason,
        Some(RebuildReason::NoIndex),
        "the first start on a vault with no index did not rebuild for NoIndex"
    );
    let before = Snapshot::take(&index_path(index_dir.path()));

    // The user deletes .serat/ -- accidentally, or because it looked corrupt, or
    // because they are following the documented recovery instruction. Whichever:
    // FR-VLT-02 and NFR-REL-02 both say the index is disposable and nothing
    // depends on it surviving (C-02).
    fs::remove_dir_all(index_dir.path()).expect("delete .serat/");

    // Second start: this is the recovery path. It must detect NoIndex again (the
    // directory is gone, not merely stale) and rebuild without being told to.
    let second_reason = start(vault_dir.path(), index_dir.path());
    assert_eq!(
        second_reason,
        Some(RebuildReason::NoIndex),
        "restarting after deleting .serat/ did not detect and rebuild"
    );
    let after = Snapshot::take(&index_path(index_dir.path()));

    assert_eq!(before.notes, after.notes, "notes differ after recovery");
    assert_eq!(before.blocks, after.blocks, "blocks differ after recovery");
    assert_eq!(before.fields, after.fields, "fields differ after recovery");
    assert_eq!(before.links, after.links, "links differ after recovery");
    assert_eq!(before.tasks, after.tasks, "tasks differ after recovery");
    assert_eq!(
        before.fts_hits, after.fts_hits,
        "full-text search results differ after recovery"
    );

    // Sanity check on the fixture itself: an equivalence test over two empty
    // snapshots proves nothing (the shape of failure this whole file exists to
    // rule out one level up). This is the check that the comparison above was
    // actually exercised.
    //
    // **All six series, not three (F-474).** The comment above was already right
    // and the code covered half of it: hollowing each series in turn so both
    // snapshots return empty left `tasks`, `fields` and `blocks` GREEN while
    // `notes` went red. Latent rather than live, since `seed()` populates all six
    // today -- but the reason it has to close is a limit of equivalence testing in
    // general. **A defect that affects both builds identically is invisible to
    // it.** If the parser stopped emitting tasks, both snapshots would be empty,
    // every `assert_eq!` above would still pass, and T1 would report equivalence
    // over a table nothing was populating -- while T1 is the evidence C-02 and
    // NFR-REL-02 rest on. These assertions are what make it the stronger claim, so
    // every series the comparison covers is held to non-emptiness and none is left
    // to be vacuously equal.
    assert!(!before.notes.is_empty(), "the fixture indexed nothing");
    assert!(!before.blocks.is_empty(), "the fixture produced no blocks");
    assert!(!before.fields.is_empty(), "the fixture produced no fields");
    assert!(!before.links.is_empty(), "the fixture produced no links");
    assert!(!before.tasks.is_empty(), "the fixture produced no tasks");
    assert!(
        !before.fts_hits.is_empty(),
        "the fixture produced no FTS hits"
    );
}

#[test]
fn a_truncated_index_recovers_the_same_vault_on_the_next_start() {
    // The other half of NFR-REL-02's acceptance criterion (also exercised against
    // FakeVault in open_or_rebuild.rs's `a_truncated_index_is_detected_and_rebuilt`):
    // corruption, not just absence, and on a real file this time. One page, not a
    // percentage -- SQLite derives the page count from the file length by integer
    // division, so a cut smaller than a page is invisible to the stamp (F-43).
    let vault_dir = tempfile::tempdir().expect("vault tempdir");
    let index_dir = tempfile::tempdir().expect("index tempdir");
    seed(vault_dir.path());

    start(vault_dir.path(), index_dir.path());
    let before = Snapshot::take(&index_path(index_dir.path()));

    let path = index_path(index_dir.path());
    let size = fs::metadata(&path).expect("stat").len();
    assert!(
        size > 8_192,
        "the fixture index is too small to truncate a page from"
    );
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open for truncation");
    file.set_len(size - 4_096).expect("truncate one page");
    drop(file);

    let reason = start(vault_dir.path(), index_dir.path());
    assert_eq!(
        reason,
        Some(RebuildReason::Corrupt),
        "a truncated index on real disk was not detected as corrupt"
    );
    let after = Snapshot::take(&index_path(index_dir.path()));

    assert_eq!(before.notes, after.notes);
    assert_eq!(before.blocks, after.blocks);
    assert_eq!(before.fields, after.fields);
    assert_eq!(before.links, after.links);
    assert_eq!(before.tasks, after.tasks);
    assert_eq!(before.fts_hits, after.fts_hits);
}

// ----------------------------------------------------------------------------
// The comparison is exercised against a real disagreement, not only a real match
// ----------------------------------------------------------------------------
//
// "Make it fail for the right reason before making it pass" (task #97's ruling;
// six earlier instances of this project's own recorded failure mode -- F-13,
// F-28, F-35, F-50, F-58 -- are an assertion that was correct while the setup
// never reached the path under test). Both tests above were run, by hand, against
// two deliberate sabotages before this file was considered done, and each turned
// the corresponding assertion red for the expected reason rather than some other
// one:
//
//   1. One row deleted from the second build's `blocks` table (a direct
//      `conn.execute("DELETE FROM blocks WHERE id = ...")` between the second
//      `start()` and its `Snapshot::take`) failed
//      `a_vault_recovered_after_deleting_serat_answers_every_query_the_same_way`
//      on `before.blocks == after.blocks`, printing a one-row-shorter Vec on the
//      `after` side -- the diff named the missing row, not a generic panic.
//   2. One field's value corrupted in place (`UPDATE fields SET val_text = 'x'
//      WHERE key = 'type'`) on the second build failed the same test on
//      `before.fields == after.fields`, and left `before.notes == after.notes`
//      passing -- proving the per-table assertions are independent rather than
//      one blob comparison that would have hidden which table actually disagreed.
//
// Both were reverted before committing; neither sabotage is checked in, because a
// test that must be run with a hand-edited line to prove itself is not a guard
// the suite carries forward -- the record of having done it is what this comment
// is for, matching this crate's own convention (rebuild.rs,
// `the_statistics_left_behind_describe_the_finished_index`).
