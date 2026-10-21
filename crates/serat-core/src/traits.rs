//! The traits that keep the crate graph acyclic.
//!
//! Cross-crate needs are expressed as traits declared here and implemented wherever
//! the capability lives (architecture §2.1). Three exist in P0, and each earns its
//! place by removing a dependency edge that would otherwise be real:
//!
//! * [`NoteParser`] lets the indexing pipeline be driven by a stub parser in a test,
//!   so `serat-index` never links `serat-parse`.
//! * [`IndexWriter`] lets `serat-vault` hand work to the index actor without
//!   depending on `serat-index`; in a test the implementation is a `Vec`.
//! * [`EchoGuard`] lets the save path in `serat-index` tell the watcher which hash to
//!   drop, without `serat-index` depending on `serat-vault` for it.
//!
//! Traits for the schema registry, the query API and extraction arrive with the
//! phases that need them. Declaring them now would be guessing.

use crate::{
    error::{IndexError, ParseError, VaultError},
    hash::ContentHash,
    note::{IndexOp, LoadedNote, ParsedNote, SourceEntry},
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
    /// Returns the entry rather than the bare path so the rebuild does not re-derive
    /// what the walk already knew: `read_dir` yields metadata, and throwing it away
    /// costs 20,000 redundant stats inside a 30 s budget (F-29b).
    fn entries(&self) -> Result<Vec<SourceEntry>, VaultError>;

    /// Read one file and hash it.
    ///
    /// # Errors
    ///
    /// [`VaultError::NotFound`] if it has gone — ordinary during a rebuild racing a
    /// delete, and the caller's cue to skip it rather than to fail.
    fn load(&self, path: &VaultPath) -> Result<LoadedNote, VaultError>;
}

/// Tells the watcher which hash is our own write, so it drops the event instead of
/// reindexing what the save path just indexed itself (architecture §3.2).
///
/// A separate one-method trait rather than a fourth method on [`NoteSource`] (F-210,
/// ruled against the ruler's own convenience). `NoteSource` means "somewhere notes can
/// be enumerated and read from"; `record_echo` is write-path bookkeeping with no
/// relationship to that, and every stub `NoteSource` a rebuild is driven by exists to
/// enumerate and read, never to produce an echo — a fourth method would oblige each of
/// them to say something about a concern they do not have. The decomposition also
/// removes a question the fourth-method version could not: `rebuild` takes only
/// `&dyn NoteSource` and is never handed an `EchoGuard` at all, so there is nothing to
/// default and no silent no-op can occur.
///
/// Implemented by `serat_vault::Vault`, alongside `NoteSource`; the save path's
/// signature takes both as separate trait objects rather than one type that happens to
/// implement both (F-218) — `serat-app`'s call site passing `Vault` twice is the thing
/// worth seeing there, not the thing to hide behind a struct invented to shorten it.
pub trait EchoGuard: Send + Sync {
    /// Record a hash the watcher should recognise as our own write and drop.
    ///
    /// Called **after** a submit has been accepted, not before (architecture §3.2,
    /// amended by F-226). The original design recorded the hash before the write, on
    /// the reasoning that a fast filesystem can deliver the watcher event before a
    /// later call would have returned — true, but it means a write recorded and then
    /// refused downstream (`IndexError::QueueFull`) permanently suppresses the only
    /// event that would ever have indexed the note (F-196, F-222). Recording late
    /// removes the race rather than narrowing it (F-224): a refused submit records
    /// nothing here, so the watcher's own event carries the save instead — redundant
    /// with nothing to redo, since a later index write is idempotent by path — and a
    /// watcher event that wins a race against this call merely indexes the same note a
    /// moment early, for the same reason.
    fn record_echo(&self, path: &VaultPath, hash: ContentHash);
}
