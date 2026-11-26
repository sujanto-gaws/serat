//! Per-sample tags for NFR-PRF-03's save series (task #196, ADR-0019 point 5).
//!
//! Verifies **NFR-PRF-03**, **NFR-PRF-08**, **FR-VLT-04** (the shared checkpoint remedy).
//!
//! The hosted `windows-latest` run comes out INDETERMINATE: p95 about 16 ms, one outlier
//! beside the 50 ms limit (runs 432, 436, 438). The gate is red by design; this module
//! says what the outlier coincided with. Three tags per submitted save, all measured by
//! the application around the timed `write_file` (the harness cannot link serat-app):
//!
//! - **checkpoint**: a WAL checkpoint was running at, or ran inside, the window
//!   (task #183's criterion, `WalHealth`).
//! - **merge**: the FTS5 segment count fell inside the window, so a merge commit landed
//!   (`serat_index::FtsProbe`; task #183's criterion).
//! - **slow commit**: a writer turn of 50 ms or more overlapped the window
//!   (`WalHealth::slow_turn_overlapping`). Added because the asserted interval ends at
//!   the submit, not at the commit (architecture 3.1.1): a long commit reaches a save
//!   only by running beside it, and this is the direct fact that one did. A merge shows
//!   up here as a slow turn; so does anything else slow in the writer.
//!
//! A sample with none of the three is the write itself (atomic write, fsync, rename) or
//! the machine: the third hypothesis the tags exist to leave standing.
//!
//! Tags are **reported, never asserted**. They do not change the verdict; ADR-0019
//! point 5 is a decision for the architect, taken on these numbers.

//!
//! **Phases (task #207).** Beside the tags, the application reports how long each phase of
//! the save path took inside the same interval: `hash` (the open-vault and per-path locks, base read and
//! hash check), `write` (temp file create and `write_all`), `sync` (the temp file's fsync),
//! `rename` (rename and directory sync), `load` (the index side's re-read and hash),
//! `parse`, `submit` (op build and queue send) and `other` (the write's return, the
//! index-handle slot's lock wait, echo recording, clock reads). They say *where* an over-limit sample
//! spent its time, so a sample no tag explains can be put on the vault's disk or on the
//! index side. Reported, never asserted, like the tags.

use std::fmt::Write as _;

use crate::Samples;
use crate::trace::SaveBenchmarkTrace;

/// The save-path phases of task #207, in the order the exit report lists them
/// (`save_phase_<name>_ms`). `other` is last: the interval less the seven measured ones.
pub const PHASES: [&str; 8] = [
    "hash", "write", "sync", "rename", "load", "parse", "submit", "other",
];

/// One submitted save and what its window overlapped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SaveTag {
    /// Position in the submitted series, from 0.
    pub index: usize,
    /// The save's elapsed time.
    pub ms: f64,
    /// A checkpoint overlapped the window.
    pub checkpoint: bool,
    /// Which kind of run it was (task #199); [`CkKind::Unknown`] when it overlapped but
    /// the report did not say, [`CkKind::No`] when none did.
    pub kind: CkKind,
    /// An FTS5 merge landed in the window.
    pub merge: bool,
    /// A writer turn of 50 ms or more overlapped the window.
    pub slow_commit: bool,
}

/// Which trigger the overlapping checkpoint run had (task #199). Reported only: an idle
/// run overlapping a save is a defect of the idle path; a size-bound one goes to the
/// architect (ADR-0019, report-only tags).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CkKind {
    /// No checkpoint overlapped.
    No,
    /// The idle gate (or the pinned `RESTART`).
    Idle,
    /// The 4,000-frame size bound.
    SizeBound,
    /// One overlapped but its kind is not known (an older binary, or it ended between
    /// two reads).
    Unknown,
}

impl CkKind {
    /// `no`, `idle`, `size-bound` or `yes`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::No => "no",
            Self::Idle => "idle",
            Self::SizeBound => "size-bound",
            Self::Unknown => "yes",
        }
    }

    /// The kind a report's word names for an entry whose overlap flag is `overlapped`.
    /// A word outside the four, or one that contradicts the flag (a kind with no overlap, `no` with one), is an error.
    ///
    /// # Errors
    ///
    /// The word is unknown or contradicts the overlap entry.
    pub fn parse(word: &str, overlapped: bool) -> Result<Self, String> {
        let kind = match word {
            "no" => Self::No,
            "yes" => Self::Unknown,
            "idle" => Self::Idle,
            "size-bound" => Self::SizeBound,
            other => {
                return Err(format!(
                    "the save benchmark report's save_checkpoint_kind has {other:?}, not idle, size-bound, no or yes"
                ));
            }
        };
        if (kind != Self::No) != overlapped {
            return Err(format!(
                "the save benchmark report's save_checkpoint_kind {word:?} contradicts its save_checkpoint_overlap entry ({overlapped})"
            ));
        }
        Ok(kind)
    }
}

