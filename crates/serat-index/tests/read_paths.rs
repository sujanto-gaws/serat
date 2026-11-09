//! The P1 read paths R1-R4 (task #15; FR-NAV-02, FR-NAV-04, FR-STR-03, FR-BIN-02;
//! architecture §4.5).

mod support;

use std::path::Path;

use rusqlite::params;
use serat_index::read::{HitKind, ParsedQuery, STATEMENTS, parse_query};
use serat_index::{Reader, SearchOutcome, index_path, rebuild, schema};
use support::{FakeVault, LineParser};

struct Fixture {
    _dir: tempfile::TempDir,
    reader: Reader,
    db: std::path::PathBuf,
}

fn fixture(files: &[(&str, &str)]) -> Fixture {
    let dir = tempfile::tempdir().expect("temp");
    rebuild(
        &FakeVault::with(files),
        &LineParser::default(),
        Path::new("/vault"),
        dir.path(),
    )
    .expect("rebuild");
    let db = index_path(dir.path());
    let reader = Reader::open(&db).expect("reader");
    Fixture {
        _dir: dir,
        reader,
        db,
    }
}

impl Fixture {
    /// Add a frontmatter field row, which the line-based test parser cannot make.
    fn frontmatter(&self, note: &str, key: &str, value: &str) {
        let conn = schema::open(&self.db).expect("writer");
        conn.execute(
            "INSERT INTO fields (note_id, block_id, key, val_text, source)
             VALUES ((SELECT id FROM notes WHERE path = ?1), NULL, ?2, ?3, 'frontmatter')",
            params![note, key, value],
        )
        .expect("insert field");
    }

    fn sidecar(&self, asset: &str, sidecar: &str) {
        let conn = schema::open(&self.db).expect("writer");
        conn.execute(
            "UPDATE notes SET sidecar_path = ?2 WHERE path = ?1",
            params![asset, sidecar],
        )
        .expect("set sidecar");
    }

    fn hits(&self, query: &str) -> Vec<serat_index::SearchHit> {
        match self.reader.search(query, 0, 50).expect("search") {
            SearchOutcome::Hits(h) => h,
            other => panic!("{query:?}: {other:?}"),
        }
    }
}

// --------------------------------------------------------------------- parsing

#[test]
fn the_query_parser_quotes_every_word_and_keeps_phrases() {
    assert_eq!(
        parse_query("alpha beta"),
        ParsedQuery::Terms {
            fts: Some("\"alpha\" \"beta\"".into()),
            property: None
        }
    );
    assert_eq!(
        parse_query("say \"hello world\" now"),
        ParsedQuery::Terms {
            fts: Some("\"say\" \"hello world\" \"now\"".into()),
            property: None
        }
    );
    // Operators and syntax are just words.
    let ParsedQuery::Terms { fts: Some(fts), .. } = parse_query("a AND b NEAR( c* -d") else {
        panic!()
    };
    assert_eq!(fts, "\"a\" \"AND\" \"b\" \"NEAR(\" \"c*\" \"-d\"");
}

#[test]
fn a_property_term_is_pulled_out_and_at_most_one_is_allowed() {
    assert_eq!(
        parse_query("budget [status:in progress]"),
        ParsedQuery::Terms {
            fts: Some("\"budget\"".into()),
            property: Some(("status".into(), "in progress".into()))
        }
    );
    assert_eq!(
        parse_query("[tags:x]"),
        ParsedQuery::Terms {
            fts: None,
            property: Some(("tags".into(), "x".into()))
        }
    );
    assert_eq!(
        parse_query("[a:b] [c:d]"),
        ParsedQuery::TooManyPropertyTerms
    );
    // A bracket that is not a property term is punctuation; a bracket in a phrase is text.
    assert_eq!(
        parse_query("[x] [y:] \"[k:v] inside\""),
        ParsedQuery::Terms {
            fts: Some("\"x\" \"y:\" \"[k:v] inside\"".into()),
            property: None
        }
    );
}

