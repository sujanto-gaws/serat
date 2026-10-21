//! Markdown note rendering.
//!
//! One note's bytes are a pure function of `(seed, note index, plan)`. Nothing
//! here reads the clock, the filesystem or a shared counter, which is what lets
//! notes be written in parallel and still come out byte-identical.
//!
//! The content is chosen to exercise the things P0–P2 have to get right rather
//! than to read well: YAML frontmatter and inline `key:: value` fields
//! (FR-STR-04), tasks with `due::`/`est::`/`owner::` (FR-LNS-05), wikilinks,
//! aliases, heading links, block references, embeds, and deliberate decoys —
//! `key:: value` inside fenced code and code spans, which FR-STR-04 says must
//! *not* produce a field. The manifest records how many of each exist, so a
//! parser test has an expected number to compare against rather than a vibe.

use crate::dates::{iso_date, iso_datetime, weekday};
use crate::plan::{Kind, NoteMeta, Plan, TAG_BODY};
use crate::rng::{Rng, splitmix64, stream_seed};
use crate::words::*;

/// What one rendered note contains. Summed across the vault, this is the
/// expected-value table an index or parser test asserts against.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoteStats {
    /// Size of the file in bytes, as written.
    pub bytes: u64,
    /// Wikilinks, excluding embeds.
    pub links_out: u32,
    /// `![[...]]` embeds, of notes and of assets.
    pub embeds: u32,
    /// The subset of `embeds` that points at a binary rather than a note.
    ///
    /// Reported separately because an asset is a row in `notes` with
    /// `kind='asset'` (ADR-0005), so a rebuild resolves `![[report.pdf]]` like
    /// any other link — while `link_degree.edges` counts note-to-note edges
    /// only. Without this the oracle is short by every asset embed and a
    /// correct rebuild is refused (F-319).
    pub asset_embeds: u32,
    /// `[title](assets/binary.pdf)` — the other way a vault references a binary.
    ///
    /// Counted apart from `asset_embeds` because it is a different construct
    /// with the same consequence: the parser emits a `kind='md'` link, it
    /// resolves to the asset row, and `edges` counts neither. The original
    /// `asset_embeds` design covered only the `![[…]]` half, which was right
    /// when written and became incomplete once a real parser existed to
    /// extract the other one (F-320).
    pub asset_md_links: u32,
    /// Markdown checkboxes.
    pub tasks: u32,
    /// Checkboxes in the `[x]` state.
    pub tasks_done: u32,
    /// `key:: value` occurrences that a conforming parser must index.
    pub inline_fields: u32,
    /// `key:: value` occurrences inside fenced code or code spans, which
    /// FR-STR-04 says must *not* be indexed. The most useful number here.
    pub decoy_fields: u32,
    /// Top-level YAML frontmatter keys.
    pub frontmatter_keys: u32,
    /// `^blockid` anchors.
    pub block_ids: u32,
    /// Block-style `aliases:` items written to frontmatter (one per row a
    /// conforming parser must index under the `aliases` key; task #57).
    pub aliases: u32,
    /// Whether the note opens with a YAML frontmatter block.
    pub has_frontmatter: bool,
    /// Whether the file uses CRLF line endings.
    pub crlf: bool,
    /// Whether the file starts with a UTF-8 byte-order mark.
    pub bom: bool,
}

impl NoteStats {
    /// Fold another note's stats into this one.
    pub fn merge(&mut self, other: &NoteStats) {
        self.bytes += other.bytes;
        self.links_out += other.links_out;
        self.embeds += other.embeds;
        self.asset_embeds += other.asset_embeds;
        self.asset_md_links += other.asset_md_links;
        self.tasks += other.tasks;
        self.tasks_done += other.tasks_done;
        self.inline_fields += other.inline_fields;
        self.decoy_fields += other.decoy_fields;
        self.frontmatter_keys += other.frontmatter_keys;
        self.block_ids += other.block_ids;
        self.aliases += other.aliases;
    }
}

