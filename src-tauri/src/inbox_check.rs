//! The periodic inbox check. GitHub review requests come from `gh` with no model call.
//! Slack questions and ClickUp tasks come from a headless `claude -p` run with read-only tools.

use crate::config::InboxConfig;
use crate::state::inbox::{Found, Kind, Source};
use chrono::{DateTime, Local};
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const GH_TIMEOUT: Duration = Duration::from_secs(30);
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(180);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const ALLOWED_TOOLS: &[&str] = &[
    "ToolSearch",
    "mcp__claude_ai_Slack__slack_read_user_profile",
    "mcp__claude_ai_Slack__slack_search_public_and_private",
    "mcp__claude_ai_Slack__slack_read_thread",
    "mcp__claude_ai_Slack__slack_read_channel",
    "mcp__claude_ai_Slack__slack_search_users",
    "mcp__claude_ai_ClickUp__clickup_filter_tasks",
    "mcp__claude_ai_ClickUp__clickup_get_task",
    "mcp__claude_ai_ClickUp__clickup_search",
    "mcp__claude_ai_ClickUp__clickup_get_workspace_members",
    "mcp__claude_ai_ClickUp__clickup_find_member_by_name",
    "mcp__claude_ai_ClickUp__clickup_resolve_assignees",
];

const SCHEMA: &str = r##"{
  "type": "object",
  "properties": {
    "slack_ok": { "type": "boolean", "description": "true if the Slack check ran to the end" },
    "clickup_ok": { "type": "boolean", "description": "true if the ClickUp check ran to the end" },
    "slack": { "type": "array", "items": { "$ref": "#/$defs/entry" } },
    "clickup": { "type": "array", "items": { "$ref": "#/$defs/entry" } }
  },
  "required": ["slack_ok", "clickup_ok", "slack", "clickup"],
  "$defs": {
    "entry": {
      "type": "object",
      "properties": {
        "key": { "type": "string", "description": "Slack permalink or ClickUp task URL" },
        "text": { "type": "string", "description": "At most 80 characters" },
        "url": { "type": "string" },
        "since": { "type": "string", "description": "ISO 8601 time the question was asked or the task was due" }
      },
      "required": ["key", "text"]
    }
  }
}"##;

fn prompt(now: DateTime<Local>) -> String {
    format!(
        "You check Slack and ClickUp for the current user. The local time is {now}. \
Only read. Never send, edit or react to anything.

1. Slack. Find the user with slack_read_user_profile. Then search for messages from the last 2 working days \
that ask the user a direct question: direct messages to the user, and messages that mention the user. \
Keep a message only if the user did not reply after it in the same direct message or thread. \
Check the thread with slack_read_thread when you are not sure. Skip bot messages, broadcasts to a whole \
channel (@here, @channel) and questions that someone else already answered. \
For each message: key and url = the message permalink, since = the message time, \
text = \"<sender first name>: <the question in a few words>\", for example \"Kari: can you review the migration?\". Do not add words such as \"mentions you\" or \"asks\".

2. ClickUp. Find tasks assigned to the user that are due today or overdue and are not closed or done. \
For each task: key and url = the task URL, since = the due date, text = the task name.

Set slack_ok or clickup_ok to false if that part failed, for example if a tool gave an error. \
Return empty lists when there is nothing. Do not guess: include only entries that you checked.",
        now = now.format("%A %Y-%m-%d %H:%M %:z")
    )
}

pub struct CheckResult {
    /// One result for each source. A failed source keeps its old entries.
    pub sources: Vec<(Source, Result<Vec<Found>, String>)>,
}

pub fn run(config: &InboxConfig, now: DateTime<Local>) -> CheckResult {
    let github = std::thread::spawn(github);
    let (slack, clickup) = match claude(config, now) {
        Ok(out) => (out.0, out.1),
        Err(e) => (Err(e.clone()), Err(e)),
    };
    let github = github.join().unwrap_or_else(|_| Err("gh check panicked".into()));
    CheckResult { sources: vec![(Source::Github, github), (Source::Slack, slack), (Source::Clickup, clickup)] }
}

fn github() -> Result<Vec<Found>, String> {
    #[derive(Deserialize)]
    struct Repo {
        name: String,
    }
    #[derive(Deserialize)]
    struct Pr {
        number: u64,
        title: String,
        url: String,
        repository: Repo,
    }
    let args = [
        "search", "prs", "--review-requested=@me", "--state=open", "--json", "number,title,url,repository", "--limit", "30",
    ];
    let out = run_process(PathBuf::from("gh"), &args, None, GH_TIMEOUT)?;
    let prs: Vec<Pr> = serde_json::from_str(&out).map_err(|e| format!("gh output: {e}"))?;
    Ok(prs
        .into_iter()
        .map(|p| Found {
            key: p.url.clone(),
            kind: Kind::Review,
            text: format!("PR #{} {}: {}", p.number, p.repository.name, p.title),
            url: Some(p.url),
            since: None,
        })
        .collect())
}

