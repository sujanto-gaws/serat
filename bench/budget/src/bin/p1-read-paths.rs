//! P1 read paths (architecture §4.5) · latency expectations, not gates.
//!
//! Serves **FR-NAV-02**, **FR-NAV-04**, **FR-STR-03**, **FR-BIN-02**. No NFR budgets
//! search, backlinks or property lookup, so §4.5 states **expectations**: R1 top-50
//! search <= 100 ms p95 over terms matching at most 20% of blocks (the ruling on #15:
//! the very common terms are reported, with their match count, and carry no
//! expectation), R2 backlinks of the highest in-degree note <= 20 ms, R3 a 500-row
//! property lookup in one `LIMIT 500` statement <= 20 ms, R1 + R3 <= 100 ms. The save
//! of the highest in-degree note, and of a note whose file name is shared by hundreds
//! of others, are timed too (NFR-PRF-03's 50 ms is asserted by prf03-save-to-index
//! once serat-app exists; until then this is the only measurement of the link
//! re-resolution a save pays). A miss is a finding for architect and
//! product-analyst, not a red build, so this job exits 0 whatever it measures and
//! only fails when it cannot measure.
//!
//! It rebuilds the index of the named benchmark vault once (untimed; NFR-PRF-04 times
//! that), then reads it through `serat_index::read::Reader` on a read-only connection,
//! the way the application will. The terms and keys it queries are discovered in the
//! index, not hard-coded, so it follows the generator.
//!
//! ```text
//! cargo run --release -p budget --bin p1-read-paths -- --vault 20k
//! cargo run --release -p budget --bin p1-read-paths -- --vault 50k --iterations 50
//! ```

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use budget::{Samples, ensure_vault};
use rusqlite::Connection;
use serat_index::read::{Reader, SearchOutcome};

/// One line of the report: a name, what was run, and its expectation in ms.
struct Row {
    name: String,
    samples: Samples,
    rows: usize,
    /// `None`: reported with no expectation (the ruling on very common terms).
    expectation_ms: Option<f64>,
}

fn time<F: FnMut() -> usize>(
    name: &str,
    expectation_ms: Option<f64>,
    iterations: usize,
    mut f: F,
) -> Row {
    // One untimed run first: the page cache and the statement cache are warm in the
    // application by the second search, and a first-ever query is not what this measures.
    let rows = f();
    let mut samples = Samples::new();
    for _ in 0..iterations {
        let started = Instant::now();
        let n = f();
        samples.push(started.elapsed());
        assert_eq!(
            n, rows,
            "{name}: a repeated read returned a different count"
        );
    }
    Row {
        name: name.to_owned(),
        samples,
        rows,
        expectation_ms,
    }
}

fn search_rows(reader: &Reader, query: &str, offset: u32, limit: u16) -> usize {
    match reader.search(query, offset, limit) {
        Ok(SearchOutcome::Hits(hits)) => hits.len(),
        Ok(other) => panic!("{query:?} produced {other:?}, not hits"),
        Err(e) => panic!("{query:?} failed: {e}"),
    }
}

