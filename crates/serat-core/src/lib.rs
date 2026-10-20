//! Domain types, trait definitions and the error taxonomy shared by every Serat crate.
//!
//! `serat-core` is the one crate every other crate may depend on, and it performs
//! **no I/O whatsoever** (`docs/03-design/architecture.md` §2.1). That is not
//! decoration: it is what lets `serat-parse` be tested without a filesystem,
//! `serat-index` be tested without a parser, and later `serat-sq` typecheck a query
//! against a schema without linking a database.
//!
//! The rule is checked mechanically by `tests/dependency_rule.rs`, which fails if
//! this crate grows an internal dependency or an external one outside its
//! allow-list. Adding `std::fs` here is not caught by that test — the allow-list is
//! — so the honest statement of the rule is: nothing in this crate opens, reads,
//! writes or watches anything.
//!
//! # Scope
//!
//! P0 only (`docs/01-planning/tasks-p0-spike.md`). These are the structures
//! `serat-parse` produces and `serat-index` consumes, plus the two traits that keep
//! those crates from depending on each other. Schema registry types, SQ types and
//! extraction types arrive in P2 with the phases that need them.

pub mod block;
pub mod error;
pub mod field;
pub mod fold;
pub mod hash;
pub mod ids;
pub mod link;
pub mod note;
pub mod path;
pub mod span;
pub mod task;
pub mod traits;

pub use block::{Block, BlockKind};
pub use error::{IndexError, ParseError, SeratError, VaultError};
pub use field::{Field, FieldKind, FieldSource, FieldValue};
pub use fold::fold_case;
pub use hash::ContentHash;
pub use ids::{BlockId, FieldId, NoteId};
pub use link::{Link, LinkKind, LinkTarget};
pub use note::{IndexOp, LoadedNote, NoteKind, NoteMeta, ParsedNote, SourceEntry};
pub use path::{Platform, VaultPath};
pub use span::ByteRange;
pub use task::Task;
pub use traits::{EchoGuard, IndexWriter, NoteParser, NoteSource};