type SourceResult = Result<Vec<Found>, String>;

fn claude(config: &InboxConfig, now: DateTime<Local>) -> Result<(SourceResult, SourceResult), String> {
    #[derive(Deserialize)]
    struct Entry {
        key: String,
        text: String,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        since: Option<String>,
    }
    #[derive(Deserialize)]
    struct Output {
        slack_ok: bool,
        clickup_ok: bool,
        slack: Vec<Entry>,
        clickup: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        result: Option<String>,
        structured_output: Option<Output>,
        #[serde(default)]
        total_cost_usd: f64,
        #[serde(default)]
        duration_ms: u64,
    }

    let exe = claude_path(config).ok_or("claude.exe not found. Set inbox.claude_path in Settings.")?;
    let tools = ALLOWED_TOOLS.join(",");
    let args = [
        "-p",
        "--model",
        config.model.as_str(),
        "--output-format",
        "json",
        "--no-session-persistence",
        "--json-schema",
        SCHEMA,
        "--allowedTools",
        tools.as_str(),
    ];
    let out = run_process(exe, &args, Some(prompt(now)), CLAUDE_TIMEOUT)?;
    let envelope: Envelope = serde_json::from_str(&out).map_err(|e| format!("claude output: {e}"))?;
    eprintln!("[inbox] claude run: {} s, ${:.2}", envelope.duration_ms / 1000, envelope.total_cost_usd);
    let output = match envelope.structured_output {
        Some(o) if !envelope.is_error => o,
        _ => return Err(format!("claude run failed: {}", envelope.result.unwrap_or_default())),
    };
    let convert = |entries: Vec<Entry>, kind: Kind| -> Vec<Found> {
        entries
            .into_iter()
            .map(|e| Found {
                key: e.key,
                kind,
                text: e.text,
                url: e.url,
                since: e.since.as_deref().and_then(parse_time),
            })
            .collect()
    };
    let slack = if output.slack_ok { Ok(convert(output.slack, Kind::Question)) } else { Err("Slack check failed".into()) };
    let clickup =
        if output.clickup_ok { Ok(convert(output.clickup, Kind::Task)) } else { Err("ClickUp check failed".into()) };
    Ok((slack, clickup))
}

fn parse_time(s: &str) -> Option<i64> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.timestamp_millis());
    }
    // A due date with no time.
    let date = chrono::NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()?;
    date.and_hms_opt(0, 0, 0)?.and_local_timezone(Local).earliest().map(|t| t.timestamp_millis())
}

/// The npm package ships claude.exe next to a .cmd shim, and Command cannot start a .cmd.
fn claude_path(config: &InboxConfig) -> Option<PathBuf> {
    if let Some(path) = config.claude_path.as_deref().filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        [dir.join("claude.exe"), dir.join("node_modules/@anthropic-ai/claude-code/bin/claude.exe")]
            .into_iter()
            .find(|p| p.is_file())
    })
}

/// Posts one line to a Slack incoming webhook.
pub fn post_to_webhook(webhook: &str, text: &str) -> Result<(), String> {
    if !webhook.starts_with("https://") {
        return Err("\"later_webhook\" must be an https URL.".into());
    }
    let body = serde_json::json!({ "text": text }).to_string();
    let args = ["-s", "-f", "-m", "10", "-X", "POST", "-H", "Content-Type: application/json", "--data-binary", "@-", webhook];
    run_process(PathBuf::from("curl"), &args, Some(body), Duration::from_secs(15)).map(|_| ())
}

/// Runs a console program with no window, and kills it after the timeout.
fn run_process(exe: PathBuf, args: &[&str], stdin: Option<String>, timeout: Duration) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    let name = exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut child = Command::new(&exe)
        .args(args)
        // Keeps the run out of the sessions lane.
        .env("MARGIN_IGNORE", "1")
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("{name}: {e}"))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = pipe.write_all(text.as_bytes());
    }
    // Read in threads, so a full pipe cannot block the child.
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stderr = child.stderr.take().ok_or("no stderr")?;
    let out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                return Err(format!("{name} did not finish in {} s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => return Err(format!("{name}: {e}")),
        }
    };
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    if !status.success() {
        let detail = err.lines().chain(out.lines()).find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        return Err(format!("{name} failed ({status}): {detail}"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_from_the_model() {
        assert!(parse_time("2026-09-29T08:15:00Z").is_some());
        assert!(parse_time("2026-09-29T10:15:00+02:00").is_some());
        assert!(parse_time("2026-09-29").is_some());
        assert_eq!(parse_time("yesterday"), None);
    }

    #[test]
    fn schema_is_valid_json() {
        let _: serde_json::Value = serde_json::from_str(SCHEMA).unwrap();
    }
}
