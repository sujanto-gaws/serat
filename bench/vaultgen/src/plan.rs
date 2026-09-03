//! The vault plan: what notes exist, where they live, and who links to whom.
//!
//! Everything here is computed before a single byte is written, for two
//! reasons. A note's body needs the titles of its link targets, and note bodies
//! are generated in parallel — so the plan has to be a pure function of the
//! seed, computed once, and note `i`'s body must depend on nothing but the plan
//! and `stream_seed(seed, tag, i)`.

use std::collections::HashSet;

use crate::dates::{days_from_civil, iso_date};
use crate::rng::{Rng, Zipf, stream_seed};
use crate::words::*;

/// Stream tag for the structural pass (titles, paths, dates).
pub const TAG_STRUCTURE: u64 = 1;
/// Stream tag for hub scoring.
pub const TAG_POPULARITY: u64 = 2;
/// Stream tag for note body generation.
pub const TAG_BODY: u64 = 3;
/// Stream tag for asset planning and asset bytes.
pub const TAG_ASSET: u64 = 4;

/// The day every generated vault is anchored to. Fixed, not "today": a vault
/// whose dates moved with the calendar would not be byte-identical tomorrow.
pub const ANCHOR: (i64, u32, u32) = (2026, 8, 31);

/// What a note is. Drives its folder, its frontmatter and its body shape.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Schema-defining note in `_types/` (FR-STR-01).
    TypeDef,
    /// Journal note, one per consecutive day.
    Daily,
    /// A project with owner, status, due date and estimate rollups.
    Project,
    /// A person, the classic backlink hub.
    Person,
    /// A dated meeting note full of tasks and attendee links.
    Meeting,
    /// A reading note with author, year and citation fields.
    Literature,
    /// A map-of-content note; the heaviest hubs in the vault.
    Area,
    /// Typed `note` — the ordinary evergreen note.
    Zettel,
    /// No `type:` key at all; a third of these have no frontmatter whatsoever.
    Untyped,
    /// Companion note for a binary (ADR-0005).
    Sidecar,
}

impl Kind {
    /// The value written to the `type:` frontmatter key.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::TypeDef => "type",
            Kind::Daily => "daily",
            Kind::Project => "project",
            Kind::Person => "person",
            Kind::Meeting => "meeting",
            Kind::Literature => "literature",
            Kind::Area => "area",
            Kind::Zettel => "note",
            Kind::Untyped => "untyped",
            Kind::Sidecar => "asset",
        }
    }

    /// Whether the generated note carries a `type:` key in frontmatter.
    pub fn is_typed(self) -> bool {
        !matches!(self, Kind::Untyped)
    }

    /// How much more likely than baseline this kind is to become a hub. Real
    /// vaults link to people, projects and areas far more than to daily notes.
    fn hub_boost(self) -> f64 {
        match self {
            Kind::Area => 6.0,
            Kind::TypeDef => 4.0,
            Kind::Project => 3.5,
            Kind::Person => 3.0,
            Kind::Zettel => 1.0,
            Kind::Literature => 0.8,
            Kind::Untyped => 0.7,
            Kind::Meeting => 0.35,
            Kind::Sidecar => 0.3,
            Kind::Daily => 0.15,
        }
    }
}

/// The twelve type notes. Fixed, because the schema registry is a fixed shape
/// and a query suite written against it should not shift with the seed.
pub const TYPE_NAMES: &[&str] = &[
    "Project",
    "Person",
    "Meeting",
    "Literature",
    "Area",
    "Task",
    "Decision",
    "Incident",
    "Experiment",
    "Company",
    "Source",
    "Note",
];

/// One planned note.
#[derive(Clone, Debug)]
pub struct NoteMeta {
    /// Position in `Plan::notes`; also the ordinal of its content stream.
    pub index: u32,
    /// What the note is.
    pub kind: Kind,
    /// Filename stem, and therefore the wikilink target: `[[title]]`.
    pub title: String,
    /// Path relative to the vault root, forward slashes, always ends `.md`.
    pub rel_path: String,
    /// Civil day number of the `created` frontmatter value.
    pub created_day: i64,
    /// Civil day number of the `modified` frontmatter value.
    pub modified_day: i64,
    /// Index into `Plan::assets`, for sidecar notes only.
    pub asset: Option<u32>,
}

