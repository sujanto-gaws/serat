//! The budget harness: one asserting job per performance budget.
//!
//! **Serves NFR-PRF-08** ("every performance budget shall be asserted in
//! continuous integration; a regression shall fail the build"), **US-35** and
//! **C-06**. The budgets themselves are NFR-PRF-01, NFR-PRF-03 and NFR-PRF-04
//! in P0; NFR-PRF-05 and NFR-PRF-09 join in P2.
//!
//! Three rules are encoded here rather than left to discipline:
//!
//! 1. **A measurement carries its conditions.** Every result records the vault's
//!    tree digest, the generator's format version and the machine profile. A
//!    number without those is not comparable with another number.
//! 2. **The vault is verified before it is measured.** The harness recomputes
//!    the tree digest and refuses to run against a vault that does not match its
//!    manifest, because a stale or hand-edited vault produces a number that
//!    looks fine and means nothing.
//! 3. **A threshold is never weakened to make a build pass.** If a budget is
//!    wrong it is re-baselined in the PRD with the measurement attached (task
//!    Q1), not edited here.

pub mod env;
pub mod subject;

use env::Environment;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Which statistic the requirement's acceptance criterion names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Statistic {
    /// The 95th percentile, nearest-rank. NFR-PRF-01 and NFR-PRF-03.
    P95,
    /// A single wall-clock figure. NFR-PRF-04.
    WallClock,
}

impl Statistic {
    /// How it prints in a report.
    pub fn label(self) -> &'static str {
        match self {
            Statistic::P95 => "p95",
            Statistic::WallClock => "wall clock",
        }
    }
}

/// Which machine the threshold applies to.
///
/// The product budget belongs to the reference machine (see
/// `docs/05-testing/reference-machine.md`). A CI runner is not that machine and
/// never will be, so CI asserts a *regression ceiling*: a separate number whose
/// job is to catch a change that made things worse, not to certify the product.
/// Keeping them in one column would mean either a green CI that proves nothing
/// or a red CI that everyone learns to ignore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// The reference machine. Asserts the number in the PRD.
    Reference,
    /// A CI runner. Asserts the regression ceiling.
    Ci,
}

impl Profile {
    /// Parse `--profile`.
    pub fn parse(s: &str) -> Result<Profile, String> {
        match s {
            "reference" | "ref" => Ok(Profile::Reference),
            "ci" => Ok(Profile::Ci),
            other => Err(format!("unknown profile {other}; expected reference or ci")),
        }
    }

    /// How it prints.
    pub fn label(self) -> &'static str {
        match self {
            Profile::Reference => "reference",
            Profile::Ci => "ci",
        }
    }
}

/// One budget from NFR-PRF.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    /// Stable job id, e.g. `prf04-full-rebuild`.
    pub id: &'static str,
    /// The requirement it asserts.
    pub requirement: &'static str,
    /// The requirement's own words, so a failure message says what is wrong
    /// rather than only that something is.
    pub statement: &'static str,
    /// The statistic the acceptance criterion names.
    pub statistic: Statistic,
    /// Iterations the acceptance criterion names.
    pub iterations: usize,
    /// The limit on the reference machine, in milliseconds. This is the number
    /// in the PRD.
    pub product_limit_ms: f64,
    /// The regression ceiling in CI, in milliseconds.
    ///
    /// `None` until the first CI run exists to set it from. Until then the CI
    /// job asserts the product limit, which will be red on a slower runner —
    /// deliberately, because a placeholder ceiling invented at a desk is worse
    /// than an honest red.
    pub ci_limit_ms: Option<f64>,
    /// Which preset vault it is measured against.
    pub vault: &'static str,
    /// **Exactly which interval is timed.**
    ///
    /// Named on the budget rather than left to the job, because P0 finding F-03
    /// showed that "save→index" has two defensible readings that differ by more
    /// than the budget itself: end-to-end from the file being written (which
    /// includes the 150 ms per-path coalescing window of architecture §3.1
    /// step 1, and so cannot fit 50 ms by construction) and the pipeline's own
    /// cost from the event being dequeued. A number reported without saying
    /// which one it is is not a measurement, it is a rumour.
    pub interval: &'static str,
}

