//! Turns what was on screen at the capture hotkey into suggested follow-up titles.
//! Pure functions, so the Windows capture code stays thin.

use serde::Serialize;

const MAX_SUGGESTIONS: usize = 3;
const MAX_CHARS: usize = 80;
const CLIPBOARD_MAX_CHARS: usize = 200;

#[derive(Debug, Clone, Default)]
pub struct Context {
    pub window_title: String,
    /// Executable name, for example "chrome.exe".
    pub process: String,
    pub url: Option<String>,
    /// Clipboard text, only if it changed shortly before the hotkey.
    pub clipboard: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Page,
    Window,
    Clipboard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    pub text: String,
    pub url: Option<String>,
    pub source: Source,
}

/// True for windows whose content must never be read.
pub fn is_private(window_title: &str, process: &str, denylist: &[String]) -> bool {
    let title = window_title.to_lowercase();
    ["incognito", "inkognito", "inprivate"].iter().any(|w| title.contains(w))
        || denylist.iter().any(|pattern| glob_match(&pattern.to_lowercase(), &process.to_lowercase()))
}

pub fn suggestions(ctx: &Context) -> Vec<Suggestion> {
    let mut list = Vec::new();
    if let Some(url) = ctx.url.as_deref() {
        let page = page_title(&ctx.window_title, &ctx.process);
        list.push(Suggestion { text: truncate(&url_text(url, &page)), url: Some(url.to_string()), source: Source::Page });
    } else if let Some(text) = window_text(&ctx.window_title, &ctx.process) {
        list.push(Suggestion { text: truncate(&text), url: None, source: Source::Window });
    }
    if let Some(clip) = ctx.clipboard.as_deref().map(str::trim).filter(|c| is_offerable_clipboard(c)) {
        let url = is_url(clip).then(|| clip.to_string());
        if url.is_none() || url.as_deref() != ctx.url.as_deref() {
            list.push(Suggestion { text: truncate(clip), url, source: Source::Clipboard });
        }
    }
    list.truncate(MAX_SUGGESTIONS);
    list
}

fn url_text(url: &str, page: &str) -> String {
    let path: Vec<&str> = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .split('/')
        .collect();
    match path.as_slice() {
        ["github.com", _org, repo, kind @ ("pull" | "issues"), n, ..] if n.chars().all(|c| c.is_ascii_digit()) => {
            let label = if *kind == "pull" { "PR" } else { "Issue" };
            match github_title(page) {
                Some(title) => format!("{label} #{n} {repo}: {title}"),
                None => format!("{label} #{n} {repo}"),
            }
        }
        ["app.clickup.com", "t", ..] => strip_suffixes(page, &[" | ClickUp", " - ClickUp"]).to_string(),
        _ if !page.is_empty() => page.to_string(),
        _ => url.to_string(),
    }
}

/// GitHub titles look like "Fix rollover by lars · Pull Request #412 · org/repo".
fn github_title(page: &str) -> Option<String> {
    let first = page.split(" · ").next()?.trim();
    if first.is_empty() || !page.contains(" · ") {
        return None;
    }
    let title = match first.rsplit_once(" by ") {
        Some((title, author)) if !author.contains(' ') => title,
        _ => first,
    };
    Some(title.trim().to_string())
}

/// The window title without the browser name, Edge profile or tab count.
fn page_title(window_title: &str, process: &str) -> String {
    let mut t = window_title.trim().to_string();
    match process.to_lowercase().as_str() {
        "chrome.exe" => t = strip_suffixes(&t, &[" - Google Chrome"]).to_string(),
        "msedge.exe" => {
            // Edge writes "Microsoft\u{200b} Edge" with a zero-width space.
            t = strip_suffixes(&t, &[" - Microsoft\u{200b} Edge", " - Microsoft Edge"]).to_string();
            if let Some((rest, profile)) = t.rsplit_once(" - ") {
                if profile == "Personal" || profile == "Work" || profile.starts_with("Profile ") {
                    t = rest.to_string();
                }
            }
            if let Some((rest, more)) = t.rsplit_once(" and ") {
                if more.ends_with(" more pages") || more.ends_with(" more page") {
                    t = rest.to_string();
                }
            }
        }
        _ => {}
    }
    t.trim().to_string()
}

fn window_text(title: &str, process: &str) -> Option<String> {
    let title = title.trim();
    if title.is_empty() {
        return None;
    }
    let text = match process.to_lowercase().as_str() {
        // "tech-on-call (Channel) - Workspace - 3 new items - Slack"
        "slack.exe" => {
            let first = title.split(" - ").next().unwrap_or(title);
            let name = match first.rsplit_once(" (") {
                Some((name, _)) => name,
                None => first,
            };
            format!("Slack: {}", name.trim())
        }
        // "main.rs - margin - Visual Studio Code"
        "code.exe" => {
            let parts: Vec<&str> = title.trim_start_matches("● ").split(" - ").collect();
            match parts.as_slice() {
                [.., workspace, _app] if parts.len() >= 3 => workspace.to_string(),
                [workspace, _app] => workspace.to_string(),
                _ => title.to_string(),
            }
        }
        // "MINGW64:/c/Users/me/dev/repo" or "✳ Session title"
        "windowsterminal.exe" => {
            let t = title.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '~').trim();
            match t.split_once(':') {
                Some((_, path)) if path.contains('/') || path.contains('\\') => {
                    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(t).to_string()
                }
                _ => t.to_string(),
            }
        }
        // "Task title | MA-27112"
        "clickup.exe" => match title.rsplit_once(" | ") {
            Some((task, _)) => task.to_string(),
            None => title.to_string(),
        },
        _ => title.to_string(),
    };
    Some(text).filter(|t| !t.trim().is_empty())
}