/// The five binary formats the vault carries (architecture §10.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssetFormat {
    /// Structurally valid single-page PDF with extractable text.
    Pdf,
    /// Structurally valid 8-bit RGB PNG.
    Png,
    /// Structurally valid OOXML word processing document.
    Docx,
    /// Structurally valid OOXML spreadsheet.
    Xlsx,
    /// Header-only JPEG placeholder — see the README.
    Jpg,
}

impl AssetFormat {
    /// Lower-case filename extension, without the dot.
    pub fn ext(self) -> &'static str {
        match self {
            AssetFormat::Pdf => "pdf",
            AssetFormat::Png => "png",
            AssetFormat::Docx => "docx",
            AssetFormat::Xlsx => "xlsx",
            AssetFormat::Jpg => "jpg",
        }
    }
}

/// One planned binary.
#[derive(Clone, Debug)]
pub struct AssetMeta {
    /// Position in `Plan::assets`; also the ordinal of its content stream.
    pub index: u32,
    /// Which format.
    pub format: AssetFormat,
    /// Filename including extension.
    pub file_name: String,
    /// Path relative to the vault root, forward slashes.
    pub rel_path: String,
    /// Human title, reused by the sidecar note and by embedding notes.
    pub title: String,
    /// Whether a companion Markdown note is planned for it (ADR-0005).
    pub has_sidecar: bool,
}

/// The whole vault, decided before anything is written.
#[derive(Debug)]
pub struct Plan {
    /// Master seed this plan was derived from.
    pub seed: u64,
    /// Every note, in write order.
    pub notes: Vec<NoteMeta>,
    /// Every binary, in write order.
    pub assets: Vec<AssetMeta>,
    /// `popularity[rank]` is the note index at that rank; rank 0 is the most
    /// linked-to note in the vault.
    pub popularity: Vec<u32>,
    /// Rank sampler over `notes.len()`.
    pub zipf: Zipf,
    /// Indices of every `Kind::Person` note. Owner fields sample from these
    /// rather than from the whole vault: a `[owner:: [[Some Meeting]]]` would
    /// make the P3 rollup tests meaningless.
    pub people: Vec<u32>,
    /// Every directory that must exist before parallel writing starts.
    pub dirs: Vec<String>,
}

/// Proportions by kind, in parts per thousand of the free note budget. Type
/// notes and sidecars are allocated separately — they are pinned to
/// `TYPE_NAMES` and to the asset count — so these sum over the remainder.
const MIX: &[(Kind, u32)] = &[
    (Kind::Daily, 70),
    (Kind::Project, 40),
    (Kind::Person, 30),
    (Kind::Meeting, 90),
    (Kind::Literature, 80),
    (Kind::Area, 10),
    (Kind::Zettel, 330),
    (Kind::Untyped, 350),
];

/// Asset format proportions, in parts per thousand.
const ASSET_MIX: &[(AssetFormat, u32)] = &[
    (AssetFormat::Pdf, 450),
    (AssetFormat::Png, 250),
    (AssetFormat::Docx, 120),
    (AssetFormat::Xlsx, 100),
    (AssetFormat::Jpg, 80),
];