#[test]
fn degenerate_queries_are_outcomes_not_errors() {
    assert_eq!(parse_query("say \"open"), ParsedQuery::UnbalancedQuote);
    assert_eq!(parse_query("\""), ParsedQuery::UnbalancedQuote);
    for nothing in ["", "   ", "!!! ... ---", "\"\"", "\"  ?  \"", "[a:b"] {
        let ParsedQuery::Terms { fts, property } = parse_query(nothing) else {
            panic!("{nothing:?}")
        };
        assert!(
            fts.is_none() && property.is_none() || nothing == "[a:b",
            "{nothing:?}: {fts:?} {property:?}"
        );
    }
}

#[test]
fn an_unbalanced_quote_runs_no_statement() {
    // A connection with no tables at all: any statement would be an error.
    let reader = Reader::from_connection(rusqlite::Connection::open_in_memory().expect("mem"));
    assert_eq!(
        reader.search("oops \"unclosed", 0, 10).expect("outcome"),
        SearchOutcome::UnbalancedQuote
    );
    assert_eq!(
        reader.search("  ", 0, 10).expect("outcome"),
        SearchOutcome::NothingToSearch
    );
    assert_eq!(
        reader.search("[a:b] [c:d]", 0, 10).expect("outcome"),
        SearchOutcome::TooManyPropertyTerms
    );
}

// ------------------------------------------------------------------------- R1

fn corpus() -> Fixture {
    fixture(&[
        (
            "10/serat.md",
            "# Serat\ntype:: project\nthe reconciler is idempotent\nsecond reconciler line\n",
        ),
        (
            "10/other.md",
            "# Other\ntype:: Contract\nnothing relevant\n",
        ),
        (
            "20/doc.md",
            "# Doc\nreconciler reconciler reconciler\ntype:: contract\n",
        ),
        ("40/x.pdf", "%PDF reconciler"),
    ])
}

#[test]
fn r1_returns_ranked_hits_with_a_marked_snippet_and_a_block_location() {
    let f = corpus();
    let hits = f.hits("reconciler");
    assert_eq!(hits.len(), 3, "{hits:#?}");
    assert!(hits.iter().all(|h| h.kind == HitKind::Note));
    let first = &hits[0];
    let snippet = first.snippet.as_deref().expect("snippet");
    assert!(
        snippet.contains('\u{2}') && snippet.contains('\u{3}'),
        "{snippet:?}"
    );
    assert!(!snippet.contains('<'), "no markup: {snippet:?}");
    let block = first.block.as_ref().expect("block");
    assert!(block.byte_end >= block.byte_start);
    assert_eq!(block.note_hash.len(), 64.min(block.note_hash.len()));
    // Never an asset's blocks.
    assert!(hits.iter().all(|h| !h.path.ends_with(".pdf")));
    // Best first: BM25 rank is lower for a better match, and every hit carries it.
    let ranks: Vec<f64> = hits.iter().map(|h| h.rank.expect("rank")).collect();
    assert!(ranks.windows(2).all(|w| w[0] <= w[1]), "{ranks:?}");
}

#[test]
fn r1_treats_every_character_as_text() {
    let f = corpus();
    for query in [
        "reconciler AND",
        "NEAR(",
        "reconciler*",
        "-reconciler",
        "reconciler OR (",
        "col:umn",
        "'",
        "reconciler ^",
    ] {
        let outcome = f.reader.search(query, 0, 50).expect(query);
        assert!(
            matches!(
                outcome,
                SearchOutcome::Hits(_) | SearchOutcome::NothingToSearch
            ),
            "{query}: {outcome:?}"
        );
    }
    assert_eq!(f.reader.fts_rejections(), 0);
    // A phrase is a phrase.
    assert_eq!(f.hits("\"second reconciler\"").len(), 1);
    assert_eq!(f.hits("\"reconciler second\"").len(), 0);
}

