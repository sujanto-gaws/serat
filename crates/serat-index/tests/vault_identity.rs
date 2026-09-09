//! Vault identity on the open path: task #120, F-458 and F-459.
//!
//! `index_meta.vault_root` answers one question — *is this index this vault's index?*
//! It used to be asked as a string comparison against whatever path the caller
//! happened to hand in, so the same directory reached by a second spelling read as a
//! different vault and bought a full rebuild (F-459; 28.6 s on the 20,200-note
//! fixture, F-458).
//!
//! These run against a real directory on real disk, because the defect is entirely in
//! how a host path is spelled and a `FakeVault` has no spelling to get wrong (F-136 is
//! the same gap on the same path). `DiskVault` is `std::fs` only, so this costs the
//! dependency rule nothing.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serat_index::{IndexStatus, RebuildReason, canonical_root, index_path, open_or_rebuild};
use support::{DiskVault, LineParser};

/// A real vault directory with three notes in it.
fn vault_on_disk(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "serat-identity-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("10-projects")).expect("fixture");
    std::fs::write(dir.join("10-projects/alpha.md"), "alpha\nbody\n").expect("fixture");
    std::fs::write(dir.join("10-projects/beta.md"), "beta\nbody\n").expect("fixture");
    std::fs::write(dir.join("readme.md"), "readme\n").expect("fixture");
    dir
}

/// Every status published, so a test asserts what a consumer saw and not only where
/// things ended up.
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

/// One open, returning whether a rebuild ran and everything that was published.
fn open_at(spelling: &Path, tree: &Path, index_dir: &Path) -> (bool, Vec<IndexStatus>) {
    let source = DiskVault::new(tree.to_path_buf());
    let parser = LineParser::default();
    let recorder = Recorder::default();
    let (handle, report) = open_or_rebuild(spelling, index_dir, &source, &parser, &recorder.sink())
        .expect("open_or_rebuild");
    handle.close().expect("close");
    (report.is_some(), recorder.seen())
}

/// The recorded identity, read straight out of the database.
fn recorded_root(index_dir: &Path) -> Option<String> {
    let conn = rusqlite::Connection::open(index_path(index_dir)).expect("open the index");
    conn.query_row(
        "SELECT vault_root FROM index_meta WHERE id = 1",
        [],
        |row| row.get(0),
    )
    .expect("read vault_root")
}

#[test]
fn a_vault_reached_by_a_second_spelling_of_the_same_path_is_not_rebuilt() {
    // F-459, and the case that cost 28.6 s in F-458. Two host paths, one directory:
    // the identity has to be the resolved directory, not the bytes of the argument.
    let tree = vault_on_disk("second-spelling");
    let index_dir = tree.join(".serat");

    let (rebuilt, _) = open_at(&tree, &tree, &index_dir);
    assert!(rebuilt, "the first open of an empty index must build it");

    // A `.` component: a different string, the same directory, and the mildest form
    // of the difference — a mapped drive or a junction is the same defect with more
    // bytes between the two spellings.
    let dotted = tree.join(".");
    assert_ne!(
        dotted.to_string_lossy(),
        tree.to_string_lossy(),
        "the fixture must actually present two spellings, or this test proves nothing"
    );

    let (rebuilt, seen) = open_at(&dotted, &tree, &index_dir);
    assert!(
        !rebuilt,
        "the same directory spelled a second way was treated as a different vault and \
         rebuilt from scratch (F-459): {seen:?}"
    );
    assert!(
        !seen
            .iter()
            .any(|status| matches!(status, IndexStatus::Rebuilding { .. })),
        "no rebuild may be published for a vault that did not move: {seen:?}"
    );

    let _ = std::fs::remove_dir_all(&tree);
}

