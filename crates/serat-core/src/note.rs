//! Notes and the message the pipeline passes between its stages.

use serde::{Deserialize, Serialize};

use crate::{
    block::Block, field::Field, hash::ContentHash, link::Link, path::VaultPath, task::Task,
};

/// Whether a vault file is parsed or extracted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    /// A Markdown note. Parsed by `serat-parse`.
    Note,
    /// Anything else: a PDF, a spreadsheet, an image. Its blocks come from an
    /// extractor in P2 (architecture §7); until then it indexes with its path and
    /// its sidecar and nothing more, which is design goal 5 — degrade, do not break.
    Asset,
}

/// What `serat-vault` knows about a file without looking inside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteMeta {
    /// Vault-relative path.
    pub path: VaultPath,
    /// BLAKE3 of the bytes. The hash gate compares this against `notes.hash` and
    /// discards the event when they match (architecture §3.1 step 3), which is what
    /// makes a sync tool touching mtime cost a hash instead of a parse.
    pub hash: ContentHash,
    /// Note or asset.
    pub kind: NoteKind,
    /// Size in bytes.
    pub size: u64,
    /// Modification time, milliseconds since the Unix epoch.
    ///
    /// Recorded for display and for ordering, never for change detection: mtime is
    /// unreliable across sync tools and filesystems, which is the whole reason the
    /// hash gate exists.
    pub mtime_ms: i64,
}

/// What `serat-parse` produces from one note's bytes.
///
/// Everything here is anchored by byte range into the bytes it was parsed from, and
/// is only valid for those bytes — which is why the meta's hash travels with it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParsedNote {
    /// The note's title: its first level-1 heading, or its `title` field, or its
    /// filename stem. Resolving that order is the parser's job.
    pub title: Option<String>,
    /// The value of the `type` field, which is what a typed query's outer scan
    /// filters on (`ix_notes_type`, architecture §4.4). `None` for an untyped note,
    /// and an untyped note is always valid.
    pub note_type: Option<String>,
    /// Blocks in document order.
    pub blocks: Vec<Block>,
    /// Fields from frontmatter and from the body.
    pub fields: Vec<Field>,
    /// Links and embeds.
    pub links: Vec<Link>,
    /// Tasks.
    pub tasks: Vec<Task>,
}

/// One unit of work for the index writer.
///
/// This is the message on ADR-0001's channel, and the reason `serat-vault` does not
/// depend on `serat-index`: the pipeline produces values of this type and hands them
/// to something implementing [`IndexWriter`](crate::traits::IndexWriter), which in a
/// test is a `Vec` and in the application is the actor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum IndexOp {
    /// A note whose content changed, already parsed. Replaces every row for that
    /// path: blocks are identified by position, so a partial update would have to
    /// diff them, and re-inserting a note's rows inside the batch transaction is
    /// cheaper than being clever.
    UpsertNote {
        /// File identity and hash.
        meta: NoteMeta,
        /// Parse output for exactly those bytes.
        parsed: Box<ParsedNote>,
    },
    /// An asset: recorded, not parsed.
    UpsertAsset {
        /// File identity and hash.
        meta: NoteMeta,
    },
    /// The file is gone.
    Remove {
        /// Vault-relative path.
        path: VaultPath,
    },
    /// The file moved. Distinct from remove-then-insert so backlinks survive the
    /// move rather than being deleted and rediscovered.
    Rename {
        /// Where it was.
        from: VaultPath,
        /// Where it is.
        to: VaultPath,
    },
}

impl IndexOp {
    /// The path this operation concerns; for a rename, the destination.
    #[must_use]
    pub fn path(&self) -> &VaultPath {
        match self {
            Self::UpsertNote { meta, .. } | Self::UpsertAsset { meta } => &meta.path,
            Self::Remove { path } => path,
            Self::Rename { to, .. } => to,
        }
    }
}

/// A file's bytes together with the identity of exactly those bytes.
///
/// The pair travels together because a byte range is only meaningful against the
/// bytes it was computed from: handing a caller bytes without the hash is how a parse
/// result ends up attributed to a file that has since changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedNote {
    /// Identity, hash and stat data.
    pub meta: NoteMeta,
    /// The bytes, exactly as they are on disk. No decoding and no line-ending
    /// translation: the parser records byte offsets into this.
    pub bytes: Vec<u8>,
}

/// One indexable file, as a [`NoteSource`](crate::NoteSource) found it.
///
/// No hash: hashing means reading every byte, and enumerating a vault should not cost
/// what indexing it does. The rebuild reads and hashes as it goes.
///
/// It carries `kind`, `size` and `mtime_ms` because the walk already had them —
/// `read_dir` returns the metadata — and discarding them would make the rebuild
/// re-derive the kind from the extension and re-stat 20,000 files it has already
/// statted, inside a 30 s budget (F-29b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEntry {
    /// Vault-relative path.
    pub path: VaultPath,
    /// Whether this file is parsed or extracted.
    pub kind: NoteKind,
    /// Size in bytes.
    pub size: u64,
    /// Modification time, milliseconds since the Unix epoch. For display and
    /// ordering only — change detection is the hash gate's job.
    pub mtime_ms: i64,
}