impl Budget {
    /// The limit that applies under `profile`.
    pub fn limit_ms(&self, profile: Profile) -> f64 {
        match profile {
            Profile::Reference => self.product_limit_ms,
            Profile::Ci => self.ci_limit_ms.unwrap_or(self.product_limit_ms),
        }
    }
}

/// NFR-PRF-01. Two marks on one timeline, so it is two budgets sharing a job.
pub const PRF01_WINDOW_VISIBLE: Budget = Budget {
    id: "prf01-window-visible",
    requirement: "NFR-PRF-01",
    statement: "The application shall present an interactive window within 400ms",
    statistic: Statistic::P95,
    iterations: 20,
    product_limit_ms: 400.0,
    ci_limit_ms: None,
    vault: "20k",
    interval: "process spawn to the first painted frame",
};

/// NFR-PRF-01, the second mark.
pub const PRF01_VAULT_USABLE: Budget = Budget {
    id: "prf01-vault-usable",
    requirement: "NFR-PRF-01",
    statement: "…and a fully usable vault within 1 second, at 20,000 notes with a warm index",
    statistic: Statistic::P95,
    iterations: 20,
    product_limit_ms: 1_000.0,
    ci_limit_ms: None,
    vault: "20k",
    interval: "process spawn to the vault-usable mark, as task D2 defines it",
};

/// NFR-PRF-03.
pub const PRF03_SAVE_TO_INDEX: Budget = Budget {
    id: "prf03-save-to-index",
    requirement: "NFR-PRF-03",
    statement: "A single note save shall be reflected in the index within 50ms at p95",
    statistic: Statistic::P95,
    iterations: 1_000,
    product_limit_ms: 50.0,
    ci_limit_ms: None,
    vault: "20k",
    interval: "watcher event dequeued to the row being visible to a reader (see F-03)",
};

/// NFR-PRF-04.
pub const PRF04_FULL_REBUILD: Budget = Budget {
    id: "prf04-full-rebuild",
    requirement: "NFR-PRF-04",
    statement: "A full index rebuild of 20,000 Markdown notes shall complete within 30 seconds",
    statistic: Statistic::WallClock,
    iterations: 3,
    product_limit_ms: 30_000.0,
    ci_limit_ms: None,
    vault: "20k",
    interval: "entry to rebuild() until it returns, writer drained and ANALYZE run",
};

/// Every P0 budget, so a CI configuration review (NFR-PRF-08's acceptance
/// criterion is `Inspect`) can check the list against the PRD rather than
/// against somebody's memory.
pub const P0_BUDGETS: &[&Budget] = &[
    &PRF01_WINDOW_VISIBLE,
    &PRF01_VAULT_USABLE,
    &PRF03_SAVE_TO_INDEX,
    &PRF04_FULL_REBUILD,
];

/// Timings, in milliseconds.
#[derive(Clone, Debug, Default)]
pub struct Samples {
    values: Vec<f64>,
}

impl Samples {
    /// Empty.
    pub fn new() -> Samples {
        Samples { values: Vec::new() }
    }

    /// Record one measurement.
    pub fn push(&mut self, d: Duration) {
        self.values.push(d.as_secs_f64() * 1000.0);
    }

    /// How many.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The raw values, in collection order.
    pub fn values(&self) -> &[f64] {
        &self.values
    }

