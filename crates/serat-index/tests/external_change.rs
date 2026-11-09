//! External-change indexing and the unclean-shutdown rule (task #14; FR-VLT-04,
//! NFR-REL-01, NFR-REL-02; ADR-0001, ADR-0011, ADR-0015 section 2).
//!
//! Real files on a real disk through `DiskVault` (`std::fs` only), so mtimes are real
//! and the tests can move them.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

use serat_core::{IndexWriter as _, NoteSource, SourceEntry, VaultPath};
use serat_index::external::{CatchUp, ChangeSignal, Outcome, Reconciler};
use serat_index::{IndexHandle, rebuild, schema};
use support::{DiskVault, LineParser, SpyEcho};

fn path(text: &str) -> VaultPath {
    VaultPath::new(text).expect("path")
}

fn write(root: &Path, relative: &str, body: &str) {
    let target = root.join(relative);
    fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
    fs::write(target, body).expect("write");
}

/// Move a file's mtime by `seconds`, so a rewrite within one filesystem tick still
/// differs from what the index recorded.
fn nudge(root: &Path, relative: &str, seconds: u64) {
    let file = fs::File::options()
        .write(true)
        .open(root.join(relative))
        .expect("open");
    file.set_modified(SystemTime::now() + Duration::from_secs(seconds))
        .expect("set mtime");
}

fn now() -> i64 {
    serat_index::rebuild::now_ms()
}

/// Put a file's mtime an hour in the past: outside any catch-up window.
fn backdate(root: &Path, relative: &str) {
    let file = fs::File::options()
        .write(true)
        .open(root.join(relative))
        .expect("open");
    file.set_modified(SystemTime::now() - Duration::from_secs(3600))
        .expect("set mtime");
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    index: PathBuf,
    parser: LineParser,
}

/// A vault with three notes, rebuilt into an index, which is not open.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path().join("vault");
    let index = dir.path().join("index");
    write(&root, "a.md", "# A\nalpha\n");
    write(&root, "b.md", "# B\nbeta\n");
    write(&root, "folder/c.md", "# C\ngamma\n");
    for name in ["a.md", "b.md", "folder/c.md"] {
        backdate(&root, name);
    }
    let parser = LineParser::default();
    rebuild(&DiskVault::new(&root), &parser, &root, &index).expect("rebuild");
    Fixture {
        _dir: dir,
        root,
        index,
        parser,
    }
}

fn hash_of(index: &Path, relative: &str) -> Option<String> {
    let conn = schema::open(&serat_index::index_path(index)).expect("open");
    conn.query_row(
        "SELECT hash FROM notes WHERE path = ?1",
        [relative],
        |row| row.get(0),
    )
    .ok()
}

fn paths_in(index: &Path) -> Vec<String> {
    let conn = schema::open(&serat_index::index_path(index)).expect("open");
    let mut statement = conn
        .prepare("SELECT path FROM notes ORDER BY path")
        .expect("prepare");
    statement
        .query_map([], |row| row.get(0))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
}

/// A source whose `load` returns the new bytes with the mtime taken before the write.
struct StatBeforeTheWrite(DiskVault, i64);

impl serat_core::NoteSource for StatBeforeTheWrite {
    fn entries(&self) -> Result<Vec<SourceEntry>, serat_core::VaultError> {
        self.0.entries()
    }

    fn load(&self, path: &VaultPath) -> Result<serat_core::LoadedNote, serat_core::VaultError> {
        let mut loaded = self.0.load(path)?;
        loaded.meta.mtime_ms = self.1;
        Ok(loaded)
    }
}

fn never_echo_guard() -> SpyEcho {
    SpyEcho::default()
}

fn mtime_of(index: &Path, relative: &str) -> i64 {
    let conn = schema::open(&serat_index::index_path(index)).expect("open");
    conn.query_row(
        "SELECT mtime FROM notes WHERE path = ?1",
        [relative],
        |row| row.get(0),
    )
    .expect("row")
}

fn clean_close(index: &Path) -> i64 {
    let conn = schema::open(&serat_index::index_path(index)).expect("open");
    conn.query_row(
        "SELECT clean_close FROM index_meta WHERE id = 1",
        [],
        |row| row.get(0),
    )
    .expect("marker")
}

fn never_echo(_: &VaultPath, _: serat_core::ContentHash) -> bool {
    false
}

fn walk(root: &Path) -> Vec<SourceEntry> {
    DiskVault::new(root).entries().expect("walk")
}

// --- watcher path ---------------------------------------------------------------

#[test]
fn an_edit_a_create_and_a_delete_reach_the_index_in_one_batch() {
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let reconciler = Reconciler::new(&handle);
    let source = DiskVault::new(&f.root);

    write(&f.root, "a.md", "# A\nalpha, edited\n");
    write(&f.root, "new.md", "# New\n");
    fs::remove_file(f.root.join("b.md")).expect("rm");

    let outcome = reconciler
        .index_paths(
            &source,
            &f.parser,
            &never_echo,
            &[path("a.md"), path("new.md"), path("b.md")],
        )
        .expect("index");
    assert_eq!(
        outcome,
        Outcome {
            upserted: 2,
            removed: 1,
            unchanged: 0,
            touched: 0,
            unparsed: 0,
            unreadable: 0
        }
    );
    assert!(outcome.changed());
    // The reconciler flushed: a reader sees it with no further wait.
    assert_eq!(paths_in(&f.index), ["a.md", "folder/c.md", "new.md"]);
    handle.close().expect("close");
}

#[test]
fn a_touched_file_whose_bytes_did_not_change_is_not_parsed() {
    // The hash gate (architecture 3.1.2 step 3). Sync tools move mtimes constantly.
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let reconciler = Reconciler::new(&handle);
    nudge(&f.root, "a.md", 60);
    let parsed_before = f.parser.calls.load(Ordering::Relaxed);

    let outcome = reconciler
        .index_paths(
            &DiskVault::new(&f.root),
            &f.parser,
            &never_echo,
            &[path("a.md")],
        )
        .expect("index");

    assert_eq!(outcome.unchanged, 1);
    assert!(!outcome.changed());
    assert_eq!(f.parser.calls.load(Ordering::Relaxed), parsed_before);
    handle.close().expect("close");
}

