//! Settings: one TOML file in the platform configuration directory.
//!
//! The first thing on the cold-start critical path (architecture §3.4, first row), so
//! it obeys the rule that governs that row: **nothing here is proportional to the size
//! of the vault, and nothing here can stop the window opening.** A file that is
//! missing, unreadable, or full of nonsense yields defaults and a recorded reason; it
//! never yields an error the caller has to handle before it can paint.
//!
//! TOML rather than JSON because a user edits this by hand when something goes wrong,
//! and the platform config directory rather than the vault because it is *not* vault
//! state: C-02 says the vault survives the application, and settings that lived in
//! `.serat/` would be deleted by the rebuild that C-02 makes routine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Directory name under the platform config root.
const DIR: &str = "Serat";

/// File name within it.
const FILE: &str = "settings.toml";

/// Everything the shell remembers between sessions.
///
/// Every field is optional and every field has a default. A settings file written by a
/// newer build is read by an older one without complaint, and a field this build does
/// not know about is dropped rather than being a parse failure — which is
/// `#[serde(default)]` plus TOML's tolerance, and is deliberate: settings are not a
/// schema (ADR-0002 is about the index, and this is not the index).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Absolute host path of the vault to reopen.
    pub last_vault: Option<String>,

    /// The note to restore, per vault root. Keyed by the canonical root because a user
    /// with two vaults expects each to reopen where they left it, and a single field
    /// would make switching vaults lose that.
    pub last_open: BTreeMap<String, String>,

    /// Window geometry.
    pub window: WindowSettings,

    /// `[editor]`.
    pub editor: EditorSettings,

    /// User rebindings, `[keybindings]`: registry command id to CodeMirror key
    /// notation. An **empty string** means deliberately unbound (TOML has no null); a
    /// command with no entry uses the registry default (FR-PLT-05).
    pub keybindings: BTreeMap<String, String>,
}

/// `[editor]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    /// Live preview on; `false` selects split preview (FR-EDT-10).
    pub live_preview: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self { live_preview: true }
    }
}

/// The longest command id or key notation accepted. Settings are hand-editable; this
/// only stops a runaway string from a bad caller reaching the file.
pub const MAX_BINDING_TEXT: usize = 128;

/// Window geometry, restored before the window is created.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    /// Logical width.
    pub width: f64,
    /// Logical height.
    pub height: f64,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            width: 1280.0,
            height: 820.0,
        }
    }
}

/// Settings as they were loaded, and anything that went wrong loading them.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The settings to use. Always present, defaults if nothing else.
    pub settings: Settings,
    /// Where they were read from, or would be written to.
    pub path: Option<PathBuf>,
    /// Why the file on disk was not used, if it was not.
    ///
    /// Not an error return, because there is no caller that could usefully handle it:
    /// the window opens either way. Reported so a user whose settings silently stopped
    /// applying can find out why.
    pub problem: Option<String>,
}

impl Settings {
    /// Where the settings file lives, or `None` on a system with no config directory.
    #[must_use]
    pub fn default_path() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join(DIR).join(FILE))
    }

    /// Load from the platform config directory.
    #[must_use]
    pub fn load() -> Loaded {
        match Self::default_path() {
            Some(path) => {
                let mut loaded = Self::load_from(&path);
                loaded.path = Some(path);
                loaded
            }
            None => Loaded {
                settings: Self::default(),
                path: None,
                problem: Some(
                    "no platform configuration directory; settings are not persisted".to_owned(),
                ),
            },
        }
    }

    /// Load from a specific file. Absent is not a problem; unreadable and malformed
    /// are, and both still return usable defaults.
    #[must_use]
    pub fn load_from(path: &Path) -> Loaded {
        let defaults = |problem: Option<String>| Loaded {
            settings: Self::default(),
            path: Some(path.to_path_buf()),
            problem,
        };

        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return defaults(None),
            Err(err) => {
                return defaults(Some(format!("{} could not be read: {err}", path.display())));
            }
        };

        match toml::from_str::<Self>(&text) {
            Ok(settings) => Loaded {
                settings,
                path: Some(path.to_path_buf()),
                problem: None,
            },
            Err(err) => defaults(Some(format!(
                "{} is not valid settings, so defaults are in use and the file is left untouched: {err}",
                path.display()
            ))),
        }
    }

    /// Write to `path`, creating the directory if it is absent.
    ///
    /// Temp-file-and-rename for the same reason `serat-vault` writes notes that way: a
    /// power cut during a save must not leave a settings file that is neither the old
    /// one nor the new one. This never runs on the critical path — the shell saves
    /// after it is usable, and on shutdown.
    ///
    /// # Errors
    ///
    /// Any filesystem failure, as a sentence. The caller logs it; there is nothing
    /// else to do, and a user whose config directory is read-only should still be able
    /// to edit notes.
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        let parent = path
            .parent()
            .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("{} could not be created: {err}", parent.display()))?;

        let text = toml::to_string_pretty(self)
            .map_err(|err| format!("settings do not serialise: {err}"))?;
        let temp = path.with_extension("toml.tmp");
        std::fs::write(&temp, text)
            .map_err(|err| format!("{} could not be written: {err}", temp.display()))?;
        std::fs::rename(&temp, path).map_err(|err| {
            format!(
                "{} could not be renamed over {}: {err}",
                temp.display(),
                path.display()
            )
        })
    }

    /// The note to restore for `vault_root`, if any.
    #[must_use]
    pub fn last_open_for(&self, vault_root: &Path) -> Option<&str> {
        self.last_open.get(&key(vault_root)).map(String::as_str)
    }

    /// Remember which note is open in `vault_root`.
    pub fn remember_open(&mut self, vault_root: &Path, note: &str) {
        self.last_vault = Some(vault_root.display().to_string());
        self.last_open.insert(key(vault_root), note.to_owned());
    }
}