    /// Nearest-rank percentile: the smallest value at or above which `q` of the
    /// samples fall.
    ///
    /// Stated explicitly because "p95" is ambiguous and the choice changes the
    /// answer at n = 20 — the exact size NFR-PRF-01 specifies. Nearest rank at
    /// q = 0.95, n = 20 is the 19th of 20 sorted samples, so one slow run out of
    /// twenty is inside the budget and two are not. Interpolating would let two
    /// slow runs pass, which is not what the requirement means.
    pub fn percentile(&self, q: f64) -> f64 {
        assert!(!self.values.is_empty(), "no samples");
        let mut sorted = self.values.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let rank = (q * sorted.len() as f64).ceil().max(1.0) as usize;
        sorted[rank.min(sorted.len()) - 1]
    }

    /// Smallest sample.
    pub fn min(&self) -> f64 {
        self.values.iter().copied().fold(f64::INFINITY, f64::min)
    }

    /// Largest sample.
    pub fn max(&self) -> f64 {
        self.values
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
    }

    /// Arithmetic mean.
    pub fn mean(&self) -> f64 {
        self.values.iter().sum::<f64>() / self.values.len() as f64
    }

    /// The statistic this budget is judged on.
    pub fn statistic(&self, s: Statistic) -> f64 {
        match s {
            Statistic::P95 => self.percentile(0.95),
            Statistic::WallClock => self.percentile(0.50),
        }
    }
}

/// What the vault was when the number was taken.
#[derive(Clone, Debug)]
pub struct VaultUnderTest {
    /// Where it is.
    pub path: PathBuf,
    /// Its recomputed tree digest.
    pub digest: String,
    /// The generator format version that produced it.
    pub format_version: u32,
    /// Notes it contains.
    pub notes: usize,
}

/// A finished measurement.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// Which budget.
    pub budget: Budget,
    /// Which machine profile the limit came from.
    pub profile: Profile,
    /// The vault it was measured against.
    pub vault: VaultUnderTest,
    /// What the machine was doing while the number was taken.
    pub environment: Environment,
    /// The timings.
    pub samples: Samples,
    /// Where the time went, for budgets that can say. P0 finding F-10.
    ///
    /// The budget still asserts the aggregate — NFR-PRF-04 is a wall-clock
    /// requirement. This is reported beside it, because a single number cannot
    /// answer architecture.md's open design question 1, and that question is
    /// what §1.2 says the parse pool exists to settle.
    pub phases: Option<subject::RebuildPhases>,
}

impl Outcome {
    /// The judged value, in milliseconds.
    pub fn value_ms(&self) -> f64 {
        self.samples.statistic(self.budget.statistic)
    }

    /// The limit that applies, in milliseconds.
    pub fn limit_ms(&self) -> f64 {
        self.budget.limit_ms(self.profile)
    }

    /// Whether the budget held.
    pub fn passed(&self) -> bool {
        self.value_ms() <= self.limit_ms()
    }

    /// Whether this number may be recorded as the gate figure.
    ///
    /// Passing and being gate eligible are different questions. A rebuild
    /// measured with the on-access virus scanner disabled can pass 30 seconds
    /// and still not be the number the charter gate is judged on, because no
    /// user runs that machine. See `env::Environment::gate_eligible`.
    pub fn gate_eligible(&self) -> bool {
        self.environment.gate_eligible()
    }

