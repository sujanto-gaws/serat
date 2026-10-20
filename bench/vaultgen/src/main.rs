//! `vaultgen` — the one documented command that builds the Serat benchmark
//! vault (NFR-MNT-02, US-35).
//!
//! ```text
//! cargo run --release -p vaultgen -- --preset 20k
//! cargo run --release -p vaultgen -- --preset 50k
//! cargo run --release -p vaultgen -- --preset smoke
//! ```
//!
//! Argument parsing is hand-written for the same reason the PRNG is: this crate
//! has no dependencies, so the vault can be generated on a machine with no
//! registry access and no build cache.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use vaultgen::{DEFAULT_SEED, Options, VAULT_FORMAT_VERSION, generate, manifest_path_for};

const USAGE: &str = "\
vaultgen — deterministic Serat benchmark vault generator (NFR-MNT-02, US-35)

USAGE:
    cargo run --release -p vaultgen -- --preset <20k|50k|smoke> [options]

PRESETS
    --preset smoke     400 notes,    24 assets  -> bench/vaults/vault-smoke
    --preset 20k    20,000 notes,   200 assets  -> bench/vaults/vault-20k
    --preset 50k    50,000 notes,   500 assets  -> bench/vaults/vault-50k

OPTIONS
    --root <DIR>       Repository root (also SERAT_REPO_ROOT). Default: the
                       [workspace] above the working directory.
    --out <DIR>        Vault root. Overrides the preset's location.
    --notes <N>        Total .md files, including type notes and sidecars.
    --assets <N>       Binaries under assets/.
    --seed <N>         Master seed. One seed produces byte-identical output.
    --manifest <FILE>  Manifest location. Default <out>.manifest.json.
    --jobs <N>         Worker threads. Default: one per core. Does not affect output.
    --expect <SHA256>  Fail unless the tree digest matches. For CI.
    --force            Regenerate over an existing vaultgen vault.
    --quiet            Print only the tree digest.
    -h, --help         This text.

The generated vault is gitignored and must never be committed; the generator is
the artefact, the vault is derived. See docs/05-testing/benchmark-vault.md.
";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("vaultgen: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// Repository root, resolved at run time (tasks #45, #54) by the shared `reporoot`
/// crate. `--root` with no value is an error, from here as well as from the
/// argument loop.
fn repo_root() -> Result<PathBuf, String> {
    reporoot::from_process(&reporoot::compiled_root(env!("CARGO_MANIFEST_DIR"))).map(|r| r.path)
}

