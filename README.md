# Margin

A Windows 11 panel that docks to a screen edge as an AppBar. It holds short-lived follow-ups with timers. `SPEC.md` gives the plan.

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

## Config

The app reads `%APPDATA%\com.petterharsem.margin\config.json` at startup and creates it with defaults if it is missing. Tray > Settings opens the file. The app applies changes when you save the file.

```json
{
  "monitor": "primary",
  "edge": "right",
  "width": 320,
  "hotkey": "Ctrl+Alt+Space",
  "focus_hotkey": "Ctrl+Alt+N",
  "autostart": true
}
```

- `monitor`: `"primary"`, an index (0 is the leftmost monitor), or a device name such as `"\\\\.\\DISPLAY2"`. Device names can change when you plug a monitor in again, so use `"primary"` or an index. If the monitor is not connected, the panel uses the primary monitor.
- `edge`: `"left"` or `"right"`.
- `width`: logical pixels at 100% scaling. The app multiplies it by the DPI scale of the target monitor.
- `hotkey`: opens the capture popup.
- `focus_hotkey`: gives the panel keyboard focus, and expands and shows it if necessary.
- `autostart`: start with Windows. If you leave it out, it is on for installed builds and off for dev builds.

The app stores items in `%APPDATA%\com.petterharsem.margin\margin.db`.

## Capture

Type one line and press Enter. An optional time at the end sets a timer: `30m`, `2h`, `1h30m`, or `14:30` (today, or tomorrow if the time is past). Esc cancels.

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

## AppBar release

The app must release the reserved space when it stops. It does this in these cases:

- A normal exit or a destroyed window
- A panic or an unhandled exception
- Ctrl+C or a closed console

A detached watchdog copy of the executable also releases the space when something kills the app. Examples are Task Manager, `taskkill /F`, and `tauri dev` restarts.

## License

MIT. See `LICENSE`.
