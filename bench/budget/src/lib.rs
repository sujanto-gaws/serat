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
pub mod trace;

use env::Environment;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Which statistic the requirement's acceptance criterion names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Statistic {
    /// The 95th percentile, nearest-rank. NFR-PRF-01 and NFR-PRF-03.
    P95,
    /// The median wall-clock figure across runs. NFR-PRF-04's acceptance
    /// criterion says "wall-clock within budget" without naming a count; one run
    /// of a thirty-second operation on a laptop is a coin toss, so the harness
    /// takes three and reports the middle one.
    WallClock,
}

impl Statistic {
    /// The machine-readable name, for the result JSON.
    pub fn label(self) -> &'static str {
        match self {
            Statistic::P95 => "p95",
            Statistic::WallClock => "wall_clock_median",
        }
    }

    /// What a reader of the gate report needs to see, including the sample
    /// count, because "wall clock = 27431 ms" over three runs reads as one run.
    /// That exact line goes into the charter gate report (F-25).
    pub fn describe(self, n: usize) -> String {
        match self {
            Statistic::P95 => format!("p95 over {n} runs"),
            Statistic::WallClock => format!("median of {n} wall-clock runs"),
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
    interval: "apply() submitted through flush() returning -- Serat's own save path, \
               no watcher, no debounce (F-227, F-268; see check_save_path)",
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

/// The share by which the window-painted mark may grow between the 20k and the
/// 50k vault before the critical path is judged to depend on vault size.
///
/// Architecture §3.4's load-bearing property is that **nothing on the critical
/// path is proportional to vault size**: the main thread never opens the index,
/// and the file tree renders from a query rather than from the walk. If that
/// were false, 2.5x the notes would show up as roughly 2.5x the time. 20% sits
/// far below that and comfortably above run-to-run noise, so the check fails on
/// the thing it is for and not on a warm afternoon.
///
/// This is the assertion that makes §3.4 a claim rather than an intention, and
/// it is the reason the cold-start job runs against two vaults.
pub const SCALE_TOLERANCE: f64 = 0.20;

/// Judge whether the critical path grew with vault size.
///
/// # Errors
/// A sentence naming both figures when it did.
pub fn assert_scale_independent(small_ms: f64, large_ms: f64) -> Result<(), String> {
    let ceiling = small_ms * (1.0 + SCALE_TOLERANCE);
    if large_ms <= ceiling {
        return Ok(());
    }
    Err(format!(
        "window painted at 50,000 notes is {large_ms:.1} ms against {small_ms:.1} ms at 20,000          — {:.0}% growth, over the {:.0}% tolerance. Architecture §3.4 says nothing on the          critical path is proportional to vault size; something on it now is.",
        (large_ms / small_ms - 1.0) * 100.0,
        SCALE_TOLERANCE * 100.0
    ))
}

/// Every P0 budget, so a CI configuration review (NFR-PRF-08's acceptance
/// criterion is `Inspect`) can check the list against the PRD rather than
/// against somebody's memory.
pub const P0_BUDGETS: &[&Budget] = &[
    &PRF01_WINDOW_VISIBLE,
    &PRF01_VAULT_USABLE,
    &PRF03_SAVE_TO_INDEX,
    &PRF04_FULL_REBUILD,
];

/// A benchmark vault the harness knows how to produce and how to recognise.
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    /// `smoke`, `20k`, `50k`.
    pub name: &'static str,
    /// Total `.md` files.
    pub notes: usize,
    /// Binaries under `assets/`.
    pub assets: usize,
    /// Location, relative to the repository root.
    pub dir: &'static str,
    /// **The canonical tree digest**, at `VAULT_FORMAT_VERSION` 1 and the
    /// canonical seed.
    ///
    /// Pinned here rather than only in CI because of P0 finding F-26. Verifying
    /// the vault against its own manifest proves self-consistency and nothing
    /// else: regenerate with `--seed 424242` and the manifest is regenerated
    /// alongside it, so the two agree perfectly about a vault nobody meant to
    /// measure. `vaultgen` prints a warning in that case and the harness never
    /// sees it. This constant is what turns "the vault matches its manifest"
    /// into "the vault is the one every recorded number was taken against".
    pub digest: &'static str,
}

/// The three vaults, with their canonical digests.
///
/// If a change to `vaultgen` changes these, `VAULT_FORMAT_VERSION` is bumped in
/// the same commit and every recorded benchmark number is re-taken. A number
/// from format version 1 and a number from format version 2 are not comparable.
pub const PRESETS: &[Preset] = &[
    Preset {
        name: "smoke",
        notes: 400,
        assets: 24,
        dir: "bench/vaults/vault-smoke",
        digest: "c461c84347186fa66ff0be205d2a2e2aab1ed5bd9f281c2fba8e8b746bd9f3e9",
    },
    Preset {
        name: "20k",
        notes: 20_000,
        assets: 200,
        dir: "bench/vaults/vault-20k",
        digest: "35fcb867759df4b8737af8914f798d81d77396dfe051f38140139d3779bd393e",
    },
    Preset {
        name: "50k",
        notes: 50_000,
        assets: 500,
        dir: "bench/vaults/vault-50k",
        digest: "42fb99b2c7fcef77c26d6a578d63efd4169f68236c4a4b066608d3e0f0276aa1",
    },
];

/// Look a preset up by name.
///
/// # Errors
/// An unknown name, listing the ones that exist.
pub fn preset(name: &str) -> Result<&'static Preset, String> {
    PRESETS.iter().find(|p| p.name == name).ok_or_else(|| {
        format!(
            "unknown preset {name}; expected one of {}",
            PRESETS
                .iter()
                .map(|p| p.name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

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

    /// The median. Nearest rank at q = 0.5, so at n = 3 it is the middle
    /// sample and emphatically not the fastest.
    pub fn median(&self) -> f64 {
        self.percentile(0.50)
    }

    /// The statistic this budget is judged on.
    pub fn statistic(&self, s: Statistic) -> f64 {
        match s {
            Statistic::P95 => self.percentile(0.95),
            Statistic::WallClock => self.median(),
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
    /// Note-to-note link edges the generator produced, from the manifest's
    /// `link_degree.edges`. The oracle for `RebuildReport.links_resolved`.
    pub note_links: usize,
}

/// A series the job measured but does not assert.
///
/// NFR-PRF-03's end-to-end interval is the case that forced this: it is real,
/// it is worth reporting, and printing it outside the report block put an
/// unasserted 812 ms above a line reading `PASS` (P0 finding F-28). Carried on
/// the [`Outcome`] and rendered in one place, so its label and its verdict
/// cannot drift apart.
#[derive(Clone, Debug)]
pub struct ReportedSeries {
    /// Short name, e.g. `end-to-end`.
    pub label: String,
    /// Why it is here and why it is not the budget figure.
    pub note: String,
    /// The timings.
    pub samples: Samples,
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
    /// The timings the budget is asserted against.
    pub samples: Samples,
    /// Series measured and reported but **not** asserted, rendered inside the
    /// report block so they cannot be read as the budget figure (F-28).
    pub reported: Vec<ReportedSeries>,
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

    /// The phase breakdown must not claim more time than the clock measured.
    ///
    /// The report already tells a reader that a sum *below* the wall clock is
    /// concurrency between the parse pool and the writer rather than an error.
    /// It named the anomaly and did not check it (P0 finding F-29). Since the
    /// phases are the whole content of open design question 1, a breakdown that
    /// double-counted would be reported as fact and would send the architect
    /// the wrong way.
    ///
    /// The 5% tolerance is for timer granularity across five separately
    /// measured spans, not for slop.
    ///
    /// # Errors
    /// When the phases sum to more than the wall clock plus tolerance.
    pub fn check_phases(&self) -> Result<(), String> {
        let Some(phases) = self.phases else {
            return Ok(());
        };
        check_bulk_load_shape(&phases)?;
        let sum_ms = phases.sum().as_secs_f64() * 1000.0;
        let total_ms = self.value_ms();
        if sum_ms <= total_ms * 1.05 {
            return Ok(());
        }
        Err(format!(
            "the phase breakdown sums to {sum_ms:.0} ms against a {total_ms:.0} ms wall clock. \
             A sum below the total is concurrency; a sum above it means a phase is counted \
             twice, and these numbers answer open design question 1 — they must not be \
             reported as fact."
        ))
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
            self.budget.statistic.describe(self.samples.len()),
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
        for extra in &self.reported {
            if extra.samples.is_empty() {
                continue;
            }
            let _ = writeln!(
                s,
                "  {:<11} p95 {:.1} ms over {} samples — reported, not asserted. {}",
                extra.label,
                extra.samples.percentile(0.95),
                extra.samples.len(),
                extra.note
            );
        }
        let _ = writeln!(s, "  digest      {}", self.vault.digest);
        let _ = writeln!(s, "  machine     {}", self.environment.summary());
        if let Some(p) = self.phases {
            let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
            let total = self.value_ms().max(f64::MIN_POSITIVE);
            let share = |d: std::time::Duration| ms(d) / total * 100.0;
            let _ = writeln!(
                s,
                "  phases      walk {:.0} ms ({:.0}%) · read+hash {:.0} ms ({:.0}%) · parse {:.0} ms ({:.0}%) · producer waited {:.0} ms ({:.0}%) · fts build {:.0} ms ({:.0}%)",
                ms(p.walk),
                share(p.walk),
                ms(p.read_hash),
                share(p.read_hash),
                ms(p.parse),
                share(p.parse),
                ms(p.producer_wait),
                share(p.producer_wait),
                ms(p.write_fts),
                share(p.write_fts)
            );
            let _ = writeln!(
                s,
                "  reading     \"producer waited\" is how long the producer was BLOCKED on the writer, not what the writer cost — near zero means the writer kept up, large means it is the bottleneck"
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
        let _ = writeln!(s, "  \"os\": \"{}\",", self.environment.os.label());
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
            let _ = writeln!(s, "    \"producer_wait\": {:.3},", ms(p.producer_wait));
            let _ = writeln!(s, "    \"write_fts\": {:.3}", ms(p.write_fts));
            let _ = writeln!(s, "  }},");
        }
        if !self.reported.is_empty() {
            let _ = writeln!(s, "  \"reported_not_asserted\": {{");
            let last = self.reported.len() - 1;
            for (i, extra) in self.reported.iter().enumerate() {
                let _ = writeln!(
                    s,
                    "    \"{}\": {{ \"p95_ms\": {:.3}, \"samples_n\": {} }}{}",
                    extra.label,
                    extra.samples.percentile(0.95),
                    extra.samples.len(),
                    if i == last { "" } else { "," }
                );
            }
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

/// What one budget's measurement produced.
///
/// One asserted series, optionally a phase breakdown, and any number of series
/// that are reported but not asserted. Every job now returns this, which is
/// what let `prf01-cold-start` stop reimplementing argument parsing and lose
/// two of the guards in the process (P0 finding F-27).
#[derive(Clone, Debug, Default)]
pub struct Measurement {
    /// The series the budget is judged on.
    pub samples: Samples,
    /// Where the time went, for budgets that can say (F-10).
    pub phases: Option<subject::RebuildPhases>,
    /// Series reported beside it.
    pub reported: Vec<ReportedSeries>,
}

impl Measurement {
    /// A measurement with nothing but the asserted series.
    pub fn new(samples: Samples) -> Measurement {
        Measurement {
            samples,
            phases: None,
            reported: Vec::new(),
        }
    }

    /// Add a series that is reported but not asserted.
    #[must_use]
    pub fn reporting(mut self, label: &str, note: &str, samples: Samples) -> Measurement {
        self.reported.push(ReportedSeries {
            label: label.to_string(),
            note: note.to_string(),
            samples,
        });
        self
    }

    /// Attach the phase breakdown.
    #[must_use]
    pub fn with_phases(mut self, phases: subject::RebuildPhases) -> Measurement {
        self.phases = Some(phases);
        self
    }
}

/// The command-line options every budget job accepts.
#[derive(Clone, Copy, Debug)]
pub struct JobArgs {
    /// Which machine profile's threshold applies.
    pub profile: Profile,
    /// How many iterations to run.
    pub iterations: usize,
    /// Whether `iterations` differs from the acceptance criterion's count, in
    /// which case the result is indicative and must not be recorded.
    pub indicative: bool,
}

/// Parse the arguments every job shares.
///
/// # Errors
/// A malformed flag, as a sentence.
pub fn parse_args(budget: &Budget, args: &[String]) -> Result<JobArgs, String> {
    let mut profile = Profile::Reference;
    if let Some(i) = args.iter().position(|a| a == "--profile") {
        let value = args
            .get(i + 1)
            .ok_or_else(|| "--profile needs a value".to_string())?;
        profile = Profile::parse(value)?;
    }
    let mut iterations = budget.iterations;
    if let Some(i) = args.iter().position(|a| a == "--iterations") {
        iterations = args
            .get(i + 1)
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| "--iterations needs a positive integer".to_string())?;
    }
    Ok(JobArgs {
        profile,
        iterations,
        indicative: iterations != budget.iterations,
    })
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
pub fn ensure_vault(name: &str) -> Result<VaultUnderTest, String> {
    let preset = preset(name)?;
    let (notes, assets) = (preset.notes, preset.assets);
    let path = repo_root().join(preset.dir);
    let manifest_path = vaultgen::manifest_path_for(&path);

    if !path.join(".vaultgen").exists() || !manifest_path.exists() {
        eprintln!("budget: generating {name} vault at {}", path.display());
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

    verify_vault_at(preset, &path)
}

/// Verify a vault that already exists, wherever it is.
///
/// Split from [`ensure_vault`] so the verification can be exercised against a
/// real vault in a temporary directory. The digest is over paths relative to the
/// vault root, so it does not depend on where the vault lives -- which is what
/// makes this testable at all.
///
/// # Errors
/// A missing or unreadable manifest, or any of [`check_vault`]'s refusals.
pub fn verify_vault_at(preset: &Preset, path: &Path) -> Result<VaultUnderTest, String> {
    let manifest_path = vaultgen::manifest_path_for(path);
    let manifest = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("reading {}: {e}", manifest_path.display()))?;
    let expected = json_string(&manifest, "tree_digest_sha256")
        .ok_or_else(|| format!("{} has no tree digest", manifest_path.display()))?;
    let format_version = json_number(&manifest, "format_version").unwrap_or(0.0) as u32;
    let notes_in_manifest = json_number(&manifest, "notes").unwrap_or(0.0) as usize;
    let note_links = json_number(&manifest, "edges").unwrap_or(0.0) as usize;

    let actual = tree_digest(path)?;
    check_vault(
        preset,
        &actual,
        &expected,
        format_version,
        notes_in_manifest,
    )
    .map_err(|e| format!("{e}\n  vault {}", path.display()))?;

    Ok(VaultUnderTest {
        path: path.to_path_buf(),
        digest: actual,
        format_version,
        notes: notes_in_manifest,
        note_links,
    })
}

/// The three things that must be true of a vault before a clock starts.
///
/// Split out of [`ensure_vault`] so it can be tested without generating 39 MiB
/// (P0 finding F-26): the on-disk bytes match the manifest, the manifest is the
/// *canonical* one, and it holds the number of notes the preset promises.
///
/// # Errors
/// A sentence naming which of the three failed and how to fix it.
pub fn check_vault(
    preset: &Preset,
    on_disk: &str,
    in_manifest: &str,
    format_version: u32,
    notes_in_manifest: usize,
) -> Result<(), String> {
    let regenerate = format!(
        "Regenerate it: cargo run --release -p vaultgen -- --preset {} --force",
        preset.name
    );
    if on_disk != in_manifest {
        return Err(format!(
            "the vault does not match its manifest\n  manifest {in_manifest}\n  on disk  \
             {on_disk}\n{regenerate}"
        ));
    }
    if on_disk != preset.digest {
        return Err(format!(
            "this is not the canonical {} vault\n  canonical {}\n  on disk   {on_disk}\n\
             It matches its own manifest, so it was generated coherently — with a different \
             seed, a different note count, or a different generator version. A number taken \
             against it is not comparable with any recorded number.\n{regenerate}",
            preset.name, preset.digest
        ));
    }
    if format_version != vaultgen::VAULT_FORMAT_VERSION {
        return Err(format!(
            "the vault was written by generator format version {format_version}; this build \
             expects {}. Numbers from two format versions are not comparable.\n{regenerate}",
            vaultgen::VAULT_FORMAT_VERSION
        ));
    }
    if notes_in_manifest != preset.notes {
        return Err(format!(
            "the manifest reports {notes_in_manifest} notes; the {} preset is {} \
             notes\n{regenerate}",
            preset.name, preset.notes
        ));
    }
    Ok(())
}

/// Whether the rebuild still used the bulk-load shape (P0 finding F-38).
///
/// `write_rows` and `write_fts` are only comparable across runs if both were
/// measured on the same shape: FTS triggers dropped, `blocks_fts` built once at
/// the end. If a rebuild ever regressed to loading *through* the triggers,
/// `write_fts` would read as roughly zero and `write_rows` would absorb the
/// whole cost — **the breakdown would look better while the total got worse**,
/// and nothing in a wall-clock figure can see that.
///
/// It is worth a check rather than a comment because of what the regression
/// costs: architect measured about 6x - 26,000 blocks at 2.1 s with
/// the triggers live, against 0.32 s for the same shape at 26,000 blocks with
/// them dropped (F-32). This is also the phase breakdown justifying its own
/// existence — the aggregate cannot distinguish these two worlds.
///
/// # Errors
/// When a rebuild that did real work reports no FTS build at all.
pub fn check_bulk_load_shape(phases: &subject::RebuildPhases) -> Result<(), String> {
    // Gated on `parse` rather than on the producer's wait: a fast writer means
    // the producer never blocks, so `producer_wait` is legitimately near zero on
    // exactly the healthy runs this check must still cover. `parse` is non-zero
    // whenever the rebuild did any work at all.
    if phases.parse.is_zero() {
        return Ok(());
    }
    if phases.write_fts.is_zero() {
        return Err(
            "the rebuild wrote rows but reports no FTS build time. blocks_fts is built once at \
             the end of the bulk load, so a zero here means the load went through the FTS \
             triggers instead — which shows as a better-looking breakdown and a worse total, \
             and cost about 6x when it was last measured: 2.1 s against 0.32 s at 26,000 blocks (F-32)."
                .to_string(),
        );
    }
    Ok(())
}

/// Whether `ANALYZE` described the finished index rather than an earlier one.
///
/// P0 finding **F-47**. `sqlite_stat1` is read from the index directly, so this
/// is an independent oracle rather than a number the rebuild reports about
/// itself.
///
/// # Errors
/// When the recorded row count is not the vault's note count.
pub fn check_analyze_freshness(recorded: usize, expected: usize) -> Result<(), String> {
    if recorded == expected {
        return Ok(());
    }
    Err(format!(
        "ANALYZE recorded {recorded} rows for `notes`; the vault holds {expected}. The \
         statistics describe an earlier index, so the planner picks ix_notes_kind over \
         ix_notes_type and every typed query degrades to a full scan (architecture §4.3). At \
         20,000 notes the opportunistic 5% re-ANALYZE never fires for the tail batch of 32, so \
         the explicit analyze() is the only thing that makes this true — and this is the only \
         scale at which its absence is visible."
    ))
}

/// Whether every link the vault contains was resolved.
///
/// The vault's manifest records the exact number of note-to-note link edges it
/// generated, and a test asserts every wikilink in it resolves. So a rebuild
/// reporting fewer has dropped links, and one reporting more has counted rows
/// touched rather than links resolved — the bug rust-core-dev hit while writing
/// the second pass, which is the rebuild-backpressure shape one layer up.
///
/// # Errors
/// When the counts disagree.
pub fn check_links_resolved(reported: usize, expected: usize) -> Result<(), String> {
    if reported == expected {
        return Ok(());
    }
    let direction = if reported < expected {
        "fewer, so links were dropped"
    } else {
        "more, so something other than resolved links is being counted"
    };
    Err(format!(
        "the rebuild resolved {reported} note-to-note links; the vault manifest generated \
         {expected}. That is {direction}."
    ))
}

/// Whether a rebuild indexed the whole vault.
///
/// A fast number over a partially indexed vault is the shape of a false pass,
/// and a wall clock cannot see it (P0 finding F-19). Extracted from the job so
/// that deleting it fails a test rather than passing silently (F-27).
///
/// # Errors
/// When the counts disagree.
pub fn check_note_count(reported: usize, expected: usize) -> Result<(), String> {
    if reported == expected {
        return Ok(());
    }
    Err(format!(
        "the rebuild reported {reported} notes, the vault manifest says {expected}. A fast \
         number over a partial index is not a passing budget."
    ))
}

/// Whether a save's timing came from Serat's own save path rather than the
/// watcher.
///
/// P0 findings **F-227, F-229, F-268**. A save routed through the watcher
/// carries FR-VLT-04's 500 ms budget and the 150 ms coalescing window;
/// NFR-PRF-03's 50 ms can only mean the direct path `apply()` then `flush()`
/// takes, since architecture §3.1.1/§3.2 mean no external change can reach
/// the index inside the coalescing window at all. A number measured on the
/// wrong path is not distinguishable from a real one by looking at it — that
/// is what makes this check load-bearing rather than decorative, and it is
/// exactly the shape F-268 found already inside the instrument meant to
/// prevent it.
///
/// # Errors
/// When `path` is not exactly `"save"`.
pub fn check_save_path(path: &str) -> Result<(), String> {
    if path == "save" {
        return Ok(());
    }
    Err(format!(
        "a save timing came from the {path:?} path, not \"save\". NFR-PRF-03 governs Serat's \
         own save path (apply() then flush()), not a save observed through the watcher, which \
         carries FR-VLT-04's 500 ms budget and the 150 ms coalescing window instead (F-227, \
         F-229, F-268). Reporting this number against NFR-PRF-03 would be wrong by \
         construction, not by a measurement error."
    ))
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
    write_result_to(&repo_root().join("bench/out/budgets"), outcome)
}

/// As [`write_result`], into a directory of the caller's choosing.
///
/// # Errors
/// Any filesystem failure.
pub fn write_result_to(dir: &Path, outcome: &Outcome) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
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

/// The whole of a budget job except printing and exiting.
///
/// Split out of [`run_job`] because a function returning an opaque `ExitCode`
/// cannot be tested, and P0 finding F-27 was four mutations that no test
/// noticed — including a job swallowing the seam error to post a fabricated
/// number, and the deletion of the note-count guard. Every one of those is now
/// a `Result` a test can inspect.
///
/// # Errors
/// A malformed argument, an unusable vault, a failed measurement, a measurement
/// that produced no samples, or a phase breakdown that does not add up.
pub fn decide<F>(budget: &Budget, args: &[String], measure: F) -> Result<Outcome, String>
where
    F: FnOnce(&VaultUnderTest, usize) -> Result<Measurement, String>,
{
    let parsed = parse_args(budget, args)?;
    if parsed.indicative {
        eprintln!(
            "budget: running {} iterations, not the {} the acceptance criterion names; this \
             result is indicative only and must not be recorded as a gate number",
            parsed.iterations, budget.iterations
        );
    }
    let vault = ensure_vault(budget.vault)?;
    let measurement = measure(&vault, parsed.iterations)?;
    outcome_from(budget, parsed.profile, vault, measurement)
}

/// Build the outcome, refusing a measurement that cannot mean anything.
///
/// # Errors
/// No samples, or phases that sum to more than the wall clock.
pub fn outcome_from(
    budget: &Budget,
    profile: Profile,
    vault: VaultUnderTest,
    measurement: Measurement,
) -> Result<Outcome, String> {
    if measurement.samples.is_empty() {
        return Err("the measurement produced no samples".to_string());
    }
    let outcome = Outcome {
        budget: *budget,
        profile,
        vault,
        environment: Environment::detect(),
        samples: measurement.samples,
        reported: measurement.reported,
        phases: measurement.phases,
    };
    outcome.check_phases()?;
    Ok(outcome)
}

/// Run one budget end to end and exit with the right code.
///
/// 0 if the budget held, 1 if it was breached or the measurement could not be
/// taken. A measurement that cannot be taken is a failure, not a skip — a
/// budget job that silently passes when the thing it measures does not exist is
/// worse than no job at all.
pub fn run_job<F>(budget: &Budget, args: &[String], measure: F) -> std::process::ExitCode
where
    F: FnOnce(&VaultUnderTest, usize) -> Result<Measurement, String>,
{
    match decide(budget, args, measure) {
        Ok(outcome) => {
            report(&outcome);
            if outcome.passed() {
                std::process::ExitCode::SUCCESS
            } else {
                std::process::ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("FAIL  {} · {}\n  {e}", budget.requirement, budget.id);
            std::process::ExitCode::FAILURE
        }
    }
}

/// Run two budgets that share one measurement, as NFR-PRF-01 does.
///
/// Both are reported; the job fails if either is breached. "Cold start failed"
/// without saying which half is a bug report nobody can act on.
pub fn run_paired_job<F>(
    first: &Budget,
    second: &Budget,
    args: &[String],
    measure: F,
) -> std::process::ExitCode
where
    F: FnOnce(&VaultUnderTest, usize) -> Result<(Measurement, Measurement), String>,
{
    let parsed = match parse_args(first, args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("budget: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let vault = match ensure_vault(first.vault) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("FAIL  {} · {}\n  {e}", first.requirement, first.id);
            return std::process::ExitCode::FAILURE;
        }
    };
    let (a, b) = match measure(&vault, parsed.iterations) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!(
                "FAIL  {} · {} and {}\n  {e}",
                first.requirement, first.id, second.id
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let mut ok = true;
    for (budget, measurement) in [(first, a), (second, b)] {
        match outcome_from(budget, parsed.profile, vault.clone(), measurement) {
            Ok(outcome) => {
                report(&outcome);
                ok &= outcome.passed();
            }
            Err(e) => {
                eprintln!("FAIL  {} · {}\n  {e}", budget.requirement, budget.id);
                ok = false;
            }
        }
    }
    if ok {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

/// The median of a list of durations.
///
/// One definition, used by the phase breakdown and agreeing with
/// `Samples::median()` by construction — P0 finding F-25 noted two definitions
/// of median in the crate with nothing holding them together.
pub fn median_duration(mut xs: Vec<Duration>) -> Duration {
    if xs.is_empty() {
        return Duration::ZERO;
    }
    xs.sort_unstable();
    // Nearest rank at q = 0.5, matching Samples::percentile exactly: at n = 3
    // the middle sample, never the fastest.
    let rank = ((0.5 * xs.len() as f64).ceil().max(1.0) as usize).min(xs.len());
    xs[rank - 1]
}

/// NFR-PRF-04's measurement: `iterations` rebuilds into a fresh index each time.
///
/// Lives in the library rather than in the binary because P0 finding F-27 was
/// four mutations no test noticed, and every one of them was in a job binary
/// that nothing covered. The logic under test is now the logic that runs.
///
/// # Errors
/// Any rebuild failure, or a rebuild whose note count disagrees with the vault
/// manifest.
pub fn measure_full_rebuild(
    vault: &VaultUnderTest,
    iterations: usize,
) -> Result<Measurement, String> {
    measure_full_rebuild_with(
        vault,
        iterations,
        subject::full_rebuild,
        subject::stat1_note_rows,
    )
}

/// As [`measure_full_rebuild`], against an injected rebuild.
///
/// The loop — fresh index per iteration, the note-count guard, the per-phase
/// median — is the part worth testing, and it cannot be tested through the real
/// seam because the real seam does not exist yet. A mutation sweep found the
/// note-count guard survived precisely because nothing exercised this function.
///
/// # Errors
/// As [`measure_full_rebuild`].
pub fn measure_full_rebuild_with<F, A>(
    vault: &VaultUnderTest,
    iterations: usize,
    mut rebuild: F,
    mut analyze: A,
) -> Result<Measurement, String>
where
    F: FnMut(&Path, &Path) -> Result<subject::RebuildTiming, String>,
    A: FnMut(&Path) -> Result<usize, String>,
{
    let mut samples = Samples::new();
    let mut runs: Vec<subject::RebuildTiming> = Vec::with_capacity(iterations);
    for i in 0..iterations {
        // A fresh index directory per iteration: a rebuild over a warm index is
        // not a rebuild, and reusing the directory would measure the second run
        // of something the requirement describes as the first.
        let index_dir = vault.path.join(format!(".serat-bench-{i}"));
        let _ = std::fs::remove_dir_all(&index_dir);
        let timing = rebuild(&vault.path, &index_dir)?;
        check_note_count(timing.notes, vault.notes)?;
        check_links_resolved(timing.links_resolved, vault.note_links)?;
        // Read before the directory goes away. F-47 is only visible here.
        let recorded = analyze(&index_dir);
        let _ = std::fs::remove_dir_all(&index_dir);
        check_analyze_freshness(recorded?, vault.notes)?;
        samples.push(timing.total);
        runs.push(timing);
    }
    // Per-phase median across runs, not the phases of the median run: one slow
    // run should not decide where the time went.
    let phases = subject::RebuildPhases {
        walk: median_duration(runs.iter().map(|r| r.phases.walk).collect()),
        read_hash: median_duration(runs.iter().map(|r| r.phases.read_hash).collect()),
        parse: median_duration(runs.iter().map(|r| r.phases.parse).collect()),
        producer_wait: median_duration(runs.iter().map(|r| r.phases.producer_wait).collect()),
        write_fts: median_duration(runs.iter().map(|r| r.phases.write_fts).collect()),
    };
    Ok(Measurement::new(samples).with_phases(phases))
}

/// NFR-PRF-03's measurement: the save-path interval, asserted, beside the two
/// watcher-path intervals kept for FR-VLT-04 (F-227, F-268).
///
/// # Errors
/// Any failure of the save path, or [`check_save_path`] rejecting a save
/// measured on the watcher path rather than Serat's own.
pub fn measure_save_to_index(
    vault: &VaultUnderTest,
    iterations: usize,
) -> Result<Measurement, String> {
    measure_save_to_index_with(vault, iterations, |v, i, it| {
        subject::save_to_index(v, i, it)
    })
}

/// As [`measure_save_to_index`], against an injected save path.
///
/// Split out so `check_save_path` refusing a watcher-path measurement can be
/// tested without the real seam existing yet (task I5) — the same reasoning
/// as `measure_full_rebuild_with`, and forced by the same lesson: F-268 was a
/// harness bug nothing exercised, because nothing could inject a `"watcher"`
/// tag to prove the guard fires on it.
///
/// # Errors
/// As [`measure_save_to_index`].
pub fn measure_save_to_index_with<F>(
    vault: &VaultUnderTest,
    iterations: usize,
    save_to_index: F,
) -> Result<Measurement, String>
where
    F: FnOnce(&Path, &Path, usize) -> Result<subject::SaveSamples, String>,
{
    let index_dir = vault.path.join(".serat-bench");
    let samples = save_to_index(&vault.path, &index_dir, iterations)?;
    check_save_path(samples.path)?;
    Ok(Measurement::new(samples.save)
        .reporting(
            "watcher-write-to-visible",
            "File written to row visible, via the watcher -- includes the 150 ms coalescing \
             window of architecture §3.1 step 1. FR-VLT-04's interval, not this budget's \
             (F-227).",
            samples.end_to_end,
        )
        .reporting(
            "watcher-dequeue-to-visible",
            "Watcher event dequeued to row visible. Still a watcher-path interval despite the \
             name -- not this budget's either (F-268).",
            samples.pipeline,
        ))
}

/// NFR-PRF-01's measurement: two marks from one set of cold starts.
///
/// # Errors
/// Any failure to start the application or to validate a trace.
pub fn measure_cold_start(
    vault: &VaultUnderTest,
    iterations: usize,
) -> Result<(Measurement, Measurement), String> {
    let (visible, usable) = subject::cold_start(&vault.path, iterations)?;
    Ok((Measurement::new(visible), Measurement::new(usable)))
}

/// Print an outcome and write its result file.
pub fn report(outcome: &Outcome) {
    print!("{}", outcome.to_report());
    match write_result(outcome) {
        Ok(p) => println!("  result      {}", p.display()),
        Err(e) => eprintln!("budget: could not write the result file: {e}"),
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
            note_links: 216_971,
        };
        let at_limit = Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault: vault.clone(),
            environment: Environment::default(),
            samples: samples(&[30_000.0]),
            reported: Vec::new(),
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
            reported: Vec::new(),
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
    fn a_critical_path_that_grows_with_the_vault_is_caught() {
        // Architecture §3.4's load-bearing property, as a test. If the main
        // thread ever opened the index, or the tree rendered from the walk,
        // 2.5x the notes would show up here.
        assert!(assert_scale_independent(300.0, 300.0).is_ok());
        assert!(
            assert_scale_independent(300.0, 360.0).is_ok(),
            "exactly at tolerance"
        );
        let err = assert_scale_independent(300.0, 361.0).expect_err("over tolerance");
        assert!(err.contains("proportional to vault size"), "{err}");
        // The shape a proportional path would actually produce.
        let err = assert_scale_independent(300.0, 750.0).expect_err("2.5x must fail");
        assert!(err.contains("150%"), "{err}");
    }

    fn a_vault() -> VaultUnderTest {
        VaultUnderTest {
            path: PathBuf::from("x"),
            digest: "d".into(),
            format_version: vaultgen::VAULT_FORMAT_VERSION,
            notes: 20_000,
            note_links: 216_971,
        }
    }

    // ---------------------------------------------------------------- F-25

    #[test]
    fn the_median_of_three_is_the_middle_run_and_not_the_fastest() {
        // The mutation this exists for: ceil() -> floor() in percentile().
        // At n = 20 and n = 1000 the two coincide, so every earlier test was
        // blind to it. At n = 3, floor(0.5 * 3) = 1 gives sorted[0] — the
        // FASTEST of three — where nearest rank gives the middle one. One
        // character turns NFR-PRF-04's median-of-three into a best-of-three.
        let s = samples(&[10.0, 30.0, 20.0]);
        assert_eq!(s.percentile(0.50), 20.0, "median of three is the middle");
        assert_ne!(
            s.percentile(0.50),
            s.min(),
            "and emphatically not the fastest"
        );
        assert_eq!(s.median(), 20.0);

        // Even n, so nobody can quietly average.
        assert_eq!(samples(&[10.0, 20.0, 30.0, 40.0]).percentile(0.50), 20.0);
    }

    #[test]
    fn the_wall_clock_statistic_is_the_median() {
        // Nothing asserted this: WallClock => percentile(0.01) left every test
        // green, so a budget could have been judged on its fastest run.
        let s = samples(&[10.0, 30.0, 20.0]);
        assert_eq!(s.statistic(Statistic::WallClock), 20.0);
        assert_eq!(s.statistic(Statistic::P95), 30.0);

        let outcome = Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: Environment::default(),
            samples: samples(&[10_000.0, 40_000.0, 20_000.0]),
            reported: Vec::new(),
            phases: None,
        };
        assert_eq!(outcome.value_ms(), 20_000.0, "the middle run decides");
        assert!(outcome.passed());
    }

    #[test]
    fn the_report_says_the_number_is_a_median_of_three() {
        // This exact line goes into the charter gate report, and "wall clock =
        // 27431 ms" over three runs reads as one run.
        assert_eq!(
            Statistic::WallClock.describe(3),
            "median of 3 wall-clock runs"
        );
        assert_eq!(Statistic::P95.describe(20), "p95 over 20 runs");

        let outcome = Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: Environment::default(),
            samples: samples(&[10_000.0, 40_000.0, 20_000.0]),
            reported: Vec::new(),
            phases: None,
        };
        assert!(
            outcome.to_report().contains("median of 3 wall-clock runs"),
            "{}",
            outcome.to_report()
        );
    }

    #[test]
    fn the_two_medians_in_the_crate_agree() {
        // Samples::median and median_duration are separate implementations of
        // the same idea; nothing held them together before.
        for n in 1..=8usize {
            let ms: Vec<f64> = (0..n).map(|i| (i as f64 + 1.0) * 10.0).collect();
            let durations: Vec<Duration> = ms
                .iter()
                .map(|v| Duration::from_secs_f64(v / 1000.0))
                .collect();
            let a = samples(&ms).median();
            let b = median_duration(durations).as_secs_f64() * 1000.0;
            assert!((a - b).abs() < 1e-9, "n={n}: {a} vs {b}");
        }
    }

    // ---------------------------------------------------------------- F-26

    #[test]
    fn a_coherent_vault_generated_from_the_wrong_seed_is_refused() {
        // The finding: regenerate with --seed 424242 and the manifest is
        // regenerated alongside it, so vault and manifest agree perfectly about
        // a vault nobody meant to measure. Self-consistency is not canonicality.
        let p = preset("20k").expect("20k exists");
        let wrong = "da0d24b738bbc20657752c8960002b2eed2adbd3c891c4767cf902fd1303e0e4";

        check_vault(
            p,
            p.digest,
            p.digest,
            vaultgen::VAULT_FORMAT_VERSION,
            p.notes,
        )
        .expect("the canonical vault passes");

        let err = check_vault(p, wrong, wrong, vaultgen::VAULT_FORMAT_VERSION, p.notes)
            .expect_err("a coherent but non-canonical vault must be refused");
        assert!(err.contains("not the canonical"), "{err}");
        assert!(err.contains("--preset 20k --force"), "{err}");
    }

    #[test]
    fn a_vault_that_disagrees_with_its_manifest_is_refused() {
        let p = preset("20k").expect("20k exists");
        let err = check_vault(p, "aaa", p.digest, vaultgen::VAULT_FORMAT_VERSION, p.notes)
            .expect_err("hand-edited vault");
        assert!(err.contains("does not match its manifest"), "{err}");
    }

    #[test]
    fn the_wrong_note_count_or_format_version_is_refused() {
        let p = preset("20k").expect("20k exists");
        let err = check_vault(p, p.digest, p.digest, vaultgen::VAULT_FORMAT_VERSION, 400)
            .expect_err("wrong note count");
        assert!(err.contains("400 notes"), "{err}");

        let err = check_vault(p, p.digest, p.digest, 99, p.notes).expect_err("wrong version");
        assert!(err.contains("format version 99"), "{err}");
    }

    #[test]
    fn every_preset_is_pinned_to_a_digest() {
        assert_eq!(PRESETS.len(), 3);
        for p in PRESETS {
            assert_eq!(p.digest.len(), 64, "{} has no canonical digest", p.name);
            assert!(
                p.digest.chars().all(|c| c.is_ascii_hexdigit()),
                "{}",
                p.name
            );
            assert!(p.notes >= 400, "{}", p.name);
        }
        assert!(preset("nope").is_err());
        // The 20k preset is the one every P0 budget measures.
        assert_eq!(preset("20k").unwrap().notes, 20_000);
    }

    // ---------------------------------------------------------------- F-27

    #[test]
    fn a_job_cannot_swallow_the_seam_error_and_post_a_number() {
        // The mutation: `subject::full_rebuild(..)?` becomes
        // `.unwrap_or_default()`, and a fabricated 1 ms number passes. decide()
        // returns the error rather than an Outcome, so there is nothing to
        // swallow that a test cannot see.
        let err = decide(&PRF04_FULL_REBUILD, &[], |_, _| {
            Err("serat-index exposes no rebuild entry point yet — task I4".to_string())
        })
        .expect_err("a failed measurement is not a passing budget");
        assert!(err.contains("task I4"), "{err}");
    }

    #[test]
    fn no_samples_is_a_failure_and_not_an_empty_pass() {
        let err = outcome_from(
            &PRF04_FULL_REBUILD,
            Profile::Reference,
            a_vault(),
            Measurement::default(),
        )
        .expect_err("zero samples cannot be a budget");
        assert!(err.contains("no samples"), "{err}");
    }

    #[test]
    fn a_partially_indexed_vault_cannot_post_a_fast_number() {
        // F-19's guard, extracted from the job so that deleting it fails here.
        check_note_count(20_000, 20_000).expect("a complete rebuild");
        let err = check_note_count(10_000, 20_000).expect_err("half a vault");
        assert!(err.contains("10000 notes"), "{err}");
        assert!(err.contains("not a passing budget"), "{err}");
    }

    #[test]
    fn the_phase_breakdown_is_printed_and_recorded() {
        // The mutation: never print the phases and never write them to JSON.
        // They are the whole content of open design question 1.
        let phases = subject::RebuildPhases {
            walk: Duration::from_millis(500),
            read_hash: Duration::from_millis(3_000),
            parse: Duration::from_millis(6_000),
            producer_wait: Duration::from_millis(3_700),
            write_fts: Duration::from_millis(1_900),
        };
        let outcome = outcome_from(
            &PRF04_FULL_REBUILD,
            Profile::Reference,
            a_vault(),
            Measurement::new(samples(&[20_000.0])).with_phases(phases),
        )
        .expect("phases sum below the wall clock");

        let report = outcome.to_report();
        for needle in ["walk", "read+hash", "parse", "producer waited", "fts build"] {
            assert!(report.contains(needle), "report lacks {needle}: {report}");
        }
        let json = outcome.to_json();
        for needle in [
            "\"walk\"",
            "\"read_hash\"",
            "\"parse\"",
            "\"producer_wait\"",
            "\"write_fts\"",
        ] {
            assert!(json.contains(needle), "json lacks {needle}: {json}");
        }
    }

    #[test]
    fn a_rebuild_that_loaded_through_the_fts_triggers_is_caught() {
        // F-38. The regression is invisible in the total: write-FTS collapses to
        // zero, write-rows absorbs the cost, and the breakdown reads as if the
        // FTS build got free while the wall clock quietly got worse.
        let bulk = subject::RebuildPhases {
            walk: Duration::from_millis(500),
            read_hash: Duration::from_millis(3_000),
            parse: Duration::from_millis(6_000),
            producer_wait: Duration::from_millis(3_700),
            write_fts: Duration::from_millis(1_900),
        };
        check_bulk_load_shape(&bulk).expect("the bulk shape is what we expect");

        let through_triggers = subject::RebuildPhases {
            write_fts: Duration::ZERO,
            producer_wait: Duration::from_millis(5_600),
            ..bulk
        };
        let err = check_bulk_load_shape(&through_triggers).expect_err("triggers live");
        assert!(err.contains("FTS triggers"), "{err}");

        // And it does not fire on a breakdown where nothing was written at all,
        // which is a different fault with a different message.
        check_bulk_load_shape(&subject::RebuildPhases::default()).expect("nothing written");

        // It is wired into the outcome, not merely available.
        let err = outcome_from(
            &PRF04_FULL_REBUILD,
            Profile::Reference,
            a_vault(),
            Measurement::new(samples(&[20_000.0])).with_phases(through_triggers),
        )
        .expect_err("an outcome must not carry a trigger-loaded breakdown");
        assert!(err.contains("FTS triggers"), "{err}");
    }

    #[test]
    fn a_phase_breakdown_that_double_counts_is_refused() {
        // The report says a sum below the wall clock is concurrency. A sum
        // above it means a phase is counted twice, and these numbers answer
        // open design question 1 — reporting them as fact would send architect
        // the wrong way.
        let phases = subject::RebuildPhases {
            walk: Duration::from_millis(10_000),
            read_hash: Duration::from_millis(10_000),
            parse: Duration::from_millis(10_000),
            producer_wait: Duration::from_millis(10_000),
            write_fts: Duration::from_millis(10_000),
        };
        let err = outcome_from(
            &PRF04_FULL_REBUILD,
            Profile::Reference,
            a_vault(),
            Measurement::new(samples(&[20_000.0])).with_phases(phases),
        )
        .expect_err("50 s of phases against a 20 s wall clock");
        assert!(err.contains("counted"), "{err}");
    }

    #[test]
    fn an_unasserted_series_is_reported_inside_the_block_and_labelled() {
        // F-28: printing it outside put an unasserted 812 ms above a line
        // reading PASS.
        let outcome = outcome_from(
            &PRF03_SAVE_TO_INDEX,
            Profile::Reference,
            a_vault(),
            Measurement::new(samples(&[31.2])).reporting(
                "F-03",
                "the external-change path, which is FR-VLT-04's",
                samples(&[812.4]),
            ),
        )
        .expect("valid");
        let report = outcome.to_report();
        let verdict = report.find("PASS").expect("a verdict");
        let extra = report.find("812.4").expect("the reported series");
        assert!(
            extra > verdict,
            "the unasserted number must sit below the verdict:\n{report}"
        );
        assert!(report.contains("reported, not asserted"), "{report}");
        assert!(
            outcome.to_json().contains("reported_not_asserted"),
            "{}",
            outcome.to_json()
        );
    }

    #[test]
    fn argument_parsing_rejects_what_it_cannot_honour() {
        let ok =
            parse_args(&PRF04_FULL_REBUILD, &["--profile".into(), "ci".into()]).expect("valid");
        assert_eq!(ok.profile, Profile::Ci);
        assert_eq!(ok.iterations, PRF04_FULL_REBUILD.iterations);
        assert!(!ok.indicative);

        let fewer =
            parse_args(&PRF04_FULL_REBUILD, &["--iterations".into(), "1".into()]).expect("valid");
        assert!(fewer.indicative, "a short run must be marked indicative");

        assert!(parse_args(&PRF04_FULL_REBUILD, &["--profile".into()]).is_err());
        assert!(parse_args(&PRF04_FULL_REBUILD, &["--iterations".into(), "0".into()]).is_err());
        assert!(parse_args(&PRF04_FULL_REBUILD, &["--profile".into(), "laptop".into()]).is_err());
    }

    // ---------------------------------------------------------------- F-30

    // ---------------------------------------------------- F-227/F-229/F-268

    #[test]
    fn check_save_path_accepts_only_save() {
        check_save_path("save").expect("the direct save path is accepted");

        let err = check_save_path("watcher").expect_err("a watcher-path save must be refused");
        assert!(err.contains("watcher"), "{err}");
        assert!(err.contains("NFR-PRF-03"), "{err}");
        assert!(err.contains("F-227"), "{err}");
        assert!(err.contains("F-229"), "{err}");
        assert!(err.contains("F-268"), "{err}");

        // SaveSamples::default()/SaveTiming::default() both give "" -- the
        // guard must fail closed on the default, not open.
        assert!(check_save_path("").is_err());
    }

    fn save_samples(path: &'static str) -> subject::SaveSamples {
        let mut save = Samples::new();
        save.push(Duration::from_millis(12));
        let mut end_to_end = Samples::new();
        end_to_end.push(Duration::from_millis(400));
        let mut pipeline = Samples::new();
        pipeline.push(Duration::from_millis(180));
        subject::SaveSamples {
            path,
            save,
            end_to_end,
            pipeline,
        }
    }

    #[test]
    fn measure_save_to_index_asserts_the_save_interval_and_reports_the_watcher_ones() {
        // F-268: this is the regression test for the defect the finding is
        // about. Before the fix this measurement asserted `pipeline` -- a
        // watcher-path interval -- and this test would have passed on the
        // wrong series, silently, the same way the real bug did.
        let m = measure_save_to_index_with(&a_vault(), 1, |_, _, _| Ok(save_samples("save")))
            .expect("a save-path measurement is accepted");
        assert_eq!(m.samples.values(), &[12.0]);
        assert_eq!(m.reported.len(), 2);
        let labels: Vec<&str> = m.reported.iter().map(|r| r.label.as_str()).collect();
        assert!(labels.contains(&"watcher-write-to-visible"), "{labels:?}");
        assert!(labels.contains(&"watcher-dequeue-to-visible"), "{labels:?}");
        // Neither watcher-path series may be silently dropped, or FR-VLT-04
        // loses the only measurement of the interval it actually governs.
        assert_eq!(
            m.reported
                .iter()
                .find(|r| r.label == "watcher-write-to-visible")
                .expect("present")
                .samples
                .values(),
            &[400.0]
        );
    }

    #[test]
    fn a_save_measured_on_the_watcher_path_is_refused_not_silently_asserted() {
        let err = measure_save_to_index_with(&a_vault(), 1, |_, _, _| Ok(save_samples("watcher")))
            .expect_err("a watcher-path measurement must not become an NFR-PRF-03 number");
        assert!(err.contains("watcher"), "{err}");
    }

    #[test]
    fn a_failed_save_measurement_propagates_rather_than_being_absorbed() {
        let err = measure_save_to_index_with(&a_vault(), 1, |_, _, _| Err("boom".into()))
            .expect_err("a failed save path is not a measurement");
        assert_eq!(err, "boom");
    }

    // ------------------------------------------------ closing the survivors

    #[test]
    fn the_rebuild_loop_refuses_a_partial_index_and_takes_per_phase_medians() {
        // The mutation sweep found `check_note_count(..)?` could be dropped from
        // measure_full_rebuild with every test still green, because nothing
        // exercised the loop. It does now, through an injected rebuild.
        fn timing(total_ms: u64, notes: usize, parse_ms: u64) -> subject::RebuildTiming {
            subject::RebuildTiming {
                total: Duration::from_millis(total_ms),
                notes,
                links_resolved: 216_971,
                phases: subject::RebuildPhases {
                    walk: Duration::from_millis(1),
                    read_hash: Duration::from_millis(2),
                    parse: Duration::from_millis(parse_ms),
                    producer_wait: Duration::from_millis(4),
                    write_fts: Duration::from_millis(5),
                },
            }
        }

        // A rebuild that indexed half the vault must not post a fast number.
        let err = measure_full_rebuild_with(
            &a_vault(),
            1,
            |_, _| Ok(timing(1, 10_000, 3)),
            |_| Ok(20_000),
        )
        .expect_err("half a vault is not a passing budget");
        assert!(err.contains("not a passing budget"), "{err}");

        // A failing rebuild propagates rather than being absorbed.
        let err =
            measure_full_rebuild_with(&a_vault(), 1, |_, _| Err("boom".into()), |_| Ok(20_000))
                .expect_err("a failed rebuild is not a measurement");
        assert_eq!(err, "boom");

        // Three good runs: the phase reported is the per-phase median, not the
        // phase of the median run and not the slowest.
        let mut n = 0;
        let m = measure_full_rebuild_with(
            &a_vault(),
            3,
            |_, _| {
                n += 1;
                Ok(timing(1_000 * n, 20_000, [30, 10, 20][(n - 1) as usize]))
            },
            |_| Ok(20_000),
        )
        .expect("three complete rebuilds");

        // And stale statistics stop the run, at the level the check runs.
        let err = measure_full_rebuild_with(
            &a_vault(),
            1,
            |_, _| Ok(timing(1, 20_000, 3)),
            |_| Ok(19_968),
        )
        .expect_err("32 notes short of the finished index");
        assert!(err.contains("ix_notes_type"), "{err}");

        // As does a link count that does not match the vault.
        let err = measure_full_rebuild_with(
            &a_vault(),
            1,
            |_, _| {
                let mut t = timing(1, 20_000, 3);
                t.links_resolved = 200_000;
                Ok(t)
            },
            |_| Ok(20_000),
        )
        .expect_err("links dropped");
        assert!(err.contains("dropped"), "{err}");
        assert_eq!(m.samples.len(), 3);
        assert_eq!(m.samples.median(), 2_000.0);
        let phases = m.phases.expect("phases");
        assert_eq!(
            phases.parse,
            Duration::from_millis(20),
            "median parse phase"
        );
    }

    #[test]
    fn stale_analyze_statistics_are_caught() {
        // F-47: the only scale at which this is visible is the real vault.
        // 20000 % 512 = 32, and 32 never crosses the 5% (1,000-note) threshold.
        check_analyze_freshness(20_000, 20_000).expect("statistics describe the finished index");
        let err = check_analyze_freshness(19_968, 20_000).expect_err("32 notes short");
        assert!(err.contains("ix_notes_type"), "{err}");
        assert!(err.contains("full scan"), "{err}");
    }

    #[test]
    fn dropped_or_over_counted_links_are_caught() {
        check_links_resolved(216_971, 216_971).expect("every link resolved");
        let dropped = check_links_resolved(200_000, 216_971).expect_err("links lost");
        assert!(dropped.contains("dropped"), "{dropped}");
        // The bug rust-core-dev hit: rows touched counted as links resolved.
        let over = check_links_resolved(230_000, 216_971).expect_err("over-counted");
        assert!(over.contains("other than resolved links"), "{over}");
    }

    #[test]
    fn the_producer_wait_phase_is_labelled_for_what_it_measures() {
        // A column called "write" holding a small number would settle open
        // design question 1 backwards: near zero means the writer KEPT UP.
        let outcome = outcome_from(
            &PRF04_FULL_REBUILD,
            Profile::Reference,
            a_vault(),
            Measurement::new(samples(&[20_000.0])).with_phases(subject::RebuildPhases {
                walk: Duration::from_millis(500),
                read_hash: Duration::from_millis(3_000),
                parse: Duration::from_millis(6_000),
                producer_wait: Duration::from_millis(40),
                write_fts: Duration::from_millis(1_900),
            }),
        )
        .expect("valid");
        let report = outcome.to_report();
        assert!(report.contains("producer waited"), "{report}");
        assert!(
            report.contains("BLOCKED on the writer"),
            "the reading must travel with the number: {report}"
        );
        // And it must be findable: a reader scanning the block for the
        // explanation looks for a labelled line, not for a sentence.
        assert!(
            report
                .lines()
                .any(|l| l.starts_with("  reading ") && l.contains("BLOCKED on the writer")),
            "{report}"
        );
        assert!(!report.contains("write rows"), "{report}");
    }

    #[test]
    fn the_fts_check_still_covers_a_run_where_the_writer_kept_up() {
        // Gating F-38 on the producer's wait would have skipped the check on
        // exactly the healthy runs it must cover, since a fast writer means the
        // producer never blocks.
        let fast_writer_no_fts = subject::RebuildPhases {
            walk: Duration::from_millis(500),
            read_hash: Duration::from_millis(3_000),
            parse: Duration::from_millis(6_000),
            producer_wait: Duration::ZERO,
            write_fts: Duration::ZERO,
        };
        let err = check_bulk_load_shape(&fast_writer_no_fts).expect_err("still checked");
        assert!(err.contains("FTS triggers"), "{err}");
    }

    #[test]
    fn the_p95_statistic_is_the_ninety_fifth_percentile() {
        // 0.95 -> 0.90 survived the first sweep: at n = 20 nearest rank moves
        // from the 19th sample to the 18th, and nothing looked at the 18th.
        let mut ms = vec![100.0; 18];
        ms.push(500.0);
        ms.push(9_999.0);
        let s = samples(&ms);
        assert_eq!(s.statistic(Statistic::P95), 500.0, "the 19th of 20 sorted");
        assert_ne!(s.statistic(Statistic::P95), s.percentile(0.90));
    }

    #[test]
    fn a_result_file_is_actually_written_and_holds_the_numbers() {
        // "write_result writes nothing" survived: nothing read the file back.
        let dir = std::env::temp_dir().join(format!("budget-result-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let outcome = Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: Environment::default(),
            samples: samples(&[19_000.0, 21_000.0, 20_000.0]),
            reported: Vec::new(),
            phases: None,
        };
        let path = write_result_to(&dir, &outcome).expect("write");
        let written = std::fs::read_to_string(&path).expect("read back");
        assert!(
            written.contains("\"budget_id\": \"prf04-full-rebuild\""),
            "{written}"
        );
        assert!(written.contains("\"value_ms\": 20000.000"), "{written}");
        assert!(
            written.contains("\"samples_ms\": [19000.000, 21000.000, 20000.000]"),
            "{written}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_outcome_taken_in_the_wrong_environment_is_not_a_gate_number() {
        // "gate_eligible always true" survived: the flag was tested on
        // Environment and never through the Outcome that carries it into a
        // report.
        let scanner_off = env::Environment {
            os: env::Os::Windows,
            cores: 16,
            realtime_scanner: Some(false),
            power_plan: Some("Energy Star".into()),
        };
        let outcome = Outcome {
            budget: PRF04_FULL_REBUILD,
            profile: Profile::Reference,
            vault: a_vault(),
            environment: scanner_off,
            samples: samples(&[20_000.0]),
            reported: Vec::new(),
            phases: None,
        };
        assert!(outcome.passed(), "20 s is inside 30 s");
        assert!(
            !outcome.gate_eligible(),
            "but a scanner-off run is not the number the charter gate is judged on"
        );
        assert!(
            outcome.to_report().contains("NOT A GATE NUMBER"),
            "{}",
            outcome.to_report()
        );
        assert!(
            outcome.to_json().contains("\"gate_eligible\": false"),
            "{}",
            outcome.to_json()
        );
    }

    #[test]
    fn every_budget_has_a_job_in_the_ci_workflow() {
        // NFR-PRF-08's acceptance criterion is Inspect. This was a shell loop
        // over a grep, and a grep that matches nothing runs zero iterations and
        // exits 0 — the job whose purpose is catching an unasserted budget was
        // itself unasserted. Iterating P0_BUDGETS instead cannot pass vacuously.
        let workflow = repo_root().join(".github/workflows/budgets.yml");
        let text = std::fs::read_to_string(&workflow)
            .unwrap_or_else(|e| panic!("reading {}: {e}", workflow.display()));

        assert!(!P0_BUDGETS.is_empty());
        for budget in P0_BUDGETS {
            // NFR-PRF-01 is one requirement with two marks measured by one job.
            let job = match budget.id {
                "prf01-window-visible" | "prf01-vault-usable" => "prf01-cold-start",
                other => other,
            };
            assert!(
                text.contains(&format!("\n  {job}:")),
                "budget {} has no asserting job in budgets.yml (NFR-PRF-08)",
                budget.id
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
