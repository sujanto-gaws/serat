//! Opening an index, and rebuilding it when it cannot be used.
//!
//! Architecture §3.4 and ADR-0002. There is no repair path: every way an index can be
//! unusable has the same answer, because everything in it is derived from files on
//! disk. That is not a simplification of a recovery strategy — it is the absence of
//! one, and it is what makes NFR-REL-02 a property rather than a feature.

use std::path::Path;

use serat_core::{IndexError, NoteParser, NoteSource};

use crate::incremental::IndexHandle;
use crate::rebuild::{self, RebuildReport};
use crate::schema::{self, IndexState, UnusableReason};

/// Why the index is being rebuilt.
///
/// The same six outcomes §3.4 names and `contracts/ipc.rs` carries, deliberately, so
/// the open check and the UI cannot drift. serat-index may not link the contracts
/// crate (§2.1), so `serat-app` maps between them — and because that mapping is the
/// one place they can diverge, it is where an exhaustive match earns its keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildReason {
    /// No index file. First open of this vault, or someone deleted `.serat/`.
    NoIndex,
    /// The file is not a database, or SQLite reported corruption.
    Corrupt,
    /// Built by a different schema version (ADR-0002: no migration, rebuild).
    SchemaChanged,
    /// A previous rebuild did not finish.
    Interrupted,
    /// Built from a different vault.
    VaultMoved,
}

impl RebuildReason {
    fn from(reason: UnusableReason) -> Self {
        match reason {
            UnusableReason::IndexVersion => Self::SchemaChanged,
            UnusableReason::RebuildInterrupted => Self::Interrupted,
            UnusableReason::VaultMoved => Self::VaultMoved,
            // A file that will not open and one whose counts disagree are the same
            // outcome to a user and the same action to us. §3.4 folds them together
            // rather than offering a distinction nobody can act on.
            UnusableReason::Corrupt
            | UnusableReason::CountMismatch
            | UnusableReason::Unopenable => Self::Corrupt,
        }
    }
}

/// What the index is doing, published as it happens.
///
/// # This is a state, not an event
///
/// §3.4 makes "the index state is known" one of the four terms of `t_usable`, and a
/// consumer establishing that term must read it from here rather than from the
/// throttled `index.changed` stream (§3.1 step 6). A `Rebuilding` whose *event* lost to
/// the throttle would leave that consumer waiting for a transition that had already
/// happened — and nothing would look like a bug, because the mark simply never fires.
///
/// Throttled events are for invalidation. They may never be what establishes that
/// something has occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexStatus {
    /// The open check is running. The only state that does not settle `t_usable`.
    Opening,
    /// The index could not be used and is being rebuilt.
    Rebuilding {
        /// Why.
        reason: RebuildReason,
    },
    /// Usable.
    Ready,
    /// A rebuild failed, and there will not be another this session.
    Unavailable,
}

/// Open the index at `index_dir`, rebuilding it from `source` if it cannot be used.
///
/// # Publishing happens before the rebuild, not after
///
/// `on_status` is called with a non-[`IndexStatus::Opening`] state as soon as the check
/// produces one, and only then does any rebuild work start. That ordering is a
/// requirement rather than a nicety: §3.4's `t_usable` waits for the index state to be
/// *known*, and `Rebuilding` settles that exactly as `Ready` does. Publishing after the
/// rebuild would make the window's budget include a full reindex, so on the vault where
/// the window most needs to be up, the application would report itself unusable for
/// thirty seconds.
///
/// # There is no retry
///
/// A rebuild that fails leaves [`IndexStatus::Unavailable`] and does not try again this
/// session, for the same reason §7.1 never re-extracts a malformed binary: it will fail
/// again, and an indexer spinning on a vault the user cannot fix is worse than one that
/// stops and says so.
///
/// # Errors
///
/// [`IndexError`] only if the index cannot be opened *after* a successful rebuild,
/// which is a genuine failure rather than a state. Every recoverable condition is a
/// status, not an error.
pub fn open_or_rebuild(
    vault_root: &Path,
    index_dir: &Path,
    source: &dyn NoteSource,
    parser: &dyn NoteParser,
    on_status: &dyn Fn(IndexStatus),
) -> Result<(IndexHandle, Option<RebuildReport>), IndexError> {
    on_status(IndexStatus::Opening);

    let path = rebuild::index_path(index_dir);
    let root = vault_root.to_string_lossy().into_owned();

    let reason = match schema::inspect_for(&path, Some(&root)) {
        Ok(IndexState::Ready) => None,
        Ok(IndexState::Absent) => Some(RebuildReason::NoIndex),
        Ok(IndexState::Unusable { reason }) => Some(RebuildReason::from(reason)),
        // The check itself failing is not a reason to give up on the vault; it is one
        // more way of learning the index cannot be trusted.
        Err(_) => Some(RebuildReason::Corrupt),
    };

    let Some(reason) = reason else {
        on_status(IndexStatus::Ready);
        return Ok((IndexHandle::open(index_dir)?, None));
    };

    // Published before the rebuild starts. Everything below this line can take thirty
    // seconds; nothing above it may.
    on_status(IndexStatus::Rebuilding { reason });

    match rebuild::rebuild(source, parser, vault_root, index_dir) {
        Ok(report) => {
            on_status(IndexStatus::Ready);
            Ok((IndexHandle::open(index_dir)?, Some(report)))
        }
        Err(error) => {
            on_status(IndexStatus::Unavailable);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_unusable_reason_maps_to_a_rebuild_reason() {
        // The mapping is exhaustive by construction — the match has no wildcard — so
        // this pins the two foldings that are judgements rather than translations.
        assert_eq!(
            RebuildReason::from(UnusableReason::CountMismatch),
            RebuildReason::Corrupt,
            "a torn transaction is corruption to a user, whatever it is to SQLite"
        );
        assert_eq!(
            RebuildReason::from(UnusableReason::Unopenable),
            RebuildReason::Corrupt
        );
        assert_eq!(
            RebuildReason::from(UnusableReason::IndexVersion),
            RebuildReason::SchemaChanged
        );
        assert_eq!(
            RebuildReason::from(UnusableReason::VaultMoved),
            RebuildReason::VaultMoved
        );
    }
}