fn run() -> Result<ExitCode, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    // A bare `--root` is an error even when `--out` means the root is never joined.
    repo_root()?;

    let mut opts = Options::default();
    let mut out_set = false;
    let mut preset: Option<String> = None;
    let mut expect: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let mut value = |what: &str| -> Result<String, String> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{what} needs a value"))
        };
        match arg {
            "--preset" => preset = Some(value("--preset")?),
            "--out" => {
                opts.out = PathBuf::from(value("--out")?);
                out_set = true;
            }
            "--notes" => {
                opts.notes = value("--notes")?
                    .parse()
                    .map_err(|_| "--notes must be a positive integer".to_string())?
            }
            "--assets" => {
                opts.assets = value("--assets")?
                    .parse()
                    .map_err(|_| "--assets must be a non-negative integer".to_string())?
            }
            "--seed" => {
                opts.seed = value("--seed")?
                    .parse()
                    .map_err(|_| "--seed must be an unsigned 64-bit integer".to_string())?
            }
            // Consumed by `repo_root()` (shared `reporoot` rule); checked here so a
            // bare `--root` is an error rather than silently ignored.
            "--root" => {
                value("--root")?;
            }
            "--manifest" => opts.manifest = Some(PathBuf::from(value("--manifest")?)),
            "--jobs" => {
                opts.jobs = Some(
                    value("--jobs")?
                        .parse()
                        .map_err(|_| "--jobs must be a positive integer".to_string())?,
                )
            }
            "--expect" => expect = Some(value("--expect")?.to_lowercase()),
            "--force" => opts.force = true,
            "--quiet" => opts.quiet = true,
            other if other.starts_with("--root=") => {}
            other => return Err(format!("unknown argument {other}\n\n{USAGE}")),
        }
        i += 1;
    }

    // A preset sets notes, assets and location; explicit flags still win, which
    // is why it is applied before them only for the values they did not set.
    if let Some(name) = &preset {
        let (notes, assets, dir) = match name.as_str() {
            "smoke" => (400usize, 24usize, "bench/vaults/vault-smoke"),
            "20k" => (20_000, 200, "bench/vaults/vault-20k"),
            "50k" => (50_000, 500, "bench/vaults/vault-50k"),
            other => {
                return Err(format!(
                    "unknown preset {other}; expected smoke, 20k or 50k"
                ));
            }
        };
        // Explicit --notes/--assets override the preset.
        let defaults = Options::default();
        if opts.notes == defaults.notes {
            opts.notes = notes;
        }
        if opts.assets == defaults.assets {
            opts.assets = assets;
        }
        if !out_set {
            opts.out = repo_root()?.join(dir);
            out_set = true;
        }
    }
    if !out_set {
        opts.out = repo_root()?.join("bench/vaults/vault-20k");
    }

    if opts.notes < 64 {
        return Err("--notes below 64 cannot hold the fixed structure".to_string());
    }

    if !opts.quiet {
        eprintln!(
            "vaultgen v{VAULT_FORMAT_VERSION} · seed {} · {} notes · {} assets -> {}",
            opts.seed,
            opts.notes,
            opts.assets,
            opts.out.display()
        );
        if opts.seed != DEFAULT_SEED {
            eprintln!(
                "  note: seed differs from the canonical {DEFAULT_SEED}; benchmark numbers \
                 from this vault are not comparable with recorded ones"
            );
        }
    }

    let started = Instant::now();
    let manifest = generate(&opts).map_err(|e| e.to_string())?;
    let elapsed = started.elapsed();

    if opts.quiet {
        println!("{}", manifest.tree_digest);
    } else {
        let mb = manifest.bytes as f64 / (1024.0 * 1024.0);
        println!("tree digest  {}", manifest.tree_digest);
        println!(
            "wrote        {} files ({} notes, {} assets, {} ignored), {:.1} MiB, {} dirs",
            manifest.files,
            manifest.notes,
            manifest.assets,
            manifest.ignored_files,
            mb,
            manifest.dirs
        );
        println!(
            "links        {} edges · max in-degree {} · p50 {} · p90 {} · p99 {} · {} orphans · top 1% hold {:.1}%",
            manifest.link_degree.edges,
            manifest.link_degree.max,
            manifest.link_degree.p50,
            manifest.link_degree.p90,
            manifest.link_degree.p99,
            manifest.link_degree.orphans,
            manifest.link_degree.top_1pct_share * 100.0
        );
        println!(
            "content      {} tasks ({} done) · {} inline fields · {} decoys in code · {} embeds · {} block ids",
            manifest.content.tasks,
            manifest.content.tasks_done,
            manifest.content.inline_fields,
            manifest.content.decoy_fields,
            manifest.content.embeds,
            manifest.content.block_ids
        );
        println!(
            "edge cases   {} notes without frontmatter · {} CRLF · {} BOM",
            manifest.notes_without_frontmatter, manifest.notes_crlf, manifest.notes_bom
        );
        println!(
            "manifest     {}",
            opts.manifest
                .clone()
                .unwrap_or_else(|| manifest_path_for(&opts.out))
                .display()
        );
        println!("elapsed      {:.2} s", elapsed.as_secs_f64());
    }

    if let Some(want) = expect
        && want != manifest.tree_digest
    {
        eprintln!(
            "vaultgen: tree digest mismatch\n  expected {want}\n  actual   {}",
            manifest.tree_digest
        );
        return Ok(ExitCode::FAILURE);
    }

    Ok(ExitCode::SUCCESS)
}