impl SaveTag {
    /// No tag set: the sample is the write or the machine, not the index.
    #[must_use]
    pub fn untagged(&self) -> bool {
        !(self.checkpoint || self.merge || self.slow_commit)
    }

    /// `checkpoint=idle|size-bound|yes|no merge slow-commit`, each yes or no.
    #[must_use]
    pub fn label(&self) -> String {
        format!(
            "checkpoint={} merge={} slow-commit={}",
            self.kind.label(),
            yes(self.merge),
            yes(self.slow_commit)
        )
    }
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// Which tag a split is by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    /// [`SaveTag::checkpoint`].
    Checkpoint,
    /// [`SaveTag::merge`].
    Merge,
    /// [`SaveTag::slow_commit`].
    SlowCommit,
}

impl Flag {
    /// Every flag, in print order.
    pub const ALL: [Flag; 3] = [Flag::Checkpoint, Flag::Merge, Flag::SlowCommit];

    fn label(self) -> &'static str {
        match self {
            Flag::Checkpoint => "checkpoint overlap",
            Flag::Merge => "FTS5 merge",
            Flag::SlowCommit => "slow writer commit",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Flag::Checkpoint => "checkpoint_overlap",
            Flag::Merge => "fts_merge",
            Flag::SlowCommit => "slow_commit",
        }
    }

    fn of(self, tag: &SaveTag) -> bool {
        match self {
            Flag::Checkpoint => tag.checkpoint,
            Flag::Merge => tag.merge,
            Flag::SlowCommit => tag.slow_commit,
        }
    }
}

/// p95 with and without one flag. A class with no samples has no p95.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlagSplit {
    /// Samples with the flag.
    pub with_n: usize,
    /// Samples without it.
    pub without_n: usize,
    /// Nearest-rank p95 of the flagged samples.
    pub with_p95: Option<f64>,
    /// Nearest-rank p95 of the rest.
    pub without_p95: Option<f64>,
}

impl FlagSplit {
    /// with / without, when both exist and the denominator is above zero.
    #[must_use]
    pub fn ratio(&self) -> Option<f64> {
        match (self.with_p95, self.without_p95) {
            (Some(w), Some(wo)) if wo > 0.0 => Some(w / wo),
            _ => None,
        }
    }
}

/// The whole tagged series for one run.
#[derive(Clone, Debug, PartialEq)]
pub struct SaveTags {
    /// One row per submitted save, in order.
    pub rows: Vec<SaveTag>,
    /// Saves whose FTS5 probe failed (counted as no merge).
    pub probe_failures: u64,
    /// Idle checkpoints deferred to work in flight, over the run.
    pub deferred_busy: u64,
    /// PASSIVE plus RESTART checkpoint runs, over the run.
    pub checkpointer_runs: u64,
    /// Idle wake-ups the frame floor skipped, over the run.
    pub floor_skips: u64,
    /// Per row, the milliseconds of each phase in [`PHASES`] order (task #207); `None`
    /// when the application did not report them.
    pub phases: Option<Vec<[f64; 8]>>,
}

/// The smallest overlapped class whose nearest-rank p95 is not its own maximum
/// (rank ceil(0.95 n) < n needs n >= 20). Below it the class p95 is the outlier itself.
pub const MIN_EXCUSING_CLASS: usize = 20;

/// The phases (indices into [`PHASES`]) that are the vault's own file I/O: `write`
/// (create the temp file, `write_all`), `sync` (the file fsync) and `rename` (the rename
/// and the directory fsync). The `write` phase holds no Serat lock and does no Serat
/// computation beyond the temp-name nonce, so a stall in it is the volume's (ADR-0019
/// second amendment 2026-11-26, task #211). The fsync placement is pinned by
/// `a_save_fsyncs_the_file_in_sync_and_on_unix_the_directory_in_rename_and_nothing_else`
/// in serat-vault (TC-185).
pub const VAULT_IO_PHASES: [usize; 3] = [1, 2, 3];

/// The share of the excused sample's whole elapsed time the vault's I/O phases must
/// carry. The denominator is the whole sample, so `other` and every unmeasured
/// millisecond count against the excusal.
pub const MIN_VAULT_IO_SHARE: f64 = 0.90;

