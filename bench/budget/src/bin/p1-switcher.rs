//! NFR-PRF-06 allocation, Rust side: `Corpus::query` over the benchmark vault's index
//! (architecture §9.2) · reported, not the gate.
//!
//! Serves **NFR-PRF-06**, **FR-NAV-01**, **US-04**. The binding figure is keystroke to
//! painted results, under 50 ms at p95 at 20,000 notes, in the real binary (task #10's
//! job, unmerged). §9.2 allots **20 ms p95** of it to `Switcher::query`, and that is
//! what this times: the matcher alone, in-process, over the corpus built from the vault's
//! index. It excludes IPC, serialisation and render, so it is a lower bound of the gate.
//!
//! The query set is derived from the index (titles, aliases, headings, paths) and is
//! deterministic: three-character title prefixes, three-character subsequences spread
//! across a title, path-stem queries, alias prefixes, queries that match nothing, and
//! the three `#` shapes that put heading mode on the gated third keystroke (`#ab...`,
//! `a#b...`, `ab#...`). Each query is typed one character at a time and every keystroke
//! is timed; keystroke 3 (counting `#`) is the gated one, the rest are reported.
//!
//! History is declared on every line: `absent`, or `fixture-2000` (2,000 deterministic
//! opens, the record's cap), because frecency changes the work and the order.
//!
//! **A run is rejected, not recorded, unless:** every no-match query returns zero
//! results; every heading-mode query shape returns results; every full-title query
//! returns its note in the top 10 (the 3-character queries cannot be held to that: a
//! three-letter prefix of "Activation 12" is shared by hundreds of notes).
//!
//! ```text
//! cargo run --release -p budget --bin p1-switcher
//! cargo run --release -p budget --bin p1-switcher -- --vault 20k --passes 3 --assert
//! ```
//!
//! `--assert` (task #9/#10, NFR-PRF-08) turns the allocation into the CI gate: exit 1 if
//! keystroke-3 p95 at **20k**, default mode or heading mode, under any declared history, is
//! over `ALLOCATION_MS`. 50k is never gated; the adversarial stress is never gated.

use std::process::ExitCode;
use std::time::Instant;

use budget::{Samples, ensure_vault};
use rusqlite::{Connection, OpenFlags};
use serat_index::history::History;
use serat_index::switcher::Corpus;
use serat_vault::Vault;

/// §9.2's allocation to `Switcher::query`, p95.
const ALLOCATION_MS: f64 = 20.0;
const NOW_MS: i64 = 1_800_000_000_000;

struct Query {
    text: String,
    /// The note a full-length query is built from, when it must come back in the top 10.
    must_find: Option<String>,
    expect_none: bool,
    heading_mode: bool,
}

fn first_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn read_strings(conn: &Connection, sql: &str) -> Result<Vec<(String, String)>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

