//! Deterministic benchmark vault generator for Serat.
//!
//! **Serves NFR-MNT-02** ("a generator for the synthetic benchmark vault shall
//! live in the repository and be runnable by one command") and **US-35**, and
//! it is the artefact every number in NFR-PRF-01..09 is measured against
//! (C-06, architecture §10.3).
//!
//! The contract this crate exists to keep is one sentence: **one seed produces
//! byte-identical output**. Everything unusual about the implementation — its
//! own PRNG, its own SHA-256, zero dependencies, per-note stream seeding —
//! follows from that. If a benchmark regresses, the vault is not a suspect.
//!
//! Run it with one command; see `docs/05-testing/benchmark-vault.md`:
//!
//! ```text
//! cargo run --release -p vaultgen -- --preset 20k
//! ```

pub mod assets;
pub mod dates;
pub mod hash;
pub mod note;
pub mod plan;
pub mod rng;
pub mod words;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use hash::{Sha256, hex};
use note::NoteStats;
use plan::{Kind, Plan};

/// Bumped whenever a change to this crate changes the bytes it emits. A
/// benchmark report records it alongside the seed, so two numbers taken from
/// "the 20k vault" are only ever compared when they mean the same vault.
pub const VAULT_FORMAT_VERSION: u32 = 1;

/// The seed every preset uses. Changing it invalidates every recorded number.
pub const DEFAULT_SEED: u64 = 20_260_903;

/// What to generate.
#[derive(Clone, Debug)]
pub struct Options {
    /// Master seed.
    pub seed: u64,
    /// Total number of `.md` files, including type notes and sidecars.
    pub notes: usize,
    /// Number of binaries under `assets/`.
    pub assets: usize,
    /// Vault root. Created if absent; see `force`.
    pub out: PathBuf,
    /// Where the manifest is written. Defaults to `<out>.manifest.json`,
    /// deliberately *outside* the vault so the vault contains nothing that a
    /// real one would not.
    pub manifest: Option<PathBuf>,
    /// Worker threads. `None` means one per available core.
    pub jobs: Option<usize>,
    /// Overwrite an existing vault. Only ever removes a directory that carries
    /// a `.vaultgen` stamp.
    pub force: bool,
    /// Suppress progress output.
    pub quiet: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            seed: DEFAULT_SEED,
            notes: 20_000,
            assets: 200,
            out: PathBuf::from("bench/vaults/vault-20k"),
            manifest: None,
            jobs: None,
            force: false,
            quiet: false,
        }
    }
}

/// Everything a test or a report needs to know about a generated vault.
#[derive(Clone, Debug, Default)]
pub struct Manifest {
    /// Format version of the generator that produced it.
    pub format_version: u32,
    /// Seed it was generated from.
    pub seed: u64,
    /// Vault root, as given on the command line.
    pub vault: String,
    /// SHA-256 over every file's path and content, in sorted path order. Two
    /// runs of the same seed produce the same digest; that is the whole test.
    pub tree_digest: String,
    /// `.md` files, including type notes and sidecars, excluding `.trash/`.
    pub notes: usize,
    /// Binaries under `assets/`.
    pub assets: usize,
    /// Every file written, including `.obsidian/`, `.trash/` and the stamp.
    pub files: usize,
    /// Directories created.
    pub dirs: usize,
    /// Total bytes of all files.
    pub bytes: u64,
    /// Note count by kind.
    pub notes_by_kind: BTreeMap<String, usize>,
    /// Asset count by format.
    pub assets_by_format: BTreeMap<String, usize>,
    /// Summed content stats over every note.
    pub content: NoteStats,
    /// Notes with no YAML frontmatter at all.
    pub notes_without_frontmatter: usize,
    /// Notes written with CRLF line endings.
    pub notes_crlf: usize,
    /// Notes written with a UTF-8 BOM.
    pub notes_bom: usize,
    /// Files a conforming vault walk must ignore (`.trash/`, `.obsidian/`).
    pub ignored_files: usize,
    /// In-degree distribution of the link graph.
    pub link_degree: LinkDegree,
}

/// The shape of the link graph. Reported because "long-tailed" is a claim, and
/// a claim in a benchmark needs a number attached (architecture §10.3).
#[derive(Clone, Debug, Default)]
pub struct LinkDegree {
    /// Total wikilink edges, embeds included.
    pub edges: u64,
    /// Backlinks held by the most-linked note.
    pub max: u32,
    /// Median in-degree.
    pub p50: u32,
    /// 90th percentile in-degree.
    pub p90: u32,
    /// 99th percentile in-degree.
    pub p99: u32,
    /// Notes with no inbound link at all.
    pub orphans: usize,
    /// Share of all inbound links held by the top 1% of notes, 0.0–1.0.
    pub top_1pct_share: f64,
}

