//! The writer's commit observer (task #175; ADR-0020 decision 3; NFR-PRF-03, FR-VLT-04).
//!
//! Every test that asserts "nothing was reported" does so after a barrier has returned:
//! the writer reports before it answers, so by then the count is final and no sleep is
//! needed.

mod support;

use std::path::Path;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serat_core::IndexWriter;
use serat_index::{
    ActorOptions, CommitObserver, CommittedBatch, IndexActor, IndexHandle, WriterHandle, schema,
};
use support::{DiskVault, LineParser, meta, vault_fixture};

const WAIT: Duration = Duration::from_secs(10);

fn asset(path: &str) -> serat_core::IndexOp {
    serat_core::IndexOp::UpsertAsset { meta: meta(path) }
}

/// An observer that records each batch together with whether a **fresh** connection could
/// see every path the batch named at the moment it was called.
fn recording_observer(index_dir: &Path) -> (CommitObserver, Receiver<(CommittedBatch, bool)>) {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let db = serat_index::index_path(index_dir);
    let observer = CommitObserver::new(move |batch| {
        let conn = schema::open(&db).expect("fresh read connection");
        let visible = batch.paths.iter().all(|path| {
            conn.query_row(
                "SELECT count(*) FROM notes WHERE path = ?1",
                [path.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .expect("read")
                == 1
        });
        let _ = tx.lock().expect("lock").send((batch, visible));
    });
    (observer, rx)
}

fn counting_observer() -> (CommitObserver, Arc<Mutex<Vec<CommittedBatch>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    (
        CommitObserver::new(move |batch| sink.lock().expect("lock").push(batch)),
        seen,
    )
}

fn file_actor(dir: &Path, observer: CommitObserver) -> IndexActor {
    std::fs::create_dir_all(dir).expect("dir");
    let conn = schema::open(&serat_index::index_path(dir)).expect("open");
    schema::bootstrap_if_empty(&conn).expect("bootstrap");
    IndexActor::with_options(
        conn,
        ActorOptions {
            commit_observer: Some(observer),
            ..ActorOptions::default()
        },
    )
}

#[test]
fn the_observer_runs_after_the_commit_and_a_fresh_connection_sees_the_row() {
    let dir = tempfile::tempdir().expect("temp");
    let (observer, rx) = recording_observer(dir.path());
    let handle = IndexHandle::open_observed(dir.path(), Some(observer)).expect("open");
    let writer = handle.writer();

    writer.submit(asset("refs/a.pdf")).expect("submit");
    let (batch, visible) = rx.recv_timeout(WAIT).expect("a report");

    assert_eq!(batch.ops, 1);
    assert_eq!(batch.paths.len(), 1);
    assert_eq!(batch.paths[0].as_str(), "refs/a.pdf");
    assert!(
        visible,
        "the observer ran before COMMIT returned: a fresh connection could not see the row"
    );
    handle.close().expect("close");
}

#[cfg(feature = "test-hooks")]
#[test]
fn nothing_is_reported_while_a_batch_is_held_before_its_commit_and_it_is_reported_on_release() {
    let dir = tempfile::tempdir().expect("temp");
    let (observer, rx) = recording_observer(dir.path());
    let handle = IndexHandle::open_observed(dir.path(), Some(observer)).expect("open");
    let writer = handle.writer();

    let hold = handle.commit_gate().hold();
    writer.submit(asset("refs/held.pdf")).expect("submit");
    assert!(hold.wait_reached(WAIT), "the writer never reached the hold");

    assert!(
        rx.try_recv().is_err(),
        "reported while the batch was still uncommitted"
    );
    let other = schema::open(&serat_index::index_path(dir.path())).expect("reader");
    let rows: i64 = other
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .expect("count");
    assert_eq!(rows, 0, "a reader saw the batch before its COMMIT");

    drop(hold);
    let (_, visible) = rx.recv_timeout(WAIT).expect("reported after release");
    assert!(visible);
    handle.close().expect("close");
}

#[test]
fn bulk_load_batches_report_nothing_and_the_first_batch_after_the_bracket_does() {
    let dir = tempfile::tempdir().expect("temp");
    let (observer, seen) = counting_observer();
    let actor = file_actor(dir.path(), observer);
    let writer = actor.handle();

    let triggers = writer.begin_bulk_load().expect("begin");
    for name in ["a.pdf", "b.pdf", "c.pdf"] {
        writer.submit_blocking(asset(name)).expect("submit");
    }
    writer.flush().expect("flush");
    writer.end_bulk_load(triggers).expect("end");
    assert!(
        seen.lock().expect("lock").is_empty(),
        "a bulk-load batch was reported"
    );

    writer.submit(asset("after.pdf")).expect("submit");
    writer.flush().expect("flush");
    let seen = seen.lock().expect("lock");
    assert_eq!(seen.len(), 1, "the bracket did not end the suppression");
    assert_eq!(seen[0].paths[0].as_str(), "after.pdf");
    drop(seen);
    actor.shutdown().expect("shutdown");
}

#[test]
fn a_rebuild_through_open_or_rebuild_reports_no_batches_and_the_handle_then_does() {
    let dir = tempfile::tempdir().expect("temp");
    let (observer, seen) = counting_observer();
    let (handle, report) = serat_index::open_or_rebuild_observed(
        Path::new("C:/vaults/serat"),
        dir.path(),
        &vault_fixture(),
        &LineParser::default(),
        &|_| {},
        Some(observer),
    )
    .expect("open");
    assert!(report.is_some(), "the fixture must have forced a rebuild");
    assert!(
        seen.lock().expect("lock").is_empty(),
        "the rebuild's own commits were reported"
    );

    let writer = handle.writer();
    writer.submit(asset("late.pdf")).expect("submit");
    writer.flush().expect("flush");
    assert_eq!(seen.lock().expect("lock").len(), 1);
    handle.close().expect("close");
}

#[test]
fn barrier_only_turns_report_nothing() {
    let dir = tempfile::tempdir().expect("temp");
    let (observer, seen) = counting_observer();
    let actor = file_actor(dir.path(), observer);
    let writer = actor.handle();

    writer.flush().expect("flush");
    writer.analyze().expect("analyze");
    writer.resolve_links().expect("resolve");
    writer.mark_ready("C:/vaults/serat", 1).expect("ready");
    writer.set_caught_up_from(2).expect("caught up");
    writer.close_clean().expect("close clean");

    assert!(
        seen.lock().expect("lock").is_empty(),
        "a barrier-only turn was reported"
    );
    actor.shutdown().expect("shutdown");
}

#[test]
fn a_batch_folded_with_a_flush_is_reported_once_with_every_op() {
    let dir = tempfile::tempdir().expect("temp");
    let (observer, seen) = counting_observer();
    let actor = file_actor(dir.path(), observer);
    let writer = actor.handle();

    for name in ["a.pdf", "b.pdf"] {
        writer.submit(asset(name)).expect("submit");
    }
    writer.flush().expect("flush");
    let total: usize = seen.lock().expect("lock").iter().map(|b| b.ops).sum();
    assert_eq!(total, 2, "every op is reported exactly once");
    actor.shutdown().expect("shutdown");
}

#[test]
fn the_observer_survives_rebuild_in_place() {
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path().join("vault");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(root.join("a.md"), "# A\nalpha\n").expect("write");
    let index = dir.path().join("index");

    let (observer, seen) = counting_observer();
    let handle = IndexHandle::open_observed(&index, Some(observer)).expect("open");
    handle.writer().submit(asset("before.pdf")).expect("submit");
    handle.writer().flush().expect("flush");
    assert_eq!(seen.lock().expect("lock").len(), 1);

    let (handle, _report) = handle
        .rebuild_in_place(
            &|| {},
            &DiskVault::new(&root),
            &LineParser::default(),
            &root,
        )
        .expect("rebuild in place");
    assert_eq!(
        seen.lock().expect("lock").len(),
        1,
        "the rebuild's own commits were reported"
    );

    let writer: WriterHandle = handle.writer();
    writer.submit(asset("after.pdf")).expect("submit");
    writer.flush().expect("flush");
    let seen = seen.lock().expect("lock");
    assert_eq!(
        seen.len(),
        2,
        "the handle a runtime rebuild produced does not report"
    );
    assert_eq!(seen[1].paths[0].as_str(), "after.pdf");
    drop(seen);
    handle.close().expect("close");
}

#[test]
fn an_observer_that_panics_does_not_stop_the_writer() {
    let dir = tempfile::tempdir().expect("temp");
    let actor = file_actor(
        dir.path(),
        CommitObserver::new(|_| panic!("observer failure")),
    );
    let writer = actor.handle();
    writer.submit(asset("a.pdf")).expect("submit");
    writer.flush().expect("the writer survived");
    writer.submit(asset("b.pdf")).expect("still accepting");
    writer.flush().expect("flush");
    actor.shutdown().expect("shutdown");
}