fn queries(db: &std::path::Path) -> Result<Vec<Query>, String> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let notes = read_strings(
        &conn,
        "SELECT path, title FROM notes WHERE kind = 'note' AND title IS NOT NULL ORDER BY path",
    )?;
    let aliases = read_strings(
        &conn,
        "SELECT n.path, f.val_text FROM fields f JOIN notes n ON n.id = f.note_id \
         WHERE f.source = 'frontmatter' AND f.key IN ('aliases','alias') \
         AND f.val_text IS NOT NULL ORDER BY n.path LIMIT 5000",
    )?;
    let headings = read_strings(
        &conn,
        "SELECT n.path, b.text FROM blocks b JOIN notes n ON n.id = b.note_id \
         WHERE b.kind = 'heading' AND b.text IS NOT NULL AND b.ord < 2 \
         ORDER BY n.path LIMIT 5000",
    )?;
    if notes.len() < 1000 {
        return Err(format!("only {} notes in the index", notes.len()));
    }
    eprintln!(
        "sources: {} notes, {} aliases, {} headings",
        notes.len(),
        aliases.len(),
        headings.len()
    );
    let pick = |v: &[(String, String)], count: usize, offset: usize| -> Vec<(String, String)> {
        if v.is_empty() {
            return Vec::new();
        }
        let step = (v.len() / count).max(1);
        (0..count)
            .filter_map(|i| v.get((i * step + offset) % v.len()).cloned())
            .collect()
    };
    let mut out = Vec::new();
    let usable = |t: &str| t.chars().filter(|c| c.is_alphanumeric()).count() >= 4;

    // Three-character title prefixes.
    for (path, title) in pick(&notes, 80, 0) {
        if usable(&title) {
            out.push(Query {
                text: first_chars(&title, 3).to_lowercase(),
                must_find: None,
                expect_none: false,
                heading_mode: false,
            });
            out.push(Query {
                text: title.to_lowercase(),
                must_find: Some(path),
                expect_none: false,
                heading_mode: false,
            });
        }
    }
    // Three-character subsequences spread across a title.
    for (_, title) in pick(&notes, 40, 7) {
        let chars: Vec<char> = title
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect();
        if chars.len() >= 4 {
            out.push(Query {
                text: [chars[0], chars[chars.len() / 2], chars[chars.len() - 1]]
                    .iter()
                    .collect(),
                must_find: None,
                expect_none: false,
                heading_mode: false,
            });
        }
    }
    // Path-stem queries: a folder name and the start of a file name.
    for (path, _) in pick(&notes, 30, 3) {
        if let Some((dir, file)) = path.rsplit_once('/') {
            let dir = dir.rsplit('/').next().unwrap_or(dir);
            out.push(Query {
                text: format!("{}/{}", first_chars(dir, 3), first_chars(file, 3)).to_lowercase(),
                must_find: None,
                expect_none: false,
                heading_mode: false,
            });
        }
    }
    // Alias prefixes.
    for (_, alias) in pick(&aliases, 20, 0) {
        if usable(&alias) {
            out.push(Query {
                text: first_chars(&alias, 3).to_lowercase(),
                must_find: None,
                expect_none: false,
                heading_mode: false,
            });
        }
    }
    // Nothing matches these: no note contains a section sign or a tilde.
    for i in 0..30 {
        let c = char::from(b'a' + u8::try_from(i % 26).unwrap_or(0));
        let text = match i % 3 {
            0 => format!("\u{a7}{c}\u{a7}"),
            1 => format!("~~{c}"),
            _ => format!("{c}\u{a7}~\u{a7}"),
        };
        out.push(Query {
            text,
            must_find: None,
            expect_none: true,
            heading_mode: false,
        });
    }
    // Heading mode, the three shapes of §9.2: `#ab...`, `a#b...`, `ab#...`.
    for (_, heading) in pick(&headings, 20, 0) {
        out.push(Query {
            text: format!("#{}", first_chars(&heading, 3).to_lowercase()),
            must_find: None,
            expect_none: false,
            heading_mode: true,
        });
    }
    for ((_, title), (_, heading)) in pick(&notes, 20, 11).into_iter().zip(pick(&headings, 20, 5)) {
        out.push(Query {
            text: format!(
                "{}#{}",
                first_chars(&title, 1).to_lowercase(),
                first_chars(&heading, 3).to_lowercase()
            ),
            must_find: None,
            expect_none: false,
            heading_mode: true,
        });
    }
    for (_, title) in pick(&notes, 20, 13) {
        out.push(Query {
            text: format!("{}#", first_chars(&title, 2).to_lowercase()),
            must_find: None,
            expect_none: false,
            heading_mode: true,
        });
    }
    Ok(out)
}

/// 2,000 deterministic opens over the vault's notes, spread over a month.
fn history_fixture(corpus_paths: &[String]) -> History {
    let mut h = History::default();
    let step = (corpus_paths.len() / 2000).max(1);
    for (n, path) in corpus_paths.iter().step_by(step).take(2000).enumerate() {
        let age = i64::try_from(n * 997 % 30).unwrap_or(0) * 86_400_000;
        h.record_open(path, NOW_MS - age);
        if n % 3 == 0 {
            h.record_open(path, NOW_MS - age / 2);
        }
    }
    h
}

fn line(label: &str, s: &Samples) {
    println!(
        "{label:<34} median {:>7.2} ms   p95 {:>7.2} ms   max {:>7.2} ms   (n={})",
        s.median(),
        s.percentile(0.95),
        s.max(),
        s.len()
    );
}

