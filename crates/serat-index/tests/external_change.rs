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

use serat_core::{IndexWriter as _, NoteSource as _, SourceEntry, VaultPath};
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

fn clean_close(index: &Path) -> i64 {
    let conn = schema::open(&serat_index::index_path(index)).expect("open");
    conn.query_row(
        "SELECT clean_close FROM index_meta WHERE id = 1",
        [],
        |row| row.get(0),
    )
    .expect("marker")
}

fn never_echo(_: serat_core::ContentHash) -> bool {
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
            unparsed: 0
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
            &|_| {
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
            CatchUp::PathsOnly,
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
        .catch_up(&source, &f.parser, &walk(&f.root), CatchUp::PathsOnly)
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
            CatchUp::PathsOnly,
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
fn a_paths_only_catch_up_does_not_read_files_the_index_already_holds() {
    let f = fixture();
    let handle = IndexHandle::open(&f.index).expect("open");
    let reconciler = Reconciler::new(&handle);
    write(&f.root, "a.md", "# A\nedited behind our back\n");
    nudge(&f.root, "a.md", 120);

    let outcome = reconciler
        .catch_up(
            &DiskVault::new(&f.root),
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsOnly,
        )
        .expect("catch up");
    assert_eq!(outcome, Outcome::default(), "paths-only compared content");

    // With the mtime comparison on, the same edit is found.
    let outcome = reconciler
        .catch_up(
            &DiskVault::new(&f.root),
            &f.parser,
            &walk(&f.root),
            CatchUp::PathsAndMtime,
        )
        .expect("catch up");
    assert_eq!(outcome.upserted, 1);
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
        )
        .expect("catch up");

    assert_eq!((outcome.upserted, outcome.unchanged), (1, 1));
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

    // The path-only catch-up, which is what a clean open would run, finds nothing:
    let paths_only = reconciler
        .catch_up(&source, &f.parser, &walk(&f.root), CatchUp::PathsOnly)
        .expect("catch up");
    assert_eq!(paths_only, Outcome::default());
    assert_eq!(hash_of(&f.index, "a.md").expect("row"), stale);

    // The rule for an unclean shutdown finds the stale row and fixes it:
    let outcome = reconciler
        .catch_up(
            &source,
            &f.parser,
            &walk(&f.root),
            CatchUp::after_unclean_shutdown(reopened.opened_after_unclean_shutdown()),
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
