//! Cold start: detecting an index that cannot be used, and rebuilding it (I6).
//!
//! NFR-REL-02 and US-07. Its acceptance criterion — given a truncated index file, when
//! the app starts, then it detects the corruption and rebuilds automatically — is the
//! only one of P0's seven requirements that nothing had exercised.

mod support;

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use serat_index::{IndexStatus, RebuildReason, open_or_rebuild, schema};
use support::{FakeVault, LineParser, vault_fixture};

/// Every status published, in order, so a test can assert what a consumer saw rather
/// than only where things ended up. The order is the requirement.
#[derive(Debug, Default)]
struct Recorder(Mutex<Vec<IndexStatus>>);

impl Recorder {
    fn sink(&self) -> impl Fn(IndexStatus) + '_ {
        move |status| {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(status);
        }
    }

    fn seen(&self) -> Vec<IndexStatus> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

fn open(dir: &Path, recorder: &Recorder) -> Result<Option<usize>, serat_core::IndexError> {
    let vault = vault_fixture();
    let parser = LineParser::default();
    let (handle, report) = open_or_rebuild(
        Path::new("C:/vaults/serat"),
        dir,
        &vault,
        &parser,
        &recorder.sink(),
    )?;
    handle.close()?;
    Ok(report.map(|report| report.notes))
}

/// Build a usable index the way the application would.
fn seed(dir: &Path) {
    let recorder = Recorder::default();
    open(dir, &recorder).expect("seed");
}

#[test]
fn an_absent_index_is_built_and_the_reason_says_so() {
    let dir = tempfile::tempdir().expect("temp");
    let recorder = Recorder::default();

    let notes = open(dir.path(), &recorder).expect("open");
    assert_eq!(notes, Some(3));
    assert_eq!(
        recorder.seen(),
        vec![
            IndexStatus::Opening,
            IndexStatus::Rebuilding {
                reason: RebuildReason::NoIndex
            },
            IndexStatus::Ready,
        ]
    );
}

#[test]
fn a_usable_index_is_opened_without_rebuilding() {
    let dir = tempfile::tempdir().expect("temp");
    seed(dir.path());

    let recorder = Recorder::default();
    let report = open(dir.path(), &recorder).expect("open");

    assert_eq!(report, None, "a usable index was rebuilt anyway");
    assert_eq!(
        recorder.seen(),
        vec![IndexStatus::Opening, IndexStatus::Ready],
        "a second open must not rebuild"
    );
}

#[test]
fn the_state_is_published_before_the_rebuild_rather_than_after() {
    // The constraint S1's measurements turned into a requirement. §3.4's t_usable waits
    // for the index state to be KNOWN, and Rebuilding settles that exactly as Ready
    // does — so publishing only after the rebuild would put a full reindex inside the
    // window budget, and on the vault that most needs the window up, the application
    // would report itself unusable for thirty seconds.
    //
    // Asserted by ordering rather than by timing: the status must be published while
    // the source has not yet been asked for anything.
    let dir = tempfile::tempdir().expect("temp");
    let vault = vault_fixture();
    let parser = LineParser::default();

    let published_before_any_work = Mutex::new(None::<bool>);
    let on_status = |status: IndexStatus| {
        if matches!(status, IndexStatus::Rebuilding { .. }) {
            let mut slot = published_before_any_work
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // The rebuild's first act is to enumerate; if it has not done so, no work
            // has started.
            *slot = Some(vault.entries_called() == 0);
        }
    };

    let (handle, _) = open_or_rebuild(
        Path::new("C:/vaults/serat"),
        dir.path(),
        &vault,
        &parser,
        &on_status,
    )
    .expect("open");
    handle.close().expect("close");

    assert_eq!(
        *published_before_any_work
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        Some(true),
        "Rebuilding was published after the rebuild had already started reading the \
         vault; a consumer waiting for the index state to be known would wait for the \
         whole rebuild"
    );
}

#[test]
fn a_truncated_index_is_detected_and_rebuilt() {
    // NFR-REL-02's acceptance criterion, and the one P0 requirement nothing had
    // exercised. One page, not a percentage: SQLite derives the page count from the
    // file length by integer division, so a cut smaller than a page is invisible to the
    // stamp (F-43, corrected twice).
    let dir = tempfile::tempdir().expect("temp");
    seed(dir.path());

    let path = serat_index::index_path(dir.path());
    let before = fs::metadata(&path).expect("stat").len();
    assert!(before > 8_192, "the fixture index is too small to truncate");

    let file = fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open");
    file.set_len(before - 4_096).expect("truncate one page");
    drop(file);

    let recorder = Recorder::default();
    let notes = open(dir.path(), &recorder).expect("open");

    assert_eq!(notes, Some(3), "the vault was not reindexed");
    let seen = recorder.seen();
    assert!(
        matches!(
            seen.as_slice(),
            [
                IndexStatus::Opening,
                IndexStatus::Rebuilding {
                    reason: RebuildReason::Corrupt
                },
                IndexStatus::Ready
            ]
        ),
        "a truncated index did not produce a corruption rebuild: {seen:?}"
    );
    assert_eq!(
        schema::inspect(&serat_index::index_path(dir.path())).expect("inspect"),
        serat_index::IndexState::Ready
    );
}

