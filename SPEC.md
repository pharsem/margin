# Now panel — spec

A Windows desktop panel that docks to a screen edge and holds the fast loop: things to follow up on within minutes or hours, and the state of parallel Claude Code sessions. It is not a task manager. Anything that isn't relevant today goes elsewhere (ClickUp, #petter-brain).

## Principles

- **Instant beats smart.** Timers, hooks and notifications are deterministic and never depend on an LLM call. AI comes later and only suggests.
- **Capture costs nothing.** One hotkey, one line of text, Enter. No fields, no tags.
- **Short list or it's useless.** The panel should rarely hold more than ~10 items. Moving things out must be as easy as adding them.
- **The Rust core owns all state; the UI is a view.** The same core will later serve an HTTP API (hooks, tablet, Stream Deck), so no business logic in the frontend.

## Stack

- Tauri 2, Rust core, Svelte + TypeScript + Vite frontend (open to change)
- SQLite via `rusqlite` in the core
- Plugins: `tauri-plugin-global-shortcut`, `tauri-plugin-notification`, `tauri-plugin-autostart`
- Windows AppBar through the `windows` crate (`SHAppBarMessage`)
- Windows 11 only. No macOS/Linux support needed.

## Build order

Build one step at a time. Stop after each step for review.

### Step 0 — AppBar spike (do this first)

AppBar is the only real technical unknown, so prove it before building UI.

- Frameless Tauri window (`decorations: false`, `skip_taskbar: true`, `always_on_top: true`)
- Get the HWND and register it as an AppBar: `ABM_NEW` → `ABM_QUERYPOS` → `ABM_SETPOS`, then move the window to the returned rect
- Subclass the window (`SetWindowSubclass`) to handle the AppBar callback message, re-querying position on `ABN_POSCHANGED` (display changes, taskbar moves)
- **Always call `ABM_REMOVE` on exit, including crash/panic paths and Ctrl+C in dev.** Otherwise the reserved space stays until Explorer restarts.
- Coordinates are physical pixels. Handle mixed DPI across monitors.
- Config: target monitor (index or device name), edge (left/right), width in px.

Done when: a maximized window on the target monitor stops at the panel's edge, the space is released on exit, and it survives unplugging/replugging a monitor.

### Step 1 — Follow-ups with timers

**Capture**
- Global hotkey (default `Ctrl+Alt+Space`, configurable) opens a small input popup centred on the active monitor. Enter saves, Esc cancels.
- Parse an optional time from the end of the text:
  - `30m`, `2h`, `1h30m` → due in that long
  - `14:30` → due today at that time (tomorrow if already past)
  - nothing → no timer, just an open item
- The parsed time is removed from the title. Show the parse result live in the popup ("due 14:35") so mistakes are caught before saving.

**Panel**
- Items sorted: overdue first, then by due time, then untimed by creation time.
- Each item shows its title and a relative time ("in 12 min", "overdue 4 min").
- An item that becomes due turns red, a Windows toast fires once, and the item stays red until handled.
- Actions per item: **done**, **snooze** (+10m, +30m, +2h), **edit**, **delete**. Keyboard shortcuts for all of these on the focused item.
- A collapse toggle shrinks the AppBar to a ~48px strip showing only a count of overdue items. The AppBar reservation shrinks with it.

**System**
- Autostart with Windows (setting, default on).
- Tray icon with show/hide, collapse, settings, quit.
- Done items are kept 7 days (for "what did I finish today") then purged.

**Data**
- Items have optional `url` and `source_app` fields from the start (used by step 1b). Items with a URL show a link icon that opens it in the default browser.

Done when: I can capture "check deploy 30m" from any app without touching the mouse, forget about it, and get pulled back at the right time.

### Step 1b — Context suggestions in capture

See CONTEXT-CAPTURE.md. The capture popup suggests notes from the window I was in (e.g. the PR open in my browser), selectable with ↓.

### Step 2 — Claude Code sessions lane (outline, spec in detail later)

- The core starts an HTTP server on `127.0.0.1:<port>` (axum). Localhost only.
- Claude Code hooks (in user-level `settings.json`) for `SessionStart`, `UserPromptSubmit`, `Stop`, `Notification` and `SessionEnd` POST their stdin JSON to the server, e.g. `curl -s --max-time 1 -X POST http://127.0.0.1:<port>/hooks -H "Content-Type: application/json" --data-binary @-`. Must fail silently and fast when the app isn't running. Verify which shell Claude Code uses for hooks on Windows.
- State per `session_id`: running (start/prompt) → **needs input** (Notification) or **done, not reviewed** (Stop) → removed (SessionEnd or manual "reviewed").
- Label: folder name from `cwd` + first ~60 chars of the latest prompt.
- The key signal is an **idle counter** since the last Stop/Notification, getting more prominent with time. Toast when a session starts waiting.
- Separate lane above or below follow-ups.

### Step 3 — Chief of staff (outline)

- Every 20–30 min during work hours, run `claude -p` headless with Slack/ClickUp/GitHub MCP access and a fixed prompt that returns JSON suggestions (unanswered direct questions, reviews waiting, stale items).
- Suggestions show as dismissable items in a third lane. They never become follow-ups without a click.
- Interruption policy: toast only for items crossing a clear threshold (e.g. direct question unanswered > 2h). Everything else is silent.
- "Later" action on any item posts it to Slack `#petter-brain` for the slow loop.

### Step 4 — Other surfaces (maybe)

- Expose the UI and API on the LAN behind a token so an old tablet can show the panel in a browser.
- Stream Deck plugin calling the same API (capture, per-session status keys).

## Out of scope

- Projects, tags, priorities, due dates beyond today, recurring items
- Sync to ClickUp
- Cloud/Cowork session tracking (can't reach localhost — revisit with a relay if needed)

## Open decisions

- Repo name and location
- Frontend framework (Svelte assumed)
- Default monitor, edge and width
- Hotkey