#[test]
fn r1_pages_with_offset_and_limit_and_caps_the_limit_at_fifty() {
    let files: Vec<(String, String)> = (0..70)
        .map(|i| {
            (
                format!("n/{i:02}.md"),
                format!("# T{i}\ncommonword here {i}\n"),
            )
        })
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let f = fixture(&refs);
    let SearchOutcome::Hits(all) = f.reader.search("commonword", 0, 500).unwrap() else {
        panic!()
    };
    assert_eq!(all.len(), 50, "the limit is capped");
    let SearchOutcome::Hits(page) = f.reader.search("commonword", 10, 5).unwrap() else {
        panic!()
    };
    assert_eq!(page.len(), 5);
    assert_eq!(
        page[0].path, all[10].path,
        "offset skips hits in rank order"
    );
}

#[test]
fn r3_alone_returns_up_to_five_hundred_in_one_statement_and_clamps_beyond() {
    let files: Vec<(String, String)> = (0..620)
        .map(|i| (format!("n/{i:03}.md"), format!("# T{i}\ncommonword {i}\n")))
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let f = fixture(&refs);
    let conn = schema::open(&f.db).expect("writer");
    conn.execute(
        "INSERT INTO fields (note_id, block_id, key, val_text, source)
         SELECT id, NULL, 'status', 'active', 'frontmatter' FROM notes",
        [],
    )
    .expect("fields");
    let len = |q: &str, limit: u16| match f.reader.search(q, 0, limit).unwrap() {
        SearchOutcome::Hits(h) => h.len(),
        other => panic!("{other:?}"),
    };
    assert_eq!(len("[status:active]", 500), 500);
    assert_eq!(
        len("[status:active]", u16::MAX),
        500,
        "clamped, not rejected"
    );
    assert_eq!(len("[status:active]", 120), 120);
    // Free text keeps R1's ceiling even beside a property term.
    assert_eq!(len("commonword [status:active]", 500), 50);
    assert_eq!(len("commonword", 500), 50);
}

// ------------------------------------------------------------------------- R3

#[test]
fn r3_matches_frontmatter_values_ignoring_case_and_returns_nothing_for_inline_fields() {
    let f = corpus();
    f.frontmatter("10/serat.md", "status", "Active");
    f.frontmatter("20/doc.md", "status", "done");
    f.frontmatter("20/doc.md", "tags", "alpha");
    f.frontmatter("20/doc.md", "tags", "Beta");
    let paths = |q: &str| -> Vec<String> { f.hits(q).into_iter().map(|h| h.path).collect() };
    assert_eq!(paths("[status:active]"), vec!["10/serat.md"]);
    assert_eq!(paths("[status:ACTIVE]"), vec!["10/serat.md"]);
    assert_eq!(
        paths("[tags:beta]"),
        vec!["20/doc.md"],
        "a list item matches"
    );
    assert_eq!(paths("[tags:alpha]"), vec!["20/doc.md"]);
    // Keys compare exactly.
    assert!(paths("[Status:active]").is_empty());
    // `type::` written inline in the fixture is an inline field: not a frontmatter match
    // for a non-type key.
    assert!(paths("[owner:x]").is_empty());
    // Property-only hits carry no block and no snippet.
    let hit = &f.hits("[status:done]")[0];
    assert!(hit.block.is_none() && hit.snippet.is_none());
}

#[test]
fn r4_the_type_filter_is_exact_case() {
    let f = corpus();
    let paths = |q: &str| -> Vec<String> { f.hits(q).into_iter().map(|h| h.path).collect() };
    assert_eq!(paths("[type:Contract]"), vec!["10/other.md"]);
    assert_eq!(paths("[type:contract]"), vec!["20/doc.md"]);
    assert_eq!(paths("[type:project]"), vec!["10/serat.md"]);
}

