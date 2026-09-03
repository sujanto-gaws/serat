//! The traits that keep the crate graph acyclic.
//!
//! Cross-crate needs are expressed as traits declared here and implemented wherever
//! the capability lives (architecture §2.1). Two exist in P0, and both earn their
//! place by removing a dependency edge that would otherwise be real:
//!
//! * [`NoteParser`] lets the indexing pipeline be driven by a stub parser in a test,
//!   so `serat-index` never links `serat-parse`.
//! * [`IndexWriter`] lets `serat-vault` hand work to the index actor without
//!   depending on `serat-index`; in a test the implementation is a `Vec`.
//!
//! Traits for the schema registry, the query API and extraction arrive with the
//! phases that need them. Declaring them now would be guessing.

use crate::{
    error::{IndexError, ParseError, VaultError},
    note::{IndexOp, LoadedNote, ParsedNote},
    path::VaultPath,
};

/// Turns a note's bytes into structures.
///
/// `Send + Sync` because the implementation runs on the rayon pool over cores minus
/// one (architecture §2.2). It is required to be pure: no file access, no shared
/// mutable state, no ordering assumptions — which is exactly what lets the pool
/// parallelise it without coordination.
pub trait NoteParser: Send + Sync {
    /// Parse `bytes` as the note at `path`.
    ///
    /// `path` is context for diagnostics and for the filename-derived title. The
    /// parser must not read it.
    ///
    /// # Errors
    ///
    /// [`ParseError`] only for input that cannot be represented at all. Malformed
    /// Markdown is not an error: the note still indexes.
    fn parse(&self, path: &VaultPath, bytes: &[u8]) -> Result<ParsedNote, ParseError>;
}

/// Accepts index mutations.
///
/// Under ADR-0001 the real implementation is a channel to the one thread owning the
/// only write connection, so `submit` returns as soon as the message is queued and
/// says nothing about when it is applied. Callers that need to know a batch landed
/// use [`IndexWriter::flush`].
///
/// The trait is deliberately narrow. Anything richer — a handle you can query, a
/// transaction you can hold open — would be a shared connection wearing a different
/// hat, which is the design this ADR rejected.
pub trait IndexWriter: Send + Sync {
    /// Queue one operation.
    ///
    /// # Errors
    ///
    /// [`IndexError::QueueFull`] when the bounded queue is full, which is
    /// backpressure and not a fault; [`IndexError::WriterStopped`] when the writer
    /// thread is gone.
    fn submit(&self, op: IndexOp) -> Result<(), IndexError>;

    /// Queue several operations. The default submits them one at a time; an
    /// implementation that can enqueue a whole batch under one lock should.
    ///
    /// # Errors
    ///
    /// As [`IndexWriter::submit`]. A failure part-way leaves earlier operations
    /// queued — the writer applies whatever it received, and the caller retries the
    /// rest.
    fn submit_all(&self, ops: Vec<IndexOp>) -> Result<(), IndexError> {
        for op in ops {
            self.submit(op)?;
        }
        Ok(())
    }

    /// Block until everything queued before this call has been applied.
    ///
    /// For tests, for shutdown, and for the rebuild path that runs `ANALYZE` after
    /// the last batch. Not for the save path: waiting on the writer there would put
    /// the index back on the keystroke path, which design goal 2 forbids.
    ///
    /// # Errors
    ///
    /// [`IndexError::WriterStopped`] if the writer died before draining.
    fn flush(&self) -> Result<(), IndexError>;
}

/// Somewhere notes can be enumerated and read from.
///
/// Implemented by `serat_vault::Vault`. It exists so that a full rebuild — which must
/// walk, read, parse and write — can be written at all: no product crate may take two
/// of those capabilities as dependencies (architecture §2.1), so the walk and the read
/// arrive as a trait object and the parse as [`NoteParser`].
///
/// # Provisional
///
/// The trait is settled; where the function that consumes it lives is not. It sits in
/// `serat-index` on the lead's interim ruling, because the signature is what
/// test-engineer's budget harness, T1 and I5 bind to, while moving the function to a
/// `serat-pipeline` crate later is a file move and an import change. Architect has the
/// ruling on the home. Do not read the current location as settled.
pub trait NoteSource: Send + Sync {
    /// Every indexable file, in a deterministic order.
    ///
    /// Deterministic because a rebuild that visits files in a different order each
    /// time makes "delete the index, rebuild, get the same answers" (FR-VLT-02) harder
    /// to assert than it needs to be.
    ///
    /// # Errors
    ///
    /// [`VaultError`] only if the source itself cannot be enumerated. An individual
    /// unreadable directory is skipped, not fatal: one bad folder must not stop a
    /// vault indexing (design goal 5).
    fn entries(&self) -> Result<Vec<VaultPath>, VaultError>;

    /// Read one file and hash it.
    ///
    /// # Errors
    ///
    /// [`VaultError::NotFound`] if it has gone — ordinary during a rebuild racing a
    /// delete, and the caller's cue to skip it rather than to fail.
    fn load(&self, path: &VaultPath) -> Result<LoadedNote, VaultError>;
}
