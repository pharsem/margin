//! Inbox: things that may need a reply, found by the periodic check (GitHub, Slack, ClickUp).
//! They are only suggestions. They become follow-ups only when the user asks for it.

use super::{Core, Event, Item, Result};
use chrono::{DateTime, Local};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Dismissed keys are kept this long, so the same entry does not come back.
const DISMISSED_RETENTION_MS: i64 = 14 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Github,
    Slack,
    Clickup,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Github => "github",
            Source::Slack => "slack",
            Source::Clickup => "clickup",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "slack" => Source::Slack,
            "clickup" => Source::Clickup,
            _ => Source::Github,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Review,
    Question,
    Task,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Review => "review",
            Kind::Question => "question",
            Kind::Task => "task",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "question" => Kind::Question,
            "task" => Kind::Task,
            _ => Kind::Review,
        }
    }
}

/// One entry as a source reports it.
#[derive(Debug, Clone, Deserialize)]
pub struct Found {
    pub key: String,
    pub kind: Kind,
    pub text: String,
    #[serde(default)]
    pub url: Option<String>,
    /// When the question was asked or the task became due.
    #[serde(default)]
    pub since: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InboxEntry {
    pub key: String,
    pub source: Source,
    pub kind: Kind,
    pub text: String,
    pub url: Option<String>,
    pub since: Option<i64>,
    pub first_seen: i64,
}

impl Core {
    pub fn inbox(&self) -> Result<Vec<InboxEntry>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT * FROM inbox WHERE dismissed_at IS NULL ORDER BY COALESCE(since, first_seen), key",
        )?;
        let entries = stmt.query_map([], entry_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(entries)
    }

    /// Replaces the open entries of one source with a new result. Entries that the source no
    /// longer reports are resolved and go away. Dismissed entries stay dismissed.
    pub fn sync_inbox(&self, source: Source, found: &[Found], now: DateTime<Local>) -> Result<()> {
        let now = now.timestamp_millis();
        {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            let mut keys = HashSet::new();
            for f in found {
                let key = f.key.trim();
                if key.is_empty() || !keys.insert(key.to_string()) {
                    continue;
                }
                let url = f.url.as_deref().filter(|u| u.starts_with("https://") || u.starts_with("http://"));
                tx.execute(
                    "INSERT INTO inbox (key, source, kind, text, url, since, first_seen, last_seen)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                     ON CONFLICT(key) DO UPDATE SET
                        kind = ?3, text = ?4, url = ?5, since = COALESCE(?6, since), last_seen = ?7",
                    params![key, source.as_str(), f.kind.as_str(), f.text.trim(), url, f.since, now],
                )?;
            }
            tx.execute(
                "DELETE FROM inbox WHERE source = ?1 AND dismissed_at IS NULL AND last_seen < ?2",
                params![source.as_str(), now],
            )?;
            tx.execute(
                "DELETE FROM inbox WHERE dismissed_at IS NOT NULL AND last_seen < ?1",
                [now - DISMISSED_RETENTION_MS],
            )?;
            tx.commit()?;
        }
        self.changed();
        Ok(())
    }

    pub fn inbox_entry(&self, key: &str) -> Result<InboxEntry> {
        self.conn()
            .query_row("SELECT * FROM inbox WHERE key = ?1", [key], entry_from_row)
            .optional()?
            .ok_or_else(|| super::Error(format!("inbox entry {key} not found")))
    }

    pub fn dismiss_inbox(&self, key: &str, now: DateTime<Local>) -> Result<()> {
        self.conn()
            .execute("UPDATE inbox SET dismissed_at = ?2 WHERE key = ?1", params![key, now.timestamp_millis()])?;
        self.changed();
        Ok(())
    }

    /// Makes a follow-up from an inbox entry and dismisses the entry. The text may add a time.
    pub fn inbox_to_follow_up(&self, key: &str, text: &str, now: DateTime<Local>) -> Result<Item> {
        let entry = self.inbox_entry(key)?;
        let text = if text.trim().is_empty() { entry.text.as_str() } else { text };
        let item = self.add_with_link(text, entry.url.as_deref(), Some(entry.source.as_str()), now)?;
        self.dismiss_inbox(key, now)?;
        Ok(item)
    }