#[test]
fn a_sidecar_hit_is_returned_as_its_asset() {
    // FR-BIN-02: "contracts" queried by a frontmatter field returns the PDF as a typed row.
    let f = fixture(&[
        ("40/contract.pdf", "%PDF"),
        ("40/contract.pdf.md", "# Contract\ntype:: contract\n"),
        ("40/plain.md", "# Plain\n"),
    ]);
    f.sidecar("40/contract.pdf", "40/contract.pdf.md");
    f.frontmatter("40/contract.pdf.md", "client", "Acme");
    f.frontmatter("40/plain.md", "client", "Acme");
    let hits = f.hits("[client:acme]");
    assert_eq!(hits.len(), 2);
    let asset = hits
        .iter()
        .find(|h| h.kind == HitKind::Asset)
        .expect("asset hit");
    assert_eq!(asset.path, "40/contract.pdf");
    assert_eq!(asset.sidecar_path.as_deref(), Some("40/contract.pdf.md"));
    assert_eq!(asset.mime.as_deref(), Some("application/pdf"));
    let note = hits
        .iter()
        .find(|h| h.kind == HitKind::Note)
        .expect("note hit");
    assert_eq!(note.path, "40/plain.md");
    assert!(note.sidecar_path.is_none() && note.mime.is_none());
    // The type filter reaches the asset too.
    let typed = f.hits("[type:contract]");
    assert_eq!(typed.len(), 1);
    assert_eq!(typed[0].path, "40/contract.pdf");
}

#[test]
fn r1_plus_r3_filters_full_text_by_a_property() {
    let f = corpus();
    f.frontmatter("20/doc.md", "status", "done");
    let hits = f.hits("reconciler [status:DONE]");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "20/doc.md");
    assert!(hits[0].snippet.is_some());
    let by_type = f.hits("reconciler [type:project]");
    // serat.md has two blocks containing the word: hits are blocks.
    assert_eq!(by_type.len(), 2);
    assert!(by_type.iter().all(|h| h.path == "10/serat.md"));
}

// ------------------------------------------------------------------------- R2

#[test]
fn r2_lists_one_entry_per_distinct_source_block() {
    // FR-NAV-04's acceptance case: three paragraphs across two notes give three entries,
    // and two links in one paragraph are one.
    let f = fixture(&[
        ("t/Target.md", "# Target\n"),
        ("a.md", "# A\n[[Target]]\nfiller\n[[Target]]\n"),
        ("b.md", "# B\n[[Target]]\n"),
        ("c.md", "# C\nnothing\n"),
    ]);
    let links = f.reader.backlinks("t/Target.md").expect("backlinks");
    assert_eq!(links.len(), 3, "{links:#?}");
    let order: Vec<_> = links.iter().map(|l| l.source_path.as_str()).collect();
    assert_eq!(
        order,
        vec!["a.md", "a.md", "b.md"],
        "ordered by path then block"
    );
    assert!(links[0].block.as_ref().unwrap().ord < links[1].block.as_ref().unwrap().ord);
    assert_eq!(links[0].excerpt, "[[Target]]");
}

#[test]
fn r2_collapses_two_links_in_one_block_and_a_frontmatter_link_is_one_blockless_entry() {
    let f = fixture(&[("Target.md", "# T\n"), ("a.md", "# A\n")]);
    {
        let conn = schema::open(&f.db).expect("writer");
        conn.execute_batch(
            "INSERT INTO blocks (note_id, ord, kind, text, byte_start, byte_end)
               VALUES ((SELECT id FROM notes WHERE path='a.md'), 9, 'para', 'two links here', 0, 5);
             INSERT INTO links (src_note, src_block, dst_path, dst_note, kind)
               SELECT s.id, b.id, 'Target', t.id, 'wiki'
                 FROM notes s, notes t, blocks b
                WHERE s.path='a.md' AND t.path='Target.md' AND b.ord = 9
               UNION ALL
               SELECT s.id, b.id, 'Target.md', t.id, 'md'
                 FROM notes s, notes t, blocks b
                WHERE s.path='a.md' AND t.path='Target.md' AND b.ord = 9
               UNION ALL
               SELECT s.id, NULL, 'Target', t.id, 'wiki'
                 FROM notes s, notes t WHERE s.path='a.md' AND t.path='Target.md'
               UNION ALL
               SELECT s.id, NULL, 'Target', t.id, 'wiki'
                 FROM notes s, notes t WHERE s.path='a.md' AND t.path='Target.md';",
        )
        .expect("seed links");
    }
    let links = f.reader.backlinks("Target.md").expect("backlinks");
    assert_eq!(links.len(), 2, "{links:#?}");
    let blockless = links
        .iter()
        .find(|l| l.block.is_none())
        .expect("frontmatter");
    assert_eq!(blockless.excerpt, "");
    assert!(links.iter().any(|l| l.excerpt == "two links here"));
}

