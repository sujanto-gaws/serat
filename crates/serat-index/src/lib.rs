//! The SQLite index: schema, the single-writer actor, incremental upsert.
//!
//! The index is disposable (C-02). Everything here is derived from files on disk and
//! can be thrown away and rebuilt, which is what removes migrations from the project
//! entirely (ADR-0002, architecture §4.3).
//!
//! # Status
//!
//! Task I1 scaffolded this crate. The schema bootstrap from `schema/001_init.sql`,
//! the single-writer actor of ADR-0001 and the batched full rebuild arrive in task
//! I4; the incremental save path in I5; corruption detection and automatic rebuild
//! in I6.

mod apply;
pub mod check;
pub mod checkpoint;
/// Whether the `test-hooks` feature is compiled in. serat-app asserts it is not in a
/// release build, because the feature exposes `SearchConn::set_progress_handler`,
/// which can disable supersession (§4.5).
pub const TEST_HOOKS: bool = cfg!(feature = "test-hooks");

pub mod external;
pub mod history;
pub mod identity;
pub mod incremental;
pub mod lock;
pub mod open;
pub mod read;
pub mod rebuild;
mod resolve;
pub mod schema;
pub mod switcher;
pub mod tree;
pub mod writer;

pub use check::{QuickCheck, quick_check};
pub use checkpoint::{Bounds, CheckpointStats, WalHealth};
pub use identity::canonical_root;
pub use incremental::{IndexHandle, apply, parse_pool_threads};
pub use open::{IndexStatus, RebuildReason, open_or_rebuild, open_or_rebuild_observed};
pub use read::{Attachment, Backlink, IndexedStat, Reader, SearchConn, SearchHit, SearchOutcome};
pub use rebuild::{RebuildReport, index_path, rebuild, rebuild_with_queue_depth};
pub use schema::{DDL, INDEX_VERSION, IndexState, UnusableReason, bootstrap, inspect, open};
#[cfg(any(test, feature = "test-hooks"))]
pub use writer::CommitHold;
pub use writer::{
    ActorOptions, BATCH_SIZE, CommitGate, CommitObserver, CommittedBatch, DEFAULT_QUEUE_DEPTH,
    IndexActor, WriterHandle,
};

/// The version of SQLite compiled into this binary.
///
/// `bundled` means this is ours rather than the host's, so it is worth recording in
/// the index and in a bug report: FTS5 and, later, `sqlite-vec` both depend on it.
#[must_use]
pub fn sqlite_version() -> &'static str {
    rusqlite::version()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_sqlite_meets_the_feature_and_security_floors() {
        let version = sqlite_version();
        let major_minor: Vec<u32> = version
            .split('.')
            .take(2)
            .filter_map(|part| part.parse().ok())
            .collect();
        assert_eq!(major_minor.len(), 2, "unexpected version string {version}");
        assert!(
            (major_minor[0], major_minor[1]) >= (3, 44),
            "bundled sqlite is {version}; R2 needs 3.44 (ORDER BY inside json_group_array)"
        );
        assert!(
            (major_minor[0], major_minor[1]) >= (3, 54),
            "bundled sqlite is {version}; RUSTSEC-2026-0459 (NFR-SEC-01): bundled SQLite must be \u{2265} 3.54.0"
        );
    }
}