#[test]
fn a_file_the_save_path_already_submitted_is_skipped_as_an_echo() {
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let reconciler = Reconciler::new(&handle);
    write(&f.root, "a.md", "# A\nsaved in serat\n");
    let echoed = std::cell::Cell::new(0);

    let outcome = reconciler
        .index_paths(
            &DiskVault::new(&f.root),
            &f.parser,
            &|_, _| {
                echoed.set(echoed.get() + 1);
                true
            },
            &[path("a.md")],
        )
        .expect("index");

    assert_eq!(
        (echoed.get(), outcome.unchanged, outcome.upserted),
        (1, 1, 0)
    );
    handle.close().expect("close");
}

#[test]
fn a_deleted_directory_takes_what_the_index_held_beneath_it() {
    // A directory that is gone arrives as the path of the directory (the watcher
    // cannot tell it from a file), and the index knows it only by its children.
    let f = fixture();
    write(&f.root, "folder/deep/d.md", "# D\n");
    let handle = IndexHandle::open(&f.index).expect("open");
    let reconciler = Reconciler::new(&handle);
    reconciler
        .catch_up(
            &DiskVault::new(&f.root),
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsAndMtime,
            Some(now()),
        )
        .expect("catch up");
    assert!(paths_in(&f.index).contains(&"folder/deep/d.md".to_owned()));

    fs::remove_dir_all(f.root.join("folder")).expect("rm -r");
    let outcome = reconciler
        .index_paths(
            &DiskVault::new(&f.root),
            &f.parser,
            &never_echo,
            &[path("folder")],
        )
        .expect("index");

    assert_eq!(outcome.removed, 2);
    assert_eq!(paths_in(&f.index), ["a.md", "b.md"]);
    // Nothing of theirs survives in the child tables either.
    let conn = schema::open(&serat_index::index_path(&f.index)).expect("open");
    for table in ["blocks", "fields", "links", "tasks"] {
        let orphans: i64 = conn
            .query_row(
                &format!(
                    "SELECT count(*) FROM {table} WHERE note_id NOT IN (SELECT id FROM notes)"
                ),
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        assert_eq!(orphans, 0, "{table} kept rows of a removed note");
    }
    drop(conn);
    assert_eq!(blocks_of(&f.index, "folder/c.md"), 0);
    handle.close().expect("close");
}

#[test]
fn a_renamed_folder_is_reconciled_by_walking_that_folder_alone() {
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let reconciler = Reconciler::new(&handle);
    let source = DiskVault::new(&f.root);

    fs::rename(f.root.join("folder"), f.root.join("renamed")).expect("rename");
    // The old name first (a path event), then the new name (a directory event).
    reconciler
        .index_paths(&source, &f.parser, &never_echo, &[path("folder")])
        .expect("old name");
    let under: Vec<SourceEntry> = walk(&f.root)
        .into_iter()
        .filter(|entry| entry.path.as_str().starts_with("renamed/"))
        .collect();
    let outcome = reconciler
        .reconcile_dir(&source, &f.parser, &path("renamed"), &under)
        .expect("reconcile");

    assert_eq!(outcome.upserted, 1);
    assert_eq!(paths_in(&f.index), ["a.md", "b.md", "renamed/c.md"]);
    handle.close().expect("close");
}

#[test]
fn reconciling_a_directory_does_not_touch_a_sibling_with_a_similar_name() {
    // `folder/` and `folder-2/` share a prefix; the range is `folder/`..`folder0`.
    let f = fixture();
    write(&f.root, "folder-2/x.md", "# X\n");
    let handle = IndexHandle::open(&f.index).expect("open");
    let reconciler = Reconciler::new(&handle);
    let source = DiskVault::new(&f.root);
    reconciler
        .catch_up(
            &source,
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsAndMtime,
            Some(now()),
        )
        .expect("catch up");

    // folder/ is now empty on disk; folder-2/x.md must survive reconciling it.
    fs::remove_file(f.root.join("folder/c.md")).expect("rm");
    let outcome = reconciler
        .reconcile_dir(&source, &f.parser, &path("folder"), &[])
        .expect("reconcile");
    assert_eq!(outcome.removed, 1);
    assert!(paths_in(&f.index).contains(&"folder-2/x.md".to_owned()));
    handle.close().expect("close");
}

// --- catch-up -------------------------------------------------------------------

#[test]
fn catch_up_submits_what_the_index_lacks_and_removes_what_the_disk_lacks() {
    // ADR-0015 section 2, both directions.
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let reconciler = Reconciler::new(&handle);
    write(&f.root, "while-closed.md", "# New\n");
    fs::remove_file(f.root.join("b.md")).expect("rm");

    let outcome = reconciler
        .catch_up(
            &DiskVault::new(&f.root),
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsAndMtime,
            Some(now()),
        )
        .expect("catch up");

    assert_eq!((outcome.upserted, outcome.removed), (1, 1));
    assert_eq!(
        paths_in(&f.index),
        ["a.md", "folder/c.md", "while-closed.md"]
    );
    handle.close().expect("close");
}

#[test]
fn a_clean_close_followed_by_an_external_edit_is_reindexed_at_the_next_open() {
    // ADR-0015 section 2 as amended: the catch-up always compares mtimes, so a file
    // edited while Serat was closed is found even though the close was clean.
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    handle.close().expect("close");
    let before = hash_of(&f.index, "a.md").expect("row");

    write(&f.root, "a.md", "# A\nedited behind our back\n");
    nudge(&f.root, "a.md", 120);

    let reopened = IndexHandle::open(&f.index).expect("reopen");
    assert!(!reopened.opened_after_unclean_shutdown());
    let outcome = Reconciler::new(&reopened)
        .catch_up(
            &DiskVault::new(&f.root),
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsAndMtime,
            Some(now()),
        )
        .expect("catch up");
    assert_eq!(outcome.upserted, 1);
    assert_ne!(hash_of(&f.index, "a.md").expect("row"), before);
    reopened.close().expect("close");
}

#[test]
fn unchanged_content_with_a_moved_mtime_is_not_parsed_and_its_mtime_is_refreshed() {
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let reconciler = Reconciler::new(&handle);
    let source = DiskVault::new(&f.root);
    backdate_by(&f.root, "a.md", 1800); // touched, same bytes, an old mtime outside any window
    let moved = walk(&f.root)
        .into_iter()
        .find(|entry| entry.path.as_str() == "a.md")
        .expect("entry")
        .mtime_ms;
    assert_ne!(mtime_of(&f.index, "a.md"), moved);
    let parsed_before = f.parser.calls.load(Ordering::Relaxed);

    let outcome = reconciler
        .catch_up(
            &source,
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsAndMtime,
            Some(now()),
        )
        .expect("catch up");

    assert_eq!(
        (outcome.upserted, outcome.unchanged, outcome.touched),
        (0, 1, 1)
    );
    assert_eq!(f.parser.calls.load(Ordering::Relaxed), parsed_before);
    assert_eq!(mtime_of(&f.index, "a.md"), moved, "row mtime not refreshed");

    // The next comparison does not read the file again.
    let again = reconciler
        .catch_up(
            &source,
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsAndMtime,
            Some(now()),
        )
        .expect("catch up");
    assert_eq!(again, Outcome::default());
    handle.close().expect("close");
}

#[test]
fn a_touch_applies_only_to_a_row_still_holding_the_verified_hash() {
    // Row (H0, t0) landed first; a touch verified against H1 must not move it to t1.
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let t0 = mtime_of(&f.index, "a.md");
    let other = serat_core::ContentHash::from_bytes([9u8; 32]);
    handle
        .writer()
        .submit_blocking(serat_core::IndexOp::TouchMtime {
            path: path("a.md"),
            hash: other,
            mtime_ms: t0 + 5_000,
        })
        .expect("touch");
    handle.writer().flush().expect("flush");
    assert_eq!(
        mtime_of(&f.index, "a.md"),
        t0,
        "touch ignored the hash guard"
    );
    handle.close().expect("close");
}

#[test]
fn touched_mtimes_leave_the_index_equal_to_a_fresh_rebuild() {
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    for name in ["a.md", "b.md", "folder/c.md"] {
        nudge(&f.root, name, 200);
    }
    let outcome = Reconciler::new(&handle)
        .catch_up(
            &DiskVault::new(&f.root),
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsAndMtime,
            Some(now()),
        )
        .expect("catch up");
    assert_eq!((outcome.upserted, outcome.touched), (0, 3));
    handle.close().expect("close");

    let fresh = f._dir.path().join("fresh");
    rebuild(&DiskVault::new(&f.root), &f.parser, &f.root, &fresh).expect("rebuild");
    let snapshot = |index: &Path| -> Vec<(String, String, i64, i64)> {
        let conn = schema::open(&serat_index::index_path(index)).expect("open");
        let mut statement = conn
            .prepare("SELECT path, hash, mtime, size FROM notes ORDER BY path")
            .expect("prepare");
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    };
    assert_eq!(snapshot(&f.index), snapshot(&fresh));
}

#[test]
fn a_row_holding_an_old_mtime_beside_the_new_hash_is_caught_by_the_next_catch_up() {
    // The comparison half of the stat/read race (the read half is a serat-vault test,
    // `a_write_between_the_stat_and_the_read...`). Vault::read stats the open handle before reading it. A write that lands after
    // the stat is then recorded as (old mtime, new hash), and the old mtime differs
    // from the file's new one, so the next comparison re-reads. The other order would
    // record (new mtime, old hash) and every later comparison would pass.
    let f = fixture();
    let old = walk(&f.root)
        .into_iter()
        .find(|entry| entry.path.as_str() == "a.md")
        .expect("entry")
        .mtime_ms;
    // The state Vault::read leaves when the write lands after its stat: the row holds
    // the new bytes' hash beside the pre-write mtime.
    let handle = IndexHandle::open(&f.index).expect("open");
    let reconciler = Reconciler::new(&handle);
    write(&f.root, "a.md", "# A\nwritten between stat and read\n");
    nudge(&f.root, "a.md", 60);
    let source = DiskVault::new(&f.root);
    handle
        .apply(
            &StatBeforeTheWrite(DiskVault::new(&f.root), old),
            &f.parser,
            &never_echo_guard(),
            &path("a.md"),
        )
        .expect("apply");
    handle.writer().flush().expect("flush");
    assert_eq!(mtime_of(&f.index, "a.md"), old);

    let parsed_before = f.parser.calls.load(Ordering::Relaxed);
    let outcome = reconciler
        .catch_up(
            &source,
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsAndMtime,
            Some(now()),
        )
        .expect("catch up");
    // Same content, so no parse; the row's mtime is brought up to the file's.
    assert_eq!((outcome.upserted, outcome.touched), (0, 1));
    assert_eq!(f.parser.calls.load(Ordering::Relaxed), parsed_before);
    assert_ne!(mtime_of(&f.index, "a.md"), old);
    handle.close().expect("close");
}

#[test]
fn only_files_whose_mtime_moved_are_hashed_and_only_changed_content_is_parsed() {
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let reconciler = Reconciler::new(&handle);
    nudge(&f.root, "a.md", 300); // touched, same bytes
    write(&f.root, "b.md", "# B\nreally changed\n");
    nudge(&f.root, "b.md", 300);
    let parsed_before = f.parser.calls.load(Ordering::Relaxed);

    let outcome = reconciler
        .catch_up(
            &DiskVault::new(&f.root),
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsAndMtime,
            Some(now()),
        )
        .expect("catch up");

    assert_eq!(
        (outcome.upserted, outcome.unchanged, outcome.touched),
        (1, 1, 1)
    );
    assert_eq!(f.parser.calls.load(Ordering::Relaxed), parsed_before + 1);
    handle.close().expect("close");
}

// --- the clean-close marker -----------------------------------------------------

#[test]
fn the_marker_is_cleared_at_open_and_set_at_close() {
    let f = fixture();
    assert_eq!(clean_close(&f.index), 1, "a finished rebuild is clean");

    let handle = IndexHandle::open(&f.index).expect("open");
    assert!(!handle.opened_after_unclean_shutdown());
    assert_eq!(
        clean_close(&f.index),
        0,
        "not cleared while a session holds it"
    );

    handle.close().expect("close");
    assert_eq!(clean_close(&f.index), 1, "not set by a clean close");

    let reopened = IndexHandle::open(&f.index).expect("reopen");
    assert!(!reopened.opened_after_unclean_shutdown());
    // Dropped, not closed: the marker stays cleared.
    drop(reopened);
    assert_eq!(clean_close(&f.index), 0);
    let after_drop = IndexHandle::open(&f.index).expect("reopen");
    assert!(
        after_drop.opened_after_unclean_shutdown(),
        "a handle that was dropped without close() was reported clean"
    );
    after_drop.close().expect("close");
}

#[test]
fn a_reader_pinning_the_wal_makes_the_close_unclean_rather_than_hanging() {
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    handle
        .apply(
            &DiskVault::new(&f.root),
            &f.parser,
            &SpyEcho::default(),
            &path("a.md"),
        )
        .expect("apply");
    handle.writer().flush().expect("flush");

    // A read transaction left open, as the invariant of architecture 2.2 forbids.
    let pin = schema::open(&serat_index::index_path(&f.index)).expect("open");
    pin.execute_batch("BEGIN; SELECT count(*) FROM notes;")
        .expect("begin");
    write(&f.root, "b.md", "# B\nnewer\n");
    handle
        .apply(
            &DiskVault::new(&f.root),
            &f.parser,
            &SpyEcho::default(),
            &path("b.md"),
        )
        .expect("apply");

    let started = Instant::now();
    handle.close().expect("close");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "close waited out the full busy timeout"
    );
    assert_eq!(clean_close(&f.index), 0, "marked clean over a pinned WAL");
    drop(pin);
}

