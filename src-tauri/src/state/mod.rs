//! Business logic and storage. Knows nothing about Tauri, so the HTTP API can share it.

pub mod context;
pub mod inbox;
pub mod parse;
pub mod sessions;

use chrono::{DateTime, Local, TimeZone};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

pub use parse::Parsed;
pub use sessions::{HookEvent, Session};

const DONE_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub id: i64,
    pub title: String,
    pub created_at: i64,
    pub due_at: Option<i64>,
    pub notified_at: Option<i64>,
    pub done_at: Option<i64>,
    pub url: Option<String>,
    /// Process name of the window the item was captured from.
    pub source_app: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Items {
    pub open: Vec<Item>,
    pub done_today: Vec<Item>,
}

#[derive(Debug, Clone)]
pub enum Event {
    Changed,
    /// Items that became due since the last tick. Sent once per item.
    Due(Vec<Item>),
    /// Sessions that started to wait for the user. Sent once per wait.
    SessionsWaiting(Vec<Session>),
    /// Inbox questions that passed the reply threshold. Sent once per entry.
    InboxOverdue(Vec<inbox::InboxEntry>),
}

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self(e.to_string())
    }
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Core {
    conn: Mutex<Connection>,
    events: broadcast::Sender<Event>,
}

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE items (
        id INTEGER PRIMARY KEY,
        title TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        due_at INTEGER,
        notified_at INTEGER,
        done_at INTEGER
    );
    CREATE INDEX items_done_at ON items(done_at);
    CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    "CREATE TABLE sessions (
        id TEXT PRIMARY KEY,
        cwd TEXT NOT NULL,
        transcript_path TEXT,
        transcript_mtime INTEGER,
        status TEXT NOT NULL,
        prompt TEXT,
        message TEXT,
        started_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        waiting_since INTEGER,
        notified_at INTEGER
    );",
    "ALTER TABLE sessions ADD COLUMN title TEXT;
    ALTER TABLE sessions ADD COLUMN entrypoint TEXT;",
    "ALTER TABLE items ADD COLUMN url TEXT;
    ALTER TABLE items ADD COLUMN source_app TEXT;",
    "CREATE TABLE inbox (
        key TEXT PRIMARY KEY,
        source TEXT NOT NULL,
        kind TEXT NOT NULL,
        text TEXT NOT NULL,
        url TEXT,
        since INTEGER,
        first_seen INTEGER NOT NULL,
        last_seen INTEGER NOT NULL,
        dismissed_at INTEGER,
        notified_at INTEGER
    );",
];