#[test]
fn r2_aggregates_distinct_frontmatter_keys_ordered_and_none_when_all_null() {
    // Task #86: links.field_key -> Backlink::frontmatter_keys.
    let f = fixture(&[
        ("Target.md", "# T\n"),
        ("a.md", "# A\n"),
        ("b.md", "# B\n"),
        ("c.md", "# C\n"),
    ]);
    {
        let conn = schema::open(&f.db).expect("writer");
        conn.execute_batch(
            "INSERT INTO blocks (note_id, ord, kind, text, byte_start, byte_end)
               VALUES ((SELECT id FROM notes WHERE path='a.md'), 9, 'para', 'body', 0, 4);
             INSERT INTO links (src_note, src_block, dst_path, dst_note, kind, field_key)
               SELECT s.id, NULL, 'Target', t.id, 'wiki', k.key
                 FROM notes s, notes t,
                      (SELECT 'b' AS key UNION ALL SELECT 'a' UNION ALL SELECT 'a') k
                WHERE s.path='a.md' AND t.path='Target.md'
               UNION ALL
               SELECT s.id, b.id, 'Target', t.id, 'wiki', NULL
                 FROM notes s, notes t, blocks b
                WHERE s.path='a.md' AND t.path='Target.md' AND b.ord = 9
               UNION ALL
               SELECT s.id, NULL, 'Target', t.id, 'wiki', NULL
                 FROM notes s, notes t WHERE s.path='b.md' AND t.path='Target.md';",
        )
        .expect("seed links");
    }
    let links = f.reader.backlinks("Target.md").expect("backlinks");
    assert_eq!(links.len(), 3, "{links:#?}");
    // a.md: body block (ord 9) sorts after the NULL-block group? order is path, b.ord.
    let keyed = links
        .iter()
        .find(|l| l.source_path == "a.md" && l.block.is_none())
        .expect("a frontmatter group");
    assert_eq!(
        keyed.frontmatter_keys,
        Some(vec!["a".to_owned(), "b".to_owned()])
    );
    let body = links
        .iter()
        .find(|l| l.source_path == "a.md" && l.block.is_some())
        .expect("a body group");
    assert_eq!(body.frontmatter_keys, None);
    let all_null = links
        .iter()
        .find(|l| l.source_path == "b.md")
        .expect("b group");
    assert_eq!(all_null.frontmatter_keys, None);
}

#[test]
fn r2_omits_unresolved_links_and_follows_the_resolvers_choice_for_ambiguous_names() {
    let f = fixture(&[
        ("x/Meeting.md", "# long\n"),
        ("Meeting.md", "# short\n"),
        ("src.md", "# S\n[[Meeting]]\n[[Ghost]]\n"),
    ]);
    assert_eq!(f.reader.backlinks("Meeting.md").unwrap().len(), 1);
    assert!(f.reader.backlinks("x/Meeting.md").unwrap().is_empty());
    assert!(f.reader.backlinks("Ghost.md").unwrap().is_empty());
    assert!(f.reader.backlinks("no/such.md").unwrap().is_empty());
}

