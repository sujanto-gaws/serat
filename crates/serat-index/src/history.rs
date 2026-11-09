//! The switcher's frecency record, `.serat/history.json` (architecture §9.2; FR-NAV-01).
//!
//! Pure bytes in, bytes out: this module does no I/O. The write goes through
//! `Vault::update_history` in serat-vault, which owns the one guarded atomic-write path;
//! serat-index may not depend on serat-vault (§2.1), so the caller composes the two:
//!
//! ```text
//! vault.update_history(|old| Some(History::bytes_after_open(old, path, now_ms)))
//! ```
//!
//! The record is a ranking signal and nothing depends on it (C-02, goal 5): missing,
//! unreadable, or of an unknown version, it is read as empty and replaced on the next
//! write. Keyed by vault-relative path, the last [`MAX_OPENS`] open times per path, at
//! most [`MAX_PATHS`] paths; frecency is the sum over the stored opens of
//! `0.5^(age_days / 7)`. A rename done in Serat moves the key ([`History::rename`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The on-disk format version. Any other value is ignored and replaced.
pub const VERSION: u32 = 1;
/// Open times kept per path.
pub const MAX_OPENS: usize = 10;
/// Paths kept in the record; the least recently opened is dropped first.
pub const MAX_PATHS: usize = 2000;
/// The half-life of one open, in days.
pub const HALF_LIFE_DAYS: f64 = 7.0;

const DAY_MS: f64 = 86_400_000.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    version: u32,
    /// Path to open times (unix milliseconds), oldest first.
    opens: BTreeMap<String, Vec<i64>>,
}

/// The frecency record, in memory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    opens: BTreeMap<String, Vec<i64>>,
}

impl History {
    /// Read a record. `None`, unparseable bytes and an unknown version all give an empty
    /// record: ranking degrades to lexical, nothing breaks.
    #[must_use]
    pub fn from_bytes(bytes: Option<&[u8]>) -> Self {
        let Some(record) = bytes.and_then(|b| serde_json::from_slice::<Record>(b).ok()) else {
            return Self::default();
        };
        if record.version != VERSION {
            return Self::default();
        }
        let mut history = Self {
            opens: record.opens,
        };
        for times in history.opens.values_mut() {
            times.sort_unstable();
            let extra = times.len().saturating_sub(MAX_OPENS);
            times.drain(..extra);
        }
        history.opens.retain(|_, times| !times.is_empty());
        history.trim();
        history
    }

    /// The record as bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let record = Record {
            version: VERSION,
            opens: self.opens.clone(),
        };
        serde_json::to_vec(&record).unwrap_or_default()
    }

    /// The closure body for `Vault::update_history` when a note was opened in an editor
    /// pane: the new bytes of the record after one open of `path` at `now_ms`.
    #[must_use]
    pub fn bytes_after_open(existing: Option<&[u8]>, path: &str, now_ms: i64) -> Vec<u8> {
        let mut history = Self::from_bytes(existing);
        history.record_open(path, now_ms);
        history.to_bytes()
    }

    /// The closure body for a rename done in Serat: the key moves, its opens stay.
    #[must_use]
    pub fn bytes_after_rename(existing: Option<&[u8]>, from: &str, to: &str) -> Vec<u8> {
        let mut history = Self::from_bytes(existing);
        history.rename(from, to);
        history.to_bytes()
    }

    /// Record one open of `path` at `now_ms` (unix milliseconds).
    pub fn record_open(&mut self, path: &str, now_ms: i64) {
        let times = self.opens.entry(path.to_owned()).or_default();
        times.push(now_ms);
        times.sort_unstable();
        let extra = times.len().saturating_sub(MAX_OPENS);
        times.drain(..extra);
        self.trim();
    }

    /// Move `from`'s opens to `to`, merging with any opens `to` already has.
    pub fn rename(&mut self, from: &str, to: &str) {
        if from == to {
            return;
        }
        if let Some(moved) = self.opens.remove(from) {
            let times = self.opens.entry(to.to_owned()).or_default();
            times.extend(moved);
            times.sort_unstable();
            let extra = times.len().saturating_sub(MAX_OPENS);
            times.drain(..extra);
        }
    }

    /// Frecency of `path` at `now_ms`; 0.0 for a path never opened. A clock that went
    /// backwards counts an open as happening now.
    #[must_use]
    pub fn frecency(&self, path: &str, now_ms: i64) -> f64 {
        self.opens
            .get(path)
            .map_or(0.0, |times| frecency_of(times, now_ms))
    }

    /// Every path with its frecency, for a scorer that wants them all at once.
    pub fn scores(&self, now_ms: i64) -> impl Iterator<Item = (&str, f64)> {
        self.opens
            .iter()
            .map(move |(path, times)| (path.as_str(), frecency_of(times, now_ms)))
    }

    /// Number of paths held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.opens.len()
    }

    /// Whether no path is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.opens.is_empty()
    }

    fn trim(&mut self) {
        while self.opens.len() > MAX_PATHS {
            let oldest = self
                .opens
                .iter()
                .min_by_key(|(path, times)| (times.last().copied().unwrap_or(i64::MIN), *path))
                .map(|(path, _)| path.clone());
            match oldest {
                Some(path) => self.opens.remove(&path),
                None => break,
            };
        }
    }
}

