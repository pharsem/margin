//! Claude Code sessions, driven by hook events.

use super::{Core, Event, Result};
use chrono::{DateTime, Local};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// A session is removed when its transcript has not changed for this long.
const STALE_MS: i64 = 12 * 60 * 60 * 1000;
/// Stop fires after every reply, so wait before a toast in case the user is still at the terminal.
const DONE_TOAST_DELAY_MS: i64 = 30 * 1000;
const LABEL_CHARS: usize = 60;
const TRANSCRIPT_TAIL_BYTES: u64 = 64 * 1024;
/// A turn that the transcript shows as ended longer ago than this gets no toast. Margin was
/// probably not running, and the lane shows the state anyway.
const LATE_TOAST_MS: i64 = 5 * 60 * 1000;
/// System entries that Claude Code writes when a turn is over.
const TURN_END_SUBTYPES: &[&str] = &["stop_hook_summary", "turn_duration", "away_summary", "local_command"];

const NEEDS_INPUT_TYPES: &[&str] = &[
    "permission_prompt",
    "elicitation_dialog",
    "elicitation_url_dialog",
    "agent_needs_input",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Started, but no prompt yet. Not shown in the lane.
    New,
    Running,
    NeedsInput,
    /// Claude finished and the user has not looked yet.
    Done,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::New => "new",
            Status::Running => "running",
            Status::NeedsInput => "needs_input",
            Status::Done => "done",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "running" => Status::Running,
            "needs_input" => Status::NeedsInput,
            "done" => Status::Done,
            _ => Status::New,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    pub cwd: String,
    pub folder: String,
    pub prompt: Option<String>,
    /// The session title that Claude Code writes to the transcript and the terminal tab.
    pub title: Option<String>,
    /// How the session was started, for example "cli".
    pub entrypoint: Option<String>,
    pub status: Status,
    pub started_at: i64,
    pub updated_at: i64,
    /// When the session started to wait for the user (Stop or a needs-input notification).
    pub waiting_since: Option<i64>,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HookEvent {
    pub session_id: String,
    pub hook_event_name: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub notification_type: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

impl Core {
    pub fn sessions(&self) -> Result<Vec<Session>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM sessions WHERE status != 'new'")?;
        let mut sessions = stmt.query_map([], session_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        // Needs input first, then the longest wait, then running sessions.
        sessions.sort_by_key(|s| match s.status {
            Status::NeedsInput => (0, s.waiting_since.unwrap_or(0)),
            Status::Done => (1, s.waiting_since.unwrap_or(0)),
            _ => (2, -s.updated_at),
        });
        Ok(sessions)
    }

    pub fn hook(&self, event: &HookEvent, now: DateTime<Local>) -> Result<()> {
        let now = now.timestamp_millis();
        let id = event.session_id.as_str();
        if id.is_empty() {
            return Ok(());
        }
        if event.hook_event_name == "SessionEnd" {
            return self.review(id);
        }

        let current = self.session_status(id)?;
        let next = match (event.hook_event_name.as_str(), current) {
            ("SessionStart", None) => Some(Status::New),
            ("SessionStart", Some(_)) => None,
            ("UserPromptSubmit", _) => Some(Status::Running),
            ("PostToolUse", Some(Status::NeedsInput)) => Some(Status::Running),
            ("PostToolUse", None) => Some(Status::Running),
            ("Notification", _) => event
                .notification_type
                .as_deref()
                .filter(|t| NEEDS_INPUT_TYPES.contains(t))
                .map(|_| Status::NeedsInput),
            ("Stop", _) => Some(Status::Done),
            _ => None,
        };

        {
            let conn = self.conn();
            conn.execute(
                "INSERT INTO sessions (id, cwd, transcript_path, status, started_at, updated_at)
                 VALUES (?1, COALESCE(?2, ''), ?3, 'new', ?4, ?4)
                 ON CONFLICT(id) DO UPDATE SET
                    updated_at = ?4,
                    cwd = COALESCE(?2, cwd),
                    transcript_path = COALESCE(?3, transcript_path)",
                params![id, event.cwd, event.transcript_path, now],
            )?;
            if let Some(status) = next {
                let waiting = matches!(status, Status::NeedsInput | Status::Done);
                // A second needs-input notification keeps the original wait start.
                let keep_wait = current == Some(status) && waiting;
                conn.execute(
                    "UPDATE sessions SET
                        status = ?2,
                        prompt = COALESCE(?3, prompt),
                        message = ?4,
                        waiting_since = CASE WHEN ?5 THEN waiting_since WHEN ?6 THEN ?7 ELSE NULL END,
                        notified_at = CASE WHEN ?5 THEN notified_at ELSE NULL END
                     WHERE id = ?1",
                    params![
                        id,
                        status.as_str(),
                        event.prompt.as_deref().map(|p| p.trim()),
                        event.message.as_deref().filter(|_| status == Status::NeedsInput),
                        keep_wait,
                        waiting,
                        now
                    ],
                )?;
            }
        }
        self.changed();
        Ok(())
    }

    pub fn session(&self, id: &str) -> Result<Session> {
        self.conn()
            .query_row("SELECT * FROM sessions WHERE id = ?1", [id], session_from_row)
            .optional()?
            .ok_or_else(|| super::Error(format!("session {id} not found")))
    }

    pub fn review(&self, id: &str) -> Result<()> {
        if self.conn().execute("DELETE FROM sessions WHERE id = ?1", [id])? > 0 {
            self.changed();
        }
        Ok(())
    }

    fn session_status(&self, id: &str) -> Result<Option<Status>> {
        Ok(self
            .conn()
            .query_row("SELECT status FROM sessions WHERE id = ?1", [id], |r| r.get::<_, String>(0))
            .optional()?
            .map(|s| Status::parse(&s)))
    }

    /// Returns sessions that should get a toast now and marks them as notified.
    pub fn tick_sessions(&self, now: DateTime<Local>) -> Result<Vec<Session>> {
        let now = now.timestamp_millis();
        let due = {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "UPDATE sessions SET notified_at = ?1
                 WHERE notified_at IS NULL AND (
                    status = 'needs_input' OR (status = 'done' AND waiting_since <= ?2))
                 RETURNING *",
            )?;
            let due = stmt
                .query_map(params![now, now - DONE_TOAST_DELAY_MS], session_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            due
        };
        if !due.is_empty() {
            let _ = self.events.send(Event::SessionsWaiting(due.clone()));
        }
        Ok(due)
    }

    /// Corrects the state from the transcript when a hook event was lost (Margin was not
    /// running) or never fires (Esc does not fire Stop). Also removes sessions whose terminal
    /// went away.
    pub fn check_transcripts(&self, now: DateTime<Local>) -> Result<()> {
        let now = now.timestamp_millis();
        type Row = (String, String, Option<String>, Option<i64>, i64, Option<String>, Option<i64>);
        let rows: Vec<Row> = {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "SELECT id, status, transcript_path, transcript_mtime, updated_at, title, waiting_since FROM sessions",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };

        let mut changed = false;
        for (id, status, path, seen_mtime, updated_at, title, waiting_since) in rows {
            let mtime = path.as_deref().and_then(file_mtime_ms);
            let last_activity = mtime.unwrap_or(0).max(updated_at);
            if now - last_activity > STALE_MS {
                self.conn().execute("DELETE FROM sessions WHERE id = ?1", [&id])?;
                changed = true;
                continue;
            }
            let Some(mtime) = mtime else { continue };
            if seen_mtime == Some(mtime) {
                continue;
            }
            let tail = path.as_deref().map(|p| read_tail(Path::new(p))).unwrap_or_default();
            if tail.title.is_some() && tail.title != title {
                changed = true;
            }
            self.conn().execute(
                "UPDATE sessions SET transcript_mtime = ?2,
                    title = COALESCE(?3, title), entrypoint = COALESCE(?4, entrypoint)
                 WHERE id = ?1",
                params![id, mtime, tail.title, tail.entrypoint],
            )?;
            match (Status::parse(&status), tail.last) {
                (Status::Running, Last::TurnEnded(at)) => {
                    let at = at.unwrap_or(now).min(now);
                    let notified = (now - at > LATE_TOAST_MS).then_some(now);
                    self.conn().execute(
                        "UPDATE sessions SET status = 'done', waiting_since = ?2, notified_at = ?3 WHERE id = ?1",
                        params![id, at, notified],
                    )?;
                    changed = true;
                }
                (Status::Done, Last::Active(Some(at))) if waiting_since.is_some_and(|w| at > w) => {
                    self.conn().execute(
                        "UPDATE sessions SET status = 'running', waiting_since = NULL, notified_at = NULL WHERE id = ?1",
                        [&id],
                    )?;
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            self.changed();
        }
        Ok(())
    }
}

fn session_from_row(row: &Row) -> rusqlite::Result<Session> {
    let cwd: String = row.get("cwd")?;
    let prompt: Option<String> = row.get("prompt")?;
    Ok(Session {
        id: row.get("id")?,
        folder: folder_name(&cwd),
        cwd,
        prompt: prompt.map(|p| truncate(&p, LABEL_CHARS)),
        title: row.get("title")?,
        entrypoint: row.get("entrypoint")?,
        status: Status::parse(&row.get::<_, String>("status")?),
        started_at: row.get("started_at")?,
        updated_at: row.get("updated_at")?,
        waiting_since: row.get("waiting_since")?,
        message: row.get("message")?,
    })
}

fn folder_name(cwd: &str) -> String {
    cwd.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(cwd)
        .to_string()
}

fn truncate(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= max && !text.trim().contains('\n') {
        return line.to_string();
    }
    let cut: String = line.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

fn file_mtime_ms(path: &str) -> Option<i64> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let ms = modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
    i64::try_from(ms).ok()
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Last {
    #[default]
    Unknown,
    /// Claude is working or the user sent a prompt. Holds the entry time.
    Active(Option<i64>),
    /// The turn is over: an end-of-turn entry or an Esc interrupt.
    TurnEnded(Option<i64>),
}

#[derive(Debug, Default)]
struct TranscriptTail {
    last: Last,
    title: Option<String>,
    entrypoint: Option<String>,
}

fn read_tail(path: &Path) -> TranscriptTail {
    let mut result = TranscriptTail::default();
    let Ok(mut file) = std::fs::File::open(path) else { return result };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(TRANSCRIPT_TAIL_BYTES)));
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return result;
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut custom_title = None;
    let mut ai_title = None;
    for line in text.lines().rev() {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let kind = entry.get("type").and_then(|t| t.as_str());
        if result.last == Last::Unknown {
            let at = entry["timestamp"]
                .as_str()
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.timestamp_millis());
            let subtype = entry["subtype"].as_str().unwrap_or("");
            result.last = match kind {
                Some("user") if is_interrupt_message(&entry) => Last::TurnEnded(at),
                Some("user") | Some("assistant") => Last::Active(at),
                Some("system") if subtype == "stop_hook_summary" && entry["preventedContinuation"] == true => {
                    Last::Active(at)
                }
                Some("system") if TURN_END_SUBTYPES.contains(&subtype) => Last::TurnEnded(at),
                _ => Last::Unknown,
            };
        }
        match kind {
            // A title set with /rename wins over the generated one.
            Some("custom-title") if custom_title.is_none() => {
                custom_title = entry["customTitle"].as_str().map(str::to_string);
            }
            Some("ai-title") if ai_title.is_none() => {
                ai_title = entry["aiTitle"].as_str().map(str::to_string);
            }
            _ => {}
        }
        if result.entrypoint.is_none() {
            result.entrypoint = entry["entrypoint"].as_str().map(str::to_string);
        }
    }
    result.title = custom_title.or(ai_title).filter(|t| !t.trim().is_empty());
    result
}

fn is_interrupt_message(entry: &serde_json::Value) -> bool {
    let content = &entry["message"]["content"];
    let texts: Vec<&str> = match content {
        serde_json::Value::String(s) => vec![s.as_str()],
        serde_json::Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect(),
        _ => Vec::new(),
    };
    texts.iter().any(|t| t.starts_with("[Request interrupted by user"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 9, 29, 10, 0, 0).unwrap()
    }

    fn event(name: &str) -> HookEvent {
        HookEvent {
            session_id: "s1".into(),
            hook_event_name: name.into(),
            cwd: Some("C:\\Users\\me\\dev\\margin".into()),
            transcript_path: None,
            prompt: None,
            notification_type: None,
            message: None,
        }
    }

    fn status(core: &Core) -> Option<Status> {
        core.session_status("s1").unwrap()
    }

    #[test]
    fn lifecycle() {
        let core = Core::open_in_memory().unwrap();
        let t = now();
        core.hook(&event("SessionStart"), t).unwrap();
        assert_eq!(status(&core), Some(Status::New));
        assert!(core.sessions().unwrap().is_empty(), "new sessions are hidden");

        let mut prompt = event("UserPromptSubmit");
        prompt.prompt = Some("fix the flaky test in the payment service please, it fails on CI".into());
        core.hook(&prompt, t).unwrap();
        let s = &core.sessions().unwrap()[0];
        assert_eq!(s.status, Status::Running);
        assert_eq!(s.folder, "margin");
        assert!(s.prompt.as_ref().unwrap().ends_with('…'));

        let mut notification = event("Notification");
        notification.notification_type = Some("permission_prompt".into());
        core.hook(&notification, t + Duration::seconds(10)).unwrap();
        assert_eq!(status(&core), Some(Status::NeedsInput));

        core.hook(&event("PostToolUse"), t + Duration::seconds(20)).unwrap();
        assert_eq!(status(&core), Some(Status::Running));

        core.hook(&event("Stop"), t + Duration::seconds(30)).unwrap();
        assert_eq!(status(&core), Some(Status::Done));

        let mut idle = event("Notification");
        idle.notification_type = Some("idle_prompt".into());
        core.hook(&idle, t + Duration::seconds(90)).unwrap();
        assert_eq!(status(&core), Some(Status::Done), "idle_prompt does not mean needs input");

        core.hook(&event("SessionEnd"), t + Duration::seconds(100)).unwrap();
        assert_eq!(status(&core), None);
    }

    #[test]
    fn toasts_wait_for_done_but_not_for_needs_input() {
        let core = Core::open_in_memory().unwrap();
        let t = now();
        core.hook(&event("Stop"), t).unwrap();
        assert!(core.tick_sessions(t + Duration::seconds(29)).unwrap().is_empty());
        assert_eq!(core.tick_sessions(t + Duration::seconds(30)).unwrap().len(), 1);
        assert!(core.tick_sessions(t + Duration::seconds(31)).unwrap().is_empty());

        let mut notification = event("Notification");
        notification.notification_type = Some("permission_prompt".into());
        core.hook(&notification, t + Duration::seconds(40)).unwrap();
        assert_eq!(core.tick_sessions(t + Duration::seconds(40)).unwrap().len(), 1);
    }

    #[test]
    fn interrupt_marker_in_transcript() {
        let dir = std::env::temp_dir().join(format!("margin-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"ai-title","aiTitle":"Old title"}"#,
                "\n",
                r#"{"type":"ai-title","aiTitle":"Fix the login"}"#,
                "\n",
                r#"{"type":"assistant","entrypoint":"cli","message":{"content":[{"type":"text","text":"working"}]}}"#,
                "\n",
                r#"{"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#,
                "\n",
                r#"{"type":"system","subtype":"x"}"#,
                "\n"
            ),
        )
        .unwrap();
        let tail = read_tail(&path);
        assert!(matches!(tail.last, Last::TurnEnded(_)));
        assert_eq!(tail.title.as_deref(), Some("Fix the login"));
        assert_eq!(tail.entrypoint.as_deref(), Some("cli"));

        let core = Core::open_in_memory().unwrap();
        let mut prompt = event("UserPromptSubmit");
        prompt.transcript_path = Some(path.to_string_lossy().into());
        core.hook(&prompt, Local::now()).unwrap();
        core.check_transcripts(Local::now()).unwrap();
        assert_eq!(status(&core), Some(Status::Done));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lost_stop_event_is_recovered_from_the_transcript() {
        let dir = std::env::temp_dir().join(format!("margin-lost-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let ended = Local::now() - Duration::minutes(40);
        let assistant = r#"{"type":"assistant","timestamp":"2026-09-29T08:29:45.000Z","message":{"content":[]}}"#;
        let summary = format!(
            r#"{{"type":"system","subtype":"stop_hook_summary","preventedContinuation":false,"timestamp":"{}"}}"#,
            ended.to_rfc3339()
        );
        std::fs::write(&path, format!("{assistant}\n{summary}\n")).unwrap();

        let core = Core::open_in_memory().unwrap();
        let mut prompt = event("UserPromptSubmit");
        prompt.transcript_path = Some(path.to_string_lossy().into());
        core.hook(&prompt, ended - Duration::minutes(1)).unwrap();
        core.check_transcripts(Local::now()).unwrap();
        let s = &core.sessions().unwrap()[0];
        assert_eq!(s.status, Status::Done);
        assert_eq!(s.waiting_since, Some(ended.timestamp_millis()));
        assert!(core.tick_sessions(Local::now()).unwrap().is_empty(), "no toast for an old turn end");

        // New activity after the turn end, with the prompt hook lost as well.
        let later = Local::now() + Duration::seconds(5);
        std::fs::write(
            &path,
            format!(r#"{{"type":"user","timestamp":"{}","message":{{"content":"next"}}}}"#, later.to_rfc3339()) + "\n",
        )
        .unwrap();
        let bumped = std::fs::File::options().append(true).open(&path).unwrap();
        bumped.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2)).unwrap();
        core.check_transcripts(Local::now()).unwrap();
        assert_eq!(status(&core), Some(Status::Running));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_sessions_are_removed() {
        let core = Core::open_in_memory().unwrap();
        let t = now();
        core.hook(&event("Stop"), t).unwrap();
        core.check_transcripts(t + Duration::hours(11)).unwrap();
        assert!(status(&core).is_some());
        core.check_transcripts(t + Duration::hours(13)).unwrap();
        assert!(status(&core).is_none());
    }

    #[test]
    fn folder_names() {
        assert_eq!(folder_name("C:\\Users\\me\\dev\\margin"), "margin");
        assert_eq!(folder_name("/home/me/proj/"), "proj");
        assert_eq!(folder_name("C:\\"), "C:");
    }
}
