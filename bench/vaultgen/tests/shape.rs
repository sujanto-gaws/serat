//! Shape — the vault is what architecture §10.3 says it is.
//!
//! Verifies: **NFR-MNT-02**, **US-35**, and by extension every NFR-PRF budget
//! measured against this vault (**C-06**). §10.3 makes four claims about the
//! vault's shape — long-tailed links, typed and untyped notes, frontmatter and
//! inline fields, tasks with dates and estimates, ~200 assets in four binary
//! formats. Each is a test here, because a benchmark vault that quietly stops
//! matching its description makes every number taken from it wrong in a way
//! nobody notices.

mod common;

use std::collections::{HashMap, HashSet};

use common::{TempDir, walk};
use vaultgen::note::NoteStats;
use vaultgen::plan::{AssetFormat, Kind, Plan};
use vaultgen::{Options, generate};

/// Render a vault in memory. Fast enough to assert distribution properties on
/// a few thousand notes without touching the filesystem.
fn simulate(seed: u64, notes: usize, assets: usize) -> (Plan, NoteStats, Vec<u32>) {
    let plan = Plan::build(seed, notes, assets);
    let mut stats = NoteStats::default();
    let mut in_degree = vec![0u32; plan.notes.len()];
    for meta in &plan.notes {
        let (_, s, targets) = vaultgen::note::render(&plan, meta);
        stats.merge(&s);
        for t in targets {
            in_degree[t as usize] += 1;
        }
    }
    (plan, stats, in_degree)
}

#[test]
fn the_note_budget_is_exact_and_the_mix_is_typed_and_untyped() {
    // §10.3: "a mix of typed and untyped notes".
    let plan = Plan::build(20_260_903, 20_000, 200);
    assert_eq!(plan.notes.len(), 20_000, "--notes must mean what it says");
    assert_eq!(plan.assets.len(), 200);

    let mut by_kind: HashMap<&str, usize> = HashMap::new();
    for n in &plan.notes {
        *by_kind.entry(n.kind.as_str()).or_default() += 1;
    }
    for kind in [
        Kind::TypeDef,
        Kind::Daily,
        Kind::Project,
        Kind::Person,
        Kind::Meeting,
        Kind::Literature,
        Kind::Area,
        Kind::Zettel,
        Kind::Untyped,
        Kind::Sidecar,
    ] {
        assert!(
            by_kind.get(kind.as_str()).copied().unwrap_or(0) > 0,
            "no notes of kind {kind:?}"
        );
    }

    let untyped = plan.notes.iter().filter(|n| !n.kind.is_typed()).count();
    let share = untyped as f64 / plan.notes.len() as f64;
    assert!(
        (0.25..0.45).contains(&share),
        "untyped notes are {:.1}% of the vault; a benchmark of only typed notes \
         would not exercise the schema-less path",
        share * 100.0
    );
}