/// The headings a note carries, and therefore the only legal targets of a
/// `[[Note#Heading]]` link into it. Derived from the index alone so the linker
/// can compute them without rendering the target.
pub fn headings_for(index: u32) -> Vec<&'static str> {
    let n = 2 + (index as usize % 4);
    (0..n)
        .map(|j| HEADINGS[(index as usize + j * 7) % HEADINGS.len()])
        .collect()
}

/// The block anchor a note carries, if any. Derived from the index alone, for
/// the same reason as [`headings_for`].
pub fn block_id(index: u32) -> Option<String> {
    if !index.is_multiple_of(5) {
        return None;
    }
    let mut z = u64::from(index) ^ 0xB10C_5EED;
    let h = splitmix64(&mut z);
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::with_capacity(6);
    let mut v = h;
    for _ in 0..6 {
        out.push(ALPHABET[(v % ALPHABET.len() as u64) as usize] as char);
        v /= ALPHABET.len() as u64;
    }
    Some(out)
}

/// Render one note.
///
/// Returns the file bytes, the content summary, and the indices of every note
/// this note links to — the caller accumulates those into the in-degree
/// distribution the manifest reports.
pub fn render(plan: &Plan, note: &NoteMeta) -> (Vec<u8>, NoteStats, Vec<u32>) {
    let mut r = Rng::new(stream_seed(plan.seed, TAG_BODY, u64::from(note.index)));
    let mut stats = NoteStats::default();
    let mut targets: Vec<u32> = Vec::new();
    let mut s = String::with_capacity(2048);

    // Length is long-tailed too: most notes are a screen, a few are essays.
    let target_len = r.lognormal(1250.0, 0.85, 200.0, 40_000.0) as usize;

    // A third of untyped notes have no frontmatter at all. A parser that
    // assumes frontmatter exists should fail on this vault, not in production.
    let wants_frontmatter = match note.kind {
        Kind::Untyped => r.chance(0.66),
        _ => true,
    };
    if wants_frontmatter {
        frontmatter(plan, note, &mut r, &mut s, &mut stats, &mut targets);
    }

    s.push_str("# ");
    s.push_str(&note.title);
    s.push_str("\n\n");

    let headings = headings_for(note.index);
    let mut heading_at = 0usize;

    while s.len() < target_len {
        match r.below(100) {
            0..=29 => paragraph(plan, note, &mut r, &mut s, &mut stats, &mut targets),
            30..=41 => {
                if heading_at < headings.len() {
                    s.push_str("## ");
                    s.push_str(headings[heading_at]);
                    s.push_str("\n\n");
                    heading_at += 1;
                }
                paragraph(plan, note, &mut r, &mut s, &mut stats, &mut targets);
            }
            42..=54 => bullets(plan, note, &mut r, &mut s, &mut stats, &mut targets),
            55..=68 => tasks(plan, note, &mut r, &mut s, &mut stats, &mut targets),
            69..=76 => inline_field_block(&mut r, &mut s, &mut stats),
            77..=84 => code_fence(&mut r, &mut s, &mut stats),
            85..=89 => table(&mut r, &mut s),
            90..=94 => callout(plan, note, &mut r, &mut s, &mut stats, &mut targets),
            _ => embed_block(plan, note, &mut r, &mut s, &mut stats, &mut targets),
        }
    }

    // Every fifth note carries a block anchor, so block references have
    // something real to point at.
    if let Some(id) = block_id(note.index) {
        s.push_str("The paragraph other notes point at when they cite this one. ^");
        s.push_str(&id);
        s.push('\n');
        stats.block_ids += 1;
    }

    // ---- hostile encodings, in small deliberate doses --------------------
    // These are the shapes that break byte-range patching (C-04, ADR-0008) and
    // are documented in docs/05-testing/benchmark-vault.md so nobody mistakes
    // them for a generator bug.
    let crlf = r.chance(0.02);
    let bom = r.chance(0.004);
    if crlf {
        s = s.replace('\n', "\r\n");
        stats.crlf = true;
    }

    let mut bytes = Vec::with_capacity(s.len() + 3);
    if bom {
        bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        stats.bom = true;
    }
    bytes.extend_from_slice(s.as_bytes());
    stats.bytes = bytes.len() as u64;
    stats.has_frontmatter = wants_frontmatter;
    (bytes, stats, targets)
}