impl Core {
    pub fn open(path: &Path) -> Result<Arc<Self>> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error(e.to_string()))?;
        }
        Self::with_connection(Connection::open(path)?)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Arc<Self>> {
        Self::with_connection(Connection::open_in_memory()?)
    }

    fn with_connection(conn: Connection) -> Result<Arc<Self>> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            conn.execute_batch(sql)?;
            conn.pragma_update(None, "user_version", i as i64 + 1)?;
        }
        // Hook events are lost while Margin is not running, so read every transcript again.
        conn.execute("UPDATE sessions SET transcript_mtime = NULL", [])?;
        let (events, _) = broadcast::channel(64);
        Ok(Arc::new(Self { conn: Mutex::new(conn), events }))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn changed(&self) {
        let _ = self.events.send(Event::Changed);
    }

    pub fn parse(&self, text: &str, now: DateTime<Local>) -> Parsed {
        parse::parse(text, &now)
    }

    pub fn items(&self, now: DateTime<Local>) -> Result<Items> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM items WHERE done_at IS NULL")?;
        let mut open = stmt.query_map([], item_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        sort_open(&mut open, now.timestamp_millis());

        let mut stmt = conn.prepare("SELECT * FROM items WHERE done_at >= ?1 ORDER BY done_at DESC")?;
        let done_today = stmt
            .query_map([start_of_day(now)], item_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Items { open, done_today })
    }

    pub fn get(&self, id: i64) -> Result<Item> {
        self.conn()
            .query_row("SELECT * FROM items WHERE id = ?1", [id], item_from_row)
            .optional()?
            .ok_or_else(|| Error(format!("item {id} not found")))
    }

    #[cfg(test)]
    pub fn add(&self, text: &str, now: DateTime<Local>) -> Result<Item> {
        self.add_with_link(text, None, None, now)
    }

    pub fn add_with_link(
        &self,
        text: &str,
        url: Option<&str>,
        source_app: Option<&str>,
        now: DateTime<Local>,
    ) -> Result<Item> {
        let url = url.filter(|u| u.starts_with("https://") || u.starts_with("http://"));
        let parsed = parse::parse(text, &now);
        if parsed.title.is_empty() {
            return Err(Error("title is empty".into()));
        }
        let id = {
            let conn = self.conn();
            conn.execute(
                "INSERT INTO items (title, created_at, due_at, url, source_app) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![parsed.title, now.timestamp_millis(), parsed.due_at, url, url.and(source_app)],
            )?;
            conn.last_insert_rowid()
        };
        self.changed();
        self.get(id)
    }

    /// The text to prefill when editing an item: the title plus its due time as HH:MM.
    pub fn edit_text(&self, id: i64) -> Result<String> {
        let item = self.get(id)?;
        Ok(match item.due_at.and_then(clock_token) {
            Some(token) => format!("{} {token}", item.title),
            None => item.title,
        })
    }

    pub fn edit(&self, id: i64, text: &str, now: DateTime<Local>) -> Result<Item> {
        let item = self.get(id)?;
        let mut parsed = parse::parse(text, &now);
        // An unchanged HH:MM keeps the original due time, so editing an overdue item does not move it to tomorrow.
        let (head, last) = parse::split_last(text.trim());
        if item.due_at.is_some() && item.due_at.and_then(clock_token).as_deref() == Some(last) {
            parsed = Parsed { title: head.trim_end().to_string(), due_at: item.due_at };
        }
        if parsed.title.is_empty() {
            return Err(Error("title is empty".into()));
        }
        let notified_at = if parsed.due_at == item.due_at { item.notified_at } else { None };
        self.conn().execute(
            "UPDATE items SET title = ?2, due_at = ?3, notified_at = ?4 WHERE id = ?1",
            params![id, parsed.title, parsed.due_at, notified_at],
        )?;
        self.changed();
        self.get(id)
    }

    pub fn done(&self, id: i64, now: DateTime<Local>) -> Result<()> {
        self.update(id, "UPDATE items SET done_at = ?2 WHERE id = ?1", now.timestamp_millis())
    }

    pub fn reopen(&self, id: i64) -> Result<()> {
        let n = self.conn().execute("UPDATE items SET done_at = NULL WHERE id = ?1", [id])?;
        self.after_update(id, n)
    }

    pub fn snooze(&self, id: i64, minutes: i64, now: DateTime<Local>) -> Result<()> {
        let due = now.timestamp_millis() + minutes * 60_000;
        self.update(id, "UPDATE items SET due_at = ?2, notified_at = NULL WHERE id = ?1", due)
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        let n = self.conn().execute("DELETE FROM items WHERE id = ?1", [id])?;
        self.after_update(id, n)
    }

    fn update(&self, id: i64, sql: &str, value: i64) -> Result<()> {
        let n = self.conn().execute(sql, params![id, value])?;
        self.after_update(id, n)
    }

    fn after_update(&self, id: i64, rows: usize) -> Result<()> {
        if rows == 0 {
            return Err(Error(format!("item {id} not found")));
        }
        self.changed();
        Ok(())
    }

    /// Marks open items whose due time has passed as notified, and returns them.
    pub fn tick(&self, now: DateTime<Local>) -> Result<Vec<Item>> {
        let now_ms = now.timestamp_millis();
        let due = {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "UPDATE items SET notified_at = ?1
                 WHERE done_at IS NULL AND notified_at IS NULL AND due_at <= ?1
                 RETURNING *",
            )?;
            let mut due = stmt.query_map([now_ms], item_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            due.sort_by_key(|i| i.due_at);
            due
        };
        if !due.is_empty() {
            let _ = self.events.send(Event::Due(due.clone()));
            self.changed();
        }
        Ok(due)
    }

    pub fn purge(&self, now: DateTime<Local>) -> Result<usize> {
        let cutoff = now.timestamp_millis() - DONE_RETENTION_MS;
        let n = self.conn().execute("DELETE FROM items WHERE done_at < ?1", [cutoff])?;
        if n > 0 {
            self.changed();
        }
        Ok(n)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
}

fn item_from_row(row: &Row) -> rusqlite::Result<Item> {
    Ok(Item {
        id: row.get("id")?,
        title: row.get("title")?,
        created_at: row.get("created_at")?,
        due_at: row.get("due_at")?,
        notified_at: row.get("notified_at")?,
        done_at: row.get("done_at")?,
        url: row.get("url")?,
        source_app: row.get("source_app")?,
    })
}