#[test]
fn r2_cuts_the_excerpt_to_three_hundred_characters() {
    let long = "é".repeat(1000);
    let f = fixture(&[("Target.md", "# T\n"), ("a.md", "# A\n[[Target]]\n")]);
    schema::open(&f.db)
        .expect("writer")
        .execute(
            "UPDATE blocks SET text = ?1 WHERE text = '[[Target]]'",
            [&long],
        )
        .expect("lengthen");
    let links = f.reader.backlinks("Target.md").unwrap();
    assert_eq!(links[0].excerpt.chars().count(), 300);
}

// ----------------------------------------------------------------------- plans

#[test]
fn every_statement_uses_the_index_it_was_designed_for() {
    // Populated and analysed, because an empty table's plan proves nothing about a
    // real one.
    let mut files: Vec<(String, String)> = (0..300)
        .map(|i| {
            (
                format!("n/{i:03}.md"),
                format!(
                    "# T{i}\ntype:: {}\nword{} commonword\n[[n/{:03}]]\n",
                    ["a", "b", "c"][i % 3],
                    i,
                    (i + 1) % 300
                ),
            )
        })
        .collect();
    files.push(("n/asset.pdf".to_owned(), "%PDF".to_owned()));
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let f = fixture(&refs);
    f.frontmatter("n/001.md", "status", "x");
    f.sidecar("n/asset.pdf", "n/001.md");
    let conn = schema::open(&f.db).expect("conn");
    conn.execute_batch("ANALYZE").expect("analyze");

    let plan_of = |name: &str| -> String {
        let sql = STATEMENTS.iter().find(|(n, _)| *n == name).expect(name).1;
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .expect("prepare");
        let names: Vec<String> = (1..=stmt.parameter_count())
            .map(|i| stmt.parameter_name(i).unwrap_or("").to_owned())
            .collect();
        let bound: Vec<&dyn rusqlite::ToSql> = names
            .iter()
            .map(|n| -> &dyn rusqlite::ToSql {
                if n == ":limit" || n == ":offset" {
                    &10i64
                } else {
                    &"x"
                }
            })
            .collect();
        let rows: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(bound), |r| r.get::<_, String>(3))
            .expect("plan")
            .map(|r| r.expect("row"))
            .collect();
        rows.join("\n")
    };

    assert!(
        plan_of("R3 type").contains("ix_notes_type"),
        "{}",
        plan_of("R3 type")
    );
    assert!(
        plan_of("R3 field").contains("ix_fields_key_text"),
        "{}",
        plan_of("R3 field")
    );
    assert!(plan_of("R3 field").contains("ix_notes_sidecar"));
    assert!(plan_of("R2").contains("ix_links_dst"), "{}", plan_of("R2"));
    assert!(plan_of("R1").contains("blocks_fts"), "{}", plan_of("R1"));

    // R5 attachments (task #17; architecture §4.5): the plan scans `assets` (~200 rows) and reaches `notes`
    // by rowid, and the resolver-choice subqueries use the folded path and file-name
    // indexes. Aliases: a, n are `notes`, x is `assets`, c the derived candidate table.
    let plan = plan_of("attachments");
    for needle in [
        "SCAN x",
        "SEARCH a USING INTEGER PRIMARY KEY",
        "ix_notes_path_fold",
        "ix_notes_file_name",
    ] {
        assert!(plan.contains(needle), "{needle} missing from:\n{plan}");
    }
    assert!(
        plan.lines()
            .all(|line| !line.starts_with("SCAN a") && !line.starts_with("SCAN n")),
        "attachments scans notes:\n{plan}"
    );
}

#[test]
fn r3_ignores_inline_fields_which_are_p2s() {
    // FR-STR-04 (inline fields) is P2: `key:: value` in a body must not answer a
    // property lookup.
    let f = fixture(&[("a.md", "# A\nstatus:: active\n"), ("b.md", "# B\n")]);
    f.frontmatter("b.md", "status", "active");
    let hits = f.hits("[status:active]");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "b.md");
}