#[test]
fn filenames_are_legal_and_unambiguous_on_every_platform() {
    // A vault that only generates on Linux is not a benchmark, it is a trap.
    const ILLEGAL: &[char] = &['<', '>', ':', '"', '|', '?', '*'];
    const RESERVED: &[&str] = &[
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let plan = Plan::build(20_260_903, 20_000, 200);
    let mut stems: HashSet<String> = HashSet::with_capacity(plan.notes.len());

    for n in &plan.notes {
        assert!(n.rel_path.ends_with(".md"), "{}", n.rel_path);
        assert!(
            n.rel_path.is_ascii(),
            "{} is not ASCII; Unicode normalisation differs between macOS and \
             Linux and would break byte-identical output",
            n.rel_path
        );
        assert!(
            n.rel_path.len() < 120,
            "{} is {} chars; Windows MAX_PATH leaves little headroom",
            n.rel_path,
            n.rel_path.len()
        );
        for segment in n.rel_path.split('/') {
            assert!(
                !segment.chars().any(|c| ILLEGAL.contains(&c)),
                "illegal character in {}",
                n.rel_path
            );
            assert!(
                !segment.ends_with(' ') && !segment.ends_with('.'),
                "Windows strips the trailing character of {}",
                n.rel_path
            );
            let base = segment.split('.').next().unwrap_or("").to_lowercase();
            assert!(
                !RESERVED.contains(&base.as_str()),
                "{} collides with a Windows device name",
                n.rel_path
            );
        }
        assert!(
            stems.insert(n.title.to_lowercase()),
            "two notes share the stem {:?}; every [[wikilink]] to it would be \
             ambiguous",
            n.title
        );
    }
}

#[test]
fn the_link_distribution_is_long_tailed_not_uniform() {
    // §10.3: "a realistic long-tailed link distribution rather than a uniform
    // one — a uniform graph makes rollups and backlinks look far cheaper than
    // they are". This is the test that keeps that sentence true.
    let (plan, _, in_degree) = simulate(20_260_903, 3_000, 30);

    let n = in_degree.len();
    let edges: u64 = in_degree.iter().map(|d| u64::from(*d)).sum();
    let mut sorted = in_degree.clone();
    sorted.sort_unstable();
    let p50 = sorted[n / 2];
    let max = *sorted.last().unwrap();
    let top_n = (n / 100).max(1);
    let top_sum: u64 = sorted[n - top_n..].iter().map(|d| u64::from(*d)).sum();
    let top_share = top_sum as f64 / edges as f64;
    let orphans = in_degree.iter().filter(|d| **d == 0).count();

    assert!(edges > n as u64 * 4, "only {edges} edges over {n} notes");
    assert!(
        f64::from(max) > f64::from(p50) * 8.0,
        "max in-degree {max} is only {p50}x the median; this graph is flat"
    );
    assert!(
        (0.10..0.45).contains(&top_share),
        "top 1% of notes hold {:.1}% of inbound links; expected a long tail",
        top_share * 100.0
    );
    assert!(
        orphans > n / 50,
        "only {orphans} orphans of {n} notes; a real vault has unlinked notes \
         and a backlink query must cope with them"
    );
    assert!(
        orphans < n / 3,
        "{orphans} of {n} notes are orphans; the graph is too sparse to be real"
    );

    // Hubs should be the note kinds real vaults link to, not daily notes.
    let top_kinds: Vec<Kind> = plan.popularity[..20]
        .iter()
        .map(|i| plan.notes[*i as usize].kind)
        .collect();
    assert!(
        top_kinds.iter().filter(|k| **k == Kind::Daily).count() <= 2,
        "daily notes dominate the hubs: {top_kinds:?}"
    );
}

#[test]
fn notes_carry_fields_tasks_links_and_deliberate_decoys() {
    // §10.3: "frontmatter and inline fields, tasks with due dates and
    // estimates". FR-STR-04: a key:: value inside a fence is not a field.
    let (plan, stats, _) = simulate(20_260_903, 2_000, 20);
    let n = plan.notes.len() as u32;

    assert!(stats.frontmatter_keys > n * 3, "too little frontmatter");
    assert!(stats.inline_fields > n * 2, "too few inline fields");
    assert!(
        stats.decoy_fields > n / 2,
        "only {} decoy fields; FR-STR-04 needs a corpus of them to be testable",
        stats.decoy_fields
    );
    assert!(stats.tasks > n * 2, "too few tasks");
    assert!(
        stats.tasks_done > 0 && stats.tasks_done < stats.tasks,
        "tasks must be a mix of done and open"
    );
    assert!(stats.embeds > n / 10, "too few embeds");
    assert_eq!(
        stats.block_ids,
        n.div_ceil(5),
        "every fifth note should carry a block anchor for [[note#^id]] to target"
    );
}

#[test]
fn a_generated_vault_is_walkable_valid_utf8_with_ignorable_folders() {
    let dir = TempDir::new("shape-disk");
    let m = generate(&Options {
        seed: 20_260_903,
        notes: 400,
        assets: 24,
        out: dir.path().to_path_buf(),
        manifest: None,
        jobs: None,
        force: true,
        quiet: true,
    })
    .expect("generate");

    let files = walk(dir.path());
    assert_eq!(files.len(), m.files);

    let md: Vec<&(String, Vec<u8>)> = files.iter().filter(|f| f.0.ends_with(".md")).collect();
    let indexable = md.iter().filter(|f| !f.0.starts_with(".trash/")).count();
    assert_eq!(indexable, m.notes, "note count disagrees with the manifest");

    // The vault carries things a conforming walk must ignore, so a walker that
    // ignores nothing fails here rather than on the user's real vault.
    assert!(files.iter().any(|f| f.0.starts_with(".obsidian/")));
    assert!(files.iter().any(|f| f.0.starts_with(".trash/")));
    assert!(files.iter().any(|f| f.0 == ".vaultgen"));

    for (path, bytes) in &md {
        let text =
            std::str::from_utf8(bytes).unwrap_or_else(|e| panic!("{path} is not valid UTF-8: {e}"));
        let body = text.strip_prefix('\u{feff}').unwrap_or(text);
        if body.starts_with("---\n") || body.starts_with("---\r\n") {
            let rest = &body[4..];
            assert!(
                rest.contains("\n---\n") || rest.contains("\r\n---\r\n"),
                "{path} opens a frontmatter block it never closes"
            );
        }
    }
}

#[test]
fn every_wikilink_resolves_except_the_documented_decoy() {
    // A dangling link would make every backlink count in the manifest wrong,
    // and a rollup benchmark would quietly measure a smaller graph.
    let dir = TempDir::new("links");
    generate(&Options {
        seed: 20_260_903,
        notes: 500,
        assets: 24,
        out: dir.path().to_path_buf(),
        manifest: None,
        jobs: None,
        force: true,
        quiet: true,
    })
    .expect("generate");

    let files = walk(dir.path());
    let mut known: HashSet<String> = HashSet::new();
    for (path, _) in &files {
        let name = path.rsplit('/').next().unwrap_or(path);
        known.insert(name.trim_end_matches(".md").to_lowercase());
        known.insert(name.to_lowercase());
    }

    let mut checked = 0usize;
    for (path, bytes) in files.iter().filter(|f| f.0.ends_with(".md")) {
        let text = String::from_utf8_lossy(bytes);
        for target in wikilink_targets(&text) {
            // The only deliberate dangling link, and it lives inside a fenced
            // code block where FR-STR-04 says nothing should be indexed at all.
            if target == "not a real note" || target == "should not resolve" {
                continue;
            }
            assert!(
                known.contains(&target),
                "{path} links to [[{target}]], which does not exist"
            );
            checked += 1;
        }
    }
    assert!(checked > 1_000, "only {checked} links checked");
}

/// Naive `[[target]]` scanner: enough to check resolvability, and deliberately
/// not a Markdown parser — that is `serat-parse`'s job, not a test fixture's.
fn wikilink_targets(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'['
            && bytes[i + 1] == b'['
            && let Some(end) = text[i + 2..].find("]]")
        {
            let inner = &text[i + 2..i + 2 + end];
            let target = inner
                .split('|')
                .next()
                .unwrap_or(inner)
                .split('#')
                .next()
                .unwrap_or(inner)
                .trim();
            if !target.is_empty() {
                out.push(target.to_lowercase());
            }
            i += 2 + end + 2;
            continue;
        }
        i += 1;
    }
    out
}