// ------------------------------------------------------------- frontmatter

fn frontmatter(
    plan: &Plan,
    note: &NoteMeta,
    r: &mut Rng,
    s: &mut String,
    stats: &mut NoteStats,
    targets: &mut Vec<u32>,
) {
    s.push_str("---\n");
    let mut keys = 0u32;

    if note.kind.is_typed() {
        s.push_str("type: ");
        s.push_str(note.kind.as_str());
        s.push('\n');
        keys += 1;
    }

    // Aliases exercise the resolver's second name for a note.
    if r.chance(0.18) {
        s.push_str("aliases:\n  - \"");
        s.push_str(&alias_for(&note.title, r));
        s.push_str("\"\n");
        keys += 1;
        stats.aliases += 1;
    }

    match note.kind {
        Kind::TypeDef => {
            // A type note declares fields; the schema registry reads these.
            s.push_str("kind: schema\nfields:\n");
            for f in ["status", "owner", "due", "est", "tags"] {
                s.push_str("  - name: ");
                s.push_str(f);
                s.push_str("\n    type: ");
                s.push_str(match f {
                    "due" => "date",
                    "est" => "number",
                    "tags" => "list",
                    _ => "text",
                });
                s.push('\n');
            }
            keys += 2;
        }
        Kind::Daily => {
            s.push_str("date: ");
            s.push_str(&iso_date(note.created_day));
            s.push('\n');
            s.push_str("weekday: ");
            s.push_str(weekday(note.created_day));
            s.push('\n');
            keys += 2;
        }
        Kind::Project => {
            s.push_str("status: ");
            s.push_str(r.pick(STATUSES));
            s.push('\n');
            s.push_str("priority: ");
            s.push_str(r.pick(PRIORITIES));
            s.push('\n');
            let owner = plan.sample_person(note.index, r);
            s.push_str("owner: \"[[");
            s.push_str(&plan.notes[owner as usize].title);
            s.push_str("]]\"\n");
            targets.push(owner);
            stats.links_out += 1;
            s.push_str("due: ");
            s.push_str(&iso_date(note.modified_day + r.range(7, 400)));
            s.push('\n');
            s.push_str("est: ");
            s.push_str(&format!("{:.1}", r.lognormal(8.0, 1.1, 0.5, 400.0)));
            s.push('\n');
            keys += 5;
        }
        Kind::Person => {
            let org = r.pick(ORGS);
            s.push_str("org: \"");
            s.push_str(org);
            s.push_str("\"\nemail: ");
            s.push_str(&format!(
                "{}@{}.example\n",
                crate::plan::slug(&note.title),
                crate::plan::slug(org)
            ));
            s.push_str("role: ");
            s.push_str(r.pick(DOMAIN_NOUNS));
            s.push('\n');
            keys += 3;
        }
        Kind::Meeting => {
            s.push_str("date: ");
            s.push_str(&iso_datetime(
                note.created_day,
                9 * 3600 + 60 * 15 * r.below(4) as u32,
            ));
            s.push('\n');
            s.push_str("attendees:\n");
            for _ in 0..r.range(2, 6) {
                let t = plan.sample_target(note.index, r);
                s.push_str("  - \"[[");
                s.push_str(&plan.notes[t as usize].title);
                s.push_str("]]\"\n");
                targets.push(t);
                stats.links_out += 1;
            }
            keys += 2;
        }
        Kind::Literature => {
            s.push_str("author: \"");
            s.push_str(r.pick(LAST_NAMES));
            s.push_str(", ");
            s.push_str(r.pick(FIRST_NAMES));
            s.push_str("\"\nyear: ");
            s.push_str(&(1998 + r.range(0, 28)).to_string());
            s.push_str("\nvenue: ");
            s.push_str(r.pick(VENUES));
            s.push_str("\ndoi: 10.1000/");
            s.push_str(&format!("{:06}", r.below(999_999)));
            s.push_str("\nrating: ");
            s.push_str(&r.range(1, 5).to_string());
            s.push('\n');
            keys += 5;
        }
        Kind::Area => {
            s.push_str("status: active\nreview: ");
            s.push_str(&iso_date(note.modified_day + 30));
            s.push('\n');
            keys += 2;
        }
        Kind::Sidecar => {
            let asset = &plan.assets[note.asset.expect("sidecar without asset") as usize];
            s.push_str("asset: \"");
            s.push_str(&asset.file_name);
            s.push_str("\"\nformat: ");
            s.push_str(asset.format.ext());
            s.push_str("\npages: ");
            s.push_str(&r.range(1, 80).to_string());
            s.push('\n');
            keys += 3;
        }
        Kind::Zettel | Kind::Untyped => {
            if r.chance(0.6) {
                s.push_str("status: ");
                s.push_str(r.pick(STATUSES));
                s.push('\n');
                keys += 1;
            }
        }
    }

    // Tags, in both YAML shapes a real vault contains.
    let tag_count = r.range(0, 4);
    if tag_count > 0 {
        keys += 1;
        if r.chance(0.5) {
            s.push_str("tags: [");
            for i in 0..tag_count {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(r.pick(TAGS));
            }
            s.push_str("]\n");
        } else {
            s.push_str("tags:\n");
            for _ in 0..tag_count {
                s.push_str("  - ");
                s.push_str(r.pick(TAGS));
                s.push('\n');
            }
        }
    }

    s.push_str("created: ");
    s.push_str(&iso_datetime(
        note.created_day,
        8 * 3600 + r.below(28_000) as u32,
    ));
    s.push_str("\nmodified: ");
    s.push_str(&iso_datetime(
        note.modified_day,
        8 * 3600 + r.below(28_000) as u32,
    ));
    s.push('\n');
    keys += 2;

    s.push_str("---\n\n");
    stats.frontmatter_keys += keys;
}

