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
    /// Processes whose windows the capture popup never reads. `*` matches any text.
    pub context_denylist: Vec<String>,
    pub inbox: InboxConfig,
    /// Slack incoming webhook for the "Later" action. Keep it out of version control.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub later_webhook: Option<String>,
    /// Unset means on for installed builds and off for dev builds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autostart: Option<bool>,
}

/// The periodic check of GitHub, Slack and ClickUp.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InboxConfig {
    pub enabled: bool,
    /// Local time, "HH:MM". The check runs from `start` until `end`.
    pub start: String,
    pub end: String,
    /// 1 is Monday, 7 is Sunday.
    pub days: Vec<u8>,
    pub interval_minutes: u32,
    pub model: String,
    /// A Slack question with no reply for this long gives a toast.
    pub question_toast_hours: f64,
    /// Path to claude.exe. If unset, Margin searches PATH.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_path: Option<String>,
}

impl Default for InboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            start: "07:00".into(),
            end: "17:00".into(),
            days: vec![1, 2, 3, 4, 5],
            interval_minutes: 30,
            model: "sonnet".into(),
            question_toast_hours: 2.0,
            claude_path: None,
        }
    }
}

impl InboxConfig {
    pub fn in_work_hours(&self, now: chrono::DateTime<chrono::Local>) -> bool {
        use chrono::{Datelike, NaiveTime};
        let day = now.weekday().number_from_monday() as u8;
        let parse = |s: &str| NaiveTime::parse_from_str(s, "%H:%M").ok();
        match (parse(&self.start), parse(&self.end)) {
            (Some(start), Some(end)) => self.days.contains(&day) && now.time() >= start && now.time() < end,
            _ => false,
        }
    }
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
            context_denylist: ["1Password.exe", "KeePass*.exe", "Bitwarden.exe"].map(String::from).to_vec(),
            inbox: InboxConfig::default(),
            later_webhook: None,
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
