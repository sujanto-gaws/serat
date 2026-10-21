//! The switcher corpus as the index loads it (architecture §9.2; FR-NAV-01, US-02).
//!
//! Serves **FR-NAV-01**. The corpus comes from three fixed-shape reads over the delivered
//! schema; this runs them against it, with rows shaped as serat-parse emits them.

use rusqlite::Connection;
use serat_index::history::History;
use serat_index::schema;
use serat_index::switcher::{Corpus, CorpusSource, Matched};

fn fixture() -> Connection {
    let conn = schema::open_in_memory().expect("open");
    schema::bootstrap(&conn).expect("bootstrap");
    conn.execute_batch(
        "INSERT INTO notes(id, path, title, kind, mtime, hash) VALUES
           (1, 'projects/Roadmap.md', 'Product roadmap', 'note', 0, 'h'),
           (2, 'daily/2026-08-12.md', '2026-08-12', 'note', 0, 'h'),
           (3, 'refs/report.pdf', 'report.pdf', 'asset', 0, 'h'),
           (4, 'archive/Roadmap.md', NULL, 'note', 0, 'h');
         INSERT INTO blocks(id, note_id, ord, kind, text, byte_start, byte_end) VALUES
           (10, 1, 0, 'heading', 'Q3 goals', 40, 50),
           (11, 1, 1, 'para', 'not a heading', 51, 70),
           (12, 1, 2, 'heading', 'Risks', 71, 80),
           (13, 2, 0, 'heading', 'Plan', 0, 6);
         INSERT INTO fields(note_id, key, val_text, source) VALUES
           (1, 'aliases', 'RM', 'frontmatter'),
           (1, 'alias', 'Plan of record', 'frontmatter'),
           (1, 'aliases', NULL, 'frontmatter'),
           (2, 'status', 'RM', 'frontmatter');
         INSERT INTO fields(note_id, block_id, key, val_text, source) VALUES
           (1, 11, 'aliases', 'inline is not an alias', 'inline');",
    )
    .expect("fixture");
    conn
}

#[test]
fn the_corpus_holds_notes_aliases_and_headings_and_not_assets_or_inline_fields() {
    let c = Corpus::from_connection(&fixture()).expect("corpus");
    assert_eq!(c.source(), CorpusSource::Index);
    assert_eq!(c.len(), 3, "three notes, no asset");
    assert_eq!(c.alias_count(), 2);
    assert_eq!(c.heading_count(), 3);
    assert!(c.query("report", 10, &History::default(), 0).is_empty());
    assert!(c.query("inline", 10, &History::default(), 0).is_empty());
}

#[test]
fn titles_aliases_paths_and_headings_are_all_reachable() {
    let c = Corpus::from_connection(&fixture()).expect("corpus");
    let h = History::default();

    let hits = c.query("roadmap", 10, &h, 0);
    assert_eq!(hits.len(), 2);
    // The archive note has no title: it falls back to its file name, an exact match.
    assert_eq!(hits[0].path, "archive/Roadmap.md");
    assert_eq!(
        hits[0].link_text, "archive/Roadmap",
        "two notes share the stem"
    );

    let hits = c.query("rm", 10, &h, 0);
    assert_eq!(hits[0].path, "projects/Roadmap.md");
    assert_eq!(hits[0].matched, Matched::Alias);
    assert_eq!(hits[0].candidate, "RM");

    let hits = c.query("plan of", 10, &h, 0);
    assert_eq!(hits[0].matched, Matched::Alias);

    let hits = c.query("#risk", 10, &h, 0);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].heading, Some(("Risks".to_owned(), 71)));

    let hits = c.query("2026#plan", 10, &h, 0);
    assert_eq!(hits[0].path, "daily/2026-08-12.md");
    assert_eq!(hits[0].link_text, "2026-08-12");
}