/// Copy the index files as they are right now.
fn snapshot(index: &Path, into: &Path) {
    fs::create_dir_all(into).expect("mkdir");
    for name in ["index.db", "index.db-wal"] {
        let source = index.join(name);
        if source.exists() {
            fs::copy(&source, into.join(name)).expect("copy");
        }
    }
}

/// Replace the index files with a snapshot: what a power cut that lost every commit
/// after the snapshot leaves behind.
fn crash_back_to(snapshot: &Path, index: &Path) {
    for name in ["index.db", "index.db-wal", "index.db-shm"] {
        let _ = fs::remove_file(index.join(name));
    }
    for name in ["index.db", "index.db-wal"] {
        let source = snapshot.join(name);
        if source.exists() {
            fs::copy(&source, index.join(name)).expect("restore");
        }
    }
}

#[test]
fn a_commit_lost_after_the_marker_cleared_is_recovered_by_the_mtime_catch_up() {
    // The simulated crash of task #14: a commit made after the session opened, never
    // checkpointed, then lost. The path-only catch-up cannot see it, because the
    // index still holds the path; the marker says to compare content.
    let f = fixture();
    let source = DiskVault::new(&f.root);
    let handle = IndexHandle::open(&f.index).expect("open");

    // The session has opened (the marker is cleared durably) and nothing else has
    // happened: this is the state a power cut can roll back to.
    let before = f._dir.path().join("snapshot");
    snapshot(&f.index, &before);

    // A save that the index commits to its WAL, without a checkpoint or an fsync.
    write(&f.root, "a.md", "# A\nthe edit the crash will lose\n");
    nudge(&f.root, "a.md", 600);
    handle
        .apply(&source, &f.parser, &SpyEcho::default(), &path("a.md"))
        .expect("apply");
    handle.writer().flush().expect("flush");
    let committed = hash_of(&f.index, "a.md").expect("row");
    drop(handle); // no close(): there is no clean shutdown in this test

    crash_back_to(&before, &f.index);
    let stale = hash_of(&f.index, "a.md").expect("row");
    assert_ne!(
        stale, committed,
        "the simulated crash did not lose the commit"
    );

    // Reopen. The marker is what tells us.
    let reopened = IndexHandle::open(&f.index).expect("reopen");
    assert!(reopened.opened_after_unclean_shutdown());
    let reconciler = Reconciler::new(&reopened);

    // The mtime comparison finds the stale row and fixes it:
    let outcome = reconciler
        .catch_up(
            &source,
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsAndMtime,
            Some(now()),
        )
        .expect("catch up");
    assert_eq!(outcome.upserted, 1);
    assert_eq!(hash_of(&f.index, "a.md").expect("row"), committed);
    reopened.close().expect("close");
}