#[test]
fn assets_span_four_formats_with_their_magic_bytes() {
    // §10.3: "roughly 200 assets across PDF, DOCX, XLSX and image formats".
    let dir = TempDir::new("assets");
    let m = generate(&Options {
        seed: 20_260_903,
        notes: 400,
        assets: 200,
        out: dir.path().to_path_buf(),
        manifest: None,
        jobs: None,
        force: true,
        quiet: true,
    })
    .expect("generate");

    assert_eq!(m.assets, 200);
    for ext in ["pdf", "png", "docx", "xlsx", "jpg"] {
        assert!(
            m.assets_by_format.get(ext).copied().unwrap_or(0) > 0,
            "no {ext} assets"
        );
    }

    let plan = Plan::build(20_260_903, 400, 200);
    for a in &plan.assets {
        let bytes = std::fs::read(dir.join(&a.rel_path)).expect("read asset");
        assert!(bytes.len() > 64, "{} is suspiciously small", a.rel_path);
        let magic: &[u8] = match a.format {
            AssetFormat::Pdf => b"%PDF-",
            AssetFormat::Png => &[0x89, b'P', b'N', b'G'],
            AssetFormat::Docx | AssetFormat::Xlsx => b"PK\x03\x04",
            AssetFormat::Jpg => &[0xFF, 0xD8],
        };
        assert!(
            bytes.starts_with(magic),
            "{} does not start with its magic bytes",
            a.rel_path
        );
    }

    // ADR-0005: a binary's metadata lives in a companion Markdown note.
    let sidecars = plan
        .notes
        .iter()
        .filter(|n| n.kind == Kind::Sidecar)
        .count();
    assert!(sidecars > 0, "no sidecar notes were generated");
    for n in plan.notes.iter().filter(|n| n.kind == Kind::Sidecar) {
        assert!(dir.join(&n.rel_path).exists(), "{} missing", n.rel_path);
    }
}

#[test]
fn the_vault_contains_the_encodings_that_break_byte_range_patching() {
    // C-04 / ADR-0008 patch byte ranges. CRLF files, a UTF-8 BOM and multi-byte
    // characters are exactly what makes that hard, so the corpus contains all
    // three — in small doses, because a vault that was mostly CRLF would not be
    // realistic either.
    let dir = TempDir::new("encodings");
    let m = generate(&Options {
        seed: 20_260_903,
        notes: 1_000,
        assets: 12,
        out: dir.path().to_path_buf(),
        manifest: None,
        jobs: None,
        force: true,
        quiet: true,
    })
    .expect("generate");

    assert!(
        m.notes_crlf > 0 && m.notes_crlf < m.notes / 10,
        "{} CRLF notes",
        m.notes_crlf
    );
    assert!(
        m.notes_bom > 0 && m.notes_bom < m.notes / 50,
        "{} BOM notes",
        m.notes_bom
    );
    assert!(
        m.notes_without_frontmatter > 0,
        "no note is missing frontmatter; the schema-less path is untested"
    );

    let multibyte = walk(dir.path())
        .iter()
        .filter(|(p, b)| p.ends_with(".md") && !b.is_ascii())
        .count();
    assert!(
        multibyte > 0,
        "no note contains a multi-byte character; byte offsets and character \
         offsets would be indistinguishable in this corpus"
    );
}
