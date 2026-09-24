//! Turning core failures into the closed set that crosses the IPC boundary.
//!
//! `contracts/ipc.rs` rule 5: `IpcError` is the only error type that crosses, and the
//! mapping from `serat_core::VaultError` is an **exhaustive match with no `_` arm**.
//! That is the whole mechanism — a new variant in the core taxonomy becomes a compile
//! error here rather than a `String` at the boundary, which is what NFR-MNT-01 is
//! asking for. It has already earned its keep once: `VaultError::NotAMarkdownTarget`
//! arrived in `serat-vault` after this file was written, and the build stopped.
//!
//! The mapping table is stated in `contracts/ipc.rs` and reproduced by
//! [`from_vault_error`]. If the two ever disagree, the contract is right.

use serat_core::{ParseError, VaultError};

use crate::ipc::IpcError;

/// Map a `VaultError` onto the boundary taxonomy.
#[must_use]
pub fn from_vault_error(error: VaultError) -> IpcError {
    match error {
        // Both are "the path may not be used", and the front end has one thing to say
        // about either. Collapsing them here is the contract's decision, not a
        // shortcut: a user does not care whether a path was rejected for spelling or
        // for where it resolved to.
        VaultError::PathEscapesVault { path } | VaultError::InvalidPath { path, .. } => {
            IpcError::PathEscapesVault { path }
        }
        VaultError::NotFound { path } => IpcError::NotFound { path },
        // Deliberately not `StaleHash`: routing a client that sent something which is
        // not 64 hex digits down §9.1's re-read-and-offer-the-choice path would show a
        // user a conflict with another writer that never happened.
        VaultError::MalformedHash { value } => IpcError::MalformedHash { value },
        VaultError::Io { path, message } => IpcError::Io { path, message },
        // An atomic write that failed left the original file intact (§6.1 step 5), so
        // there is nothing for the user to recover — it is an I/O failure with a better
        // message.
        VaultError::AtomicWriteFailed { path, message } => IpcError::Io { path, message },
        // serat-vault's own T-06 guard (P0 finding F-14). The boundary refuses this
        // first, so reaching it means something inside the application tried to write a
        // binary — which is exactly the case the two guards exist to make impossible
        // between them.
        VaultError::NotAMarkdownTarget { path } => IpcError::NotWritable { path },
        // The contract says this never reaches a command, and in this crate it cannot:
        // the watcher is started on the vault thread and its failure is published as
        // index status. The arm exists because the match is total, and mapping it to
        // something plausible is safer than an `unreachable!` that would panic the
        // command handler if that ever stopped being true.
        VaultError::Watcher { message } => IpcError::Io {
            path: String::new(),
            message,
        },
        // ADR-0016 §2's editor-save path (task #33, serat-vault::Vault::write_patch).
        // `IpcError::StaleHash` already exists for exactly this — the optimistic lock
        // failing on a plain whole-document write and on a patch set are the same
        // fact to the front end, "re-read and offer the choice" (§9.1) — so the
        // patch path reuses it rather than growing a second variant for one race.
        VaultError::StaleHash { path, found, .. } => IpcError::StaleHash {
            path,
            current_hash: found,
        },
        // No dedicated contract variant yet: `contracts/ipc.rs` has not caught up
        // with ADR-0016's patch-set save path (the IPC contract is still the one
        // piece of that design not yet merged, per the task that added this arm).
        // A malformed patch is a client bug — ranges out of order, a position past
        // a line's end — not an operating-system failure, but until the contract
        // grows its own variant this is the closest existing one and, like the
        // `Watcher` arm above, a plausible mapping is safer than leaving the match
        // non-exhaustive. Revisit when tauri-app-dev lands the patch-set IPC types.
        VaultError::MalformedPatch { path, reason } => IpcError::Io {
            path,
            message: reason,
        },
    }
}