/// Why one over-limit save does not move the verdict (ADR-0019 amendment 2026-11-26,
/// task #205; second amendment, task #211; NFR-PRF-03).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Excusal {
    /// The save overlapped a checkpoint and its class's own p95 is within the limit.
    Checkpoint {
        /// The excused save: position, elapsed time and the kind of checkpoint it overlapped.
        sample: SaveTag,
        /// Saves that overlapped a checkpoint, the excused one included.
        class_n: usize,
        /// That class's own nearest-rank p95: its reported bound, within the limit.
        class_p95_ms: f64,
        /// That class's maximum, the excused save when it is the slowest.
        class_max_ms: f64,
    },
    /// The save was carried by the vault's file I/O: the volume stalled, not Serat.
    VaultIo {
        /// The excused save.
        sample: SaveTag,
        /// Its `write` phase.
        write_ms: f64,
        /// Its `sync` phase.
        sync_ms: f64,
        /// Its `rename` phase.
        rename_ms: f64,
        /// `(write + sync + rename) / sample.ms`.
        share: f64,
        /// `sample.ms - (write + sync + rename)`, within the all-saves p95.
        remainder_ms: f64,
        /// The all-saves nearest-rank p95 the remainder is bounded by.
        series_p95_ms: f64,
    },
}

impl Excusal {
    /// The excused save.
    #[must_use]
    pub fn sample(&self) -> &SaveTag {
        match self {
            Excusal::Checkpoint { sample, .. } | Excusal::VaultIo { sample, .. } => sample,
        }
    }

    /// The JSON key value under `save_tags.excused_by`.
    #[must_use]
    pub fn kind_key(&self) -> &'static str {
        match self {
            Excusal::Checkpoint { .. } => "checkpoint",
            Excusal::VaultIo { .. } => "vault-io",
        }
    }

    /// The report line; names the excusal explicitly so it cannot be read as a plain pass.
    #[must_use]
    pub fn describe(&self, limit_ms: f64) -> String {
        match self {
            Excusal::Checkpoint {
                sample,
                class_n,
                class_p95_ms,
                class_max_ms,
            } => format!(
                "  excused     1 over-limit save excused: sample {} ({:.1} ms) overlapped a {} checkpoint; overlapped class p95 {class_p95_ms:.1} ms (n={class_n}, max {class_max_ms:.1} ms) within {limit_ms:.1} ms. The all-saves p95 is still the asserted figure (ADR-0019, task #205)",
                sample.index,
                sample.ms,
                sample.kind.label(),
            ),
            Excusal::VaultIo {
                sample,
                write_ms,
                sync_ms,
                rename_ms,
                share,
                remainder_ms,
                series_p95_ms,
            } => format!(
                "  excused     1 over-limit save excused: sample {} ({:.1} ms) carried by the vault's file I/O: write {write_ms:.1}, sync {sync_ms:.1}, rename {rename_ms:.1} ms = {:.1}% of the sample; remainder {remainder_ms:.1} ms within the all-saves p95 {series_p95_ms:.1} ms. The all-saves p95 is still the asserted figure (ADR-0019, task #211)",
                sample.index,
                sample.ms,
                share * 100.0,
            ),
        }
    }
}

impl SaveTags {
    /// The tags of a parsed run, or `None` when the application did not report them
    /// (an older binary, or an index with no `WalHealth`). Absence is not an error: tags are
    /// diagnostic and a run without them still gates. A length mismatch is a contract
    /// error, refused by name at parse time (`SaveBenchmarkTrace::parse`); a trace built
    /// by hand that mismatches also yields `None` here rather than mispairing samples.
    #[must_use]
    pub fn from_trace(trace: &SaveBenchmarkTrace) -> Option<SaveTags> {
        let n = trace.submitted_ms.len();
        let complete = |len: usize| len == n;
        if n == 0
            || !complete(trace.checkpoint_overlap.len())
            || !complete(trace.fts_merge.len())
            || !complete(trace.slow_commit.len())
        {
            return None;
        }
        let rows = (0..n)
            .map(|i| SaveTag {
                index: i,
                ms: trace.submitted_ms[i],
                checkpoint: trace.checkpoint_overlap[i],
                // Validated by `SaveBenchmarkTrace::parse`; a hand-built trace that
                // contradicts itself falls back to the flag alone.
                kind: {
                    let flag_only = if trace.checkpoint_overlap[i] {
                        CkKind::Unknown
                    } else {
                        CkKind::No
                    };
                    trace.checkpoint_kind.get(i).map_or(flag_only, |word| {
                        CkKind::parse(word, trace.checkpoint_overlap[i]).unwrap_or(flag_only)
                    })
                },
                merge: trace.fts_merge[i],
                slow_commit: trace.slow_commit[i],
            })
            .collect();
        // Complete or absent: `SaveBenchmarkTrace::parse` refuses anything between, and a
        // hand-built trace that is between yields no phases rather than a mispairing.
        let phases = (trace.phase_ms.len() == PHASES.len()
            && trace.phase_ms.iter().all(|series| series.len() == n))
        .then(|| {
            (0..n)
                .map(|i| std::array::from_fn(|p| trace.phase_ms[p][i]))
                .collect()
        });
        Some(SaveTags {
            rows,
            phases,
            probe_failures: trace.flag_probe_failures,
            deferred_busy: trace.checkpointer_deferred_busy,
            checkpointer_runs: trace.checkpointer_runs,
            floor_skips: trace.checkpointer_floor_skips,
        })
    }