/// Generate the vault.
///
/// # Errors
/// Any filesystem failure, and a refusal if `out` is a non-empty directory that
/// this generator did not create.
pub fn generate(opts: &Options) -> io::Result<Manifest> {
    prepare_out_dir(&opts.out, opts.force)?;

    let plan = Plan::build(opts.seed, opts.notes, opts.assets);

    for dir in &plan.dirs {
        fs::create_dir_all(opts.out.join(dir))?;
    }

    let jobs = opts
        .jobs
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(1)
        .max(1);

    // ---- notes, in parallel -------------------------------------------------
    // Worker `w` takes notes w, w+jobs, w+2*jobs...  Content depends only on
    // (seed, index), so the striping changes throughput and nothing else.
    struct Shard {
        stats: NoteStats,
        no_frontmatter: usize,
        crlf: usize,
        bom: usize,
        in_degree: Vec<u32>,
        hashes: Vec<(String, [u8; 32])>,
        error: Option<io::Error>,
    }

    let plan_ref = &plan;
    let out_ref = opts.out.as_path();
    let shards: Vec<Shard> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..jobs)
            .map(|w| {
                scope.spawn(move || {
                    let mut shard = Shard {
                        stats: NoteStats::default(),
                        no_frontmatter: 0,
                        crlf: 0,
                        bom: 0,
                        in_degree: vec![0u32; plan_ref.notes.len()],
                        hashes: Vec::new(),
                        error: None,
                    };
                    let mut i = w;
                    while i < plan_ref.notes.len() {
                        let meta = &plan_ref.notes[i];
                        let (bytes, stats, targets) = note::render(plan_ref, meta);
                        if let Err(e) = fs::write(out_ref.join(&meta.rel_path), &bytes) {
                            shard.error = Some(e);
                            return shard;
                        }
                        shard
                            .hashes
                            .push((meta.rel_path.clone(), Sha256::digest(&bytes)));
                        shard.stats.merge(&stats);
                        if !stats.has_frontmatter {
                            shard.no_frontmatter += 1;
                        }
                        if stats.crlf {
                            shard.crlf += 1;
                        }
                        if stats.bom {
                            shard.bom += 1;
                        }
                        for t in targets {
                            shard.in_degree[t as usize] += 1;
                        }
                        i += jobs;
                    }
                    shard
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("note worker panicked"))
            .collect()
    });

    let mut content = NoteStats::default();
    let mut in_degree = vec![0u32; plan.notes.len()];
    let mut hashes: Vec<(String, [u8; 32])> =
        Vec::with_capacity(plan.notes.len() + plan.assets.len() + 8);
    let (mut no_frontmatter, mut crlf, mut bom) = (0usize, 0usize, 0usize);
    for shard in shards {
        if let Some(e) = shard.error {
            return Err(e);
        }
        content.merge(&shard.stats);
        no_frontmatter += shard.no_frontmatter;
        crlf += shard.crlf;
        bom += shard.bom;
        for (i, d) in shard.in_degree.iter().enumerate() {
            in_degree[i] += d;
        }
        hashes.extend(shard.hashes);
    }

    // ---- assets -------------------------------------------------------------
    let mut total_bytes = content.bytes;
    for asset in &plan.assets {
        let bytes = assets::render(opts.seed, asset);
        fs::write(opts.out.join(&asset.rel_path), &bytes)?;
        total_bytes += bytes.len() as u64;
        hashes.push((asset.rel_path.clone(), Sha256::digest(&bytes)));
    }

    // ---- files a conforming walk must ignore --------------------------------
    // FR-VLT/ignore rules have to be tested against something. A vault with no
    // `.obsidian/` and no `.trash/` would let a walker that ignores nothing
    // pass, and then fail on the first real vault it meets.
    let ignored: Vec<(&str, Vec<u8>)> = vec![
        (
            ".obsidian/app.json",
            b"{\n  \"alwaysUpdateLinks\": true,\n  \"newFileLocation\": \"folder\"\n}\n".to_vec(),
        ),
        (
            ".obsidian/core-plugins.json",
            b"[\n  \"file-explorer\",\n  \"global-search\",\n  \"backlink\"\n]\n".to_vec(),
        ),
        (
            ".trash/Deleted draft.md",
            b"---\ntype: note\n---\n\n# Deleted draft\n\nThis note is in the trash and must not be indexed. [[Should Not Resolve]]\n".to_vec(),
        ),
        (
            ".trash/Deleted meeting.md",
            b"---\ntype: meeting\n---\n\n# Deleted meeting\n\n- [ ] A task that must not appear in any task query [due:: 2026-01-01]\n".to_vec(),
        ),
    ];
    for (rel, bytes) in &ignored {
        fs::write(opts.out.join(rel), bytes)?;
        total_bytes += bytes.len() as u64;
        hashes.push(((*rel).to_string(), Sha256::digest(bytes)));
    }

    // ---- stamp --------------------------------------------------------------
    // Doubles as the guard that stops `--force` from deleting a directory this
    // generator did not create.
    let stamp = format!(
        "{{\"generator\":\"vaultgen\",\"format_version\":{},\"seed\":{},\"notes\":{},\"assets\":{}}}\n",
        VAULT_FORMAT_VERSION, opts.seed, opts.notes, opts.assets
    );
    fs::write(opts.out.join(".vaultgen"), stamp.as_bytes())?;
    total_bytes += stamp.len() as u64;
    hashes.push((".vaultgen".to_string(), Sha256::digest(stamp.as_bytes())));

    // ---- tree digest --------------------------------------------------------
    // Sorted by path, so thread count and write order cannot affect it.
    hashes.sort_by(|a, b| a.0.cmp(&b.0));
    let mut tree = Sha256::new();
    for (path, digest) in &hashes {
        tree.update(path.as_bytes());
        tree.update(b"\0");
        tree.update(digest);
        tree.update(b"\n");
    }
    let tree_digest = hex(&tree.finish());

    // ---- manifest -----------------------------------------------------------
    let mut notes_by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for n in &plan.notes {
        *notes_by_kind
            .entry(kind_name(n.kind).to_string())
            .or_default() += 1;
    }
    let mut assets_by_format: BTreeMap<String, usize> = BTreeMap::new();
    for a in &plan.assets {
        *assets_by_format
            .entry(a.format.ext().to_string())
            .or_default() += 1;
    }

    let manifest = Manifest {
        format_version: VAULT_FORMAT_VERSION,
        seed: opts.seed,
        vault: opts.out.display().to_string().replace('\\', "/"),
        tree_digest,
        notes: plan.notes.len(),
        assets: plan.assets.len(),
        files: hashes.len(),
        dirs: plan.dirs.len(),
        bytes: total_bytes,
        notes_by_kind,
        assets_by_format,
        content,
        notes_without_frontmatter: no_frontmatter,
        notes_crlf: crlf,
        notes_bom: bom,
        ignored_files: ignored.len(),
        link_degree: link_degree(&in_degree),
    };

    let manifest_path = opts
        .manifest
        .clone()
        .unwrap_or_else(|| manifest_path_for(&opts.out));
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = fs::File::create(&manifest_path)?;
    f.write_all(manifest.to_json().as_bytes())?;

    Ok(manifest)
}