#[test]
fn the_cleared_marker_survives_the_crash_that_loses_later_commits() {
    // The reason the clear is forced to synchronous = FULL: it must be in the part of
    // the log that survives. The snapshot above is taken after open, and this checks
    // that what a crash back to that point leaves is marked unclean.
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let before = f._dir.path().join("snapshot");
    snapshot(&f.index, &before);
    drop(handle);
    crash_back_to(&before, &f.index);
    assert_eq!(clean_close(&f.index), 0);
}

// --- a note the parser refuses (task #58) -----------------------------------------

fn table_counts(index: &Path) -> Vec<i64> {
    let conn = schema::open(&serat_index::index_path(index)).expect("open");
    ["notes", "blocks", "fields", "links", "tasks", "blocks_fts"]
        .iter()
        .map(|table| {
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("count")
        })
        .collect()
}

#[test]
fn a_note_edited_into_a_refused_form_is_one_bare_row_and_a_rebuild_agrees() {
    let f = fixture();
    write(
        &f.root,
        "a.md",
        "# A\nalpha\ntype:: person\n[[b]]\n- [ ] todo\n",
    );
    write(&f.root, "b.md", "# B\n[[a]]\n");
    rebuild(&DiskVault::new(&f.root), &f.parser, &f.root, &f.index).expect("rebuild");
    let handle = IndexHandle::open(&f.index).expect("open");
    let reconciler = Reconciler::new(&handle);
    let source = DiskVault::new(&f.root);
    let with_structure = table_counts(&f.index);

    // Externally edited into a form the parser refuses, then indexed.
    f.parser
        .refuses
        .lock()
        .expect("lock")
        .push("a.md".to_owned());
    write(&f.root, "a.md", "\u{0}\u{0} not parseable\n");
    let outcome = reconciler
        .index_paths(&source, &f.parser, &never_echo, &[path("a.md")])
        .expect("index");
    assert_eq!((outcome.upserted, outcome.unparsed), (1, 1));
    handle.close().expect("close");

    let conn = schema::open(&serat_index::index_path(&f.index)).expect("open");
    let (rows, title, blocks): (i64, String, i64) = conn
        .query_row(
            "SELECT count(*), max(title), (SELECT count(*) FROM blocks WHERE note_id = n.id)
               FROM notes n WHERE path = 'a.md'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("row");
    assert_eq!((rows, title.as_str(), blocks), (1, "a", 0));
    let stale: i64 = conn
        .query_row(
            "SELECT (SELECT count(*) FROM fields f JOIN notes n ON n.id = f.note_id WHERE n.path = 'a.md')
                  + (SELECT count(*) FROM links l JOIN notes n ON n.id = l.src_note WHERE n.path = 'a.md')
                  + (SELECT count(*) FROM tasks t JOIN notes n ON n.id = t.note_id WHERE n.path = 'a.md')",
            [],
            |row| row.get(0),
        )
        .expect("stale");
    assert_eq!(stale, 0, "structure from the earlier version survived");
    // Still a link target: b's [[a]] resolves to the bare row.
    let resolved: i64 = conn
        .query_row(
            "SELECT count(*) FROM links l JOIN notes b ON b.id = l.src_note
               JOIN notes a ON a.id = l.dst_note WHERE b.path = 'b.md' AND a.path = 'a.md'",
            [],
            |row| row.get(0),
        )
        .expect("link");
    assert_eq!(resolved, 1, "the bare row is not a link target");
    drop(conn);
    let incremental = table_counts(&f.index);
    assert!(incremental[1] < with_structure[1]);

    // A full rebuild of the same vault yields the same rows (C-02).
    rebuild(&DiskVault::new(&f.root), &f.parser, &f.root, &f.index).expect("rebuild");
    assert_eq!(table_counts(&f.index), incremental);
}

#[test]
fn a_second_event_for_the_same_refused_bytes_is_discarded_by_the_hash_gate() {
    // A2: the bare row stores the hash of the bytes that failed, so the gate works.
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let reconciler = Reconciler::new(&handle);
    let source = DiskVault::new(&f.root);
    f.parser
        .refuses
        .lock()
        .expect("lock")
        .push("a.md".to_owned());
    write(&f.root, "a.md", "unparseable\n");

    let first = reconciler
        .index_paths(&source, &f.parser, &never_echo, &[path("a.md")])
        .expect("index");
    assert_eq!((first.upserted, first.unparsed), (1, 1));
    let calls = f.parser.calls.load(Ordering::Relaxed);

    let second = reconciler
        .index_paths(&source, &f.parser, &never_echo, &[path("a.md")])
        .expect("index");
    assert_eq!(second.unchanged, 1);
    assert_eq!(f.parser.calls.load(Ordering::Relaxed), calls);
    handle.close().expect("close");
}

#[test]
fn a_save_of_a_refused_note_is_a_bare_row_and_records_its_echo() {
    // A2, the save path.
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    f.parser
        .refuses
        .lock()
        .expect("lock")
        .push("a.md".to_owned());
    write(&f.root, "a.md", "unparseable\n");
    let echo = SpyEcho::default();

    handle
        .apply(&DiskVault::new(&f.root), &f.parser, &echo, &path("a.md"))
        .expect("apply");
    handle.writer().flush().expect("flush");

    let recorded = echo.recorded.lock().expect("lock").clone();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, path("a.md"));
    assert_eq!(
        hash_of(&f.index, "a.md").expect("row"),
        recorded[0].1.to_hex()
    );
    handle.close().expect("close");
}