    /// p95 with and without `flag`.
    #[must_use]
    pub fn split(&self, flag: Flag) -> FlagSplit {
        let class = |want: bool| {
            let mut s = Samples::new();
            for row in self.rows.iter().filter(|r| flag.of(r) == want) {
                s.push(std::time::Duration::from_secs_f64(row.ms / 1000.0));
            }
            (s.len(), (!s.is_empty()).then(|| s.percentile(0.95)))
        };
        let (with_n, with_p95) = class(true);
        let (without_n, without_p95) = class(false);
        FlagSplit {
            with_n,
            without_n,
            with_p95,
            without_p95,
        }
    }

    /// Every sample above `limit_ms`, in series order.
    #[must_use]
    pub fn over_limit(&self, limit_ms: f64) -> Vec<&SaveTag> {
        self.rows.iter().filter(|r| r.ms > limit_ms).collect()
    }

    /// ADR-0019: the one over-limit save a series may carry and still be determinate.
    /// `None` unless exactly one save is over `limit_ms` (strictly). Then, in order:
    ///
    /// 1. [`Excusal::Checkpoint`] (amendment 2026-11-26, task #205): it overlapped a
    ///    checkpoint of any kind and the overlapped class has at least
    ///    [`MIN_EXCUSING_CLASS`] saves with its own nearest-rank p95 within `limit_ms`.
    ///    A class of n < 20 would excuse the outlier by its own value.
    /// 2. [`Excusal::VaultIo`] (second amendment 2026-11-26, task #211): phases were
    ///    reported for every row, there are at least [`MIN_EXCUSING_CLASS`] saves, the
    ///    sample's `write + sync + rename` is at least [`MIN_VAULT_IO_SHARE`] of its whole
    ///    time, and the remainder is within the all-saves nearest-rank p95.
    ///
    /// Two or more over, or neither rule holding, is `None`: INDETERMINATE.
    #[must_use]
    pub fn excusal(&self, limit_ms: f64) -> Option<Excusal> {
        let over = self.over_limit(limit_ms);
        let [one] = over.as_slice() else {
            return None;
        };
        self.checkpoint_excusal(one, limit_ms)
            .or_else(|| self.vault_io_excusal(one))
    }

    fn checkpoint_excusal(&self, one: &SaveTag, limit_ms: f64) -> Option<Excusal> {
        if !one.checkpoint {
            return None;
        }
        let class: Vec<f64> = self
            .rows
            .iter()
            .filter(|r| r.checkpoint)
            .map(|r| r.ms)
            .collect();
        if class.len() < MIN_EXCUSING_CLASS {
            return None;
        }
        let class_p95_ms = crate::percentile_of(&class, 0.95);
        if class_p95_ms > limit_ms {
            return None;
        }
        Some(Excusal::Checkpoint {
            sample: *one,
            class_n: class.len(),
            class_p95_ms,
            class_max_ms: class.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        })
    }

    fn vault_io_excusal(&self, one: &SaveTag) -> Option<Excusal> {
        let phases = self.phases.as_ref()?;
        if phases.len() != self.rows.len() || self.rows.len() < MIN_EXCUSING_CLASS {
            return None;
        }
        let row = phases.get(one.index)?;
        let io: f64 = VAULT_IO_PHASES.iter().map(|&p| row[p]).sum();
        // The boundary is an exact `>=` on the quotient, as the ADR states it: 90.0% is
        // excused. It must not gain a tolerance without an amendment. A non-finite figure
        // or an I/O sum above the whole sample (a negative remainder) is not evidence; past
        // those two checks the quotient is a finite number in [0, 1], so `<` is total.
        if !io.is_finite() || io > one.ms || io / one.ms < MIN_VAULT_IO_SHARE {
            return None;
        }
        // The same nearest-rank p95 as the asserted figure (`Samples`), over raw f64 ms
        // here and `Duration` there: equal up to nanosecond rounding.
        let all: Vec<f64> = self.rows.iter().map(|r| r.ms).collect();
        let series_p95_ms = crate::percentile_of(&all, 0.95);
        let remainder_ms = one.ms - io;
        if remainder_ms > series_p95_ms {
            return None;
        }
        Some(Excusal::VaultIo {
            sample: *one,
            write_ms: row[VAULT_IO_PHASES[0]],
            sync_ms: row[VAULT_IO_PHASES[1]],
            rename_ms: row[VAULT_IO_PHASES[2]],
            share: io / one.ms,
            remainder_ms,
            series_p95_ms,
        })
    }