fn alias_for(title: &str, r: &mut Rng) -> String {
    let first = title.split_whitespace().next().unwrap_or(title);
    let short = first.to_lowercase();
    // A daily note's title is one word, so the short form would equal the
    // title; `[[2026-08-12|2026-08-12]]` is noise, not an alias.
    if r.chance(0.5) && short != title.to_lowercase() {
        short
    } else {
        format!("{} ({})", first, r.pick(MODIFIERS))
    }
}

// ------------------------------------------------------------------ blocks

fn paragraph(
    plan: &Plan,
    note: &NoteMeta,
    r: &mut Rng,
    s: &mut String,
    stats: &mut NoteStats,
    targets: &mut Vec<u32>,
) {
    let sentences = r.range(2, 5);
    for i in 0..sentences {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(r.pick(SENTENCE_OPENERS));
        s.push(' ');
        if r.chance(0.55) {
            s.push_str(&link(plan, note, r, stats, targets));
        } else {
            s.push_str(r.pick(MODIFIERS));
            s.push(' ');
            s.push_str(r.pick(DOMAIN_NOUNS));
        }
        s.push(' ');
        s.push_str(r.pick(SENTENCE_MIDDLES));
        s.push('.');
        if r.chance(0.25) {
            s.push(' ');
            s.push_str(r.pick(SENTENCE_CLOSERS));
        }
        // An inline field mid-sentence, the Dataview bracket form.
        if r.chance(0.12) {
            s.push_str(" (");
            s.push_str(r.pick(&["confidence", "revisit", "source", "effort"]));
            s.push_str(":: ");
            s.push_str(r.pick(&["high", "low", "2026-05-01", "3"]));
            s.push(')');
            stats.inline_fields += 1;
        }
        // A code span containing what looks like a field but is not one.
        if r.chance(0.06) {
            s.push_str(" The literal `status:: blocked` is a code span, not a field.");
            stats.decoy_fields += 1;
        }
    }
    s.push_str("\n\n");
}