/// A source whose reads of one path fail the way a permission error or a scanner's
/// sharing violation does: the file exists and cannot be read.
struct Unreadable<'a> {
    inner: DiskVault,
    path: &'a str,
}

impl NoteSource for Unreadable<'_> {
    fn entries(&self) -> Result<Vec<SourceEntry>, serat_core::VaultError> {
        self.inner.entries()
    }

    fn load(&self, path: &VaultPath) -> Result<serat_core::LoadedNote, serat_core::VaultError> {
        if path.as_str() == self.path {
            return Err(serat_core::VaultError::Io {
                path: path.as_str().to_owned(),
                message: "permission denied".to_owned(),
            });
        }
        self.inner.load(path)
    }
}

fn blocks_of(index: &Path, relative: &str) -> i64 {
    let conn = schema::open(&serat_index::index_path(index)).expect("open");
    conn.query_row(
        "SELECT count(*) FROM blocks b JOIN notes n ON n.id = b.note_id WHERE n.path = ?1",
        [relative],
        |row| row.get(0),
    )
    .expect("count")
}

#[test]
fn an_unreadable_file_keeps_its_row_and_blocks_on_the_watcher_path_and_the_save_path() {
    // A1/S1: only NotFound is a delete.
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let before = blocks_of(&f.index, "a.md");
    assert!(before > 0);
    let source = Unreadable {
        inner: DiskVault::new(&f.root),
        path: "a.md",
    };

    let outcome = Reconciler::new(&handle)
        .index_paths(&source, &f.parser, &never_echo, &[path("a.md")])
        .expect("index");
    assert_eq!(outcome.removed, 0);

    let echo = SpyEcho::default();
    handle
        .apply(&source, &f.parser, &echo, &path("a.md"))
        .expect("apply");
    handle.writer().flush().expect("flush");
    assert!(echo.recorded.lock().expect("lock").is_empty());

    assert_eq!(paths_in(&f.index).len(), 3, "a row was removed");
    assert_eq!(blocks_of(&f.index, "a.md"), before);
    handle.close().expect("close");
}