/// Default manifest location for a vault directory: `<out>.manifest.json`.
pub fn manifest_path_for(out: &Path) -> PathBuf {
    let name = out
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "vault".to_string());
    match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join(format!("{name}.manifest.json")),
        _ => PathBuf::from(format!("{name}.manifest.json")),
    }
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::TypeDef => "type",
        Kind::Daily => "daily",
        Kind::Project => "project",
        Kind::Person => "person",
        Kind::Meeting => "meeting",
        Kind::Literature => "literature",
        Kind::Area => "area",
        Kind::Zettel => "note",
        Kind::Untyped => "untyped",
        Kind::Sidecar => "sidecar",
    }
}

fn link_degree(in_degree: &[u32]) -> LinkDegree {
    if in_degree.is_empty() {
        return LinkDegree::default();
    }
    let edges: u64 = in_degree.iter().map(|d| u64::from(*d)).sum();
    let orphans = in_degree.iter().filter(|d| **d == 0).count();
    let mut sorted = in_degree.to_vec();
    sorted.sort_unstable();
    let at = |q: f64| sorted[((sorted.len() as f64 - 1.0) * q) as usize];
    let top_n = (sorted.len() / 100).max(1);
    let top_sum: u64 = sorted[sorted.len() - top_n..]
        .iter()
        .map(|d| u64::from(*d))
        .sum();
    LinkDegree {
        edges,
        max: *sorted.last().unwrap_or(&0),
        p50: at(0.50),
        p90: at(0.90),
        p99: at(0.99),
        orphans,
        top_1pct_share: if edges == 0 {
            0.0
        } else {
            top_sum as f64 / edges as f64
        },
    }
}