    /// Questions that waited longer than the threshold, once each.
    pub fn tick_inbox(&self, now: DateTime<Local>, question_toast_ms: i64) -> Result<Vec<InboxEntry>> {
        let now = now.timestamp_millis();
        let due = {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "UPDATE inbox SET notified_at = ?1
                 WHERE dismissed_at IS NULL AND notified_at IS NULL AND kind = 'question'
                   AND COALESCE(since, first_seen) <= ?2
                 RETURNING *",
            )?;
            let due = stmt
                .query_map(params![now, now - question_toast_ms], entry_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            due
        };
        if !due.is_empty() {
            let _ = self.events.send(Event::InboxOverdue(due.clone()));
        }
        Ok(due)
    }
}

fn entry_from_row(row: &Row) -> rusqlite::Result<InboxEntry> {
    Ok(InboxEntry {
        key: row.get("key")?,
        source: Source::parse(&row.get::<_, String>("source")?),
        kind: Kind::parse(&row.get::<_, String>("kind")?),
        text: row.get("text")?,
        url: row.get("url")?,
        since: row.get("since")?,
        first_seen: row.get("first_seen")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 9, 29, 10, 0, 0).unwrap()
    }

    fn found(key: &str, kind: Kind, since: Option<DateTime<Local>>) -> Found {
        Found {
            key: key.into(),
            kind,
            text: format!("text {key}"),
            url: Some(format!("https://example.com/{key}")),
            since: since.map(|t| t.timestamp_millis()),
        }
    }

    #[test]
    fn sync_resolves_missing_entries_per_source() {
        let core = Core::open_in_memory().unwrap();
        let t = now();
        core.sync_inbox(Source::Github, &[found("a", Kind::Review, None), found("b", Kind::Review, None)], t).unwrap();
        core.sync_inbox(Source::Slack, &[found("q", Kind::Question, Some(t))], t).unwrap();
        assert_eq!(core.inbox().unwrap().len(), 3);

        core.sync_inbox(Source::Github, &[found("a", Kind::Review, None)], t + Duration::minutes(30)).unwrap();
        let keys: Vec<_> = core.inbox().unwrap().into_iter().map(|e| e.key).collect();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"q".to_string()), "another source is untouched");
        assert!(!keys.contains(&"b".to_string()));
    }

    #[test]
    fn dismissed_entries_do_not_come_back() {
        let core = Core::open_in_memory().unwrap();
        let t = now();
        core.sync_inbox(Source::Github, &[found("a", Kind::Review, None)], t).unwrap();
        core.dismiss_inbox("a", t).unwrap();
        core.sync_inbox(Source::Github, &[found("a", Kind::Review, None)], t + Duration::minutes(30)).unwrap();
        assert!(core.inbox().unwrap().is_empty());
    }

    #[test]
    fn follow_up_keeps_the_link_and_dismisses() {
        let core = Core::open_in_memory().unwrap();
        let t = now();
        core.sync_inbox(Source::Slack, &[found("q", Kind::Question, Some(t))], t).unwrap();
        let item = core.inbox_to_follow_up("q", "answer Kari 30m", t).unwrap();
        assert_eq!(item.title, "answer Kari");
        assert_eq!(item.url.as_deref(), Some("https://example.com/q"));
        assert_eq!(item.source_app.as_deref(), Some("slack"));
        assert!(core.inbox().unwrap().is_empty());
    }

    #[test]
    fn question_toast_after_threshold_once() {
        let core = Core::open_in_memory().unwrap();
        let t = now();
        let two_hours = Duration::hours(2).num_milliseconds();
        core.sync_inbox(
            Source::Slack,
            &[found("q", Kind::Question, Some(t - Duration::minutes(119))), found("r", Kind::Review, None)],
            t,
        )
        .unwrap();
        assert!(core.tick_inbox(t, two_hours).unwrap().is_empty());
        assert_eq!(core.tick_inbox(t + Duration::minutes(1), two_hours).unwrap().len(), 1);
        assert!(core.tick_inbox(t + Duration::minutes(2), two_hours).unwrap().is_empty());
    }
}