/// Map a `ParseError` onto the boundary taxonomy.
///
/// Only `NotUtf8` can reach a P0 command — `read_file` refuses to decode lossily,
/// because a lossy read followed by a save silently rewrites the user's bytes (design
/// goal 1, C-04). `Frontmatter` is a parse diagnostic recorded against a note, not a
/// failed command, so it maps to the closest thing the boundary has and is not
/// reachable in P0.
#[must_use]
pub fn from_parse_error(error: ParseError) -> IpcError {
    match error {
        ParseError::NotUtf8 { path, .. } => IpcError::NotUtf8 { path },
        ParseError::Frontmatter { path, message } => IpcError::Io { path, message },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_vault_error_maps_as_the_contract_says() {
        // The table in contracts/ipc.rs, asserted rather than trusted. Change one arm
        // of `from_vault_error` and exactly one of these fails.
        let cases: Vec<(VaultError, IpcError)> = vec![
            (
                VaultError::PathEscapesVault {
                    path: "../etc/passwd".into(),
                },
                IpcError::PathEscapesVault {
                    path: "../etc/passwd".into(),
                },
            ),
            (
                VaultError::InvalidPath {
                    path: "C:\\x.md".into(),
                    reason: "is absolute".into(),
                },
                IpcError::PathEscapesVault {
                    path: "C:\\x.md".into(),
                },
            ),
            (
                VaultError::NotFound {
                    path: "a.md".into(),
                },
                IpcError::NotFound {
                    path: "a.md".into(),
                },
            ),
            (
                VaultError::MalformedHash {
                    value: "notahash".into(),
                },
                IpcError::MalformedHash {
                    value: "notahash".into(),
                },
            ),
            (
                VaultError::Io {
                    path: "a.md".into(),
                    message: "disk full".into(),
                },
                IpcError::Io {
                    path: "a.md".into(),
                    message: "disk full".into(),
                },
            ),
            (
                VaultError::AtomicWriteFailed {
                    path: "a.md".into(),
                    message: "rename failed".into(),
                },
                IpcError::Io {
                    path: "a.md".into(),
                    message: "rename failed".into(),
                },
            ),
            (
                VaultError::NotAMarkdownTarget {
                    path: "contract.pdf".into(),
                },
                IpcError::NotWritable {
                    path: "contract.pdf".into(),
                },
            ),
            (
                VaultError::StaleHash {
                    path: "a.md".into(),
                    expected: "aa".repeat(32),
                    found: "bb".repeat(32),
                },
                IpcError::StaleHash {
                    path: "a.md".into(),
                    current_hash: "bb".repeat(32),
                },
            ),
            (
                VaultError::MalformedPatch {
                    path: "a.md".into(),
                    reason: "edits overlap".into(),
                },
                IpcError::Io {
                    path: "a.md".into(),
                    message: "edits overlap".into(),
                },
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(
                from_vault_error(input.clone()),
                expected,
                "mapping {input:?}"
            );
        }
    }

    #[test]
    fn a_malformed_hash_is_not_reported_as_a_conflict_with_another_writer() {
        // The distinction the contract added: StaleHash sends the front end into §9.1's
        // "changed on disk, here are your two options" flow. A client that sent
        // nonsense has no conflict to resolve, and telling it there is one invents an
        // editor nobody else was using.
        let mapped = from_vault_error(VaultError::MalformedHash {
            value: "notahash".into(),
        });
        assert!(
            !matches!(mapped, IpcError::StaleHash { .. }),
            "a client bug was reported as a concurrent edit: {mapped:?}"
        );
    }

    #[test]
    fn a_non_utf8_file_is_refused_rather_than_decoded() {
        // The behaviour C-04 depends on: there is no arm here that produces text.
        assert_eq!(
            from_parse_error(ParseError::NotUtf8 {
                path: "a.md".into(),
                offset: 12,
            }),
            IpcError::NotUtf8 {
                path: "a.md".into()
            }
        );
    }

    #[test]
    fn every_ipc_error_survives_the_boundary_it_is_named_for() {
        // A variant that cannot be serialised becomes a String at the boundary, which
        // is how a typed taxonomy quietly stops being one.
        let cases = [
            IpcError::VaultNotFound {
                path: "d:/gone".into(),
            },
            IpcError::PathEscapesVault {
                path: "../x".into(),
            },
            IpcError::NotFound {
                path: "a.md".into(),
            },
            IpcError::NotWritable {
                path: "a.pdf".into(),
            },
            IpcError::NotUtf8 {
                path: "a.md".into(),
            },
            IpcError::MalformedHash { value: "zz".into() },
            IpcError::StaleHash {
                path: "a.md".into(),
                current_hash: "00".repeat(32),
            },
            IpcError::Io {
                path: "a.md".into(),
                message: "denied".into(),
            },
        ];
        for case in cases {
            let json = serde_json::to_string(&case).expect("serialise");
            assert!(json.contains("\"kind\":"), "{json}");
            let back: IpcError = serde_json::from_str(&json).expect("deserialise");
            assert_eq!(back, case);
        }
    }
}