/// Vault roots are keyed by their display form, normalised to forward slashes so the
/// same vault does not acquire two entries because one session spelled it with
/// backslashes.
fn key(vault_root: &Path) -> String {
    vault_root.display().to_string().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            crate::test_temp_dir().join(format!("serat-settings-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn an_absent_file_is_defaults_and_not_a_problem() {
        let loaded = Settings::load_from(&temp_dir("absent").join("nothing-here.toml"));
        assert_eq!(loaded.settings, Settings::default());
        assert!(loaded.problem.is_none(), "{:?}", loaded.problem);
    }

    #[test]
    fn a_malformed_file_yields_defaults_and_says_so_rather_than_failing_the_start() {
        // The mutation to worry about is someone making this a `Result` that `main`
        // propagates: a stray character in a TOML file then stops the application from
        // opening at all, which is the opposite of design goal 5.
        let path = temp_dir("malformed").join("settings.toml");
        std::fs::write(&path, "last_vault = [ this is not toml").expect("write");

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.settings, Settings::default());
        let problem = loaded.problem.expect("a malformed file is reported");
        assert!(problem.contains("left untouched"), "{problem}");

        // And the bad file is still there: a parse failure must not delete the user's
        // settings on its way past.
        assert!(path.exists());
    }

    #[test]
    fn a_file_written_by_a_newer_build_still_loads() {
        let path = temp_dir("forward").join("settings.toml");
        std::fs::write(
            &path,
            "last_vault = \"D:/vault\"\nsomething_from_p4 = true\n\n[window]\nwidth = 900.0\nheight = 600.0\n",
        )
        .expect("write");

        let loaded = Settings::load_from(&path);
        assert!(loaded.problem.is_none(), "{:?}", loaded.problem);
        assert_eq!(loaded.settings.last_vault.as_deref(), Some("D:/vault"));
        assert!((loaded.settings.window.width - 900.0).abs() < f64::EPSILON);
    }

    #[test]
    fn round_trips_through_the_file() {
        let path = temp_dir("roundtrip").join("settings.toml");
        let mut settings = Settings::default();
        settings.remember_open(Path::new(r"D:\Projects\vault"), "10-projects/serat.md");
        settings.save_to(&path).expect("save");

        let loaded = Settings::load_from(&path);
        assert!(loaded.problem.is_none(), "{:?}", loaded.problem);
        assert_eq!(loaded.settings, settings);
        assert_eq!(
            loaded
                .settings
                .last_open_for(Path::new("D:/Projects/vault")),
            Some("10-projects/serat.md"),
            "a vault spelled with backslashes and one spelled with slashes are one vault"
        );
    }

    #[test]
    fn two_vaults_keep_two_open_notes() {
        let mut settings = Settings::default();
        settings.remember_open(Path::new("/a"), "one.md");
        settings.remember_open(Path::new("/b"), "two.md");
        assert_eq!(settings.last_open_for(Path::new("/a")), Some("one.md"));
        assert_eq!(settings.last_open_for(Path::new("/b")), Some("two.md"));
    }
}
