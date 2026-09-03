//! Fields: the typed key/value pairs that make a vault queryable.

use serde::{Deserialize, Serialize};

use crate::span::ByteRange;

/// Where a field was written.
///
/// Both forms are ordinary Markdown, which is C-01. Obsidian vaults use both, so
/// both are read; nothing in Serat requires either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldSource {
    /// A key in the YAML frontmatter block.
    Frontmatter,
    /// An inline `key:: value` in the body.
    Inline,
}

/// Which typed column in `fields` a value occupies.
///
/// The index writes into `val_text`, `val_num`, `val_date` or `val_bool`, chosen at
/// index time, so the query compiler knows at plan time which column a comparison
/// touches and a date range becomes an indexed scan rather than a per-row string
/// parse (architecture §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    /// `val_text`.
    Text,
    /// `val_num`.
    Num,
    /// `val_date`, ISO-8601.
    Date,
    /// `val_bool`.
    Bool,
    /// Several rows with the same key, one per element.
    List,
}

/// A field's value.
///
/// In P0 the kind is inferred from the literal, because the schema registry that
/// declares field types is P2 (FR-STR-01). When it lands, the declared type wins and
/// inference becomes the fallback for untyped notes — which is the "structure is
/// opt-in, then enforced" principle, in the one place it has teeth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum FieldValue {
    /// Anything that is not one of the others.
    Text(String),
    /// A number. `f64` because SQLite's `REAL` is one, so a narrower type here would
    /// only move the lossy conversion somewhere less visible.
    Num(f64),
    /// A date or timestamp, ISO-8601, exactly as it will be written to `val_date`.
    /// Held as a string because SQLite has no date type and comparisons are made on
    /// the lexicographic ordering that ISO-8601 guarantees.
    Date(String),
    /// A boolean.
    Bool(bool),
    /// A list. Stored as one row per element, which is why a list-valued field is
    /// filtered with `EXISTS` rather than joined (architecture §5.2).
    List(Vec<FieldValue>),
}

impl FieldValue {
    /// Which column this value occupies.
    #[must_use]
    pub const fn kind(&self) -> FieldKind {
        match self {
            Self::Text(_) => FieldKind::Text,
            Self::Num(_) => FieldKind::Num,
            Self::Date(_) => FieldKind::Date,
            Self::Bool(_) => FieldKind::Bool,
            Self::List(_) => FieldKind::List,
        }
    }

    /// The value as text, for display and for `val_text`.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(value) | Self::Date(value) => Some(value),
            _ => None,
        }
    }
}

/// One field occurrence in one note.
///
/// A field carries two ranges because they answer different questions. `range` is
/// what the parser consumed, so a re-parse can find it again; `value_range` is what
/// the write-back patcher replaces, and nothing outside it may change (C-04,
/// ADR-0008, NFR-MNT-03). Recording both at parse time costs nothing and is the only
/// moment the information is available without parsing again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    /// The key, as written. Case is preserved; matching is the schema's business.
    pub key: String,
    /// The parsed value.
    pub value: FieldValue,
    /// Frontmatter or inline.
    pub source: FieldSource,
    /// The whole `key: value` occurrence.
    pub range: ByteRange,
    /// Just the value, which is the only thing a write-back may replace.
    pub value_range: ByteRange,
    /// The block the field was found in, for an inline field. `None` for
    /// frontmatter, which belongs to the note rather than to a block.
    pub block_ord: Option<u32>,
}

impl Field {
    /// Whether the recorded value range still holds `expected` in `bytes`.
    ///
    /// This is architecture §6.1 step 3 — the check that turns a moved range into
    /// `Refused{RangeMoved}` instead of a corrupted file. It lives here so the
    /// patcher and its property test share one definition of "still there".
    #[must_use]
    pub fn value_range_still_holds(&self, bytes: &[u8], expected: &str) -> bool {
        self.value_range.slice(bytes) == Some(expected.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_kind_picks_the_column() {
        assert_eq!(FieldValue::Num(8.0).kind(), FieldKind::Num);
        assert_eq!(
            FieldValue::Date("2026-09-03".into()).kind(),
            FieldKind::Date
        );
        assert_eq!(FieldValue::Bool(false).kind(), FieldKind::Bool);
        assert_eq!(
            FieldValue::List(vec![FieldValue::Text("pm".into())]).kind(),
            FieldKind::List
        );
    }

    #[test]
    fn a_moved_value_range_is_detectable() {
        let before = b"status:: active\n";
        let field = Field {
            key: "status".into(),
            value: FieldValue::Text("active".into()),
            source: FieldSource::Inline,
            range: ByteRange::new(0, 15),
            value_range: ByteRange::new(9, 15),
            block_ord: Some(0),
        };
        assert!(field.value_range_still_holds(before, "active"));

        // Someone inserted a line above; the range now points at other bytes.
        let after = b"owner:: sujan\nstatus:: active\n";
        assert!(!field.value_range_still_holds(after, "active"));
    }
}