fn is_url(text: &str) -> bool {
    (text.starts_with("https://") || text.starts_with("http://")) && !text.contains(char::is_whitespace)
}

fn is_offerable_clipboard(text: &str) -> bool {
    !text.is_empty() && !text.contains('\n') && (is_url(text) || text.chars().count() < CLIPBOARD_MAX_CHARS)
}

fn strip_suffixes<'a>(text: &'a str, suffixes: &[&str]) -> &'a str {
    suffixes.iter().find_map(|s| text.strip_suffix(s)).unwrap_or(text).trim()
}

fn truncate(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= MAX_CHARS {
        return text.to_string();
    }
    let cut: String = text.chars().take(MAX_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Case-sensitive match where `*` matches any run of characters.
fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || !text.ends_with(last) || text.len() < first.len() + last.len() {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(title: &str, process: &str, url: Option<&str>) -> Context {
        Context { window_title: title.into(), process: process.into(), url: url.map(Into::into), clipboard: None }
    }

    fn first(c: &Context) -> String {
        suggestions(c)[0].text.clone()
    }

    #[test]
    fn github_pull_request_and_issue() {
        let pr = ctx(
            "Fix credit rollover by lars · Pull Request #412 · acme/backend - Google Chrome",
            "chrome.exe",
            Some("https://github.com/acme/backend/pull/412/files"),
        );
        assert_eq!(first(&pr), "PR #412 backend: Fix credit rollover");
        assert_eq!(suggestions(&pr)[0].url.as_deref(), Some("https://github.com/acme/backend/pull/412/files"));

        let issue = ctx(
            "Login fails on Safari · Issue #77 · acme/web - Microsoft\u{200b} Edge",
            "msedge.exe",
            Some("https://github.com/acme/web/issues/77"),
        );
        assert_eq!(first(&issue), "Issue #77 web: Login fails on Safari");
    }

    #[test]
    fn other_pages_use_the_page_title() {
        let argo = ctx("myapp - Application Details Tree - Argo CD - Google Chrome", "chrome.exe", Some("https://argo.example/x"));
        assert_eq!(first(&argo), "myapp - Application Details Tree - Argo CD");
        let edge = ctx("Docs and 3 more pages - Work - Microsoft\u{200b} Edge", "msedge.exe", Some("https://d.example"));
        assert_eq!(first(&edge), "Docs");
        let clickup = ctx("Fix the crash | ClickUp - Google Chrome", "chrome.exe", Some("https://app.clickup.com/t/86abc"));
        assert_eq!(first(&clickup), "Fix the crash");
    }

    #[test]
    fn app_windows() {
        assert_eq!(first(&ctx("tech-on-call (Channel) - Acme - 3 new items - Slack", "slack.exe", None)), "Slack: tech-on-call");
        assert_eq!(first(&ctx("Kari Nordmann (DM) - Acme - Slack", "Slack.exe", None)), "Slack: Kari Nordmann");
        assert_eq!(first(&ctx("● main.rs - margin - Visual Studio Code", "Code.exe", None)), "margin");
        assert_eq!(first(&ctx("MINGW64:/c/Users/me/dev/backend-monorepo", "WindowsTerminal.exe", None)), "backend-monorepo");
        assert_eq!(first(&ctx("✳ SPEC.md step 0 implementation", "WindowsTerminal.exe", None)), "SPEC.md step 0 implementation");
        assert_eq!(first(&ctx("email-ms: fix the crash | MA-27112", "ClickUp.exe", None)), "email-ms: fix the crash");
        assert_eq!(first(&ctx("Untitled - Notepad", "notepad.exe", None)), "Untitled - Notepad");
    }

    #[test]
    fn clipboard_is_offered_once_and_only_when_short() {
        let mut c = ctx("PR · Pull Request #1 · a/b - Google Chrome", "chrome.exe", Some("https://github.com/a/b/pull/1"));
        c.clipboard = Some("https://github.com/a/b/pull/1".into());
        assert_eq!(suggestions(&c).len(), 1, "same URL as the tab");
        c.clipboard = Some("https://example.com/other".into());
        assert_eq!(suggestions(&c)[1].source, Source::Clipboard);
        assert_eq!(suggestions(&c)[1].url.as_deref(), Some("https://example.com/other"));
        c.clipboard = Some("line one\nline two".into());
        assert_eq!(suggestions(&c).len(), 1);
        c.clipboard = Some("x".repeat(250));
        assert_eq!(suggestions(&c).len(), 1);
    }

    #[test]
    fn long_titles_are_truncated() {
        let text = first(&ctx(&"a".repeat(120), "notepad.exe", None));
        assert_eq!(text.chars().count(), MAX_CHARS);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn private_windows_and_denylist() {
        let deny = vec!["1Password.exe".to_string(), "KeePass*.exe".to_string()];
        assert!(is_private("New tab - Google Chrome (Incognito)", "chrome.exe", &deny));
        assert!(is_private("Ny fane – Inkognito", "chrome.exe", &deny));
        assert!(is_private("InPrivate - Microsoft Edge", "msedge.exe", &deny));
        assert!(is_private("Vault", "1password.exe", &deny));
        assert!(is_private("db.kdbx", "KeePassXC.exe", &deny));
        assert!(!is_private("GitHub - Google Chrome", "chrome.exe", &deny));
    }
}