    /// Human-readable report. Printed by the job, pasted into a gate report.
    pub fn to_report(&self) -> String {
        let mut s = String::new();
        let verdict = if self.passed() { "PASS" } else { "FAIL" };
        let _ = writeln!(
            s,
            "{verdict}  {} · {}",
            self.budget.requirement, self.budget.id
        );
        let _ = writeln!(s, "  budget      {}", self.budget.statement);
        let _ = writeln!(s, "  interval    {}", self.budget.interval);
        let _ = writeln!(
            s,
            "  measured    {} = {:.1} ms   limit {:.1} ms ({} profile)",
            self.budget.statistic.label(),
            self.value_ms(),
            self.limit_ms(),
            self.profile.label()
        );
        let _ = writeln!(
            s,
            "  samples     n={}  min {:.1}  mean {:.1}  max {:.1} ms",
            self.samples.len(),
            self.samples.min(),
            self.samples.mean(),
            self.samples.max()
        );
        let _ = writeln!(
            s,
            "  vault       {} ({} notes, generator v{})",
            self.vault.path.display(),
            self.vault.notes,
            self.vault.format_version
        );
        let _ = writeln!(s, "  digest      {}", self.vault.digest);
        let _ = writeln!(s, "  machine     {}", self.environment.summary());
        if let Some(p) = self.phases {
            let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
            let total = self.value_ms().max(f64::MIN_POSITIVE);
            let share = |d: std::time::Duration| ms(d) / total * 100.0;
            let _ = writeln!(
                s,
                "  phases      walk {:.0} ms ({:.0}%) · read+hash {:.0} ms ({:.0}%) · parse {:.0} ms ({:.0}%) · write {:.0} ms ({:.0}%)",
                ms(p.walk),
                share(p.walk),
                ms(p.read_hash),
                share(p.read_hash),
                ms(p.parse),
                share(p.parse),
                ms(p.write),
                share(p.write)
            );
            let _ = writeln!(
                s,
                "  F-10        phases sum to {:.0} ms against a {:.0} ms wall clock; a sum below the total is concurrency, not error",
                ms(p.sum()),
                total
            );
        }
        if let Some(reason) = self.environment.ineligibility_reason() {
            let _ = writeln!(s, "  NOT A GATE NUMBER: {reason}");
        }
        if self.budget.ci_limit_ms.is_none() && self.profile == Profile::Ci {
            let _ = writeln!(
                s,
                "  note        no CI regression ceiling is set yet, so the product limit is \
                 being asserted on a runner that is not the reference machine"
            );
        }
        s
    }

    /// Machine-readable result, for trend tracking across runs.
    pub fn to_json(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "{{");
        let _ = writeln!(s, "  \"budget_id\": \"{}\",", self.budget.id);
        let _ = writeln!(s, "  \"requirement\": \"{}\",", self.budget.requirement);
        let _ = writeln!(s, "  \"interval\": \"{}\",", self.budget.interval);
        let _ = writeln!(s, "  \"profile\": \"{}\",", self.profile.label());
        let _ = writeln!(s, "  \"statistic\": \"{}\",", self.budget.statistic.label());
        let _ = writeln!(s, "  \"value_ms\": {:.3},", self.value_ms());
        let _ = writeln!(s, "  \"limit_ms\": {:.3},", self.limit_ms());
        let _ = writeln!(s, "  \"passed\": {},", self.passed());
        let _ = writeln!(s, "  \"samples_n\": {},", self.samples.len());
        let _ = writeln!(s, "  \"min_ms\": {:.3},", self.samples.min());
        let _ = writeln!(s, "  \"mean_ms\": {:.3},", self.samples.mean());
        let _ = writeln!(s, "  \"max_ms\": {:.3},", self.samples.max());
        let _ = writeln!(s, "  \"vault_digest_sha256\": \"{}\",", self.vault.digest);
        let _ = writeln!(
            s,
            "  \"vault_format_version\": {},",
            self.vault.format_version
        );
        let _ = writeln!(s, "  \"vault_notes\": {},", self.vault.notes);
        let _ = writeln!(s, "  \"os\": \"{}\",", self.environment.os);
        let _ = writeln!(s, "  \"cores\": {},", self.environment.cores);
        let _ = writeln!(
            s,
            "  \"realtime_scanner\": {},",
            match self.environment.realtime_scanner {
                Some(true) => "true",
                Some(false) => "false",
                None => "null",
            }
        );
        let _ = writeln!(
            s,
            "  \"power_plan\": \"{}\",",
            self.environment.power_plan.as_deref().unwrap_or("unknown")
        );
        let _ = writeln!(s, "  \"gate_eligible\": {},", self.gate_eligible());
        if let Some(p) = self.phases {
            let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
            let _ = writeln!(s, "  \"phases_ms\": {{");
            let _ = writeln!(s, "    \"walk\": {:.3},", ms(p.walk));
            let _ = writeln!(s, "    \"read_hash\": {:.3},", ms(p.read_hash));
            let _ = writeln!(s, "    \"parse\": {:.3},", ms(p.parse));
            let _ = writeln!(s, "    \"write\": {:.3}", ms(p.write));
            let _ = writeln!(s, "  }},");
        }
        let _ = write!(s, "  \"samples_ms\": [");
        for (i, v) in self.samples.values().iter().enumerate() {
            if i > 0 {
                let _ = write!(s, ", ");
            }
            let _ = write!(s, "{v:.3}");
        }
        let _ = writeln!(s, "]");
        let _ = writeln!(s, "}}");
        s
    }
}