fn run(args: &[String]) -> Result<bool, String> {
    let mut vault_names = "20k,50k".to_owned();
    let mut passes = 2usize;
    let mut assert = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--vault" => vault_names = it.next().ok_or("--vault needs names")?.clone(),
            "--assert" => assert = true,
            "--passes" => {
                passes = it
                    .next()
                    .ok_or("--passes needs a number")?
                    .parse()
                    .map_err(|e| format!("--passes: {e}"))?;
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    if assert && !vault_names.split(',').any(|n| n == "20k") {
        return Err("--assert needs the 20k vault in --vault; nothing else gates".to_owned());
    }
    let mut breaches = Vec::new();
    for name in vault_names.split(',') {
        // Only the 20k vault gates (NFR-PRF-06's figure is "at 20,000 notes").
        measure(name, passes, name == "20k", &mut breaches)?;
    }
    stress();
    if assert && !breaches.is_empty() {
        for b in &breaches {
            eprintln!("NFR-PRF-06: over allocation (--assert): {b}");
        }
        return Ok(false);
    }
    Ok(true)
}

/// Adversarial candidates: long, dense in one letter, 50k of them (review S1). No vault
/// is involved; the corpus is built from generated paths.
fn stress() {
    let paths: Vec<String> = (0..50_000)
        .map(|i| {
            format!(
                "{}/{}{i}.md",
                "e".repeat(80 + i % 40),
                "e".repeat(100 + i % 60)
            )
        })
        .collect();
    let corpus = Corpus::from_paths(&paths);
    println!(
        "\nstress (adversarial, reported and not gated): 50000 notes, 100-160 character e-dense \
names under 80-120 character e-dense folders, history absent"
    );
    let history = History::default();
    for (label, query) in [
        ("3 chars \"mee\" (no match)", "mee".to_owned()),
        ("3 chars \"eee\"", "eee".to_owned()),
        ("8 x \"e\"", "e".repeat(8)),
        ("16 x \"e\"", "e".repeat(16)),
        (
            "23-character spaced query",
            "e e e e e e e e e e e ee".to_owned(),
        ),
    ] {
        let mut samples = Samples::new();
        for _ in 0..5 {
            let started = Instant::now();
            std::hint::black_box(corpus.query(&query, 50, &history, NOW_MS));
            samples.push(started.elapsed());
        }
        line(&format!("  {label}"), &samples);
    }
}

fn measure(
    vault_name: &str,
    passes: usize,
    gate: bool,
    breaches: &mut Vec<String>,
) -> Result<(), String> {
    let vault_name = vault_name.to_owned();
    println!("\n==================== vault {vault_name} ====================");
    let under_test = ensure_vault(&vault_name)?;
    let vault = Vault::open(&under_test.path).map_err(|e| format!("open vault: {e}"))?;
    let scratch = std::env::temp_dir().join(format!("serat-bench-switcher-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    serat_index::rebuild(
        &vault,
        &serat_parse::CommonMarkParser,
        &under_test.path,
        &scratch,
    )
    .map_err(|e| format!("rebuild (untimed setup): {e}"))?;
    let db = serat_index::index_path(&scratch);

    // Corpus build, reported.
    let mut build = Samples::new();
    let mut corpus = None;
    for _ in 0..5 {
        let started = Instant::now();
        let c = Corpus::from_index(&db).map_err(|e| e.to_string())?;
        build.push(started.elapsed());
        corpus = Some(c);
    }
    let corpus = corpus.ok_or("no corpus")?;
    let tree_started = Instant::now();
    let paths: Vec<String> = {
        let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
        read_strings(
            &conn,
            "SELECT path, path FROM notes WHERE kind = 'note' ORDER BY path",
        )?
        .into_iter()
        .map(|(p, _)| p)
        .collect()
    };
    let tree = Corpus::from_paths(&paths);
    let tree_build = tree_started.elapsed();

    let set = queries(&db)?;
    let absent = History::default();
    let fixture = history_fixture(&paths);

    // Validity: rejected, not recorded.
    for q in &set {
        let hits = corpus.query(&q.text, 50, &absent, NOW_MS);
        if q.expect_none && !hits.is_empty() {
            return Err(format!(
                "no-match query {:?} returned {} results; run rejected",
                q.text,
                hits.len()
            ));
        }
        if q.heading_mode && hits.is_empty() {
            return Err(format!(
                "heading-mode query {:?} returned nothing; run rejected",
                q.text
            ));
        }
        if let Some(want) = &q.must_find
            && !hits.iter().take(10).any(|h| &h.path == want)
        {
            return Err(format!(
                "{:?} did not return {want} in its top 10; run rejected",
                q.text
            ));
        }
    }
    let empty = corpus.query("", 20, &fixture, NOW_MS);
    if empty.is_empty() {
        return Err("empty query returned nothing with a history; run rejected".into());
    }

    println!(
        "p1-switcher · vault {vault_name} · {} notes, {} aliases, {} headings · {} queries · \
         {passes} passes",
        corpus.len(),
        corpus.alias_count(),
        corpus.heading_count(),
        set.len()
    );
    println!(
        "alias-prefix queries: up to 20 generated, drawn from the {} alias rows in the index \
         (block-style lists indexed since #57; each is checked non-empty like any 3-character prefix)",
        corpus.alias_count()
    );
    println!(
        "validity: the top-10 rule is asserted for full-title queries; 3-character prefixes are \
         checked non-empty only; no-match queries return zero; heading-mode queries return results."
    );
    println!("query set: derived from the index, deterministic; headings drawn from vaultgen's");
    println!("small fixed vocabulary, so heading-mode match counts overstate a real vault.\n");
    line("corpus build (from index)", &build);
    println!(
        "{:<34} {:.1} ms  (Tree corpus of {} notes from {} paths)",
        "corpus build (from paths)",
        tree_build.as_secs_f64() * 1000.0,
        tree.len(),
        paths.len()
    );

    for (label, history) in [("absent", &absent), ("fixture-2000", &fixture)] {
        let (mut gated, mut gated_default, mut gated_heading, mut all) = (
            Samples::new(),
            Samples::new(),
            Samples::new(),
            Samples::new(),
        );
        for _ in 0..passes {
            for q in &set {
                let chars: Vec<(usize, char)> = q.text.char_indices().collect();
                for (k, &(at, c)) in chars.iter().enumerate() {
                    let typed = &q.text[..at + c.len_utf8()];
                    let started = Instant::now();
                    let hits = corpus.query(typed, 50, history, NOW_MS);
                    let took = started.elapsed();
                    std::hint::black_box(&hits);
                    all.push(took);
                    if k == 2 {
                        gated.push(took);
                        if q.heading_mode {
                            gated_heading.push(took);
                        } else {
                            gated_default.push(took);
                        }
                    }
                }
            }
        }
        println!("\nhistory: {label}");
        line("  keystroke 3 (gated position)", &gated);
        line("    default mode", &gated_default);
        line("    heading mode", &gated_heading);
        line("  every keystroke (reported)", &all);
        let heading_p95 = gated_heading.percentile(0.95);
        if gate && heading_p95 > ALLOCATION_MS {
            breaches.push(format!(
                "heading mode, history {label}: p95 {heading_p95:.2} ms"
            ));
        }
        println!(
            "  heading mode, own row, history {label}: p95 {heading_p95:.2} ms against \
             {ALLOCATION_MS:.0} ms ({:.0}% of allocation). {}",
            100.0 * heading_p95 / ALLOCATION_MS,
            if heading_p95 <= ALLOCATION_MS {
                "Within."
            } else {
                "OVER."
            }
        );
        let p95 = gated.percentile(0.95);
        if gate && p95 > ALLOCATION_MS {
            breaches.push(format!("keystroke 3, history {label}: p95 {p95:.2} ms"));
        }
        println!(
            "  NFR-PRF-06 allocation, history {label}: Switcher::query p95 {p95:.2} ms against \
             {ALLOCATION_MS:.0} ms ({:.0}% of allocation). {}",
            100.0 * p95 / ALLOCATION_MS,
            if p95 <= ALLOCATION_MS {
                "Within."
            } else {
                "OVER: a finding, not a failure."
            }
        );
    }
    println!(
        "\nBasis: in-process, one thread, release build expected; excludes IPC, serialisation \
         and render. The binding figure is keystroke to painted results in the real binary (task #10)."
    );
    let _ = std::fs::remove_dir_all(&scratch);
    if vault_name == "50k" {
        println!("(50k is reported beside the 20k figures; 20k gates.)");
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("p1-switcher: {e}");
            ExitCode::FAILURE
        }
    }
}