    /// Nearest-rank p95 of each phase over every submitted save, in [`PHASES`] order.
    #[must_use]
    pub fn phase_p95(&self) -> Option<[f64; 8]> {
        let phases = self.phases.as_ref()?;
        if phases.is_empty() {
            return None;
        }
        Some(std::array::from_fn(|p| {
            let series: Vec<f64> = phases.iter().map(|row| row[p]).collect();
            crate::percentile_of(&series, 0.95)
        }))
    }

    /// One sample's phases as `hash 0.4, write 1.2, ...` and the phase that carried it
    /// (the largest). `None` when no phases were reported.
    #[must_use]
    pub fn phase_breakdown(&self, index: usize) -> Option<(String, &'static str)> {
        let row = self.phases.as_ref()?.get(index)?;
        let text = PHASES
            .iter()
            .zip(row)
            .map(|(name, ms)| format!("{name} {ms:.1}"))
            .collect::<Vec<_>>()
            .join(", ");
        let (at, _) = row
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .expect("eight phases");
        Some((text, PHASES[at]))
    }

    /// The report lines, indented to sit inside the report block.
    #[must_use]
    pub fn describe(&self, limit_ms: f64) -> Vec<String> {
        let p = |v: Option<f64>| v.map_or("n/a".to_owned(), |v| format!("{v:.1} ms"));
        let mut lines = Vec::new();
        for flag in Flag::ALL {
            let s = self.split(flag);
            let saturated = if s.without_n < MIN_EXCUSING_CLASS {
                " — saturated: discriminates nothing on this run"
            } else {
                ""
            };
            lines.push(format!(
                "  tag split   {} p95: with {} (n={}), without {} (n={}), ratio {} — reported, not asserted{saturated}",
                flag.label(),
                p(s.with_p95),
                s.with_n,
                p(s.without_p95),
                s.without_n,
                s.ratio().map_or("n/a".to_owned(), |r| format!("{r:.2}x")),
            ));
        }
        lines.push(format!(
            "  tag source  {} checkpoint runs, {} idle starts deferred to work in flight, {} idle wake-ups skipped below the frame floor, {} FTS5 probe failures",
            self.checkpointer_runs, self.deferred_busy, self.floor_skips, self.probe_failures
        ));
        if let Some(p95) = self.phase_p95() {
            lines.push(format!(
                "  phase p95   {} — reported, not asserted",
                PHASES
                    .iter()
                    .zip(p95)
                    .map(|(name, ms)| format!("{name} {ms:.2} ms"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let over = self.over_limit(limit_ms);
        if over.is_empty() {
            lines.push(format!(
                "  over limit  none of {} samples above {limit_ms:.1} ms",
                self.rows.len()
            ));
        } else {
            lines.push(format!(
                "  over limit  {} of {} samples above {limit_ms:.1} ms; with checkpoint {} (idle {}, size-bound {}), with merge {}, with slow commit {}, with none of the three {}",
                over.len(),
                self.rows.len(),
                over.iter().filter(|r| r.checkpoint).count(),
                over.iter().filter(|r| r.kind == CkKind::Idle).count(),
                over.iter().filter(|r| r.kind == CkKind::SizeBound).count(),
                over.iter().filter(|r| r.merge).count(),
                over.iter().filter(|r| r.slow_commit).count(),
                over.iter().filter(|r| r.untagged()).count(),
            ));
            for row in over {
                lines.push(format!(
                    "    sample {:>4}  {:>7.1} ms  {}",
                    row.index,
                    row.ms,
                    row.label()
                ));
                if let Some((text, carried)) = self.phase_breakdown(row.index) {
                    lines.push(format!(
                        "      sample {} {:.1} ms: {text} (carried by {carried})",
                        row.index, row.ms
                    ));
                }
            }
        }
        lines
    }

    /// The JSON object (ending in a comma, as an object member followed by
    /// more keys), two-space indented to sit under the outcome's own keys.
    #[must_use]
    pub fn to_json(&self, limit_ms: f64) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "  \"save_tags\": {{");
        let _ = writeln!(s, "    \"limit_ms\": {limit_ms:.3},");
        let _ = writeln!(s, "    \"probe_failures\": {},", self.probe_failures);
        let _ = writeln!(s, "    \"checkpointer_runs\": {},", self.checkpointer_runs);
        let _ = writeln!(s, "    \"deferred_busy\": {},", self.deferred_busy);
        let _ = writeln!(s, "    \"floor_skips\": {},", self.floor_skips);
        if let Some(p95) = self.phase_p95() {
            let body = PHASES
                .iter()
                .zip(p95)
                .map(|(name, ms)| format!("\"{name}\": {ms:.3}"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(s, "    \"phase_p95_ms\": {{ {body} }},");
        }
        let _ = writeln!(s, "    \"split\": {{");
        let num = |v: Option<f64>| v.map_or("null".to_owned(), |v| format!("{v:.3}"));
        for (i, flag) in Flag::ALL.iter().enumerate() {
            let sp = self.split(*flag);
            let _ = writeln!(
                s,
                "      \"{}\": {{ \"with_n\": {}, \"with_p95_ms\": {}, \"without_n\": {}, \"without_p95_ms\": {} }}{}",
                flag.key(),
                sp.with_n,
                num(sp.with_p95),
                sp.without_n,
                num(sp.without_p95),
                if i + 1 == Flag::ALL.len() { "" } else { "," }
            );
        }
        let _ = writeln!(s, "    }},");
        let over = self.over_limit(limit_ms);
        match self.excusal(limit_ms) {
            Some(
                e @ Excusal::Checkpoint {
                    sample,
                    class_n,
                    class_p95_ms,
                    class_max_ms,
                },
            ) => {
                let _ = writeln!(
                    s,
                    "    \"excused\": true, \"excused_by\": \"{}\", \"excused_index\": {}, \"excused_ms\": {:.3}, \"excused_checkpoint_kind\": \"{}\", \"overlapped_class_n\": {class_n}, \"overlapped_class_p95_ms\": {class_p95_ms:.3}, \"overlapped_class_max_ms\": {class_max_ms:.3},",
                    e.kind_key(),
                    sample.index,
                    sample.ms,
                    sample.kind.label(),
                );
            }
            Some(
                e @ Excusal::VaultIo {
                    sample,
                    share,
                    remainder_ms,
                    ..
                },
            ) => {
                let _ = writeln!(
                    s,
                    "    \"excused\": true, \"excused_by\": \"{}\", \"excused_index\": {}, \"excused_ms\": {:.3}, \"excused_io_share\": {share:.4}, \"excused_remainder_ms\": {remainder_ms:.3},",
                    e.kind_key(),
                    sample.index,
                    sample.ms,
                );
            }
            None => {
                let _ = writeln!(s, "    \"excused\": false,");
            }
        }
        let _ = writeln!(s, "    \"over_limit\": [");
        for (i, row) in over.iter().enumerate() {
            let phase_ms = self
                .phases
                .as_ref()
                .and_then(|rows| rows.get(row.index))
                .map(|ms| {
                    let body = PHASES
                        .iter()
                        .zip(ms)
                        .map(|(name, v)| format!("\"{name}\": {v:.3}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(", \"phase_ms\": {{ {body} }}")
                })
                .unwrap_or_default();
            let _ = writeln!(
                s,
                "      {{ \"index\": {}, \"ms\": {:.3}, \"checkpoint_overlap\": {}, \"checkpoint_kind\": \"{}\", \"fts_merge\": {}, \"slow_commit\": {}{phase_ms} }}{}",
                row.index,
                row.ms,
                row.checkpoint,
                row.kind.label(),
                row.merge,
                row.slow_commit,
                if i + 1 == over.len() { "" } else { "," }
            );
        }
        let _ = writeln!(s, "    ]");
        let _ = writeln!(s, "  }},");
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(index: usize, ms: f64, checkpoint: bool, merge: bool, slow_commit: bool) -> SaveTag {
        SaveTag {
            index,
            ms,
            checkpoint,
            kind: if checkpoint {
                CkKind::Unknown
            } else {
                CkKind::No
            },
            merge,
            slow_commit,
        }
    }

    fn tags(rows: Vec<SaveTag>) -> SaveTags {
        SaveTags {
            rows,
            phases: None,
            probe_failures: 0,
            deferred_busy: 0,
            checkpointer_runs: 0,
            floor_skips: 0,
        }
    }

    #[test]
    fn the_split_reports_p95_per_class_and_the_ratio() {
        // Nearest-rank p95: 20 samples of 10 ms plus one 40 ms outlier tagged merge.
        let mut rows: Vec<SaveTag> = (0..19).map(|i| tag(i, 10.0, false, false, false)).collect();
        rows.push(tag(19, 40.0, false, true, true));
        rows.push(tag(20, 60.0, false, true, true));
        let t = tags(rows);
        let merge = t.split(Flag::Merge);
        assert_eq!((merge.with_n, merge.without_n), (2, 19));
        assert_eq!(merge.with_p95, Some(60.0));
        assert_eq!(merge.without_p95, Some(10.0));
        assert_eq!(merge.ratio(), Some(6.0));
        let cp = t.split(Flag::Checkpoint);
        assert_eq!((cp.with_n, cp.with_p95, cp.ratio()), (0, None, None));
        assert_eq!(cp.without_n, 21);
    }

    #[test]
    fn a_class_that_is_the_whole_series_has_no_ratio() {
        let t = tags(vec![tag(0, 5.0, true, false, false)]);
        let s = t.split(Flag::Checkpoint);
        assert_eq!(
            (s.with_p95, s.without_p95, s.ratio()),
            (Some(5.0), None, None)
        );
    }

    #[test]
    fn every_sample_over_the_limit_is_listed_with_all_three_flags() {
        let t = tags(vec![
            tag(0, 49.9, false, false, false),
            tag(1, 50.0, false, false, false), // exactly the limit is not over it
            tag(2, 56.7, false, true, true),
            tag(3, 110.7, false, false, false), // over, and nothing coincided
            tag(4, 51.0, true, false, false),
        ]);
        let over = t.over_limit(50.0);
        assert_eq!(over.iter().map(|r| r.index).collect::<Vec<_>>(), [2, 3, 4]);
        let text = t.describe(50.0).join("\n");
        assert!(text.contains("3 of 5 samples above 50.0 ms"), "{text}");
        assert!(
            text.contains(
                "with checkpoint 1 (idle 0, size-bound 0), with merge 1, with slow commit 1, with none of the three 1"
            ),
            "{text}"
        );
        assert!(
            text.contains("sample    2")
                && text.contains("checkpoint=no merge=yes slow-commit=yes"),
            "{text}"
        );
        assert!(
            text.contains("sample    3") && text.contains("checkpoint=no merge=no slow-commit=no"),
            "{text}"
        );
        assert!(
            text.contains("sample    4") && text.contains("checkpoint=yes merge=no slow-commit=no"),
            "{text}"
        );
    }

    #[test]
    fn the_checkpoint_tag_names_the_kind_of_the_overlapping_run_and_decides_nothing() {
        // Task #199 (ADR-0019, report-only): `checkpoint=idle|size-bound|no`; the split and
        // the over-limit counts are by overlap, whatever the kind.
        let mut idle = tag(0, 80.0, true, false, false);
        idle.kind = CkKind::Idle;
        let mut size = tag(1, 90.0, true, false, false);
        size.kind = CkKind::SizeBound;
        let t = tags(vec![idle, size, tag(2, 10.0, false, false, false)]);
        let text = t.describe(50.0).join("\n");
        assert!(
            text.contains("with checkpoint 2 (idle 1, size-bound 1)"),
            "{text}"
        );
        assert!(
            text.contains("sample    0") && text.contains("checkpoint=idle merge=no"),
            "{text}"
        );
        assert!(
            text.contains("sample    1") && text.contains("checkpoint=size-bound merge=no"),
            "{text}"
        );
        assert!(
            t.to_json(50.0)
                .contains("\"checkpoint_kind\": \"size-bound\"")
        );
        assert_eq!(
            t.split(Flag::Checkpoint).with_n,
            2,
            "overlap decides the split"
        );
        // From a trace: kinds pair with samples; an older binary's report has none.
        let mut trace = SaveBenchmarkTrace {
            attempted: 2,
            submitted_ms: vec![1.0, 2.0],
            checkpoint_overlap: vec![true, true],
            fts_merge: vec![false, false],
            slow_commit: vec![false, false],
            ..SaveBenchmarkTrace::default()
        };
        let old = SaveTags::from_trace(&trace).expect("complete");
        assert_eq!(
            (old.rows[0].kind, old.rows[1].kind),
            (CkKind::Unknown, CkKind::Unknown)
        );
        trace.checkpoint_kind = vec!["idle".to_owned(), "size-bound".to_owned()];
        let new = SaveTags::from_trace(&trace).expect("complete");
        assert_eq!(
            (new.rows[0].kind, new.rows[1].kind),
            (CkKind::Idle, CkKind::SizeBound)
        );
    }

    #[test]
    fn a_clean_run_says_nothing_was_over() {
        let t = tags(vec![tag(0, 10.0, false, false, false)]);
        let text = t.describe(50.0).join("\n");
        assert!(text.contains("none of 1 samples above 50.0 ms"), "{text}");
        assert!(text.contains("reported, not asserted"), "{text}");
    }

    #[test]
    fn the_json_carries_the_split_and_every_over_limit_sample() {
        let t = tags(vec![
            tag(0, 10.0, false, false, false),
            tag(1, 80.0, true, false, true),
        ]);
        let json = t.to_json(50.0);
        assert!(json.contains("\"save_tags\": {"), "{json}");
        assert!(
            json.contains(
                "\"fts_merge\": { \"with_n\": 0, \"with_p95_ms\": null, \"without_n\": 2"
            ),
            "{json}"
        );
        assert!(
            json.contains("\"checkpoint_overlap\": { \"with_n\": 1, \"with_p95_ms\": 80.000"),
            "{json}"
        );
        assert!(json.contains("{ \"index\": 1, \"ms\": 80.000, \"checkpoint_overlap\": true, \"checkpoint_kind\": \"yes\", \"fts_merge\": false, \"slow_commit\": true }"), "{json}");
        // Valid as an object member: braces balance and the member ends in a comma.
        assert_eq!(json.matches('{').count(), json.matches('}').count());
        assert!(json.trim_end().ends_with("},"));
    }

    #[test]
    fn tags_come_from_a_trace_only_when_complete() {
        let mut trace = SaveBenchmarkTrace {
            attempted: 2,
            submitted_ms: vec![1.0, 2.0],
            ..SaveBenchmarkTrace::default()
        };
        assert_eq!(SaveTags::from_trace(&trace), None, "an untagged report");
        trace.checkpoint_overlap = vec![false, true];
        trace.fts_merge = vec![true, false];
        assert_eq!(
            SaveTags::from_trace(&trace),
            None,
            "one array still missing"
        );
        trace.slow_commit = vec![false, false];
        let t = SaveTags::from_trace(&trace).expect("complete");
        assert_eq!(t.rows[0], tag(0, 1.0, false, true, false));
        assert_eq!(t.rows[1], tag(1, 2.0, true, false, false));
    }

    fn with_phases(mut t: SaveTags, rows: Vec<[f64; 8]>) -> SaveTags {
        t.phases = Some(rows);
        t
    }

    #[test]
    fn the_p95_of_each_phase_is_reported_over_every_save() {
        // Task #207, NFR-PRF-03: 20 saves whose sync is 1..=20 ms, so the nearest-rank p95
        // (the 19th) is 19 and differs from the median (10) and the maximum (20).
        let rows: Vec<SaveTag> = (0..20).map(|i| tag(i, 6.0, false, false, false)).collect();
        let phases: Vec<[f64; 8]> = (0..20)
            .map(|i| [0.1, 0.5, (i + 1) as f64, 0.2, 0.1, 0.1, 0.1, 0.0])
            .collect();
        let t = with_phases(tags(rows), phases);
        let p95 = t.phase_p95().expect("reported");
        assert_eq!((p95[1], p95[2]), (0.5, 19.0));
        let text = t.describe(50.0).join("\n");
        assert!(
            text.contains("phase p95   hash 0.10 ms, write 0.50 ms, sync 19.00 ms"),
            "{text}"
        );
        assert!(text.contains("reported, not asserted"), "{text}");
        assert_eq!(tags(vec![]).phase_p95(), None);
    }

    #[test]
    fn an_over_limit_sample_prints_its_phase_breakdown_and_what_carried_it() {
        let t = with_phases(
            tags(vec![
                tag(0, 5.0, false, false, false),
                tag(1, 62.7, false, false, false),
            ]),
            vec![
                [0.1, 0.3, 3.0, 0.2, 0.1, 0.1, 0.1, 0.0],
                [0.4, 1.2, 58.9, 0.3, 0.2, 1.1, 0.9, 0.0],
            ],
        );
        let text = t.describe(50.0).join("\n");
        assert!(
            text.contains(
                "sample 1 62.7 ms: hash 0.4, write 1.2, sync 58.9, rename 0.3, load 0.2, parse 1.1, submit 0.9, other 0.0 (carried by sync)"
            ),
            "{text}"
        );
        assert!(
            !text.contains("sample 0 5.0 ms"),
            "only over-limit samples: {text}"
        );
        let json = t.to_json(50.0);
        assert!(json.contains("\"phase_p95_ms\": {"), "{json}");
        assert!(
            json.contains("\"slow_commit\": false, \"phase_ms\": { \"hash\": 0.400, \"write\": 1.200, \"sync\": 58.900"),
            "{json}"
        );
        assert_eq!(json.matches('{').count(), json.matches('}').count());
        // Without phases the lines are the ones the tags always printed.
        let bare = tags(vec![tag(0, 80.0, false, false, false)]);
        let text = bare.describe(50.0).join("\n");
        assert!(
            !text.contains("phase p95") && !text.contains("carried by"),
            "{text}"
        );
    }

    #[test]
    fn phases_come_from_a_trace_only_when_all_eight_series_are_as_long_as_the_samples() {
        let mut trace = SaveBenchmarkTrace {
            attempted: 2,
            submitted_ms: vec![1.0, 2.0],
            checkpoint_overlap: vec![false, false],
            fts_merge: vec![false, false],
            slow_commit: vec![false, false],
            ..SaveBenchmarkTrace::default()
        };
        assert_eq!(SaveTags::from_trace(&trace).expect("tags").phases, None);
        trace.phase_ms = (0..8).map(|p| vec![p as f64, p as f64 + 0.5]).collect();
        let t = SaveTags::from_trace(&trace).expect("tags");
        let rows = t.phases.expect("phases");
        assert_eq!(rows[1], [0.5, 1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5]);
        trace.phase_ms[3].pop();
        assert_eq!(
            SaveTags::from_trace(&trace).expect("tags").phases,
            None,
            "a hand-built mismatch never mispairs"
        );
    }

    #[test]
    fn an_untagged_sample_is_the_write_or_the_machine() {
        assert!(tag(0, 1.0, false, false, false).untagged());
        assert!(!tag(0, 1.0, false, false, true).untagged());
    }
}