#[test]
fn a_copy_of_a_just_saved_note_is_indexed_and_the_saved_one_is_suppressed() {
    // The (path, hash) key: save A, write A's bytes to B before A's event drains.
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let source = DiskVault::new(&f.root);
    write(&f.root, "A.md", "# Same bytes\n");
    let echo = SpyEcho::default();
    handle
        .apply(&source, &f.parser, &echo, &path("A.md"))
        .expect("save A");
    handle.writer().flush().expect("flush");
    write(&f.root, "B.md", "# Same bytes\n"); // cp A.md B.md

    let pending = std::cell::RefCell::new(echo.recorded.lock().expect("lock").clone());
    let take = |p: &VaultPath, h: serat_core::ContentHash| {
        let mut pending = pending.borrow_mut();
        match pending.iter().position(|(rp, rh)| rp == p && *rh == h) {
            Some(i) => {
                pending.remove(i);
                true
            }
            None => false,
        }
    };
    let reconciler = Reconciler::new(&handle);
    let parsed = f.parser.calls.load(Ordering::Relaxed);

    let b = reconciler
        .index_paths(&source, &f.parser, &take, &[path("B.md")])
        .expect("B");
    assert_eq!(b.upserted, 1, "B was taken for A's echo");
    assert_eq!(f.parser.calls.load(Ordering::Relaxed), parsed + 1);

    let a = reconciler
        .index_paths(&source, &f.parser, &take, &[path("A.md")])
        .expect("A");
    assert_eq!(a.unchanged, 1, "A's own event was not suppressed");
    assert!(paths_in(&f.index).contains(&"B.md".to_owned()));
    handle.close().expect("close");
}

#[test]
fn a_save_during_a_large_catch_up_is_accepted_not_refused() {
    // ADR-0011 item 4: bulk work must not keep the writer's channel full.
    let f = fixture();
    for i in 0..3000 {
        write(&f.root, &format!("bulk/n{i:04}.md"), "# Bulk\nbody\n");
    }
    // A channel only a little deeper than one pacing batch, so the catch-up would fill it
    // if it were not paced, and the 3,000 entries are many times that.
    let handle = IndexHandle::with_queue_depth(&f.index, 600).expect("open");
    let reconciler = Reconciler::new(&handle);
    let source = DiskVault::new(&f.root);
    let entries = walk(&f.root);
    let writer = handle.writer();
    let done = std::sync::atomic::AtomicBool::new(false);
    let mut refused = 0;
    let mut saves = 0;

    std::thread::scope(|scope| {
        scope.spawn(|| {
            reconciler
                .catch_up(
                    &source,
                    &f.parser,
                    &entries,
                    CatchUp::PathsAndMtime,
                    Some(now()),
                )
                .expect("catch up");
            done.store(true, Ordering::Relaxed);
        });
        while !done.load(Ordering::Relaxed) {
            match serat_index::apply(
                &writer,
                &source,
                &f.parser,
                &SpyEcho::default(),
                &path("a.md"),
            ) {
                Ok(()) => saves += 1,
                Err(serat_core::IndexError::QueueFull { .. }) => refused += 1,
                Err(other) => panic!("{other}"),
            }
            // Saves are keystroke-paced, not a hot loop.
            std::thread::sleep(Duration::from_millis(2));
        }
    });
    assert!(saves > 0, "the catch-up finished before a save was tried");
    assert_eq!(
        refused, 0,
        "{refused} saves were refused during the catch-up"
    );
    handle.close().expect("close");
}

#[test]
fn an_op_submitted_after_a_clean_close_reopens_the_marker() {
    // B2: a clone of the writer handle outliving close_clean must not commit under
    // clean_close = 1.
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let writer = handle.writer();
    assert!(writer.close_clean().expect("close_clean"));
    assert_eq!(clean_close(&f.index), 1);

    handle
        .apply(
            &DiskVault::new(&f.root),
            &f.parser,
            &SpyEcho::default(),
            &path("a.md"),
        )
        .expect("apply");
    writer.flush().expect("flush");
    assert_eq!(clean_close(&f.index), 0, "committed under a clean marker");
    drop(handle);
}

// --- runtime rebuild ------------------------------------------------------------

#[test]
fn a_runtime_rebuild_releases_the_file_before_it_deletes_it() {
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    write(&f.root, "added-later.md", "# Later\n");
    let db = serat_index::index_path(&f.index);
    let released = std::cell::Cell::new(None);

    let (new_handle, report) = handle
        .rebuild_in_place(
            &|| released.set(Some(db.exists())),
            &DiskVault::new(&f.root),
            &f.parser,
            &f.root,
        )
        .expect("rebuild in place");

    assert_eq!(
        released.get(),
        Some(true),
        "release() must run once, while the old file is still there to be released"
    );
    assert_eq!(report.notes, 4);
    assert_eq!(paths_in(&f.index).len(), 4);
    assert!(!new_handle.opened_after_unclean_shutdown());
    new_handle.close().expect("close");
}

// --- index://changed --------------------------------------------------------------

