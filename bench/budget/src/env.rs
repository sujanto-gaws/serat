//! What the machine was doing while the number was taken.
//!
//! A rebuild of 20,000 notes reads 20,000 files, so on Windows the state of the
//! on-access virus scanner moves the result more than most code changes will.
//! The reference machine (`docs/05-testing/reference-machine.md`) happens to
//! have real-time protection disabled, which makes every number taken on it a
//! best case that no ordinary user will see.
//!
//! The lead's instruction for task B2 is therefore that NFR-PRF-04 is measured
//! **both ways**, and that the scanner-enabled figure is the one the charter's
//! "< 30 s" gate is judged against. This module is how that survives contact
//! with a tired engineer at 11pm: the harness detects the scanner state itself,
//! stamps it on every result, and marks a scanner-disabled run as **not gate
//! eligible**. You cannot post a Defender-off number as the gate number by
//! forgetting to mention it.
//!
//! The harness never *changes* the scanner state. Toggling real-time protection
//! is an administrator action with consequences beyond this repository, and a
//! benchmark tool that turns off your antivirus to look good is exactly the kind
//! of thing this project should not contain.

use std::process::Command;

/// Which operating system the measurement was taken on.
///
/// An enum rather than a string, and defaulting to `Unknown` rather than to an
/// empty name, because [`Environment::gate_eligible`] is a safety check: a
/// default-constructed environment used to fall through to "eligible", which is
/// the wrong way for a check like this to fail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Os {
    /// Not determined. Never gate eligible.
    #[default]
    Unknown,
    /// Windows, where the on-access scanner question arises.
    Windows,
    /// macOS.
    MacOs,
    /// Linux.
    Linux,
}

impl Os {
    /// How it prints.
    pub fn label(self) -> &'static str {
        match self {
            Os::Unknown => "unknown",
            Os::Windows => "windows",
            Os::MacOs => "macos",
            Os::Linux => "linux",
        }
    }
}

/// The measurement environment, detected at run time.
#[derive(Clone, Debug, Default)]
pub struct Environment {
    /// Which platform.
    pub os: Os,
    /// Logical processors.
    pub cores: usize,
    /// Windows Defender real-time protection. `None` where it does not apply or
    /// could not be determined — which is itself reported, never assumed false.
    pub realtime_scanner: Option<bool>,
    /// The active Windows power plan, by name. Boost behaviour and therefore
    /// single-thread throughput ride on it, and on a laptop so do thermals.
    pub power_plan: Option<String>,
}

impl Environment {
    /// Detect it, once per process.
    ///
    /// Cached because detection shells out to PowerShell twice and the answer
    /// cannot change during a run — without the cache the harness's own test
    /// suite spent most of its time asking Windows the same two questions.
    pub fn detect() -> Environment {
        static CACHED: std::sync::OnceLock<Environment> = std::sync::OnceLock::new();
        CACHED.get_or_init(Environment::detect_uncached).clone()
    }

    /// Detect it, ignoring the cache.
    pub fn detect_uncached() -> Environment {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0);
        let os = if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        };
        let (realtime_scanner, power_plan) = if cfg!(target_os = "windows") {
            (windows_defender_realtime(), windows_power_plan())
        } else {
            (None, None)
        };
        Environment {
            os,
            cores,
            realtime_scanner,
            power_plan,
        }
    }

    /// Whether a number taken in this environment may be recorded as the gate
    /// number for a filesystem-bound budget.
    ///
    /// On Windows that means the on-access scanner was running. Elsewhere there
    /// is no equivalent switch, so the question does not arise.
    pub fn gate_eligible(&self) -> bool {
        match self.os {
            Os::Windows => self.realtime_scanner == Some(true),
            // Unknown is not the same as fine: if the harness cannot say where
            // it ran, it cannot say the number is a gate number.
            Os::Unknown => false,
            Os::MacOs | Os::Linux => true,
        }
    }

    /// Why it is not gate eligible, if it is not.
    pub fn ineligibility_reason(&self) -> Option<&'static str> {
        if self.gate_eligible() {
            return None;
        }
        if self.os == Os::Unknown {
            return Some(
                "the platform could not be determined, so this run cannot be attributed to any environment.",
            );
        }
        Some(match self.realtime_scanner {
            Some(false) => {
                "real-time virus scanning is disabled, so this is a best case no user will \
                 see. Useful as the comparable figure; not the gate number. Re-run with \
                 real-time protection enabled for the number the charter gate is judged on."
            }
            _ => {
                "the real-time scanner state could not be determined, so this run cannot be \
                 attributed to either environment."
            }
        })
    }

    /// One line for a report header.
    pub fn summary(&self) -> String {
        let scanner = match self.realtime_scanner {
            Some(true) => "on",
            Some(false) => "OFF",
            None => "unknown",
        };
        let plan = self.power_plan.as_deref().unwrap_or("unknown");
        format!(
            "{} · {} cores · realtime scanner {scanner} · power plan {plan}",
            self.os.label(),
            self.cores
        )
    }
}

fn powershell(script: &str) -> Option<String> {
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn windows_defender_realtime() -> Option<bool> {
    // Get-MpComputerStatus rather than Get-MpPreference: the preference object
    // reports what was configured, the status object reports what is actually
    // running, and those differ on a machine where a policy has been applied.
    match powershell("(Get-MpComputerStatus).RealTimeProtectionEnabled")?.as_str() {
        "True" => Some(true),
        "False" => Some(false),
        _ => None,
    }
}

fn windows_power_plan() -> Option<String> {
    let raw = powershell("(powercfg /getactivescheme)")?;
    // "Power Scheme GUID: <guid>  (Balanced)" — the human name is what a report
    // reader recognises, so prefer it and fall back to the whole line.
    match (raw.rfind('('), raw.rfind(')')) {
        (Some(a), Some(b)) if b > a + 1 => Some(raw[a + 1..b].to_string()),
        _ => Some(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_reports_something_about_this_machine() {
        let env = Environment::detect();
        assert!(env.cores > 0);
        assert!(!env.summary().is_empty());
    }

    #[test]
    fn a_scanner_disabled_windows_run_is_not_gate_eligible() {
        // The rule the lead set for B2, asserted rather than remembered.
        let off = Environment {
            os: Os::Windows,
            cores: 8,
            realtime_scanner: Some(false),
            power_plan: Some("Energy Star".into()),
        };
        assert!(!off.gate_eligible());
        assert!(off.ineligibility_reason().unwrap().contains("best case"));

        let on = Environment {
            realtime_scanner: Some(true),
            ..off.clone()
        };
        assert!(on.gate_eligible());
        assert!(on.ineligibility_reason().is_none());

        // Unknown is not the same as fine.
        let unknown = Environment {
            realtime_scanner: None,
            ..off
        };
        assert!(!unknown.gate_eligible());
    }

    #[test]
    fn a_non_windows_run_has_no_scanner_question_to_answer() {
        let linux = Environment {
            os: Os::Linux,
            cores: 4,
            realtime_scanner: None,
            power_plan: None,
        };
        assert!(linux.gate_eligible());

        // But a default environment is not eligible: an unknown platform is not
        // a safe platform, and this is a safety check.
        assert!(!Environment::default().gate_eligible());
        assert!(
            Environment::default()
                .ineligibility_reason()
                .unwrap()
                .contains("could not be determined")
        );
    }
}
