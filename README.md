# Margin

A Windows 11 panel that docks to a screen edge as an AppBar. It holds short-lived follow-ups with timers. The original plan and its reasons are in [docs/spec.md](docs/spec.md) and [docs/context-capture.md](docs/context-capture.md).

Status: early work. Windows 11 only.

## Prerequisites

- Rust (stable, MSVC toolchain) through rustup
- Visual Studio 2022 Build Tools with the "Desktop development with C++" workload
- WebView2 runtime (Windows 11 includes it)
- Node.js and pnpm

## Run

```
pnpm install
pnpm tauri dev
```

## Install

```
pnpm tauri build
```

This makes `src-tauri\target\release\bundle\nsis\Margin_<version>_x64-setup.exe`, a per-user installer that needs no admin rights. It installs to `%LOCALAPPDATA%\Margin` and adds a Start menu shortcut. The installed app starts with Windows, and its toasts show "Margin" as the sender.

The installed app and `pnpm tauri dev` use the same identifier, config and database. Only one can run at a time. Quit the installed app from the tray before you start `pnpm tauri dev`, or the dev build gives focus to the installed app and exits.

To change the icon, edit `src-tauri/icons/icon.svg` and run `pnpm tauri icon src-tauri/icons/icon.svg`. Then delete the `android` and `ios` folders that it makes.

## Config

The app reads `%APPDATA%\com.petterharsem.margin\config.json` at startup and creates it with defaults if it is missing. Tray > Settings opens the file. The app applies changes when you save the file.

```json
{
  "monitor": "primary",
  "edge": "right",
  "width": 320,
  "hotkey": "Ctrl+Alt+Space",
  "focus_hotkey": "Ctrl+Alt+N",
  "port": 47811,
  "context_denylist": ["1Password.exe", "KeePass*.exe", "Bitwarden.exe"],
  "inbox": { "enabled": true, "start": "07:00", "end": "17:00", "days": [1, 2, 3, 4, 5], "interval_minutes": 30, "model": "sonnet", "question_toast_hours": 2 },
  "later_webhook": "https://hooks.slack.com/services/…",
  "autostart": true
}
```

- `monitor`: `"primary"`, an index (0 is the leftmost monitor), or a device name such as `"\\\\.\\DISPLAY2"`. Device names can change when you plug a monitor in again, so use `"primary"` or an index. If the monitor is not connected, the panel uses the primary monitor.
- `edge`: `"left"` or `"right"`.
- `width`: logical pixels at 100% scaling. The app multiplies it by the DPI scale of the target monitor.
- `hotkey`: opens the capture popup.
- `focus_hotkey`: gives the panel keyboard focus, and expands and shows it if necessary.
- `port`: localhost port for the Claude Code hooks.
- `context_denylist`: the capture popup never reads windows of these processes. `*` matches any text.
- `inbox`: when and how the Inbox check runs. `days` uses 1 for Monday and 7 for Sunday. Set `claude_path` if Margin cannot find `claude.exe`.
- `later_webhook`: a Slack incoming webhook for the **Later** action. Do not commit this URL anywhere.
- `autostart`: start with Windows. If you leave it out, it is on for installed builds and off for dev builds.

The app stores items in `%APPDATA%\com.petterharsem.margin\margin.db`.

## Capture

Type one line and press Enter. An optional time at the end sets a timer: `30m`, `2h`, `1h30m`, or `14:30` (today, or tomorrow if the time is past). Esc cancels.

The popup suggests up to 3 lines from the window you were in when you pressed the hotkey:

- **Tab:** the page in Chrome or Edge, with its URL. A GitHub pull request shows as `PR #412 backend: Fix credit rollover`.
- **Window:** the app window, for example `Slack: tech-on-call` or the VS Code workspace name.
- **Clipboard:** a URL or one short line that you copied in the last 60 seconds.

Press Down to select a suggestion. If the input is empty, the suggestion becomes the text, and you can add a time such as ` 30m`. If you typed something first, your text stays and the item gets the link of the suggestion. Press Up to go back to your own text. Items with a link show a link icon in the panel. Click it, or press O, to open the link.

The popup reads only the window title, the process name, the address bar and the clipboard. It does not store any of this, unless you save an item with a suggestion. It reads nothing from incognito or InPrivate windows, from processes in `context_denylist`, or from clipboard entries that a password manager marks as private.

## Panel keys

| Key | Action |
|---|---|
| Up / Down | Select an item |
| Enter or D | Done |
| 1 / 2 / 3 | Snooze +10 min / +30 min / +2 h |
| E | Edit |
| Delete | Delete |
| Esc | Give focus back to the previous window |

## Claude Code sessions

The panel shows a lane with your Claude Code sessions above the follow-ups. To connect Claude Code, click **Install Claude Code hooks** in the tray menu. Margin backs up `~/.claude/settings.json`, then adds its hooks to the file. Your other hooks stay as they are. Sessions that start after the install use the new hooks.

| State | When |
|---|---|
| Running | You send a prompt, or a tool finishes after a permission prompt |
| Needs input | Claude Code shows a permission prompt or an MCP input form |
| Done | Claude stops, or you interrupt it with Esc |

Click a session, or select it and press Enter, to go to its window. Margin finds the Windows Terminal tab by the session title, selects the tab and brings the window to the front. For a session in the Claude desktop app, Margin brings the Claude window to the front. A session with no title yet cannot be matched.

A session leaves the lane when it ends, when you mark it reviewed (R), or when its transcript does not change for 12 hours. The time under the state shows how long the session waited for you. It turns amber after 5 minutes and red after 15 minutes. A permission prompt gives a toast at once. A finished reply gives a toast after 30 seconds with no new prompt.

The hooks run `curl` in the background (`"async": true`), so Claude does not wait for them. If Margin is not running, they fail with no message. Set the environment variable `MARGIN_IGNORE=1` for sessions that you do not want in the lane, for example `claude -p` scripts. The hooks post to `127.0.0.1` on the port in `config.json` (`"port"`, default 47811). If you change the port, install the hooks again.

## Inbox

The Inbox lane shows things that may need a reply. Margin checks every 30 minutes, from 07:00 to 17:00 on weekdays. It skips the check when the screen is locked or nobody used the PC for 30 minutes. Tray > **Check inbox now** runs a check at once.

| Source | What | How |
|---|---|---|
| GitHub | Open pull requests that ask for your review | `gh search prs --review-requested=@me`, no model |
| Slack | Direct questions to you with no reply, from the last 2 working days | `claude -p` with read-only Slack tools |
| ClickUp | Your tasks that are due today or overdue | `claude -p` with read-only ClickUp tools |

An entry goes away when a new check no longer finds it. A dismissed entry does not come back. A Slack question with no reply for 2 hours gives one toast, and a click on the toast opens the message.

Keys on a selected entry: Enter or O opens it, F makes a follow-up, L posts it to Slack for later, D or Delete dismisses it. L also works on follow-ups.

**Later** posts the text and the link to a Slack incoming webhook. To set it up, create a Slack app with an incoming webhook for `#petter-brain`, then put the webhook URL in `later_webhook`.

Each check runs one `claude -p` call, with `MARGIN_IGNORE=1` so that it does not show in the sessions lane. The log shows its duration and cost.

## AppBar release

The app must release the reserved space when it stops. It does this in these cases:

- A normal exit or a destroyed window
- A panic or an unhandled exception
- Ctrl+C or a closed console

A detached watchdog copy of the executable also releases the space when something kills the app. Examples are Task Manager, `taskkill /F`, and `tauri dev` restarts.

## License

MIT. See `LICENSE`.