/// Repository root, resolved at compile time from this crate's location.
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Make sure the preset vault exists, generating it if it does not, then verify
/// that what is on disk is what the manifest says it is.
///
/// # Errors
/// If generation fails, if the manifest is missing, or if the vault on disk does
/// not match its manifest — in which case the number that would follow is
/// attributable to nothing and must not be taken.
pub fn ensure_vault(preset: &str) -> Result<VaultUnderTest, String> {
    let (notes, assets, dir) = match preset {
        "smoke" => (400usize, 24usize, "bench/vaults/vault-smoke"),
        "20k" => (20_000, 200, "bench/vaults/vault-20k"),
        "50k" => (50_000, 500, "bench/vaults/vault-50k"),
        other => return Err(format!("unknown preset {other}")),
    };
    let path = repo_root().join(dir);
    let manifest_path = vaultgen::manifest_path_for(&path);

    if !path.join(".vaultgen").exists() || !manifest_path.exists() {
        eprintln!("budget: generating {preset} vault at {}", path.display());
        let opts = vaultgen::Options {
            seed: vaultgen::DEFAULT_SEED,
            notes,
            assets,
            out: path.clone(),
            manifest: None,
            jobs: None,
            force: true,
            quiet: true,
        };
        vaultgen::generate(&opts).map_err(|e| format!("generating the vault: {e}"))?;
    }

    let manifest = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("reading {}: {e}", manifest_path.display()))?;
    let expected = json_string(&manifest, "tree_digest_sha256")
        .ok_or_else(|| format!("{} has no tree digest", manifest_path.display()))?;
    let format_version = json_number(&manifest, "format_version").unwrap_or(0.0) as u32;
    let notes_in_manifest = json_number(&manifest, "notes").unwrap_or(0.0) as usize;

    let actual = tree_digest(&path)?;
    if actual != expected {
        return Err(format!(
            "the vault at {} does not match its manifest\n  manifest {expected}\n  on disk  {actual}\n\
             Regenerate it: cargo run --release -p vaultgen -- --preset {preset} --force",
            path.display()
        ));
    }

    Ok(VaultUnderTest {
        path,
        digest: actual,
        format_version,
        notes: notes_in_manifest,
    })
}

/// Recompute the vault's tree digest from what is on disk, exactly as the
/// generator computes it: SHA-256 over `path\0filehash\n` in sorted path order.
///
/// # Errors
/// Any filesystem failure while walking.
pub fn tree_digest(root: &Path) -> Result<String, String> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect(root, root, &mut files).map_err(|e| format!("walking {}: {e}", root.display()))?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut tree = vaultgen::hash::Sha256::new();
    for (rel, abs) in &files {
        let bytes = std::fs::read(abs).map_err(|e| format!("reading {}: {e}", abs.display()))?;
        tree.update(rel.as_bytes());
        tree.update(b"\0");
        tree.update(&vaultgen::hash::Sha256::digest(&bytes));
        tree.update(b"\n");
    }
    Ok(vaultgen::hash::hex(&tree.finish()))
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| std::io::Error::other(e.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}

