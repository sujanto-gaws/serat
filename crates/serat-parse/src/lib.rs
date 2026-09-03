//! Markdown bytes in, structures out.
//!
//! This crate is pure: it takes bytes and returns
//! [`ParsedNote`](serat_core::ParsedNote), and it opens nothing (architecture §2.1,
//! §3.1 step 4). That is what lets the parse pool run it across cores minus one with
//! no coordination at all, and what makes a parser test a string literal rather than
//! a fixture directory.
//!
//! # Status
//!
//! Task I1 scaffolded this crate. The comrak-based implementation of
//! [`NoteParser`](serat_core::NoteParser) — blocks with byte ranges, YAML
//! frontmatter and inline `key:: value` fields, wikilinks and embeds, tasks —
//! arrives in task I3 (NFR-PRF-04).