fn bullets(
    plan: &Plan,
    note: &NoteMeta,
    r: &mut Rng,
    s: &mut String,
    stats: &mut NoteStats,
    targets: &mut Vec<u32>,
) {
    for _ in 0..r.range(2, 6) {
        s.push_str("- ");
        if r.chance(0.5) {
            s.push_str(&link(plan, note, r, stats, targets));
            s.push_str(" — ");
        }
        s.push_str(r.pick(SENTENCE_OPENERS));
        s.push(' ');
        s.push_str(r.pick(DOMAIN_NOUNS));
        s.push('\n');
        if r.chance(0.3) {
            s.push_str("  - ");
            s.push_str(r.pick(CONCEPTS));
            s.push('\n');
        }
    }
    s.push('\n');
}

fn tasks(
    plan: &Plan,
    note: &NoteMeta,
    r: &mut Rng,
    s: &mut String,
    stats: &mut NoteStats,
    targets: &mut Vec<u32>,
) {
    for _ in 0..r.range(1, 7) {
        let done = r.chance(0.35);
        s.push_str(if done { "- [x] " } else { "- [ ] " });
        s.push_str(r.pick(TASK_VERBS));
        s.push(' ');
        if r.chance(0.4) {
            s.push_str(&link(plan, note, r, stats, targets));
        } else {
            s.push_str("the ");
            s.push_str(r.pick(MODIFIERS));
            s.push(' ');
            s.push_str(r.pick(DOMAIN_NOUNS));
        }

        // Two ways real vaults write task metadata. The bracketed inline fields
        // are what FR-LNS-05 indexes; the emoji form is what the Obsidian Tasks
        // plugin writes, and any parser meeting NFR-CMP-01 sees both.
        if r.chance(0.7) {
            s.push_str(" [due:: ");
            s.push_str(&iso_date(note.modified_day + r.range(-30, 200)));
            s.push(']');
            stats.inline_fields += 1;
            if r.chance(0.7) {
                s.push_str(" [est:: ");
                s.push_str(&format!("{:.1}", r.lognormal(2.0, 0.9, 0.25, 40.0)));
                s.push(']');
                stats.inline_fields += 1;
            }
            if r.chance(0.35) {
                let owner = plan.sample_person(note.index, r);
                s.push_str(" [owner:: [[");
                s.push_str(&plan.notes[owner as usize].title);
                s.push_str("]]]");
                targets.push(owner);
                stats.links_out += 1;
                stats.inline_fields += 1;
            }
        } else {
            s.push_str(" 📅 ");
            s.push_str(&iso_date(note.modified_day + r.range(-30, 200)));
            if r.chance(0.4) {
                s.push_str(" ⏫");
            }
        }
        s.push('\n');
        stats.tasks += 1;
        if done {
            stats.tasks_done += 1;
        }
    }
    s.push('\n');
}

fn inline_field_block(r: &mut Rng, s: &mut String, stats: &mut NoteStats) {
    // The line form: `key:: value` on its own line, anywhere in the body.
    for _ in 0..r.range(2, 5) {
        let key = r.pick(&[
            "status", "owner", "due", "est", "effort", "impact", "reviewed", "source", "stage",
        ]);
        s.push_str(key);
        s.push_str(":: ");
        match key {
            "due" | "reviewed" => s.push_str(&iso_date(r.range(20_000, 21_000))),
            "est" | "effort" | "impact" => {
                s.push_str(&format!("{:.1}", r.lognormal(3.0, 1.0, 0.25, 90.0)))
            }
            "status" | "stage" => s.push_str(r.pick(STATUSES)),
            "owner" => {
                s.push_str(r.pick(FIRST_NAMES));
                s.push(' ');
                s.push_str(r.pick(LAST_NAMES));
            }
            _ => {
                s.push_str(r.pick(MODIFIERS));
                s.push(' ');
                s.push_str(r.pick(DOMAIN_NOUNS));
            }
        }
        s.push('\n');
        stats.inline_fields += 1;
    }
    s.push('\n');
}