/// Write the result where the gate report and any trend tooling will find it.
///
/// # Errors
/// Any filesystem failure.
pub fn write_result(outcome: &Outcome) -> std::io::Result<PathBuf> {
    let dir = repo_root().join("bench/out/budgets");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", outcome.budget.id));
    std::fs::write(&path, outcome.to_json())?;
    Ok(path)
}

// A manifest reader rather than a JSON parser: the manifest is written by
// `vaultgen`, its shape is fixed, and pulling in serde to read three fields
// would be the tail wagging the dog.
fn json_string(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let start = text.find(&needle)? + needle.len();
    let rest = &text[start..];
    let open = rest.find('"')? + 1;
    let close = rest[open..].find('"')? + open;
    Some(rest[open..close].to_string())
}

fn json_number(text: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{key}\":");
    let start = text.find(&needle)? + needle.len();
    let rest = text[start..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Run one budget end to end and exit with the right code.
///
/// Returns the process exit code: 0 if the budget held, 1 if it was breached or
/// the measurement could not be taken. A measurement that cannot be taken is a
/// failure, not a skip — a budget job that silently passes when the thing it
/// measures does not exist is worse than no job at all.
pub fn run_job<F>(budget: &Budget, args: &[String], measure: F) -> std::process::ExitCode
where
    F: FnOnce(&VaultUnderTest, usize) -> Result<Samples, String>,
{
    run_job_with_phases(budget, args, |vault, iterations| {
        measure(vault, iterations).map(|s| (s, None))
    })
}

/// As [`run_job`], for a budget whose measurement can also say where the time
/// went (P0 finding F-10).
///
/// # Panics
/// Never; the signature mirrors [`run_job`].
pub fn run_job_with_phases<F>(
    budget: &Budget,
    args: &[String],
    measure: F,
) -> std::process::ExitCode
where
    F: FnOnce(&VaultUnderTest, usize) -> Result<(Samples, Option<subject::RebuildPhases>), String>,
{
    let profile = match args.iter().position(|a| a == "--profile") {
        Some(i) => match args.get(i + 1).map(|s| Profile::parse(s)) {
            Some(Ok(p)) => p,
            Some(Err(e)) => {
                eprintln!("budget: {e}");
                return std::process::ExitCode::FAILURE;
            }
            None => {
                eprintln!("budget: --profile needs a value");
                return std::process::ExitCode::FAILURE;
            }
        },
        None => Profile::Reference,
    };
    let iterations = match args.iter().position(|a| a == "--iterations") {
        Some(i) => match args.get(i + 1).and_then(|s| s.parse::<usize>().ok()) {
            Some(n) if n > 0 => n,
            _ => {
                eprintln!("budget: --iterations needs a positive integer");
                return std::process::ExitCode::FAILURE;
            }
        },
        None => budget.iterations,
    };
    if iterations != budget.iterations {
        eprintln!(
            "budget: running {iterations} iterations, not the {} the acceptance criterion names; \
             this result is indicative only and must not be recorded as a gate number",
            budget.iterations
        );
    }

    let vault = match ensure_vault(budget.vault) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("budget: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let mut phases;
    let samples = match measure(&vault, iterations) {
        Ok((s, p)) if !s.is_empty() => {
            phases = p;
            s
        }
        Ok(_) => {
            eprintln!("budget: the measurement produced no samples");
            return std::process::ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("FAIL  {} · {}\n  {e}", budget.requirement, budget.id);
            return std::process::ExitCode::FAILURE;
        }
    };

    let outcome = Outcome {
        budget: *budget,
        profile,
        vault,
        environment: Environment::detect(),
        samples,
        phases: phases.take(),
    };
    print!("{}", outcome.to_report());
    match write_result(&outcome) {
        Ok(p) => println!("  result      {}", p.display()),
        Err(e) => eprintln!("budget: could not write the result file: {e}"),
    }

    if outcome.passed() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(ms: &[f64]) -> Samples {
        let mut s = Samples::new();
        for v in ms {
            s.push(Duration::from_secs_f64(v / 1000.0));
        }
        s
    }

    #[test]
    fn nearest_rank_p95_over_twenty_runs_allows_exactly_one_slow_run() {
        // NFR-PRF-01 says "over 20 runs; p95 within budget". With nearest rank
        // that is the 19th of 20 sorted samples: one outlier passes, two do not.
        // An interpolating percentile would let two through, which is not what
        // the requirement means, so the choice is asserted rather than assumed.
        let mut ms = vec![100.0; 19];
        ms.push(9_999.0);
        assert_eq!(samples(&ms).percentile(0.95), 100.0);

        let mut ms = vec![100.0; 18];
        ms.push(9_999.0);
        ms.push(9_999.0);
        assert_eq!(samples(&ms).percentile(0.95), 9_999.0);
    }

    #[test]
    fn percentiles_are_order_independent() {
        let a = samples(&[5.0, 1.0, 4.0, 2.0, 3.0]);
        let b = samples(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(a.percentile(0.5), b.percentile(0.5));
        assert_eq!(a.percentile(0.95), b.percentile(0.95));
        assert_eq!(a.max(), 5.0);
        assert_eq!(a.min(), 1.0);
        assert_eq!(a.mean(), 3.0);
    }

    #[test]
    fn a_breach_is_a_failure_and_the_boundary_passes() {
        let vault = VaultUnderTest {
            path: PathBuf::from("x"),
            digest: "d".into(),
            format_version: 1,
            notes: 20_000,
        };
        let at_limit = Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault: vault.clone(),
            environment: Environment::default(),
            samples: samples(&[30_000.0]),
            phases: None,
        };
        assert!(
            at_limit.passed(),
            "a value exactly at the limit is within it"
        );

        let over = Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault,
            environment: Environment::default(),
            samples: samples(&[30_001.0]),
            phases: None,
        };
        assert!(!over.passed());
        assert!(over.to_report().starts_with("FAIL"));
        assert!(over.to_json().contains("\"passed\": false"));
    }

    #[test]
    fn every_p0_budget_matches_the_number_in_the_prd() {
        // NFR-PRF-08's acceptance criterion is Inspect: "CI configuration review
        // confirms each budget has an asserting job". This is that review, as a
        // test, so the list cannot drift from the PRD unnoticed.
        assert_eq!(P0_BUDGETS.len(), 4);
        assert_eq!(PRF01_WINDOW_VISIBLE.product_limit_ms, 400.0);
        assert_eq!(PRF01_WINDOW_VISIBLE.iterations, 20);
        assert_eq!(PRF01_VAULT_USABLE.product_limit_ms, 1_000.0);
        assert_eq!(PRF01_VAULT_USABLE.iterations, 20);
        assert_eq!(PRF03_SAVE_TO_INDEX.product_limit_ms, 50.0);
        assert_eq!(PRF03_SAVE_TO_INDEX.iterations, 1_000);
        assert_eq!(PRF04_FULL_REBUILD.product_limit_ms, 30_000.0);
        for b in P0_BUDGETS {
            assert!(b.requirement.starts_with("NFR-PRF-"));
            assert_eq!(b.vault, "20k", "{} measures the wrong vault", b.id);
            assert!(
                !b.interval.is_empty(),
                "{} does not say which interval it times, and P0 finding F-03 is what happens when nobody writes that down",
                b.id
            );
        }
    }

    #[test]
    fn the_manifest_reader_finds_what_it_needs() {
        let manifest = r#"{
  "format_version": 1,
  "seed": 20260903,
  "tree_digest_sha256": "abc123",
  "counts": { "notes": 20000, "assets": 200 }
}"#;
        assert_eq!(
            json_string(manifest, "tree_digest_sha256").as_deref(),
            Some("abc123")
        );
        assert_eq!(json_number(manifest, "format_version"), Some(1.0));
        assert_eq!(json_number(manifest, "notes"), Some(20_000.0));
        assert_eq!(json_string(manifest, "nope"), None);
    }
}
