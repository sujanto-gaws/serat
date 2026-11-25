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

use std::fmt::Write as _;

use crate::Samples;
use crate::trace::SaveBenchmarkTrace;

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

    fn parse(word: &str, overlapped: bool) -> Self {
        match word {
            "idle" => Self::Idle,
            "size-bound" => Self::SizeBound,
            _ if overlapped => Self::Unknown,
            _ => Self::No,
        }
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
                kind: CkKind::parse(
                    trace.checkpoint_kind.get(i).map_or("", String::as_str),
                    trace.checkpoint_overlap[i],
                ),
                merge: trace.fts_merge[i],
                slow_commit: trace.slow_commit[i],
            })
            .collect();
        Some(SaveTags {
            rows,
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

    /// The report lines, indented to sit inside the report block.
    #[must_use]
    pub fn describe(&self, limit_ms: f64) -> Vec<String> {
        let p = |v: Option<f64>| v.map_or("n/a".to_owned(), |v| format!("{v:.1} ms"));
        let mut lines = Vec::new();
        for flag in Flag::ALL {
            let s = self.split(flag);
            lines.push(format!(
                "  tag split   {} p95: with {} (n={}), without {} (n={}), ratio {} — reported, not asserted",
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
        let _ = writeln!(s, "    \"over_limit\": [");
        for (i, row) in over.iter().enumerate() {
            let _ = writeln!(
                s,
                "      {{ \"index\": {}, \"ms\": {:.3}, \"checkpoint_overlap\": {}, \"checkpoint_kind\": \"{}\", \"fts_merge\": {}, \"slow_commit\": {} }}{}",
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

    #[test]
    fn an_untagged_sample_is_the_write_or_the_machine() {
        assert!(tag(0, 1.0, false, false, false).untagged());
        assert!(!tag(0, 1.0, false, false, true).untagged());
    }
}
