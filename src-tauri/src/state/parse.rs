//! Parses an optional due time from the last word of a captured line.

use chrono::{DateTime, Duration, NaiveTime, TimeZone};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Parsed {
    pub title: String,
    pub due_at: Option<i64>,
}

pub fn parse<Tz: TimeZone>(text: &str, now: &DateTime<Tz>) -> Parsed {
    let text = text.trim();
    let (head, last) = split_last(text);
    match due_from_token(last, now) {
        Some(due) => Parsed { title: head.trim_end().to_string(), due_at: Some(due) },
        None => Parsed { title: text.to_string(), due_at: None },
    }
}

/// Splits trimmed text into everything before the last word, and the last word.
pub fn split_last(text: &str) -> (&str, &str) {
    match text.rfind(char::is_whitespace) {
        Some(i) => (&text[..i], text[i..].trim_start()),
        None => ("", text),
    }
}

pub fn due_from_token<Tz: TimeZone>(token: &str, now: &DateTime<Tz>) -> Option<i64> {
    if let Some(duration) = parse_duration(token) {
        return Some((now.clone() + duration).timestamp_millis());
    }
    let time = parse_clock(token)?;
    let tz = now.timezone();
    // A time inside a skipped DST hour gives no local time that day, so try the next day.
    (0..=2).find_map(|offset| {
        let date = now.date_naive() + Duration::days(offset);
        let at = tz.from_local_datetime(&date.and_time(time)).earliest()?;
        (at > *now).then(|| at.timestamp_millis())
    })
}

fn parse_duration(token: &str) -> Option<Duration> {
    let token = token.to_ascii_lowercase();
    let (hours, rest) = match token.split_once('h') {
        Some((h, rest)) => (number(h)?, rest),
        None => (0, token.as_str()),
    };
    let has_hours = token.contains('h');
    let minutes = match rest.strip_suffix('m') {
        Some(m) => number(m)?,
        None if rest.is_empty() && has_hours => 0,
        // "1h30" reads as 1 h 30 min.
        None if has_hours => number(rest)?,
        None => return None,
    };
    let total = hours * 60 + minutes;
    (total > 0).then(|| Duration::minutes(total))
}

fn parse_clock(token: &str) -> Option<NaiveTime> {
    let (h, m) = token.split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    NaiveTime::from_hms_opt(number(h)? as u32, number(m)? as u32, 0)
}

fn number(s: &str) -> Option<i64> {
    if s.is_empty() || s.len() > 4 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn at(h: u32, m: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(2 * 3600).unwrap().with_ymd_and_hms(2026, 9, 28, h, m, 0).unwrap()
    }

    fn due_in_minutes(text: &str, now: &DateTime<FixedOffset>) -> Option<i64> {
        parse(text, now).due_at.map(|d| (d - now.timestamp_millis()) / 60_000)
    }

    #[test]
    fn durations() {
        let now = at(10, 0);
        assert_eq!(due_in_minutes("check deploy 30m", &now), Some(30));
        assert_eq!(due_in_minutes("x 2h", &now), Some(120));
        assert_eq!(due_in_minutes("x 1h30m", &now), Some(90));
        assert_eq!(due_in_minutes("x 1h30", &now), Some(90));
        assert_eq!(due_in_minutes("x 90M", &now), Some(90));
        assert_eq!(parse("check deploy 30m", &now).title, "check deploy");
    }

    #[test]
    fn clock_today_and_tomorrow() {
        let now = at(10, 0);
        assert_eq!(due_in_minutes("standup 14:30", &now), Some(4 * 60 + 30));
        assert_eq!(due_in_minutes("standup 9:05", &now), Some(23 * 60 + 5));
        assert_eq!(due_in_minutes("standup 10:00", &now), Some(24 * 60));
        assert_eq!(parse("standup 14:30", &now).title, "standup");
    }

    #[test]
    fn not_a_time() {
        let now = at(10, 0);
        for text in ["plain text", "x 0m", "x 25:00", "x 12:5", "x 30", "x m", "x h", "x 1.5h", "x 12:60", "x 123:00"] {
            let p = parse(text, &now);
            assert_eq!(p.due_at, None, "{text}");
            assert_eq!(p.title, text, "{text}");
        }
    }

    #[test]
    fn only_the_last_word_counts() {
        let now = at(10, 0);
        let p = parse("30m review of 14:30 notes", &now);
        assert_eq!(p.due_at, None);
    }

    #[test]
    fn token_alone_gives_empty_title() {
        let p = parse("  30m  ", &at(10, 0));
        assert_eq!(p.title, "");
        assert!(p.due_at.is_some());
    }
}
