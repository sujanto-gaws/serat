//! What the process was asked to do.
//!
//! Four flags, three of which exist for the cold-start budget rather than for a user.
//! The budget harness spawns this binary (`bench/budget/src/subject.rs`), so the
//! command line is part of the NFR-PRF-01 contract and is parsed by hand: a spike shell
//! with four flags does not need an argument-parsing crate, and the dependency would
//! land in every teammate's build for it.

use std::path::PathBuf;
use std::time::Duration;

use crate::startup::TRACE_ENV;

/// How long `--exit-after-ready` waits for the vault-usable mark before giving up.
pub const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(60);

/// The parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Boot {
    /// `--vault <dir>`: open this vault at start, before the window is created, so the
    /// index actor and the vault thread are already running while the webview loads
    /// (architecture §3.4, the "in parallel" column).
    pub vault: Option<PathBuf>,
    /// `--open <vault-relative-path>`: the note to restore. Overrides the settings
    /// file's memory of this vault.
    pub open: Option<String>,
    /// `--exit-after-ready`: write the startup trace and exit as soon as the vault is
    /// usable. For the budget harness; a user never passes it.
    pub exit_after_ready: bool,
    /// `--exit-after-walk`: as `--exit-after-ready`, but also wait for the vault walk
    /// to finish, so `t0_to_walk_complete_ms` is populated.
    ///
    /// That mark is the one quantity the **no-index** path is judged on (P0 finding
    /// F-151): with no index the file tree comes from the streaming walk, so walk
    /// completion is when a tree could first render. It is absent from an ordinary
    /// `--exit-after-ready` run on any vault large enough to matter, because the
    /// process exits at "usable" while the walk is still going — which is correct for
    /// NFR-PRF-01 and useless for FR-VLT-01.
    pub exit_after_walk: bool,
    /// `--ready-timeout-ms <n>`: how long to wait before declaring the start failed.
    pub ready_timeout: Duration,
    /// `SERAT_STARTUP_TRACE`: where the trace line goes.
    pub trace: Option<PathBuf>,
}

impl Default for Boot {
    fn default() -> Self {
        Self {
            vault: None,
            open: None,
            exit_after_ready: false,
            exit_after_walk: false,
            ready_timeout: DEFAULT_READY_TIMEOUT,
            trace: None,
        }
    }
}

impl Boot {
    /// Parse the real command line and environment.
    ///
    /// # Errors
    ///
    /// A sentence naming the offending argument. `main` prints it and exits non-zero
    /// rather than starting: a benchmark run with a typo in a flag must fail, not
    /// silently measure the default.
    pub fn from_env() -> Result<Self, String> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let trace = std::env::var_os(TRACE_ENV).map(PathBuf::from);
        Self::parse(&args, trace)
    }

    /// Parse `args` (without the program name) and an already-read trace path.
    ///
    /// # Errors
    ///
    /// As [`Boot::from_env`].
    pub fn parse(args: &[String], trace: Option<PathBuf>) -> Result<Self, String> {
        let mut boot = Self {
            trace,
            ..Self::default()
        };

        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            let mut value = |flag: &str| {
                rest.next()
                    .cloned()
                    .ok_or_else(|| format!("{flag} needs a value"))
            };
            match arg.as_str() {
                "--vault" => boot.vault = Some(PathBuf::from(value("--vault")?)),
                "--open" => boot.open = Some(value("--open")?),
                "--exit-after-ready" => boot.exit_after_ready = true,
                // Implies the ready behaviour: every guard and every mark that applies
                // to a benchmark run applies to this one too, and stating it here means
                // no later check has to remember to test for both.
                "--exit-after-walk" => {
                    boot.exit_after_walk = true;
                    boot.exit_after_ready = true;
                }
                "--ready-timeout-ms" => {
                    let raw = value("--ready-timeout-ms")?;
                    let ms: u64 = raw
                        .parse()
                        .map_err(|_| format!("--ready-timeout-ms wants a number, got {raw:?}"))?;
                    boot.ready_timeout = Duration::from_millis(ms);
                }
                other => return Err(format!("unknown argument {other:?}")),
            }
        }

        if boot.exit_after_ready && boot.vault.is_none() {
            return Err(
                "--exit-after-ready has nothing to wait for without --vault: a cold start with no \
                 vault would report a number that measures an empty window."
                    .to_owned(),
            );
        }

        Ok(boot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_harness_invocation_parses() {
        let boot = Boot::parse(
            &args(&[
                "--vault",
                "D:/bench/vault-20k",
                "--open",
                "10-projects/a.md",
                "--exit-after-ready",
                "--ready-timeout-ms",
                "5000",
            ]),
            Some(PathBuf::from("trace.json")),
        )
        .expect("parse");

        assert_eq!(boot.vault, Some(PathBuf::from("D:/bench/vault-20k")));
        assert_eq!(boot.open.as_deref(), Some("10-projects/a.md"));
        assert!(boot.exit_after_ready);
        assert_eq!(boot.ready_timeout, Duration::from_millis(5_000));
        assert_eq!(boot.trace, Some(PathBuf::from("trace.json")));
    }

    #[test]
    fn waiting_for_the_walk_implies_waiting_for_ready() {
        // F-151's mark rides on the same guards as NFR-PRF-01's. If `--exit-after-walk`
        // did not imply `--exit-after-ready`, every check written against the latter --
        // needing a vault, needing `--open`, refusing the settings fallback -- would
        // silently not apply to it.
        let boot = Boot::parse(
            &args(&["--vault", "d:/v", "--open", "a.md", "--exit-after-walk"]),
            None,
        )
        .expect("parse");
        assert!(boot.exit_after_walk);
        assert!(
            boot.exit_after_ready,
            "--exit-after-walk must inherit every guard --exit-after-ready carries"
        );

        // And it is refused for the same reasons.
        assert!(Boot::parse(&args(&["--exit-after-walk"]), None).is_err());
    }

    #[test]
    fn a_typo_in_a_flag_fails_rather_than_measuring_the_default() {
        // The failure this exists to prevent: `--exit-after-redy` silently ignored, the
        // process never exits, the budget job times out and nobody knows why.
        assert!(Boot::parse(&args(&["--exit-after-redy"]), None).is_err());
        assert!(Boot::parse(&args(&["--vault"]), None).is_err());
        assert!(Boot::parse(&args(&["--ready-timeout-ms", "soon"]), None).is_err());
    }

    #[test]
    fn timing_a_start_with_no_vault_is_refused() {
        let err = Boot::parse(&args(&["--exit-after-ready"]), None).expect_err("must refuse");
        assert!(err.contains("--vault"), "{err}");
    }

    #[test]
    fn no_arguments_is_a_normal_launch() {
        let boot = Boot::parse(&[], None).expect("parse");
        assert_eq!(boot, Boot::default());
    }
}