/// Terms by how many blocks contain them, most common first, with that count.
/// `max_doc` keeps only terms matching at most that many blocks.
fn terms_by_frequency(
    conn: &Connection,
    max_doc: i64,
    take: usize,
) -> Result<Vec<(String, i64)>, String> {
    let mut stmt = conn
        .prepare(
            // Only plain letters: a term like `0` or `---` is not what a person
            // searches for, and quoting makes the rest of the vocabulary safe.
            "SELECT term, doc FROM temp.v WHERE term GLOB '[a-z][a-z][a-z]*' AND doc <= ?1
              ORDER BY doc DESC, term LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([max_doc, i64::try_from(take).unwrap_or(i64::MAX)], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

fn one_string(conn: &Connection, sql: &str) -> Result<String, String> {
    conn.query_row(sql, [], |r| r.get(0))
        .map_err(|e| format!("{sql}: {e}"))
}

fn run(args: &[String]) -> Result<(), String> {
    let mut vault_name = "20k".to_owned();
    let mut iterations = 50usize;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--vault" => vault_name = it.next().ok_or("--vault needs a name")?.clone(),
            "--iterations" => {
                iterations = it
                    .next()
                    .ok_or("--iterations needs a number")?
                    .parse()
                    .map_err(|e| format!("--iterations: {e}"))?;
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    let vault = ensure_vault(&vault_name)?;
    let index_dir = std::env::temp_dir().join(format!("serat-bench-read-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&index_dir);
    let source = serat_vault::Vault::open(&vault.path).map_err(|e| format!("open vault: {e}"))?;
    let rebuilt = Instant::now();
    serat_index::rebuild(
        &source,
        &serat_parse::CommonMarkParser,
        &vault.path,
        &index_dir,
    )
    .map_err(|e| format!("rebuild: {e}"))?;
    eprintln!(
        "p1-read-paths: rebuilt {} notes in {:.1} s (untimed setup)",
        vault.notes,
        rebuilt.elapsed().as_secs_f64()
    );
    let result = measure(
        &serat_index::index_path(&index_dir),
        &index_dir,
        &vault.path,
        &vault_name,
        iterations,
    );
    let _ = std::fs::remove_dir_all(&index_dir);
    result
}

/// An echo guard that records nothing: the bench has no watcher to suppress.
struct NoEcho;

impl serat_core::EchoGuard for NoEcho {
    fn record_echo(&self, _path: &serat_core::VaultPath, _hash: serat_core::ContentHash) {}
}

/// Time one content save of `path` through the incremental path (parse, submit, commit),
/// the work NFR-PRF-03 budgets. Re-saves the unchanged file, which the writer applies in
/// full: there is no skip-if-same-hash.
fn save_row(
    name: &str,
    index_dir: &Path,
    vault_root: &Path,
    path: &str,
    iterations: usize,
) -> Result<Row, String> {
    use serat_core::IndexWriter as _;
    let source = serat_vault::Vault::open(vault_root).map_err(|e| format!("open vault: {e}"))?;
    let handle = serat_index::IndexHandle::open(index_dir).map_err(|e| e.to_string())?;
    let path = serat_core::VaultPath::new(path).map_err(|e| format!("{path:?}: {e}"))?;
    let row = time(name, Some(50.0), iterations, || {
        serat_index::apply(
            &handle.writer(),
            &source,
            &serat_parse::CommonMarkParser,
            &NoEcho,
            &path,
        )
        .expect("apply");
        handle.writer().flush().expect("flush");
        1
    });
    handle.close().map_err(|e| e.to_string())?;
    Ok(row)
}

/// One iteration = create `zz/index.md` and index it, delete it and index the removal.
fn create_delete_row(
    name: &str,
    index_dir: &Path,
    vault_root: &Path,
    iterations: usize,
) -> Result<Row, String> {
    use serat_core::IndexWriter as _;
    let source = serat_vault::Vault::open(vault_root).map_err(|e| format!("open vault: {e}"))?;
    let handle = serat_index::IndexHandle::open(index_dir).map_err(|e| e.to_string())?;
    let rel = "zz/index.md";
    let path = serat_core::VaultPath::new(rel).map_err(|e| format!("{rel:?}: {e}"))?;
    let file = vault_root.join(rel);
    std::fs::create_dir_all(file.parent().ok_or("no parent")?).map_err(|e| e.to_string())?;
    let row = time(name, Some(50.0), iterations, || {
        for create in [true, false] {
            if create {
                std::fs::write(&file, "# Index zz\n").expect("write");
            } else {
                std::fs::remove_file(&file).expect("remove");
            }
            serat_index::apply(
                &handle.writer(),
                &source,
                &serat_parse::CommonMarkParser,
                &NoEcho,
                &path,
            )
            .expect("apply");
            handle.writer().flush().expect("flush");
        }
        1
    });
    handle.close().map_err(|e| e.to_string())?;
    Ok(row)
}

/// Many notes sharing a file name: 300 `dN/index.md`, 300 `dN/README.md` and a root
/// `index.md`, with 300 notes linking `[[index]]` and `[[README]]`. Saving any of the
/// namesakes is the worst case for link re-resolution.
fn duplicate_names_rows(iterations: usize) -> Result<Vec<Row>, String> {
    let root = std::env::temp_dir().join(format!("serat-bench-dupnames-{}", std::process::id()));
    let index_dir =
        std::env::temp_dir().join(format!("serat-bench-dupnames-ix-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&index_dir);
    let write = |rel: &str, body: &str| -> Result<(), String> {
        let file = root.join(rel);
        std::fs::create_dir_all(file.parent().ok_or("no parent")?).map_err(|e| e.to_string())?;
        std::fs::write(file, body).map_err(|e| e.to_string())
    };
    write("index.md", "# Root index\n")?;
    for i in 0..300 {
        write(&format!("d{i}/index.md"), &format!("# Index {i}\n"))?;
        write(&format!("d{i}/README.md"), &format!("# Readme {i}\n"))?;
        write(
            &format!("n{i}.md"),
            &format!("# Linker {i}\n\n[[index]] and [[README]] and [[d{i}/index]]\n"),
        )?;
    }
    let source = serat_vault::Vault::open(&root).map_err(|e| format!("open vault: {e}"))?;
    serat_index::rebuild(&source, &serat_parse::CommonMarkParser, &root, &index_dir)
        .map_err(|e| format!("rebuild: {e}"))?;
    let mut rows = Vec::new();
    for (what, path) in [
        (
            "the root index.md, the shortest of 301 namesakes, 300 inbound",
            "index.md",
        ),
        ("d7/index.md, one of 301 namesakes", "d7/index.md"),
        ("d7/README.md, one of 300 namesakes", "d7/README.md"),
    ] {
        rows.push(save_row(
            &format!("Save of {what}"),
            &index_dir,
            &root,
            path,
            iterations,
        )?);
    }
    // The case where RIVALS still runs: a note appears under a spelling 301 others
    // share (a content save of an existing path skips it), then goes again. Each
    // iteration creates a 302nd index.md and then deletes it; both saves are timed
    // together, plus two small file operations.
    rows.push(create_delete_row(
        "Create then delete a 302nd index.md (RIVALS runs on the create; one iteration = both saves)",
        &index_dir,
        &root,
        iterations,
    )?);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&index_dir);
    Ok(rows)
}

fn measure(
    db: &Path,
    index_dir: &Path,
    vault_root: &Path,
    vault_name: &str,
    iterations: usize,
) -> Result<(), String> {
    // Discovery runs on its own read-write connection (a temp vocabulary table), never
    // on the Reader under test.
    let probe = Connection::open(db).map_err(|e| e.to_string())?;
    probe
        .execute_batch("CREATE VIRTUAL TABLE temp.v USING fts5vocab(main, blocks_fts, 'row')")
        .map_err(|e| format!("fts5vocab: {e}"))?;
    let blocks: i64 = probe
        .query_row("SELECT count(*) FROM blocks", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    // Terms matching more than 20% of blocks are reported, not judged (the ruling on
    // #15): a term in a fifth of the vault is not something a person searches for.
    let (top_term, top_n) = terms_by_frequency(&probe, i64::MAX, 1)?
        .into_iter()
        .next()
        .ok_or("the index has no terms")?;
    let terms = terms_by_frequency(&probe, blocks / 5, 1100)?;
    if terms.len() < 3 {
        return Err("the index has too few terms to build a query set".to_owned());
    }
    let (commonest, commonest_n) = terms[0].clone();
    let dense = one_string(
        &probe,
        "SELECT p.path FROM links l JOIN notes p ON p.id = l.dst_note
          WHERE l.kind IN ('wiki','embed','md')
          GROUP BY l.dst_note ORDER BY count(*) DESC, p.path LIMIT 1",
    )?;
    let indegree: i64 = probe
        .query_row(
            "SELECT count(*) FROM links WHERE kind IN ('wiki','embed','md')
                AND dst_note = (SELECT id FROM notes WHERE path = ?1)",
            [&dense],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let big_key = one_string(
        &probe,
        "SELECT key FROM fields WHERE source = 'frontmatter' AND val_text IS NOT NULL
          AND key <> 'type' GROUP BY key ORDER BY count(*) DESC, key LIMIT 1",
    )?;
    // The value of the largest key whose frontmatter row count is nearest 500.
    let (near_500, near_500_n): (String, i64) = probe
        .query_row(
            "SELECT val_text, count(*) c FROM fields
              WHERE key = ?1 AND source = 'frontmatter' AND val_text IS NOT NULL
              GROUP BY val_text ORDER BY abs(count(*) - 500), val_text LIMIT 1",
            [&big_key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let (type_val, type_n): (String, i64) = probe
        .query_row(
            "SELECT type, count(*) FROM notes WHERE type IS NOT NULL
              GROUP BY type ORDER BY abs(count(*) - 500), type LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    drop(probe);

    let reader = Reader::open(db).map_err(|e| e.to_string())?;
    let mut rows: Vec<Row> = Vec::new();

    // R1, reported with no expectation: the single most common term of the whole index,
    // and the most common one inside the 20% line.
    rows.push(time(
        &format!(
            "R1 top term {top_term:?} ({top_n} of {blocks} blocks match), top 50 (no expectation)"
        ),
        None,
        iterations,
        || search_rows(&reader, &top_term, 0, 50),
    ));
    rows.push(time(
        &format!(
            "R1 most common term inside the 20% line {commonest:?} ({commonest_n} blocks), top 50"
        ),
        Some(100.0),
        iterations,
        || search_rows(&reader, &commonest, 0, 50),
    ));
    // The progress handler's cost, measured on ONE search connection: the same
    // connection, the same warm-up, the two states alternating iteration by iteration.
    // #47 compared two different connections and read +8-12 ms, which was most likely
    // page-cache warmth rather than the handler (task #51).
    let search_conn = serat_index::SearchConn::open(db).map_err(|e| e.to_string())?;
    let searched = Reader::open(db)
        .map_err(|e| e.to_string())?
        .with_search(search_conn.clone());
    for (label, term, expectation) in [
        (format!("R1 top term {top_term:?}"), &top_term, None),
        (format!("R1 {commonest:?}"), &commonest, Some(100.0)),
    ] {
        let (mut with, mut without) = (Samples::new(), Samples::new());
        let rows_n = search_rows(&searched, term, 0, 50);
        // Warm both states, untimed.
        for on in [true, false, true, false] {
            search_conn
                .set_progress_handler(on)
                .map_err(|e| e.to_string())?;
            search_rows(&searched, term, 0, 50);
        }
        for _ in 0..iterations {
            for on in [true, false] {
                search_conn
                    .set_progress_handler(on)
                    .map_err(|e| e.to_string())?;
                let started = Instant::now();
                assert_eq!(search_rows(&searched, term, 0, 50), rows_n);
                (if on { &mut with } else { &mut without }).push(started.elapsed());
            }
        }
        search_conn
            .set_progress_handler(true)
            .map_err(|e| e.to_string())?;
        rows.push(Row {
            name: format!(
                "{label}, top 50, search connection, handler ON (same connection, alternating)"
            ),
            samples: with,
            rows: rows_n,
            expectation_ms: expectation,
        });
        rows.push(Row {
            name: format!(
                "{label}, top 50, search connection, handler OFF (same connection, alternating)"
            ),
            samples: without,
            rows: rows_n,
            expectation_ms: expectation,
        });
    }
    // ... and a query set spread across the 20%-restricted vocabulary, as one p95.
    let picks = [0usize, 1, 9, 99, 499, 999];
    let mut set = Samples::new();
    let mut set_rows = 0;
    for &i in picks.iter().filter(|&&i| i < terms.len()) {
        let q = &terms[i].0;
        let r = time("", None, iterations, || search_rows(&reader, q, 0, 50));
        for v in r.samples.values() {
            set.push(std::time::Duration::from_secs_f64(v / 1000.0));
        }
        set_rows += r.rows;
    }
    let pair = format!("{} {}", terms[0].0, terms[1].0);
    let r = time("", None, iterations, || search_rows(&reader, &pair, 0, 50));
    for v in r.samples.values() {
        set.push(std::time::Duration::from_secs_f64(v / 1000.0));
    }
    rows.push(Row {
        name: format!(
            "R1 query set (terms matching <= 20% of blocks): terms ranked {picks:?} by frequency + the AND of the top two ({} queries)",
            picks.len() + 1
        ),
        samples: set,
        rows: set_rows + r.rows,
        expectation_ms: Some(100.0),
    });

    // R2: the highest in-degree note.
    rows.push(time(
        &format!("R2 backlinks of {dense:?} (in-degree {indegree})"),
        Some(20.0),
        iterations,
        || reader.backlinks(&dense).map_or(0, |b| b.len()),
    ));

    // R3: one page of 50, then the whole 500-row lookup in one LIMIT 500 statement.
    let by_key = format!("[{big_key}:{near_500}]");
    rows.push(time(
        &format!("R3 {by_key} ({near_500_n} rows), first page of 50"),
        Some(20.0),
        iterations,
        || search_rows(&reader, &by_key, 0, 50),
    ));
    rows.push(time(
        &format!("R3 {by_key}, LIMIT 500"),
        Some(20.0),
        iterations,
        || search_rows(&reader, &by_key, 0, 500),
    ));
    let by_type = format!("[type:{type_val}]");
    rows.push(time(
        &format!("R4 {by_type} ({type_n} rows, exact), LIMIT 500"),
        Some(20.0),
        iterations,
        || search_rows(&reader, &by_type, 0, 500),
    ));

    // Attachments (task #17): every asset with its resolving link, one statement.
    rows.push(time(
        "R5 attachments (all assets, resolver choice by name and path)",
        Some(20.0),
        iterations,
        || reader.attachments().map_or(0, |a| a.len()),
    ));

    // R1 + R3: the commonest in-line term filtered by a property.
    let combined = format!("{commonest} {by_key}");
    rows.push(time(
        &format!("R1+R3 {combined:?}, top 50"),
        Some(100.0),
        iterations,
        || search_rows(&reader, &combined, 0, 50),
    ));
    drop(reader);

    // NFR-PRF-03 stand-ins: one save of the highest in-degree note of this vault.
    rows.push(save_row(
        &format!("Save of {dense:?}, the highest in-degree note ({indegree} inbound links)"),
        index_dir,
        vault_root,
        &dense,
        iterations,
    )?);
    rows.extend(duplicate_names_rows(iterations)?);

    println!(
        "P1 read paths · {vault_name} vault · {iterations} timed runs per read (expectations, not gates)"
    );
    println!();
    for row in &rows {
        let p95 = row.samples.percentile(0.95);
        let (expectation, verdict) = match row.expectation_ms {
            None => ("none".to_owned(), ""),
            Some(e) if p95 <= e => (format!("{e:.0} ms"), ": within"),
            Some(e) => (
                format!("{e:.0} ms"),
                ": OVER (a finding for architect and product-analyst, not a red build)",
            ),
        };
        println!(
            "{}\n    p50 {:.2} ms · p95 {:.2} ms · max {:.2} ms · {} rows · expectation {expectation}{verdict}",
            row.name,
            row.samples.percentile(0.5),
            p95,
            row.samples.max(),
            row.rows,
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("p1-read-paths: {e}");
            ExitCode::FAILURE
        }
    }
}
