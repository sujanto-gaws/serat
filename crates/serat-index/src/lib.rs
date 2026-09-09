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
pub mod identity;
pub mod incremental;
pub mod open;
pub mod rebuild;
pub mod schema;
pub mod writer;

pub use identity::canonical_root;
pub use incremental::{IndexHandle, apply, parse_pool_threads};
pub use open::{IndexStatus, RebuildReason, open_or_rebuild};
pub use rebuild::{RebuildReport, index_path, rebuild, rebuild_with_queue_depth};
pub use schema::{DDL, INDEX_VERSION, IndexState, UnusableReason, bootstrap, inspect, open};
pub use writer::{BATCH_SIZE, DEFAULT_QUEUE_DEPTH, IndexActor, WriterHandle};

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
    fn the_bundled_sqlite_is_new_enough_for_fts5() {
        let version = sqlite_version();
        let major_minor: Vec<u32> = version
            .split('.')
            .take(2)
            .filter_map(|part| part.parse().ok())
            .collect();
        assert_eq!(major_minor.len(), 2, "unexpected version string {version}");
        assert!(
            (major_minor[0], major_minor[1]) >= (3, 40),
            "bundled sqlite is {version}; the index needs 3.40 or newer"
        );
    }
}