#[test]
fn a_file_that_is_not_a_database_is_rebuilt_rather_than_refused() {
    let dir = tempfile::tempdir().expect("temp");
    fs::create_dir_all(dir.path()).expect("mkdir");
    fs::write(
        serat_index::index_path(dir.path()),
        b"this is a text file that someone put here",
    )
    .expect("write");

    let recorder = Recorder::default();
    let notes = open(dir.path(), &recorder).expect("open");

    assert_eq!(notes, Some(3));
    assert!(recorder.seen().contains(&IndexStatus::Rebuilding {
        reason: RebuildReason::Corrupt
    }));
}

#[test]
fn a_schema_version_that_does_not_match_rebuilds_rather_than_migrates() {
    // ADR-0002. There is no migration path and a mismatch is not an error; it is the
    // ordinary way a new build meets an old index.
    let dir = tempfile::tempdir().expect("temp");
    seed(dir.path());
    {
        let conn = schema::open(&serat_index::index_path(dir.path())).expect("open");
        conn.execute_batch("PRAGMA user_version = 99;")
            .expect("bump");
    }

    let recorder = Recorder::default();
    open(dir.path(), &recorder).expect("open");
    assert!(recorder.seen().contains(&IndexStatus::Rebuilding {
        reason: RebuildReason::SchemaChanged
    }));
}

#[test]
fn an_interrupted_rebuild_is_finished_on_the_next_open() {
    // The property that makes a crash mid-rebuild recoverable: state stays 'building'
    // until the last step, so an index whose process died is rebuilt rather than
    // trusted.
    let dir = tempfile::tempdir().expect("temp");
    seed(dir.path());
    {
        let conn = schema::open(&serat_index::index_path(dir.path())).expect("open");
        conn.execute_batch("UPDATE index_meta SET state = 'building';")
            .expect("interrupt");
    }

    let recorder = Recorder::default();
    open(dir.path(), &recorder).expect("open");
    assert!(recorder.seen().contains(&IndexStatus::Rebuilding {
        reason: RebuildReason::Interrupted
    }));
}

#[test]
fn an_index_built_from_another_vault_is_rebuilt() {
    // §3.4 step 5. Not corruption — the database is perfectly sound — but every path in
    // it describes files that are not there, which is as useless as if it were.
    let dir = tempfile::tempdir().expect("temp");
    seed(dir.path());

    let vault = vault_fixture();
    let parser = LineParser::default();
    let recorder = Recorder::default();
    let (handle, report) = open_or_rebuild(
        Path::new("C:/vaults/somewhere-else"),
        dir.path(),
        &vault,
        &parser,
        &recorder.sink(),
    )
    .expect("open");
    handle.close().expect("close");

    assert!(report.is_some(), "an index from another vault was reused");
    assert!(recorder.seen().contains(&IndexStatus::Rebuilding {
        reason: RebuildReason::VaultMoved
    }));
}

#[test]
fn a_rebuild_that_fails_leaves_unavailable_rather_than_retrying() {
    // §7.1's rule applied to the index: a rebuild that fails will fail again, and an
    // indexer spinning on a vault the user cannot fix is worse than one that stops and
    // says so. The vault refuses to be enumerated at all, which is the one failure a
    // rebuild cannot skip past.
    let dir = tempfile::tempdir().expect("temp");
    let vault = FakeVault::with(&[]).refusing_to_enumerate();
    let parser = LineParser::default();
    let recorder = Recorder::default();

    let outcome = open_or_rebuild(
        Path::new("C:/vaults/serat"),
        dir.path(),
        &vault,
        &parser,
        &recorder.sink(),
    );

    assert!(
        outcome.is_err(),
        "a rebuild that could not run reported success"
    );
    let seen = recorder.seen();
    assert_eq!(
        seen.last(),
        Some(&IndexStatus::Unavailable),
        "a failed rebuild did not end in Unavailable: {seen:?}"
    );
    assert_eq!(
        seen.iter()
            .filter(|status| matches!(status, IndexStatus::Rebuilding { .. }))
            .count(),
        1,
        "the rebuild was retried; §7.1 says it must not be"
    );
}