/// Overdue first, then by due time, then untimed items by creation time.
fn sort_open(items: &mut [Item], now_ms: i64) {
    items.sort_by_key(|i| match i.due_at {
        Some(due) if due <= now_ms => (0, due, i.id),
        Some(due) => (1, due, i.id),
        None => (2, i.created_at, i.id),
    });
}

fn start_of_day(now: DateTime<Local>) -> i64 {
    let midnight = now.date_naive().and_hms_opt(0, 0, 0).expect("valid time");
    Local
        .from_local_datetime(&midnight)
        .earliest()
        .map(|t| t.timestamp_millis())
        .unwrap_or_else(|| now.timestamp_millis() - 24 * 60 * 60 * 1000)
}

fn clock_token(ms: i64) -> Option<String> {
    Local.timestamp_millis_opt(ms).single().map(|t| t.format("%H:%M").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 9, 28, 10, 0, 0).unwrap()
    }

    #[test]
    fn sort_order() {
        let core = Core::open_in_memory().unwrap();
        let t = now();
        core.add("untimed first", t).unwrap();
        core.add("later 2h", t).unwrap();
        core.add("soon 10m", t).unwrap();
        core.add("untimed second", t + Duration::seconds(1)).unwrap();
        let overdue = core.add("overdue 5m", t - Duration::minutes(30)).unwrap();
        assert!(overdue.due_at.unwrap() < t.timestamp_millis());

        let titles: Vec<_> = core.items(t).unwrap().open.into_iter().map(|i| i.title).collect();
        assert_eq!(titles, ["overdue", "soon", "later", "untimed first", "untimed second"]);
    }

    #[test]
    fn tick_notifies_once_and_snooze_rearms() {
        let core = Core::open_in_memory().unwrap();
        let t = now();
        let item = core.add("check deploy 30m", t).unwrap();
        assert!(core.tick(t + Duration::minutes(29)).unwrap().is_empty());
        assert_eq!(core.tick(t + Duration::minutes(30)).unwrap().len(), 1);
        assert!(core.tick(t + Duration::minutes(31)).unwrap().is_empty());

        core.snooze(item.id, 10, t + Duration::minutes(31)).unwrap();
        assert!(core.tick(t + Duration::minutes(40)).unwrap().is_empty());
        assert_eq!(core.tick(t + Duration::minutes(41)).unwrap().len(), 1);
    }

    #[test]
    fn done_items_leave_the_open_list_and_are_purged() {
        let core = Core::open_in_memory().unwrap();
        let t = now();
        let item = core.add("ship it", t).unwrap();
        core.done(item.id, t).unwrap();
        let items = core.items(t).unwrap();
        assert!(items.open.is_empty());
        assert_eq!(items.done_today.len(), 1);
        assert!(core.tick(t + Duration::hours(1)).unwrap().is_empty());

        assert_eq!(core.purge(t + Duration::days(6)).unwrap(), 0);
        assert_eq!(core.purge(t + Duration::days(8)).unwrap(), 1);
    }

    #[test]
    fn edit_keeps_due_time_when_token_unchanged() {
        let core = Core::open_in_memory().unwrap();
        let t = now();
        let item = core.add("standup 10:30", t).unwrap();
        let later = t + Duration::hours(1);
        let text = core.edit_text(item.id).unwrap();
        assert_eq!(text, "standup 10:30");

        let edited = core.edit(item.id, "daily standup 10:30", later).unwrap();
        assert_eq!(edited.title, "daily standup");
        assert_eq!(edited.due_at, item.due_at);

        let moved = core.edit(item.id, "daily standup 15m", later).unwrap();
        assert_eq!(moved.due_at, Some((later + Duration::minutes(15)).timestamp_millis()));
        assert_eq!(moved.notified_at, None);
    }

    #[test]
    fn links_are_kept_only_for_web_urls() {
        let core = Core::open_in_memory().unwrap();
        let item = core.add_with_link("PR #1 x 30m", Some("https://github.com/a/b/pull/1"), Some("chrome.exe"), now()).unwrap();
        assert_eq!(item.url.as_deref(), Some("https://github.com/a/b/pull/1"));
        assert_eq!(item.source_app.as_deref(), Some("chrome.exe"));
        let item = core.add_with_link("x", Some("file:///c:/secret"), Some("chrome.exe"), now()).unwrap();
        assert_eq!((item.url, item.source_app), (None, None));
    }

    #[test]
    fn empty_title_is_rejected() {
        let core = Core::open_in_memory().unwrap();
        assert!(core.add("30m", now()).is_err());
        assert!(core.add("   ", now()).is_err());
    }
}
