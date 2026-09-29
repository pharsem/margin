//! Adds Margin's hooks to the user-level Claude Code settings, and removes them again.

use crate::server::HOOK_PATH;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// Event name, and whether the event needs a matcher.
const EVENTS: &[(&str, bool)] = &[
    ("SessionStart", false),
    ("UserPromptSubmit", false),
    ("PostToolUse", true),
    ("Notification", true),
    ("Stop", false),
    ("SessionEnd", false),
];

pub fn settings_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("USERPROFILE").ok_or("USERPROFILE is not set")?;
    Ok(PathBuf::from(home).join(".claude").join("settings.json"))
}

fn hook_command(port: u16) -> String {
    // async keeps Claude from waiting. The short connect timeout matters because Windows
    // retries a refused localhost connection. stdout must stay empty: for some events
    // Claude Code adds it to the model context.
    format!(
        "[ -n \"$MARGIN_IGNORE\" ] || curl -s -o /dev/null --connect-timeout 0.2 --max-time 1 \
         -X POST http://127.0.0.1:{port}{HOOK_PATH} -H \"Content-Type: application/json\" \
         --data-binary @- || true"
    )
}

fn is_ours(hook: &Value) -> bool {
    hook.get("command").and_then(Value::as_str).is_some_and(|c| c.contains(HOOK_PATH))
}

/// Returns the path of the backup file.
pub fn install(port: u16) -> Result<PathBuf, String> {
    install_at(&settings_path()?, port)
}

pub fn uninstall() -> Result<PathBuf, String> {
    update(&settings_path()?, remove_ours)
}

fn install_at(path: &Path, port: u16) -> Result<PathBuf, String> {
    update(path, |hooks| {
        remove_ours(hooks);
        for (event, needs_matcher) in EVENTS {
            let mut group = json!({
                "hooks": [{ "type": "command", "command": hook_command(port), "async": true, "timeout": 5 }]
            });
            if *needs_matcher {
                group["matcher"] = json!("*");
            }
            let list = hooks.entry(*event).or_insert_with(|| json!([]));
            if let Some(list) = list.as_array_mut() {
                list.push(group);
            }
        }
    })
}

fn remove_ours(hooks: &mut Map<String, Value>) {
    for list in hooks.values_mut() {
        let Some(groups) = list.as_array_mut() else { continue };
        for group in groups.iter_mut() {
            if let Some(inner) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                inner.retain(|h| !is_ours(h));
            }
        }
        groups.retain(|g| g.get("hooks").and_then(Value::as_array).is_none_or(|h| !h.is_empty()));
    }
    hooks.retain(|_, list| list.as_array().is_none_or(|l| !l.is_empty()));
}

fn update(path: &Path, change: impl FnOnce(&mut Map<String, Value>)) -> Result<PathBuf, String> {
    let mut settings = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("{} is not valid JSON, so Margin did not change it: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let root = settings.as_object_mut().ok_or("settings.json does not hold a JSON object")?;
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("\"hooks\" in settings.json is not an object")?;
    change(hooks);
    if hooks.is_empty() {
        root.remove("hooks");
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup = path.with_file_name(format!("settings.json.margin-backup-{stamp}"));
    if path.exists() {
        std::fs::copy(path, &backup).map_err(|e| format!("Could not back up settings.json: {e}"))?;
    }
    let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())? + "\n";
    let tmp = path.with_extension("json.margin-tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_is_idempotent_and_uninstall_restores() {
        let dir = std::env::temp_dir().join(format!("margin-hooks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let original = r#"{
            "model": "x",
            "hooks": {
                "Stop": [{ "hooks": [{ "type": "command", "command": "node a.js" }] }],
                "PostToolUse": [{ "matcher": "*", "hooks": [{ "type": "command", "command": "node b.js" }] }]
            },
            "tui": "fullscreen"
        }"#;
        std::fs::write(&path, original).unwrap();
        let before: Value = serde_json::from_str(original.trim_start_matches('\u{feff}')).unwrap();

        install_at(&path, 47811).unwrap();
        install_at(&path, 47811).unwrap();
        let installed: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for (event, _) in EVENTS {
            let ours = installed["hooks"][event]
                .as_array()
                .unwrap()
                .iter()
                .filter(|g| g["hooks"].as_array().unwrap().iter().any(is_ours))
                .count();
            assert_eq!(ours, 1, "{event}");
        }

        update(&path, remove_ours).unwrap();
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after, before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_keeps_other_hooks() {
        let mut settings = json!({
            "PostToolUse": [
                { "matcher": "*", "hooks": [{ "type": "command", "command": "node other.js" }] },
                { "matcher": "*", "hooks": [{ "type": "command", "command": hook_command(1) }] }
            ],
            "SessionEnd": [{ "hooks": [{ "type": "command", "command": hook_command(1) }] }]
        });
        remove_ours(settings.as_object_mut().unwrap());
        assert_eq!(
            settings,
            json!({ "PostToolUse": [{ "matcher": "*", "hooks": [{ "type": "command", "command": "node other.js" }] }] })
        );
    }
}