impl Plan {
    /// Build the plan.
    ///
    /// `note_budget` counts every `.md` file the vault will contain, including
    /// the twelve type notes and the asset sidecars, so `--notes 20000` really
    /// does produce 20,000 notes and not 20,000 plus change.
    ///
    /// # Panics
    /// If `note_budget` is too small to hold the fixed structure.
    pub fn build(seed: u64, note_budget: usize, asset_count: usize) -> Plan {
        let anchor = days_from_civil(ANCHOR.0, ANCHOR.1, ANCHOR.2);
        let mut rng = Rng::new(stream_seed(seed, TAG_STRUCTURE, 0));

        // Assets are planned first: their sidecars consume part of the budget.
        let assets = Self::plan_assets(seed, asset_count);
        let sidecar_count = assets.iter().filter(|a| a.has_sidecar).count();

        let fixed = TYPE_NAMES.len() + sidecar_count;
        assert!(
            note_budget > fixed + MIX.len(),
            "note budget {note_budget} too small for {fixed} fixed notes plus one of each kind"
        );
        let free = note_budget - fixed;

        // ---- how many of each kind ----------------------------------------
        let total_parts: u32 = MIX.iter().map(|(_, p)| p).sum();
        let mut counts: Vec<(Kind, usize)> = Vec::with_capacity(MIX.len());
        let mut allocated = 0usize;
        for (i, (kind, parts)) in MIX.iter().enumerate() {
            let n = if i == MIX.len() - 1 {
                free - allocated
            } else {
                (free as u64 * u64::from(*parts) / u64::from(total_parts)) as usize
            };
            allocated += n;
            counts.push((*kind, n));
        }

        let mut notes: Vec<NoteMeta> = Vec::with_capacity(note_budget);
        let mut taken: HashSet<String> = HashSet::with_capacity(note_budget * 2);
        let mut dirs: HashSet<String> = HashSet::new();
        dirs.insert("assets".to_string());
        dirs.insert(".obsidian".to_string());
        dirs.insert(".trash".to_string());

        // ---- the twelve type notes ----------------------------------------
        for name in TYPE_NAMES {
            let idx = notes.len() as u32;
            let title = unique_title(&mut taken, name);
            dirs.insert("_types".to_string());
            notes.push(NoteMeta {
                index: idx,
                kind: Kind::TypeDef,
                rel_path: format!("_types/{title}.md"),
                title,
                created_day: anchor - 900,
                modified_day: anchor - rng.range(0, 400),
                asset: None,
            });
        }

        // ---- the rest ------------------------------------------------------
        for (kind, n) in counts {
            for k in 0..n {
                let idx = notes.len() as u32;
                let mut r = Rng::new(stream_seed(seed, TAG_STRUCTURE, u64::from(idx) + 1_000));
                let (raw_title, rel_dir, created, modified) =
                    Self::plan_one(kind, k, anchor, &mut r);
                let title = unique_title(&mut taken, &raw_title);
                let rel_path = if rel_dir.is_empty() {
                    format!("{title}.md")
                } else {
                    dirs.insert(rel_dir.clone());
                    format!("{rel_dir}/{title}.md")
                };
                notes.push(NoteMeta {
                    index: idx,
                    kind,
                    title,
                    rel_path,
                    created_day: created,
                    modified_day: modified,
                    asset: None,
                });
            }
        }

        // ---- sidecar notes, paired with their binaries ---------------------
        for asset in assets.iter().filter(|a| a.has_sidecar) {
            let idx = notes.len() as u32;
            // ADR-0005 does not pin the sidecar filename. `<binary>.md` is used
            // here because it cannot collide when report-0001.pdf and
            // report-0001.png both exist. See docs/05-testing/benchmark-vault.md.
            let title = unique_title(&mut taken, &asset.file_name);
            let mut r = Rng::new(stream_seed(seed, TAG_STRUCTURE, 9_000_000 + u64::from(idx)));
            notes.push(NoteMeta {
                index: idx,
                kind: Kind::Sidecar,
                rel_path: format!("assets/{title}.md"),
                title,
                created_day: anchor - r.range(30, 1400),
                modified_day: anchor - r.range(0, 30),
                asset: Some(asset.index),
            });
        }

        assert_eq!(notes.len(), note_budget, "note accounting drifted");

        // ---- popularity: who becomes a hub ---------------------------------
        // An exponential score scaled by kind. Sorted descending it becomes the
        // permutation the Zipf sampler draws through, which is what turns a
        // uniform link draw into a long-tailed in-degree distribution.
        let mut scored: Vec<(f64, u32)> = notes
            .iter()
            .map(|n| {
                let mut r = Rng::new(stream_seed(seed, TAG_POPULARITY, u64::from(n.index)));
                let base = -(r.unit().max(f64::MIN_POSITIVE)).ln();
                (base * n.kind.hub_boost(), n.index)
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        let popularity: Vec<u32> = scored.into_iter().map(|(_, i)| i).collect();

        // Exponent 1.0 with an offset of 50 puts roughly a quarter of all links
        // in the top 1% of notes and leaves a long thin tail — see the Zipf
        // docs for why a pure Zipf is the wrong shape here.
        let zipf = Zipf::new(notes.len(), 1.0, 50.0);

        let people: Vec<u32> = notes
            .iter()
            .filter(|n| n.kind == Kind::Person)
            .map(|n| n.index)
            .collect();

        let mut dirs: Vec<String> = dirs.into_iter().collect();
        dirs.sort();

        Plan {
            seed,
            notes,
            assets,
            popularity,
            zipf,
            people,
            dirs,
        }
    }

    fn plan_one(kind: Kind, k: usize, anchor: i64, r: &mut Rng) -> (String, String, i64, i64) {
        match kind {
            Kind::Daily => {
                // Consecutive days ending at the anchor, so the daily folder
                // looks like a journal rather than a random scatter.
                let day = anchor - k as i64;
                let iso = iso_date(day);
                let dir = format!("daily/{}/{}", &iso[0..4], &iso[5..7]);
                (iso, dir, day, day)
            }
            Kind::Project => {
                let title = format!("{} {}", r.pick(PROJECT_CODENAMES), r.pick(DOMAIN_NOUNS));
                let created = anchor - r.range(60, 1600);
                let modified = (created + r.range(1, 400)).min(anchor);
                (title, "projects".into(), created, modified)
            }
            Kind::Person => {
                let title = format!("{} {}", r.pick(FIRST_NAMES), r.pick(LAST_NAMES));
                let created = anchor - r.range(30, 1800);
                (title, "people".into(), created, anchor - r.range(0, 300))
            }
            Kind::Meeting => {
                let day = anchor - r.range(0, 1400);
                let iso = iso_date(day);
                let title = format!("{} {} sync", iso, r.pick(PROJECT_CODENAMES));
                let dir = format!("meetings/{}", &iso[0..7]);
                (title, dir, day, day)
            }
            Kind::Literature => {
                let year = 1998 + r.range(0, 28);
                let title = format!("{} {} {}", r.pick(LAST_NAMES), year, r.pick(DOMAIN_NOUNS));
                let created = anchor - r.range(0, 1700);
                let modified = (created + r.range(0, 90)).min(anchor);
                (title, "literature".into(), created, modified)
            }
            Kind::Area => {
                let title = capitalise(r.pick(DOMAIN_NOUNS));
                (
                    title,
                    "areas".into(),
                    anchor - 1500,
                    anchor - r.range(0, 200),
                )
            }
            Kind::Zettel => {
                let title = match r.below(3) {
                    0 => format!("{} {}", r.pick(ACTIONS), r.pick(DOMAIN_NOUNS)),
                    1 => format!("{} {}", capitalise(r.pick(MODIFIERS)), r.pick(DOMAIN_NOUNS)),
                    _ => capitalise(r.pick(CONCEPTS)),
                };
                let created = anchor - r.range(0, 1800);
                let modified = (created + r.range(0, 300)).min(anchor);
                (title, "notes".into(), created, modified)
            }
            Kind::Untyped => {
                let title = match r.below(3) {
                    0 => format!("{} {}", r.pick(ACTIONS), r.pick(CONCEPTS)),
                    1 => format!(
                        "{} {} {}",
                        capitalise(r.pick(MODIFIERS)),
                        r.pick(DOMAIN_NOUNS),
                        r.pick(DOMAIN_NOUNS)
                    ),
                    _ => capitalise(r.pick(DOMAIN_NOUNS)),
                };
                // Untyped notes are where the mess lives: an inbox, loose notes,
                // and files sitting in the vault root.
                let dir = match r.below(10) {
                    0..=3 => "inbox",
                    4..=7 => "notes",
                    _ => "",
                };
                let created = anchor - r.range(0, 1900);
                let modified = (created + r.range(0, 500)).min(anchor);
                (title, dir.into(), created, modified)
            }
            Kind::TypeDef | Kind::Sidecar => unreachable!("allocated separately"),
        }
    }

    fn plan_assets(seed: u64, count: usize) -> Vec<AssetMeta> {
        let total_parts: u32 = ASSET_MIX.iter().map(|(_, p)| p).sum();
        let mut formats: Vec<AssetFormat> = Vec::with_capacity(count);
        let mut allocated = 0usize;
        for (i, (fmt, parts)) in ASSET_MIX.iter().enumerate() {
            let n = if i == ASSET_MIX.len() - 1 {
                count - allocated
            } else {
                (count as u64 * u64::from(*parts) / u64::from(total_parts)) as usize
            };
            allocated += n;
            for _ in 0..n {
                formats.push(fmt.to_owned());
            }
        }

        let mut out = Vec::with_capacity(count);
        for (i, fmt) in formats.into_iter().enumerate() {
            let mut r = Rng::new(stream_seed(seed, TAG_ASSET, i as u64));
            let noun = match fmt {
                AssetFormat::Pdf => "report",
                AssetFormat::Docx => "brief",
                AssetFormat::Xlsx => "model",
                AssetFormat::Png => "diagram",
                AssetFormat::Jpg => "photo",
            };
            let title = format!(
                "{} {} {noun}",
                capitalise(r.pick(MODIFIERS)),
                r.pick(DOMAIN_NOUNS)
            );
            // Asset filenames are ASCII, lower case and index-suffixed: unique
            // on a case-insensitive filesystem and short enough for Windows.
            let file_name = format!("{}-{:04}.{}", slug(&title), i, fmt.ext());
            out.push(AssetMeta {
                index: i as u32,
                format: fmt,
                rel_path: format!("assets/{file_name}"),
                file_name,
                title,
                has_sidecar: i % 4 == 0,
            });
        }
        out
    }

    /// Sample a person note. Falls back to any note in a vault too small to
    /// contain one.
    pub fn sample_person(&self, from: u32, rng: &mut Rng) -> u32 {
        if self.people.is_empty() {
            return self.sample_target(from, rng);
        }
        self.people[rng.below(self.people.len() as u64) as usize]
    }

    /// Sample a link target for note `from`, never `from` itself.
    pub fn sample_target(&self, from: u32, rng: &mut Rng) -> u32 {
        for _ in 0..8 {
            let target = self.popularity[self.zipf.sample(rng)];
            if target != from {
                return target;
            }
        }
        // Degenerate fallback; reachable only in a vault of one note.
        self.popularity[(from as usize + 1) % self.popularity.len()]
    }
}

/// A stable ASCII slug: lower case, alphanumerics separated by single hyphens.
pub fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_dash = true;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Windows device names. A file called `aux.md` cannot be created on Windows at
/// all, so the generator must never plan one.
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Make a filename stem that is legal everywhere and unique case-insensitively.
///
/// Uniqueness is enforced on the *stem*, not on the path, because a wikilink is
/// `[[stem]]`: two notes with the same stem in different folders would make
/// every link to either of them ambiguous, and link resolution is not what the
/// benchmark is here to measure.
fn unique_title(taken: &mut HashSet<String>, raw: &str) -> String {
    let mut base = sanitise(raw);
    if base.is_empty() {
        base = "note".to_string();
    }
    let mut candidate = base.clone();
    let mut n = 2;
    while !taken.insert(candidate.to_lowercase()) {
        candidate = format!("{base} {n}");
        n += 1;
    }
    candidate
}

fn sanitise(raw: &str) -> String {
    const ILLEGAL: &[char] = &[
        '<', '>', ':', '"', '/', '\\', '|', '?', '*', '#', '^', '[', ']',
    ];
    let mut out: String = raw
        .chars()
        .filter(|c| c.is_ascii() && !c.is_ascii_control() && !ILLEGAL.contains(c))
        .collect();
    out = out.trim().to_string();
    if out.len() > 60 {
        out.truncate(60);
    }
    // Windows silently strips trailing dots and spaces, which would make the
    // written path differ from the planned one.
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }
    let lower = out.to_lowercase();
    if RESERVED.contains(&lower.split('.').next().unwrap_or("")) {
        out.push_str(" note");
    }
    out
}
