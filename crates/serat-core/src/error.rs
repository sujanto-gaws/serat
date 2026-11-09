//! The error taxonomy.
//!
//! One enum per crate boundary, and one enum that wraps them for callers that
//! straddle several. Two rules shape these types:
//!
//! * **They carry no `std::io::Error`.** Errors reach the UI over IPC (ADR-0007),
//!   and a variant that cannot be serialised becomes a `String` at the boundary,
//!   which is how a typed error taxonomy quietly stops being one. `VaultError::io`
//!   converts at the point the I/O happens, where the context is still available.
//! * **Refusals are not errors.** A stale write-back and a file with no extractor
//!   are expected outcomes with their own return types (architecture §6.1, §7.3).
//!   Only genuine failures live here.

use serde::{Deserialize, Serialize};

/// Failures from the filesystem layer: walking, reading, watching, writing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VaultError {
    /// A path resolved outside the vault root. Refused before any read.
    #[error("path escapes the vault root: {path}")]
    PathEscapesVault {
        /// The offending path, as given.
        path: String,
    },

    /// A path could not be represented as a vault path.
    #[error("invalid vault path {path}: {reason}")]
    InvalidPath {
        /// The offending path, as given.
        path: String,
        /// Why it was rejected.
        reason: String,
    },

    /// A hash was not 64 hex digits.
    #[error("malformed content hash: {value}")]
    MalformedHash {
        /// The text that failed to parse.
        value: String,
    },

    /// The file is gone. Ordinary during a rebuild racing a delete.
    #[error("no such file in vault: {path}")]
    NotFound {
        /// Vault-relative path.
        path: String,
    },

    /// An underlying I/O failure, flattened at the call site.
    #[error("io error on {path}: {message}")]
    Io {
        /// Vault-relative path, or the best available description of the target.
        path: String,
        /// The operating system's message.
        message: String,
    },

    /// The temp-file-and-rename write did not complete.
    ///
    /// The invariant this protects is that a reader never observes a half-written
    /// note (design goal 1); a failure here means the original file is intact.
    #[error("atomic write failed for {path}: {message}")]
    AtomicWriteFailed {
        /// Vault-relative path of the intended target.
        path: String,
        /// What went wrong.
        message: String,
    },

    /// A write was aimed at something that is not a Markdown note.
    ///
    /// Threat T-06 and FR-BIN-19: binaries are read, extracted, annotated and
    /// queried — never rewritten. The mechanism the threat model names is "no write
    /// path accepts a non-Markdown target", so the refusal lives in the one function
    /// that writes rather than in each of its callers.
    #[error("refusing to write {path}: only Markdown notes are written, never binaries")]
    NotAMarkdownTarget {
        /// Vault-relative path of the refused target.
        path: String,
    },

    /// The filesystem watcher could not be started, or it died.
    #[error("watcher failure: {message}")]
    Watcher {
        /// What went wrong.
        message: String,
    },

    /// The optimistic lock failed: the file's hash was not the caller's `base_hash`.
    ///
    /// ADR-0016 §2: raised both when the pre-patch read disagrees with `base_hash`
    /// and when the immediate-before-rename re-hash finds the target moved during
    /// the splice. The two cases share one variant because the caller's response is
    /// the same either way — refuse, discard the temp file, call
    /// `request_external_diff` — and distinguishing them would let a caller branch
    /// on a distinction ADR-0016 says does not matter.
    #[error("stale hash for {path}: expected {expected}, found {found}")]
    StaleHash {
        /// Vault-relative path of the target.
        path: String,
        /// The hash the caller supplied.
        expected: String,
        /// The hash actually on disk.
        found: String,
    },

    /// A patch set could not be applied to its base.
    ///
    /// Distinct from `StaleHash`: the hash matched, but the patch itself is
    /// malformed against that base — a position past the end of a line, ranges out
    /// of order, or ranges that overlap (ADR-0016 §2 step 2). A client bug, not a
    /// race, so it is reported rather than silently clamped or reordered.
    #[error("malformed patch for {path}: {reason}")]
    MalformedPatch {
        /// Vault-relative path of the target.
        path: String,
        /// Why the patch was refused.
        reason: String,
    },

    /// A patch is well formed but applying it would fuse a lone `\r` and a `\n` of a
    /// mixed-ending note into one `\r\n`, swallowing a line (ADR-0016 §2).
    ///
    /// Not a client bug and not a race: the user made a legitimate edit and the only
    /// byte-preserving outcome is to refuse it. The file is untouched.
    #[error("saving {path} would fuse two line endings at line {line}")]
    LineEndingsWouldFuse {
        /// Vault-relative path of the target.
        path: String,
        /// 0-based line, in base coordinates, of the `\r` that would fuse.
        line: u32,
    },
}

