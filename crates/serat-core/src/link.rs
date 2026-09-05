//! Links between notes.

use serde::{Deserialize, Serialize};

use crate::span::ByteRange;

/// How the link was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// `[[Note]]`.
    Wikilink,
    /// `![[Note]]` — transcluded rather than referenced.
    Embed,
    /// `[text](path.md)`.
    Markdown,
    /// `#tag`. Names no note and never resolves to one — `dst_note` stays NULL by
    /// construction (schema/001_init.sql's CHECK on this), not because resolution
    /// failed.
    Tag,
}

/// What a link points at, before resolution.
///
/// Unresolved on purpose: resolution needs the whole vault (which file does
/// `[[Meeting]]` mean?), and parsing is pure. `serat-index` resolves against the
/// notes table; an unresolved link is still recorded, because a link to a note that
/// does not exist yet is how people write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkTarget {
    /// The note part, as written: a name, a partial path or a full path.
    pub note: String,
    /// The `#heading` part.
    pub heading: Option<String>,
    /// The `#^block-ref` part.
    pub block_ref: Option<String>,
    /// The `|alias` part, or a Markdown link's text.
    pub alias: Option<String>,
}

impl LinkTarget {
    /// A bare link to a note.
    #[must_use]
    pub fn note(note: impl Into<String>) -> Self {
        Self {
            note: note.into(),
            heading: None,
            block_ref: None,
            alias: None,
        }
    }

    /// What the reader sees.
    #[must_use]
    pub fn display_text(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.note)
    }
}

/// One link occurrence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// Wikilink, embed or Markdown link.
    pub kind: LinkKind,
    /// Where it points.
    pub target: LinkTarget,
    /// The whole link occurrence in the source bytes. A rename rewrites exactly
    /// these bytes, which is what makes promote-across-tiers a byte-range patch
    /// rather than a re-serialisation (ADR-0008).
    pub range: ByteRange,
    /// The block the link was found in.
    pub block_ord: Option<u32>,
}