fn prepare_out_dir(out: &Path, force: bool) -> io::Result<()> {
    if out.exists() {
        let empty = fs::read_dir(out)?.next().is_none();
        if !empty {
            if !force {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "{} exists and is not empty; pass --force to regenerate it",
                        out.display()
                    ),
                ));
            }
            // Refuse to delete anything this generator did not write. `--force`
            // on a mistyped path must not be able to remove a real vault.
            if !out.join(".vaultgen").exists() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "{} is not empty and has no .vaultgen stamp; refusing to delete it",
                        out.display()
                    ),
                ));
            }
            fs::remove_dir_all(out)?;
        }
    }
    fs::create_dir_all(out)?;
    Ok(())
}

impl Manifest {
    /// Serialise to JSON. Hand-written because this crate has no dependencies;
    /// every value is a number or an ASCII path, so escaping is minimal.
    pub fn to_json(&self) -> String {
        let mut s = String::with_capacity(4096);
        s.push_str("{\n");
        s.push_str("  \"generator\": \"vaultgen\",\n");
        s.push_str("  \"serves\": [\"NFR-MNT-02\", \"US-35\", \"C-06\"],\n");
        s.push_str(&format!("  \"format_version\": {},\n", self.format_version));
        s.push_str(&format!("  \"seed\": {},\n", self.seed));
        s.push_str(&format!("  \"vault\": \"{}\",\n", json_escape(&self.vault)));
        s.push_str(&format!(
            "  \"tree_digest_sha256\": \"{}\",\n",
            self.tree_digest
        ));
        s.push_str("  \"counts\": {\n");
        s.push_str(&format!("    \"notes\": {},\n", self.notes));
        s.push_str(&format!("    \"assets\": {},\n", self.assets));
        s.push_str(&format!("    \"files\": {},\n", self.files));
        s.push_str(&format!("    \"dirs\": {},\n", self.dirs));
        s.push_str(&format!("    \"bytes\": {},\n", self.bytes));
        s.push_str(&format!("    \"ignored_files\": {}\n", self.ignored_files));
        s.push_str("  },\n");
        s.push_str("  \"notes_by_kind\": {\n");
        push_map(&mut s, &self.notes_by_kind);
        s.push_str("  },\n");
        s.push_str("  \"assets_by_format\": {\n");
        push_map(&mut s, &self.assets_by_format);
        s.push_str("  },\n");
        s.push_str("  \"content\": {\n");
        s.push_str(&format!("    \"links_out\": {},\n", self.content.links_out));
        s.push_str(&format!("    \"embeds\": {},\n", self.content.embeds));
        s.push_str(&format!(
            "    \"asset_embeds\": {},\n",
            self.content.asset_embeds
        ));
        s.push_str(&format!(
            "    \"asset_md_links\": {},\n",
            self.content.asset_md_links
        ));
        s.push_str(&format!("    \"tasks\": {},\n", self.content.tasks));
        s.push_str(&format!(
            "    \"tasks_done\": {},\n",
            self.content.tasks_done
        ));
        s.push_str(&format!(
            "    \"inline_fields\": {},\n",
            self.content.inline_fields
        ));
        s.push_str(&format!(
            "    \"decoy_fields\": {},\n",
            self.content.decoy_fields
        ));
        s.push_str(&format!(
            "    \"frontmatter_keys\": {},\n",
            self.content.frontmatter_keys
        ));
        s.push_str(&format!("    \"block_ids\": {},\n", self.content.block_ids));
        s.push_str(&format!("    \"aliases\": {},\n", self.content.aliases));
        s.push_str(&format!(
            "    \"notes_without_frontmatter\": {},\n",
            self.notes_without_frontmatter
        ));
        s.push_str(&format!("    \"notes_crlf\": {},\n", self.notes_crlf));
        s.push_str(&format!("    \"notes_bom\": {}\n", self.notes_bom));
        s.push_str("  },\n");
        s.push_str("  \"link_degree\": {\n");
        s.push_str(&format!("    \"edges\": {},\n", self.link_degree.edges));
        s.push_str(&format!("    \"max\": {},\n", self.link_degree.max));
        s.push_str(&format!("    \"p50\": {},\n", self.link_degree.p50));
        s.push_str(&format!("    \"p90\": {},\n", self.link_degree.p90));
        s.push_str(&format!("    \"p99\": {},\n", self.link_degree.p99));
        s.push_str(&format!("    \"orphans\": {},\n", self.link_degree.orphans));
        s.push_str(&format!(
            "    \"top_1pct_share\": {:.4}\n",
            self.link_degree.top_1pct_share
        ));
        s.push_str("  }\n");
        s.push_str("}\n");
        s
    }
}

fn push_map(s: &mut String, map: &BTreeMap<String, usize>) {
    let last = map.len().saturating_sub(1);
    for (i, (k, v)) in map.iter().enumerate() {
        s.push_str(&format!(
            "    \"{}\": {}{}\n",
            json_escape(k),
            v,
            if i == last { "" } else { "," }
        ));
    }
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