impl VaultError {
    /// Flatten a `std::io::Error` at the point it occurred.
    ///
    /// Referring to the type is not performing I/O; this crate still opens nothing.
    #[must_use]
    pub fn io(path: impl Into<String>, error: &std::io::Error) -> Self {
        let path = path.into();
        if error.kind() == std::io::ErrorKind::NotFound {
            Self::NotFound { path }
        } else {
            Self::Io {
                path,
                message: error.to_string(),
            }
        }
    }
}

/// Failures from turning bytes into structures.
///
/// Deliberately short. Malformed Markdown is not an error — CommonMark has no
/// parse failures, and design goal 5 says an unsupported input must not stop the
/// vault indexing. Only inputs we genuinely cannot represent appear here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParseError {
    /// The bytes are not valid UTF-8.
    #[error("{path} is not valid utf-8 at byte {offset}")]
    NotUtf8 {
        /// Vault-relative path.
        path: String,
        /// Byte offset of the first invalid sequence.
        offset: usize,
    },

    /// YAML frontmatter was present but could not be read.
    ///
    /// The note still indexes: its body parses normally and this is recorded
    /// against it, because a typo in one note's frontmatter must not remove that
    /// note from search.
    #[error("frontmatter in {path}: {message}")]
    Frontmatter {
        /// Vault-relative path.
        path: String,
        /// What went wrong.
        message: String,
    },
}

/// Failures from the index.
///
/// `SchemaMismatch` and `Corrupt` are the two the cold-start path treats as
/// "drop it and rebuild" rather than as fatal (ADR-0002, NFR-REL-02).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IndexError {
    /// SQLite said no.
    #[error("sqlite: {message}")]
    Sqlite {
        /// The message from SQLite.
        message: String,
    },

    /// The database on disk was built by a different schema version.
    ///
    /// Not a migration trigger: there are no migrations (ADR-0002). The index is
    /// dropped and rebuilt.
    #[error("index schema version {found} does not match {expected}; rebuilding")]
    SchemaMismatch {
        /// Version found on disk.
        found: i64,
        /// Version this build expects.
        expected: i64,
    },

    /// The database failed an integrity check, or could not be opened at all.
    #[error("index is corrupt: {message}")]
    Corrupt {
        /// What was detected.
        message: String,
    },

    /// The writer thread is gone, so no further writes can be accepted.
    ///
    /// Under ADR-0001 this is the only way a write fails for a reason that is not
    /// SQLite's: there is one owner, so there is no lock to fail to take.
    #[error("index writer has stopped")]
    WriterStopped,

    /// The bounded work queue is full.
    ///
    /// Backpressure, not a fault: the caller waits or drops. Visible queue depth is
    /// the point of the single-writer design (ADR-0001, consequences).
    #[error("index write queue is full ({depth} queued)")]
    QueueFull {
        /// Depth at the moment of refusal.
        depth: usize,
    },
}

/// Anything that can go wrong in the core pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "layer", rename_all = "snake_case")]
pub enum SeratError {
    /// See [`VaultError`].
    #[error(transparent)]
    Vault(#[from] VaultError),
    /// See [`ParseError`].
    #[error(transparent)]
    Parse(#[from] ParseError),
    /// See [`IndexError`].
    #[error(transparent)]
    Index(#[from] IndexError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_not_found_narrows_to_not_found() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "nope");
        assert!(matches!(
            VaultError::io("10-projects/a.md", &err),
            VaultError::NotFound { .. }
        ));
    }

    #[test]
    fn every_error_survives_the_ipc_boundary() {
        // The property that matters: no variant holds a type that cannot be
        // serialised, so the UI sees the taxonomy and not a string.
        let cases = [
            SeratError::from(VaultError::Io {
                path: "a.md".into(),
                message: "disk full".into(),
            }),
            SeratError::from(ParseError::NotUtf8 {
                path: "a.md".into(),
                offset: 12,
            }),
            SeratError::from(IndexError::SchemaMismatch {
                found: 1,
                expected: 2,
            }),
            SeratError::from(IndexError::QueueFull { depth: 4096 }),
        ];
        for case in cases {
            let json = serde_json::to_string(&case).expect("serialise");
            let back: SeratError = serde_json::from_str(&json).expect("deserialise");
            assert_eq!(back, case);
        }
    }
}