#[test]
fn the_change_signal_emits_at_once_then_at_most_ten_a_second_and_never_loses_the_last() {
    let mut signal = ChangeSignal::new(Duration::from_millis(100));
    let t0 = Instant::now();

    assert_eq!(signal.due(t0), None, "nothing changed yet");
    assert_eq!(signal.changed(t0), Some(1), "the leading edge is immediate");

    // A burst inside the interval is deferred, and is one emission when it ends.
    assert_eq!(signal.changed(t0 + Duration::from_millis(10)), None);
    assert_eq!(signal.changed(t0 + Duration::from_millis(20)), None);
    assert_eq!(signal.due(t0 + Duration::from_millis(99)), None);
    assert_eq!(
        signal.next_due(),
        Some(t0 + Duration::from_millis(100)),
        "the owner needs to know when to look again"
    );
    assert_eq!(signal.due(t0 + Duration::from_millis(100)), Some(3));
    assert_eq!(signal.due(t0 + Duration::from_millis(500)), None);
    assert_eq!(signal.next_due(), None);

    // After a quiet period the next change is immediate again, and generations only
    // go up.
    assert_eq!(signal.changed(t0 + Duration::from_secs(5)), Some(4));
    assert_eq!(signal.generation(), 4);
}

#[test]
fn a_catch_up_counts_a_file_that_exists_but_cannot_be_read() {
    // The read fails with something other than NotFound: the row stays and the file
    // is counted, as index_paths counts it.
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let entries = vec![SourceEntry {
        path: path("a.md"),
        kind: serat_core::NoteKind::Note,
        size: 0,
        mtime_ms: mtime_of(&f.index, "a.md") + 1,
    }];
    let outcome = Reconciler::new(&handle)
        .catch_up(
            &DeniedSource(DiskVault::new(&f.root)),
            &f.parser,
            &entries,
            CatchUp::PathsAndMtime,
            Some(now()),
        )
        .expect("catch up");
    assert_eq!(outcome.unreadable, 1);
    assert!(paths_in(&f.index).contains(&"a.md".to_owned()));
    handle.close().expect("close");
}

/// A source whose every `load` fails with a permission error.
struct DeniedSource(DiskVault);

impl serat_core::NoteSource for DeniedSource {
    fn entries(&self) -> Result<Vec<SourceEntry>, serat_core::VaultError> {
        self.0.entries()
    }

    fn load(&self, path: &VaultPath) -> Result<serat_core::LoadedNote, serat_core::VaultError> {
        Err(serat_core::VaultError::Io {
            path: path.as_str().to_owned(),
            message: "permission denied".to_owned(),
        })
    }
}

// ---- task #66: size and the catch-up window (ADR-0015 section 2 as amended) ----------

fn disk_mtime(root: &Path, relative: &str) -> SystemTime {
    fs::metadata(root.join(relative))
        .expect("stat")
        .modified()
        .expect("mtime")
}

/// Rewrite a file and put its mtime back to `mtime`, as a coarse-mtime filesystem would.
fn rewrite_keeping_mtime(root: &Path, relative: &str, body: &str, mtime: SystemTime) {
    write(root, relative, body);
    fs::File::options()
        .write(true)
        .open(root.join(relative))
        .expect("open")
        .set_modified(mtime)
        .expect("set mtime");
}

fn size_of(index: &Path, relative: &str) -> i64 {
    let conn = schema::open(&serat_index::index_path(index)).expect("open");
    conn.query_row(
        "SELECT size FROM notes WHERE path = ?1",
        [relative],
        |row| row.get(0),
    )
    .expect("row")
}

fn run_catch_up(f: &Fixture, handle: &IndexHandle, started_at_ms: i64) -> Outcome {
    Reconciler::new(handle)
        .catch_up(
            &DiskVault::new(&f.root),
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsAndMtime,
            Some(started_at_ms),
        )
        .expect("catch up")
}

#[test]
fn rows_carry_the_size_of_the_file_they_were_built_from() {
    let f = fixture();
    assert_eq!(size_of(&f.index, "a.md"), "# A\nalpha\n".len() as i64);
}

#[test]
fn a_same_mtime_rewrite_of_a_different_size_is_reindexed() {
    let f = fixture();
    let old = disk_mtime(&f.root, "a.md");
    let handle = IndexHandle::open(&f.index).expect("open");
    let before = hash_of(&f.index, "a.md");
    rewrite_keeping_mtime(&f.root, "a.md", "# A\nalpha, and then some\n", old);

    let outcome = run_catch_up(&f, &handle, now());

    assert_eq!(outcome.upserted, 1, "{outcome:?}");
    assert_ne!(hash_of(&f.index, "a.md"), before);
    assert_eq!(
        size_of(&f.index, "a.md"),
        "# A\nalpha, and then some\n".len() as i64
    );
    handle.close().expect("close");
}

#[test]
fn a_same_mtime_same_size_rewrite_inside_the_window_is_reindexed() {
    // Files stamped now, then indexed: the rebuild's window starts at its walk, so the
    // stamp is inside it. A rewrite of the same length that keeps the stamp is the case
    // a 2 s mtime tick produces.
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path().join("vault");
    let index = dir.path().join("index");
    write(&root, "a.md", "# A\nalpha\n");
    let stamp = SystemTime::now();
    fs::File::options()
        .write(true)
        .open(root.join("a.md"))
        .expect("open")
        .set_modified(stamp)
        .expect("set mtime");
    let parser = LineParser::default();
    rebuild(&DiskVault::new(&root), &parser, &root, &index).expect("rebuild");
    let handle = IndexHandle::open(&index).expect("open");
    let before = hash_of(&index, "a.md");
    rewrite_keeping_mtime(&root, "a.md", "# A\nalphb\n", stamp);
    let entries = walk(&root);
    assert_eq!(
        entries[0].size,
        u64::try_from(size_of(&index, "a.md")).expect("size")
    );

    let outcome = Reconciler::new(&handle)
        .catch_up(
            &DiskVault::new(&root),
            &parser,
            &entries,
            CatchUp::PathsAndMtime,
            Some(now()),
        )
        .expect("catch up");

    assert_eq!(outcome.upserted, 1, "{outcome:?}");
    assert_ne!(hash_of(&index, "a.md"), before);
    handle.close().expect("close");
}

