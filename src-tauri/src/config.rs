use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Left,
    Right,
}

/// A monitor index (0 = leftmost), `"primary"`, or a device name such as `\\.\DISPLAY2`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MonitorSelector {
    Index(usize),
    Name(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub monitor: MonitorSelector,
    pub edge: Edge,
    /// Logical pixels (100% scaling). Scaled by the target monitor's DPI.
    pub width: u32,
    pub hotkey: String,
    pub focus_hotkey: String,
    /// Localhost port for the Claude Code hooks.
    pub port: u16,
    /// Unset means on for installed builds and off for dev builds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autostart: Option<bool>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            monitor: MonitorSelector::Name("primary".into()),
            edge: Edge::Right,
            width: 320,
            hotkey: "Ctrl+Alt+Space".into(),
            focus_hotkey: "Ctrl+Alt+N".into(),
            port: 47811,
            autostart: None,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // PowerShell 5 and some editors write a UTF-8 BOM, which serde_json rejects.
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Returns the config and an error message if the file is invalid.
    pub fn load_or_create(path: &Path) -> (Self, Option<String>) {
        if path.exists() {
            return match Self::load(path) {
                Ok(config) => (config, None),
                Err(e) => (Self::default(), Some(format!("Invalid config, using defaults. {e}"))),
            };
        }
        let config = Self::default();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(&config) {
            let _ = std::fs::write(path, text);
        }
        (config, None)
    }

    pub fn autostart(&self) -> bool {
        self.autostart.unwrap_or(!cfg!(debug_assertions))
    }
}
