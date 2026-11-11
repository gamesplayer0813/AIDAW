//! Tiny per-user settings file (%APPDATA%\AIDAW\settings.json on Windows).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::i18n::Lang;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    pub show_welcome: bool,
    pub tutorial_done: bool,
    /// Pro mode: every control. Off = Easy mode, the default for new users.
    pub pro: bool,
    /// Interface language; `None` follows the system language.
    pub lang: Option<Lang>,
    /// Sound driver ("WASAPI", "ASIO"); `None` = the system's default.
    pub driver: Option<String>,
    /// Speakers / headphones by name; `None` = the system default.
    pub output: Option<String>,
    /// Microphone / line input for recording; `None` = the system default.
    pub input: Option<String>,
    /// Click on every beat while playing.
    pub metronome: bool,
    /// One bar of clicks before recording starts.
    pub count_in: bool,
    /// Hear the input through the speakers while a track is armed.
    pub monitor: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            show_welcome: true,
            tutorial_done: false,
            pro: false,
            lang: None,
            driver: None,
            output: None,
            input: None,
            metronome: false,
            count_in: true,
            monitor: false,
        }
    }
}

/// AIDAW's own folder for settings and the log: `%APPDATA%\AIDAW` on
/// Windows, `~/.config/AIDAW` elsewhere.
pub fn dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("AIDAW"))
}

fn path() -> Option<PathBuf> {
    Some(dir()?.join("settings.json"))
}

impl Settings {
    pub fn load() -> Settings {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, s);
        }
    }
}