#[test]
fn a_same_mtime_same_size_rewrite_outside_the_window_is_not_seen() {
    // The documented residual (ADR-0015 section 2 as amended, A-03): nothing but the
    // watcher could have told us.
    let f = fixture();
    let old = disk_mtime(&f.root, "a.md");
    let handle = IndexHandle::open(&f.index).expect("open");
    let before = hash_of(&f.index, "a.md");
    rewrite_keeping_mtime(&f.root, "a.md", "# A\nalphb\n", old);

    let outcome = run_catch_up(&f, &handle, now());

    assert_eq!(outcome, Outcome::default());
    assert_eq!(hash_of(&f.index, "a.md"), before);
    handle.close().expect("close");
}

#[test]
fn the_window_advances_when_a_catch_up_completes_and_not_for_a_compare_only_run() {
    // That an unfinished catch-up leaves it alone is the unit test in external.rs
    // (it needs the writer's fault injector).
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let from_rebuild = handle.caught_up_from_ms().expect("value");
    assert!(from_rebuild > 0, "a finished rebuild writes it");

    // `None` is a rescan: compare, never write.
    Reconciler::new(&handle)
        .catch_up(
            &DiskVault::new(&f.root),
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsAndMtime,
            None,
        )
        .expect("catch up");
    assert_eq!(handle.caught_up_from_ms().expect("value"), from_rebuild);

    let started = from_rebuild + 10_000;
    run_catch_up(&f, &handle, started);
    assert_eq!(handle.caught_up_from_ms().expect("value"), started);
    handle.close().expect("close");
}

/// A source whose `load` fails (not NotFound) for the named paths.
struct FlakySource {
    inner: DiskVault,
    failing: Vec<&'static str>,
}

impl serat_core::NoteSource for FlakySource {
    fn entries(&self) -> Result<Vec<SourceEntry>, serat_core::VaultError> {
        self.inner.entries()
    }

    fn load(&self, path: &VaultPath) -> Result<serat_core::LoadedNote, serat_core::VaultError> {
        if self.failing.contains(&path.as_str()) {
            return Err(serat_core::VaultError::Io {
                path: path.as_str().to_owned(),
                message: "sharing violation".to_owned(),
            });
        }
        self.inner.load(path)
    }
}

#[test]
fn a_catch_up_that_could_not_read_a_file_does_not_move_the_window_past_it() {
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    // A same-size rewrite that kept its mtime: only the window can make it a candidate.
    let kept = disk_mtime(&f.root, "a.md");
    rewrite_keeping_mtime(&f.root, "a.md", "# A\nomega\n", kept);
    assert_eq!(size_of(&f.index, "a.md"), 10);
    let m = mtime_of(&f.index, "a.md");
    handle.writer().set_caught_up_from(m).expect("window");

    let flaky = FlakySource {
        inner: DiskVault::new(&f.root),
        failing: vec!["a.md"],
    };
    let reconciler = Reconciler::new(&handle);
    let outcome = reconciler
        .catch_up(
            &flaky,
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsAndMtime,
            Some(now()),
        )
        .expect("catch up");
    assert_eq!(outcome.unreadable, 1);
    let window = handle.caught_up_from_ms().expect("value");
    assert!(
        window <= m,
        "the window {window} moved past the unread file at {m}"
    );

    // The next catch-up, with the file readable, re-hashes it.
    let outcome = run_catch_up(&f, &handle, now());
    assert_eq!(outcome.upserted, 1, "the missed rewrite is found");
    assert!(handle.caught_up_from_ms().expect("value") > m);
    handle.close().expect("close");
}

#[test]
fn a_rebuild_records_the_time_its_walk_began() {
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path().join("vault");
    let index = dir.path().join("index");
    write(&root, "a.md", "# A\n");
    let before = now();
    rebuild(
        &DiskVault::new(&root),
        &LineParser::default(),
        &root,
        &index,
    )
    .expect("rebuild");
    let after = now();
    let handle = IndexHandle::open(&index).expect("open");
    let value = handle.caught_up_from_ms().expect("value");
    assert!(
        (before..=after).contains(&value),
        "{before} {value} {after}"
    );
    handle.close().expect("close");
}

#[test]
fn a_fresh_index_has_no_window_so_everything_is_a_candidate() {
    let dir = tempfile::tempdir().expect("temp");
    let handle = IndexHandle::open(&dir.path().join("index")).expect("open");
    assert_eq!(handle.caught_up_from_ms().expect("value"), 0);
    handle.close().expect("close");
}

#[test]
fn window_zero_makes_a_same_mtime_same_size_rewrite_a_candidate() {
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    handle.writer().set_caught_up_from(0).expect("window");
    let kept = disk_mtime(&f.root, "a.md");
    rewrite_keeping_mtime(&f.root, "a.md", "# A\nomega\n", kept);
    let outcome = run_catch_up(&f, &handle, now());
    assert_eq!(outcome.upserted, 1);
    handle.close().expect("close");
}

#[test]
fn a_window_ahead_of_the_clock_is_ignored() {
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    handle
        .writer()
        .set_caught_up_from(now() + 3_600_000)
        .expect("window");
    let kept = disk_mtime(&f.root, "a.md");
    rewrite_keeping_mtime(&f.root, "a.md", "# A\nomega\n", kept);
    let outcome = run_catch_up(&f, &handle, now());
    assert_eq!(outcome.upserted, 1);
    handle.close().expect("close");
}

#[test]
fn a_future_mtime_with_unchanged_bytes_is_read_and_not_parsed_every_catch_up() {
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    nudge(&f.root, "a.md", 3600);
    run_catch_up(&f, &handle, now());
    let parsed = f.parser.calls.load(Ordering::Relaxed);
    for _ in 0..2 {
        let outcome = run_catch_up(&f, &handle, now());
        assert_eq!((outcome.unchanged, outcome.upserted), (1, 0));
    }
    assert_eq!(f.parser.calls.load(Ordering::Relaxed), parsed, "no parse");
    handle.close().expect("close");
}

/// Set a file's mtime `seconds` in the past (a sync tool restoring an old stamp).
fn backdate_by(root: &Path, relative: &str, seconds: u64) {
    fs::File::options()
        .write(true)
        .open(root.join(relative))
        .expect("open")
        .set_modified(SystemTime::now() - Duration::from_secs(seconds))
        .expect("set mtime");
}
