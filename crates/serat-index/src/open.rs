//! Opening an index, and rebuilding it when it cannot be used.
//!
//! Architecture §3.4 and ADR-0002. There is no repair path: every way an index can be
//! unusable has the same answer, because everything in it is derived from files on
//! disk. That is not a simplification of a recovery strategy — it is the absence of
//! one, and it is what makes NFR-REL-02 a property rather than a feature.

use std::path::Path;

use serat_core::{IndexError, NoteParser, NoteSource};

use crate::identity::{self, Verdict};
use crate::incremental::IndexHandle;
use crate::rebuild::{self, RebuildReport};
use crate::schema::{self, IndexState, UnusableReason};
use crate::writer::{CommitObserver, DEFAULT_QUEUE_DEPTH};

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
    /// The index's own rows describe a tree that is not here.
    ///
    /// A `.serat/` copied between vaults, in practice. Renamed from `VaultMoved` on
    /// task #120, because a vault that *moved* is the one case in which the old name's
    /// stated rationale -- "every path in it describes files that are not there" -- is
    /// false: `notes.path` is vault-relative, so a moved vault's index is still exactly
    /// right. The name now states the condition rather than the one case where it does
    /// not hold, and the trigger is [`identity::describes`]'s finding rather than a
    /// path comparison (F-459).
    ForeignVault,
}