#[test]
fn a_vault_that_really_moved_keeps_its_index_and_records_the_new_root() {
    // `notes.path` is vault-relative (`schema/001_init.sql:134`), so a vault that
    // moved carries an index in which every row is still exactly right. Task #120
    // demotes the mismatch from a rebuild trigger to a reported signal: adopt the new
    // root, say so, do not throw 20,000 rows away.
    let origin = vault_on_disk("moved-origin");
    let index_dir = origin.join(".serat");
    let (rebuilt, _) = open_at(&origin, &origin, &index_dir);
    assert!(rebuilt, "the first open must build the index");

    // Move the whole vault, index and all, exactly as a user dragging a folder does.
    let moved = vault_on_disk("moved-destination");
    let _ = std::fs::remove_dir_all(&moved);
    std::fs::rename(&origin, &moved).expect("move the vault");
    let moved_index = moved.join(".serat");

    let (rebuilt, seen) = open_at(&moved, &moved, &moved_index);
    assert!(
        !rebuilt,
        "a vault that moved had its index discarded, though every path in it is \
         vault-relative and still correct: {seen:?}"
    );
    assert!(
        seen.iter()
            .any(|status| matches!(status, IndexStatus::RootAdopted { .. })),
        "the root change must be reported — a silent adoption and a silent rebuild are \
         indistinguishable in NFR-REL-02's log, which is F-459's complaint: {seen:?}"
    );
    assert_eq!(
        recorded_root(&moved_index),
        Some(canonical_root(&moved)),
        "the adopted root must be the canonical directory, not the argument's spelling"
    );

    // Recorded, so the next open is an ordinary one.
    let (_, seen) = open_at(&moved, &moved, &moved_index);
    assert_eq!(
        seen,
        vec![IndexStatus::Opening, IndexStatus::Ready],
        "the adopted root was not written back, so every subsequent open re-adopts it"
    );

    let _ = std::fs::remove_dir_all(&moved);
}

#[test]
fn an_index_whose_rows_describe_no_file_at_this_root_is_rebuilt() {
    // The case the check exists for, and the only one worth a rebuild: a `.serat/`
    // copied between vaults. The trigger is now what reconciliation would have found —
    // that not one row in the index names a file that is here — rather than a string
    // comparison that cannot tell this apart from a rename.
    let foreign = vault_on_disk("foreign-origin");
    // Deliberately outside the tree: `DiskVault` has no ignore rules, so an index built
    // inside the vault gets a row for its own `.serat/index.db`, and the host below
    // would then share that one path with it. `serat-vault` ignores `.serat/` and a
    // real vault never carries that row -- this keeps the stub from manufacturing
    // evidence the production walk would not produce.
    let foreign_index = vault_on_disk("foreign-origin-index");
    let (rebuilt, _) = open_at(&foreign, &foreign, &foreign_index);
    assert!(rebuilt, "the first open must build the index");

    // A second vault with entirely different files, and the first vault's index
    // dropped into it.
    let host = vault_on_disk("foreign-host");
    std::fs::remove_dir_all(host.join("10-projects")).expect("clear");
    std::fs::remove_file(host.join("readme.md")).expect("clear");
    std::fs::write(host.join("unrelated.md"), "nothing in common\n").expect("fixture");
    let host_index = host.join(".serat");
    std::fs::create_dir_all(&host_index).expect("index dir");
    std::fs::copy(index_path(&foreign_index), index_path(&host_index)).expect("copy the index");

    let (rebuilt, seen) = open_at(&host, &host, &host_index);
    assert!(
        rebuilt,
        "an index describing another vault's files was served as this vault's: {seen:?}"
    );
    assert!(
        seen.contains(&IndexStatus::Rebuilding {
            reason: RebuildReason::ForeignVault
        }),
        "the rebuild must name the finding that triggered it: {seen:?}"
    );

    let _ = std::fs::remove_dir_all(&foreign);
    let _ = std::fs::remove_dir_all(&foreign_index);
    let _ = std::fs::remove_dir_all(&host);
}
