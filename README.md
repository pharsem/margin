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

## AppBar release

The app must release the reserved space when it stops. It does this in these cases:

- A normal exit or a destroyed window
- A panic or an unhandled exception
- Ctrl+C or a closed console

A detached watchdog copy of the executable also releases the space when something kills the app. Examples are Task Manager, `taskkill /F`, and `tauri dev` restarts.

## License

MIT. See `LICENSE`.