impl RebuildReason {
    fn from(reason: UnusableReason) -> Self {
        match reason {
            UnusableReason::IndexVersion => Self::SchemaChanged,
            UnusableReason::RebuildInterrupted => Self::Interrupted,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexStatus {
    /// The open check is running. The only state that does not settle `t_usable`.
    Opening,
    /// The index was recorded under a different root, and this open adopted it.
    ///
    /// Not a fault and not a rebuild (§3.4 step 5, task #120): `notes.path` is
    /// vault-relative, so an index recorded elsewhere can describe this tree exactly,
    /// and [`identity::describes`] confirmed that it does. The new root is written back
    /// so the next open is an ordinary one.
    ///
    /// Published rather than swallowed because F-459's real complaint is silence: a
    /// vault whose identity changed and a vault whose index was thrown away looked the
    /// same from outside, and NFR-REL-02's log could tell neither from corruption.
    ///
    /// Like [`IndexStatus::Opening`] this does **not** settle `t_usable`: it is
    /// information about an open still in progress, and a terminal state follows it.
    RootAdopted {
        /// What the index said it was built from. `None` if the column was null.
        recorded: Option<String>,
        /// The canonical root it is recorded under now.
        adopted: String,
    },
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
    open_or_rebuild_observed(vault_root, index_dir, source, parser, on_status, None)
}

/// As [`open_or_rebuild`], with a [`CommitObserver`] installed on the handle's writer
/// (ADR-0020 decision 3). The rebuild itself reports nothing: its commits are inside
/// the bulk-load bracket. The observer is carried by the handle, so one made later by
/// [`IndexHandle::rebuild_in_place`] reports to it too.
///
/// # Errors
///
/// As [`open_or_rebuild`].
pub fn open_or_rebuild_observed(
    vault_root: &Path,
    index_dir: &Path,
    source: &dyn NoteSource,
    parser: &dyn NoteParser,
    on_status: &dyn Fn(IndexStatus),
    commit_observer: Option<CommitObserver>,
) -> Result<(IndexHandle, Option<RebuildReport>), IndexError> {
    on_status(IndexStatus::Opening);

    // Task #154: one writer per vault across processes. Taken before the check, the
    // rebuild and the handle open, and kept by the handle. A holder elsewhere means
    // this session has no index; the caller reports it unavailable.
    let lock = match crate::lock::IndexLock::acquire(index_dir) {
        Ok(lock) => lock,
        Err(error) => {
            on_status(IndexStatus::Unavailable);
            return Err(error);
        }
    };

    let path = rebuild::index_path(index_dir);
    // The resolved directory, not the bytes of the argument. `serat-app` reaches this
    // root through `Vault::open(..).root()` and the benchmark harness through a
    // `CARGO_MANIFEST_DIR` join; those are one identity now by construction rather than
    // by coincidence, which is F-458's lesson (its own fix was the same line in
    // `bench/`).
    let root = identity::canonical_root(vault_root);

    let reason = match schema::inspect_for(&path, Some(&root)) {
        Ok(IndexState::Ready) => None,
        Ok(IndexState::Absent) => Some(RebuildReason::NoIndex),
        Ok(IndexState::RootChanged { recorded }) => {
            adopt_or_reject(&path, vault_root, &root, recorded, on_status)
        }
        Ok(IndexState::Unusable { reason }) => Some(RebuildReason::from(reason)),
        // The check itself failing is not a reason to give up on the vault; it is one
        // more way of learning the index cannot be trusted.
        Err(_) => Some(RebuildReason::Corrupt),
    };

    let Some(reason) = reason else {
        on_status(IndexStatus::Ready);
        return Ok((
            IndexHandle::open_locked(index_dir, DEFAULT_QUEUE_DEPTH, lock, commit_observer)?,
            None,
        ));
    };

    // Published before the rebuild starts. Everything below this line can take thirty
    // seconds; nothing above it may.
    on_status(IndexStatus::Rebuilding { reason });

    match rebuild::rebuild_locked(
        source,
        parser,
        vault_root,
        index_dir,
        DEFAULT_QUEUE_DEPTH,
        &lock,
    ) {
        Ok(report) => {
            on_status(IndexStatus::Ready);
            Ok((
                IndexHandle::open_locked(index_dir, DEFAULT_QUEUE_DEPTH, lock, commit_observer)?,
                Some(report),
            ))
        }
        Err(error) => {
            on_status(IndexStatus::Unavailable);
            Err(error)
        }
    }
}

/// Decide what a changed `index_meta.vault_root` means, and act on it.
///
/// # Why this is not simply "report it and carry on"
///
/// Task #120's ruling demoted the mismatch from a rebuild trigger to a reported signal
/// and said to fall through to *"the reconciliation the open path already performs"*.
/// It performs none: nothing consumes `serat_vault::walk::paths_of`, and §3.4 puts
/// catch-up reconciliation in the *after usable* column, where nothing is built. Taken
/// literally the demotion would serve a genuinely foreign index as `Ready` for the life
/// of the session, with no mechanism that could ever notice -- the ruling's own
/// accepted cost, *"reconciliation has to discover that every file differs"*, being a
/// cost nothing is currently paying.
///
/// So the trigger moves to reconciliation's *finding* rather than to reconciliation,
/// which is the repair that ruling named for exactly this objection.
/// [`identity::describes`] asks the one question the trigger needs -- does any row in
/// this index name a file that is here -- over a bounded sample. A path comparison
/// never triggers a rebuild again; evidence about the index's contents does.
///
/// Returns `None` when the index is kept, having reported and recorded the new root.
fn adopt_or_reject(
    path: &Path,
    vault_root: &Path,
    canonical: &str,
    recorded: Option<String>,
    on_status: &dyn Fn(IndexStatus),
) -> Option<RebuildReason> {
    let Ok(conn) = schema::open(path) else {
        // Unreachable in practice -- `inspect_for` has just read this file -- but a
        // database that became unopenable between the two is the stamp's `Corrupt`
        // case, not an identity question.
        return Some(RebuildReason::Corrupt);
    };

    if identity::describes(&conn, vault_root) == Verdict::Foreign {
        return Some(RebuildReason::ForeignVault);
    }

    // Adopt: record the new identity so the next open is an ordinary one, and say so.
    //
    // Written on a bare connection rather than through the writer actor, which does not
    // exist yet: `open_or_rebuild` has not created the `IndexHandle` and nothing else
    // in this process holds the file. ADR-0001's single-writer rule is about the actor
    // being the only writer while it runs, and this is the same window in which
    // `rebuild` deletes and recreates the whole database.
    //
    // A failed update is not fatal and does not become one. The index is correct either
    // way and the only cost is re-adopting at the next open; turning a good open into a
    // 28.6 s rebuild because one row would not take an UPDATE is precisely the trade
    // task #120 exists to stop making.
    let written = conn
        .execute(
            "UPDATE index_meta SET vault_root = ?1 WHERE id = 1",
            rusqlite::params![canonical],
        )
        .is_ok();
    drop(conn);

    if written {
        on_status(IndexStatus::RootAdopted {
            recorded,
            adopted: canonical.to_owned(),
        });
    }
    None
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
    }

    #[test]
    fn the_stamp_can_no_longer_reach_the_foreign_vault_reason() {
        // Task #120. `UnusableReason` is the stamp's *fault* set and a changed root is
        // not a fault, so the identity question left this enumeration rather than being
        // renamed inside it. The match above is exhaustive by construction; this is the
        // half a compiler cannot state -- that no fault the stamp can produce reaches
        // `ForeignVault`, which is now reachable only from `adopt_or_reject`, only on
        // evidence about the index's own rows. `vault_identity.rs` pins that path.
        for reason in [
            UnusableReason::IndexVersion,
            UnusableReason::RebuildInterrupted,
            UnusableReason::Corrupt,
            UnusableReason::CountMismatch,
            UnusableReason::Unopenable,
        ] {
            assert_ne!(
                RebuildReason::from(reason),
                RebuildReason::ForeignVault,
                "{reason:?} reached ForeignVault without anything looking at the vault"
            );
        }
    }
}
