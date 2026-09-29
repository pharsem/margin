# Context suggestions in quick capture — spec

Extends step 1 of SPEC.md. Build after step 1 works.

## Goal

When I press the capture hotkey, the popup suggests notes based on what I was looking at. Example: I'm on a GitHub pull request in Chrome, press the hotkey, press ↓, type ` 30m`, press Enter, and get a follow-up item titled after the PR, with its URL attached, due in 30 minutes.

## Principles

- **Capture happens at the hotkey press, before the popup takes focus.** After that the foreground window is our own popup, and the context is gone.
- **Local and deterministic.** Read window metadata and accessibility info only. No screenshots, OCR or LLM calls. It must feel instant.
- **Never block the popup.** The input shows and is focused right away. Suggestions appear when ready. If capture fails or is slow, there are no suggestions and nothing else changes.
- **Private by default.** Context lives in memory for the lifetime of the popup only. It's never logged or stored unless I save a suggestion.

## Data model change

Items get two optional fields, `url` and `source_app` (process name). Add them in step 1 already, so no migration is needed. The panel shows a small link icon on items with a URL; clicking it opens the URL in the default browser.

## Capture sources (in priority order)

1. **Foreground window** — `GetForegroundWindow` → `GetWindowTextW` for the title, `GetWindowThreadProcessId` + `QueryFullProcessImageNameW` for the process. Always available, near-free.
2. **Browser URL** (Chrome, Edge) via UI Automation, on a background thread with a hard 300 ms timeout:
   - Find the address bar through the UIA tree and read its `ValuePattern`. **Don't match on the element's name**; it's localised (my Chrome may be in Norwegian). Match on control type and position in the toolbar instead.
   - Chrome shows the URL without a scheme. Prepend `https://` when missing.
   - Spike this first and check it against both browsers. If it proves unreliable, use the extension fallback below.
3. **Clipboard** — if the clipboard changed within the last 60 s and holds a URL or a short single line of text (< 200 chars), offer it. Track change time with `AddClipboardFormatListener` rather than guessing.

Never simulate keystrokes (e.g. sending Ctrl+C to grab selected text). It interferes with the app I'm in.

**Skip capture entirely** when the foreground window is a private/incognito browser window (title contains "InPrivate" or "Incognito") or the process is on a configurable denylist (default: password managers such as `1Password.exe`, `KeePass*.exe`, `Bitwarden.exe`).

## Recognisers

Turn raw context into readable suggestion text. Each recogniser gets `{url?, window_title, process}` and returns a suggestion or nothing. Keep them as small pure functions with unit tests.

| Match | Suggestion text | Attach |
|---|---|---|
| `github.com/<org>/<repo>/pull/<n>` | `PR #<n> <repo>: <PR title>` | URL |
| `github.com/<org>/<repo>/issues/<n>` | `Issue #<n> <repo>: <title>` | URL |
| `app.clickup.com/t/<id>` | `<task title>` | URL |
| Slack desktop (`slack.exe`) | `Slack: <channel or DM name>` | — |
| VS Code / terminal | `<workspace or folder name>` | — |
| Any other URL | `<page title>` | URL |
| Any other window | `<window title>` | — |

Titles come from the window title with the browser suffix and site boilerplate stripped (e.g. GitHub's `· Pull Request #123 · org/repo - Google Chrome`). Truncate to ~80 chars.

Show at most 3 suggestions, most specific first. Deduplicate: if the clipboard URL equals the tab URL, show it once.

## Popup interaction

```
┌──────────────────────────────────────┐
│ ▌                                    │  ← input, focused, empty
├──────────────────────────────────────┤
│  PR #412 backend: Fix credit rollover│  ← suggestions
│  Clipboard: https://…                │
└──────────────────────────────────────┘
```

- **↓ / ↑** moves the highlight between the input and suggestions.
- **Input empty + suggestion highlighted:** the input shows the suggestion text with the cursor at the end. Typing appends (e.g. ` 30m`). The URL is attached to the item.
- **Input already has text + suggestion highlighted:** the typed text stays as the title, and the suggestion's URL is attached. Shown as a small "🔗 PR #412" chip under the input. So "ping Lars about this 30m" + ↓ works too.
- **↑ back to the input** restores what I had typed and detaches the link.
- **Enter** saves. **Esc** cancels and discards the captured context.
- Time parsing (`30m`, `14:30`) runs on the final text, same as step 1.
- Mouse click on a suggestion does the same as highlighting it.

## Performance budget

- Popup visible and focused: < 50 ms after the hotkey.
- Window title + process: synchronous, < 5 ms.
- UIA URL read: async, 300 ms timeout. A late result is discarded if I've already saved or cancelled.

## Extension fallback (only if UIA isn't reliable)

After step 2 adds the local HTTP server, a tiny Chrome/Edge MV3 extension can POST `{url, title}` to `127.0.0.1:<port>/context` on `tabs.onActivated` and `tabs.onUpdated`. The capture code then takes the latest browser context if it's newer than the last focus change. Full URL, no scraping. The cost is one more thing to install.

## Out of scope

- Screenshots, OCR, vision models
- Reading page content beyond the URL and window title
- Firefox (add later if needed)
- LLM-written suggestion text

## Done when

- On a GitHub PR in Chrome: hotkey → ↓ → ` 30m` → Enter gives a correctly titled item with the PR link, due in 30 minutes.
- Same flow works in Edge.
- In an incognito window or a password manager, no suggestions appear.
- With UIA artificially delayed past the timeout, the popup still behaves normally, just without the URL suggestion.
