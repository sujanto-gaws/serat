//! Row identities assigned by the index.
//!
//! These are `INTEGER PRIMARY KEY` values from `schema/001_init.sql`, so they are
//! `i64` and they are meaningless outside the index that issued them. Nothing may
//! persist one in the vault: the index is disposable (C-02) and a rebuild reissues
//! every id.

use serde::{Deserialize, Serialize};

macro_rules! row_id {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl $name {
            /// The underlying SQLite rowid.
            #[must_use]
            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl From<i64> for $name {
            fn from(value: i64) -> Self {
                Self(value)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

row_id!(NoteId, "Identity of a row in `notes`.");
row_id!(BlockId, "Identity of a row in `blocks`.");
row_id!(
    FieldId,
    "Identity of a row in `fields`.\n\nThis is the `__fid` a view carries so an edited cell can be written back to the byte range it came from (architecture §5.2, ADR-0008)."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_of_different_kinds_are_not_interchangeable() {
        // The point of the newtypes: this file would not compile if a NoteId
        // could be passed where a BlockId is expected. What is testable is the
        // round trip.
        let note = NoteId::from(7);
        assert_eq!(note.get(), 7);
        assert_eq!(note.to_string(), "7");
        assert_eq!(serde_json::to_string(&note).expect("serialise"), "7");
    }
}
