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

/// The measurement environment, detected at run time.
#[derive(Clone, Debug, Default)]
pub struct Environment {
    /// `windows`, `macos`, `linux`.
    pub os: &'static str,
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
    /// Detect it.
    pub fn detect() -> Environment {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0);
        let os = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
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
            "windows" => self.realtime_scanner == Some(true),
            _ => true,
        }
    }

    /// Why it is not gate eligible, if it is not.
    pub fn ineligibility_reason(&self) -> Option<&'static str> {
        if self.gate_eligible() {
            return None;
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
            self.os, self.cores
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
            os: "windows",
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
            os: "linux",
            cores: 4,
            realtime_scanner: None,
            power_plan: None,
        };
        assert!(linux.gate_eligible());
    }
}
