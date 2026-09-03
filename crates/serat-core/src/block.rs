//! Blocks: the unit Serat indexes.
//!
//! Serat indexes at block level rather than note level (architecture §4.1). Block
//! references, precise AI citations, writable views and binary extraction all fall
//! out of one table of blocks anchored by byte range; under note-level indexing each
//! of them would need its own machinery.

use serde::{Deserialize, Serialize};

use crate::span::ByteRange;

/// What kind of block this is.
///
/// Coarser than CommonMark's node set on purpose: the index needs to tell apart the
/// things a query or a citation distinguishes, not to reconstruct a document. The
/// document is on disk; this is a lookup structure over it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlockKind {
    /// YAML frontmatter. Exactly one, first, when present.
    Frontmatter,
    /// A heading. Also what builds every following block's heading path.
    Heading {
        /// 1 for `#` through 6 for `######`.
        level: u8,
    },
    /// An ordinary paragraph.
    Paragraph,
    /// A list item that is not a task.
    ListItem {
        /// Nesting depth, 0 for a top-level item.
        depth: u8,
    },
    /// A list item carrying a checkbox. Its detail is in [`Task`](crate::Task).
    Task {
        /// Nesting depth, 0 for a top-level item.
        depth: u8,
    },
    /// A fenced or indented code block.
    Code {
        /// The info string of a fenced block, lowercased: `rust`, `sq`, and so on.
        language: Option<String>,
    },
    /// A block quote.
    Quote,
    /// A GFM table.
    Table,
    /// A thematic break.
    Rule,
    /// Raw HTML.
    Html,
}

/// One indexed block of a note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    /// Position in document order, starting at 0. Stable for a given byte sequence,
    /// which is what lets fields, links and tasks refer to a block before the index
    /// has assigned it an id.
    pub ord: u32,
    /// What kind of block this is.
    pub kind: BlockKind,
    /// The block's extent in the note's bytes, including its marker.
    pub range: ByteRange,
    /// Plain text of the block, with inline markup removed. This is what goes to
    /// full-text search and, later, to the embedder.
    pub text: String,
    /// Headings above this block, outermost first. Prepended as context when the
    /// block is chunked for embedding (architecture §8.1).
    pub heading_path: Vec<String>,
    /// The `^anchor` at the end of the block, if the author wrote one. This is what
    /// a block reference resolves to.
    pub block_ref: Option<String>,
}

impl Block {
    /// The block's bytes, or `None` if the range does not fit `bytes`.
    #[must_use]
    pub fn bytes<'a>(&self, bytes: &'a [u8]) -> Option<&'a [u8]> {
        self.range.slice(bytes)
    }

    /// The heading path joined for display and for embedding context.
    #[must_use]
    pub fn heading_context(&self) -> String {
        self.heading_path.join(" › ")
    }
}