#[allow(clippy::cast_precision_loss)]
fn frecency_of(times: &[i64], now_ms: i64) -> f64 {
    times
        .iter()
        .map(|&t| {
            let age_days = (now_ms - t).max(0) as f64 / DAY_MS;
            (-age_days / HALF_LIFE_DAYS).exp2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;

    #[test]
    fn a_week_old_open_is_worth_half_of_one_made_now() {
        let mut h = History::default();
        h.record_open("a.md", 0);
        assert!((h.frecency("a.md", 0) - 1.0).abs() < 1e-9);
        assert!((h.frecency("a.md", 7 * DAY) - 0.5).abs() < 1e-9);
        assert_eq!(h.frecency("b.md", 0), 0.0);
        assert!((h.frecency("a.md", -5 * DAY) - 1.0).abs() < 1e-9, "skew");
    }

    #[test]
    fn frequency_adds_and_only_the_last_ten_opens_count() {
        let mut h = History::default();
        for i in 0..15 {
            h.record_open("a.md", i);
        }
        assert_eq!(h.opens["a.md"].len(), MAX_OPENS);
        assert_eq!(h.opens["a.md"][0], 5);
        assert!((h.frecency("a.md", 14) - 10.0).abs() < 1e-3);
    }

    #[test]
    fn the_least_recently_opened_path_is_dropped_past_the_cap() {
        let mut h = History::default();
        for i in 0..=MAX_PATHS {
            h.record_open(&format!("n{i}.md"), i64::try_from(i).unwrap());
        }
        assert_eq!(h.len(), MAX_PATHS);
        assert_eq!(h.frecency("n0.md", 0), 0.0);
        assert!(h.frecency("n1.md", 1) > 0.0);
    }

    #[test]
    fn a_rename_moves_the_key() {
        let mut h = History::default();
        h.record_open("a.md", 1);
        h.record_open("b.md", 2);
        h.rename("a.md", "b.md");
        assert_eq!(h.opens["b.md"], vec![1, 2]);
        assert!(!h.opens.contains_key("a.md"));
    }

    #[test]
    fn missing_garbage_and_unknown_versions_read_as_empty() {
        assert!(History::from_bytes(None).is_empty());
        assert!(History::from_bytes(Some(b"not json")).is_empty());
        assert!(History::from_bytes(Some(br#"{"version":99,"opens":{"a.md":[1]}}"#)).is_empty());
        assert!(History::from_bytes(Some(br#"{"version":1}"#)).is_empty());
        let after = History::bytes_after_open(Some(b"\0\0"), "a.md", 5);
        assert_eq!(History::from_bytes(Some(&after)).len(), 1);
    }

    #[test]
    fn it_round_trips_and_trims_a_hostile_file() {
        let mut h = History::default();
        h.record_open("a.md", 3);
        h.record_open("a.md", 1);
        assert_eq!(History::from_bytes(Some(&h.to_bytes())), h);
        let many: Vec<String> = (0..50).map(|i| i.to_string()).collect();
        let hostile = format!(
            r#"{{"version":1,"opens":{{"a.md":[{}],"empty.md":[]}}}}"#,
            many.join(",")
        );
        let h = History::from_bytes(Some(hostile.as_bytes()));
        assert_eq!(h.opens["a.md"].len(), MAX_OPENS);
        assert!(!h.opens.contains_key("empty.md"));
    }
}