fn code_fence(r: &mut Rng, s: &mut String, stats: &mut NoteStats) {
    // FR-STR-04: a `key:: value` inside a fence must produce no field. Every
    // fence here contains one, so a parser that gets this wrong is caught by
    // the count in the manifest rather than by inspection.
    let tilde = r.chance(0.15);
    let fence = if tilde { "~~~" } else { "```" };
    let lang = r.pick(&["", "yaml", "rust", "sql", "text"]);
    s.push_str(fence);
    s.push_str(lang);
    s.push('\n');
    for _ in 0..r.range(2, 5) {
        s.push_str(r.pick(DOMAIN_NOUNS));
        s.push_str(":: ");
        s.push_str(r.pick(STATUSES));
        s.push('\n');
        stats.decoy_fields += 1;
    }
    s.push_str("// not a field, not a link: [[Not A Real Note]]\n");
    s.push_str(fence);
    s.push_str("\n\n");
}

fn table(r: &mut Rng, s: &mut String) {
    s.push_str("| metric | before | after |\n|---|---|---|\n");
    for _ in 0..r.range(2, 5) {
        s.push_str("| ");
        s.push_str(r.pick(DOMAIN_NOUNS));
        s.push_str(&format!(" | {} | {} |\n", r.below(900), r.below(900)));
    }
    s.push('\n');
}

fn callout(
    plan: &Plan,
    note: &NoteMeta,
    r: &mut Rng,
    s: &mut String,
    stats: &mut NoteStats,
    targets: &mut Vec<u32>,
) {
    s.push_str("> [!");
    s.push_str(r.pick(CALLOUT_KINDS));
    s.push_str("]\n> ");
    s.push_str(r.pick(SENTENCE_OPENERS));
    s.push(' ');
    s.push_str(&link(plan, note, r, stats, targets));
    s.push(' ');
    s.push_str(r.pick(SENTENCE_MIDDLES));
    s.push_str(".\n\n");
}

fn embed_block(
    plan: &Plan,
    note: &NoteMeta,
    r: &mut Rng,
    s: &mut String,
    stats: &mut NoteStats,
    targets: &mut Vec<u32>,
) {
    if !plan.assets.is_empty() && r.chance(0.5) {
        let a = &plan.assets[r.below(plan.assets.len() as u64) as usize];
        if r.chance(0.6) {
            s.push_str("![[");
            s.push_str(&a.file_name);
            s.push_str("]]\n\n");
            stats.embeds += 1;
            stats.asset_embeds += 1;
        } else {
            // A Markdown link to a binary, the other way vaults reference them.
            s.push_str(&format!("[{}]({})\n\n", a.title, a.rel_path));
            stats.asset_md_links += 1;
        }
    } else {
        let t = plan.sample_target(note.index, r);
        s.push_str("![[");
        s.push_str(&plan.notes[t as usize].title);
        s.push_str("]]\n\n");
        targets.push(t);
        stats.embeds += 1;
    }
}

/// One wikilink, in one of the five shapes a vault actually contains.
fn link(
    plan: &Plan,
    note: &NoteMeta,
    r: &mut Rng,
    stats: &mut NoteStats,
    targets: &mut Vec<u32>,
) -> String {
    let t = plan.sample_target(note.index, r);
    let target = &plan.notes[t as usize];
    targets.push(t);
    stats.links_out += 1;
    match r.below(100) {
        0..=61 => format!("[[{}]]", target.title),
        62..=76 => format!("[[{}|{}]]", target.title, alias_for(&target.title, r)),
        77..=88 => {
            let hs = headings_for(target.index);
            format!(
                "[[{}#{}]]",
                target.title,
                hs[r.below(hs.len() as u64) as usize]
            )
        }
        _ => match block_id(target.index) {
            Some(id) => format!("[[{}#^{}]]", target.title, id),
            None => format!("[[{}]]", target.title),
        },
    }
}
