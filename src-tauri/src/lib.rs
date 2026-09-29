#[cfg(windows)]
mod appbar;
mod config;
#[cfg(windows)]
mod focus;
mod hooks_install;
mod server;
#[cfg(windows)]
mod shell;
mod state;
mod tray;

use chrono::Local;
use config::Config;
use serde::Serialize;
use state::sessions::Status;
use state::{Core, Error, Event, Item, Parsed, Session};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WindowEvent};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tauri_plugin_notification::NotificationExt;

const PANEL: &str = "main";
const CAPTURE: &str = "capture";
const COLLAPSED_KEY: &str = "collapsed";
const PURGE_EVERY: Duration = Duration::from_secs(60 * 60);

struct AppState {
    core: Arc<Core>,
    config_path: PathBuf,
    config: Mutex<Config>,
    config_error: Mutex<Option<String>>,
    hotkey_errors: Mutex<Vec<String>>,
    visible: AtomicBool,
    collapsed: AtomicBool,
    /// The window that had focus before a hotkey took it, so focus can go back.
    previous_foreground: AtomicIsize,
    editing: Mutex<Option<i64>>,
    server: Mutex<Option<server::Server>>,
    server_error: Mutex<Option<String>>,
}

fn app_state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn err(e: impl std::fmt::Display) -> Error {
    Error(e.to_string())
}

#[derive(Clone, Serialize)]
struct Snapshot {
    sessions: Vec<Session>,
    open: Vec<Item>,
    done_today: Vec<Item>,
    collapsed: bool,
    errors: Vec<String>,
    hotkey: String,
    focus_hotkey: String,
}

fn snapshot(app: &AppHandle) -> Result<Snapshot, Error> {
    let st = app_state(app);
    let items = st.core.items(Local::now())?;
    let mut errors: Vec<String> = lock(&st.config_error).iter().cloned().collect();
    errors.extend(lock(&st.hotkey_errors).iter().cloned());
    errors.extend(lock(&st.server_error).iter().cloned());
    let config = lock(&st.config).clone();
    Ok(Snapshot {
        sessions: st.core.sessions()?,
        open: items.open,
        done_today: items.done_today,
        collapsed: st.collapsed.load(Ordering::SeqCst),
        errors,
        hotkey: config.hotkey,
        focus_hotkey: config.focus_hotkey,
    })
}

fn emit_snapshot(app: &AppHandle) {
    match snapshot(app) {
        Ok(s) => {
            let _ = app.emit_to(PANEL, "snapshot", s);
        }
        Err(e) => eprintln!("[snapshot] {e}"),
    }
}

#[tauri::command]
fn get_snapshot(app: AppHandle) -> Result<Snapshot, Error> {
    snapshot(&app)
}

#[tauri::command]
fn parse_capture(app: AppHandle, text: String) -> Parsed {
    app_state(&app).core.parse(&text, Local::now())
}

#[tauri::command]
fn submit_capture(app: AppHandle, text: String) -> Result<(), Error> {
    let st = app_state(&app);
    let editing = *lock(&st.editing);
    match editing {
        Some(id) => st.core.edit(id, &text, Local::now())?,
        None => st.core.add(&text, Local::now())?,
    };
    hide_capture(&app, true);
    Ok(())
}

#[tauri::command]
fn cancel_capture(app: AppHandle) {
    hide_capture(&app, true);
}

#[tauri::command]
fn open_capture(app: AppHandle) -> Result<(), Error> {
    show_capture(&app, None)
}

#[tauri::command]
fn edit_item(app: AppHandle, id: i64) -> Result<(), Error> {
    show_capture(&app, Some(id))
}

#[tauri::command]
fn done_item(app: AppHandle, id: i64) -> Result<(), Error> {
    app_state(&app).core.done(id, Local::now())
}

#[tauri::command]
fn reopen_item(app: AppHandle, id: i64) -> Result<(), Error> {
    app_state(&app).core.reopen(id)
}

#[tauri::command]
fn snooze_item(app: AppHandle, id: i64, minutes: i64) -> Result<(), Error> {
    app_state(&app).core.snooze(id, minutes, Local::now())
}

#[tauri::command]
fn delete_item(app: AppHandle, id: i64) -> Result<(), Error> {
    app_state(&app).core.delete(id)
}

#[tauri::command]
fn review_session(app: AppHandle, id: String) -> Result<(), Error> {
    app_state(&app).core.review(&id)
}

#[tauri::command]
async fn focus_session(app: AppHandle, id: String) -> Result<(), Error> {
    let session = app_state(&app).core.session(&id)?;
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(move || {
            focus::focus_session(session.title.as_deref(), session.entrypoint.as_deref())
        })
        .await
        .map_err(err)?
        .map_err(Error)
    }
    #[cfg(not(windows))]
    {
        let _ = session;
        Ok(())
    }
}

#[tauri::command]
fn set_collapsed(app: AppHandle, collapsed: bool) {
    apply_collapsed(&app, collapsed);
}

#[tauri::command]
fn release_focus(app: AppHandle) {
    #[cfg(windows)]
    shell::set_foreground_window(app_state(&app).previous_foreground.load(Ordering::SeqCst));
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

#[cfg(windows)]
#[tauri::command]
fn appbar_state() -> Option<appbar::AppBarState> {
    appbar::state()
}

fn own_hwnds(app: &AppHandle) -> Vec<isize> {
    [PANEL, CAPTURE]
        .iter()
        .filter_map(|l| app.get_webview_window(l))
        .filter_map(|w| w.hwnd().ok())
        .map(|h| h.0 as isize)
        .collect()
}

#[cfg(windows)]
fn remember_foreground(app: &AppHandle) -> isize {
    let fg = shell::foreground_window();
    if !own_hwnds(app).contains(&fg) {
        app_state(app).previous_foreground.store(fg, Ordering::SeqCst);
    }
    app_state(app).previous_foreground.load(Ordering::SeqCst)
}

fn show_capture(app: &AppHandle, edit: Option<i64>) -> Result<(), Error> {
    let st = app_state(app);
    let text = match edit {
        Some(id) => st.core.edit_text(id)?,
        None => String::new(),
    };
    *lock(&st.editing) = edit;
    let window = app.get_webview_window(CAPTURE).ok_or_else(|| err("capture window missing"))?;

    #[cfg(windows)]
    {
        let target = remember_foreground(app);
        if let Some(work) = shell::work_area_for(target) {
            // Moving to a monitor with another DPI resizes the window, so centre a second time.
            for _ in 0..2 {
                let size = window.outer_size().map_err(err)?;
                let x = work.left + (work.right - work.left - size.width as i32) / 2;
                let y = work.top + (work.bottom - work.top - size.height as i32) / 3;
                window.set_position(PhysicalPosition::new(x, y)).map_err(err)?;
            }
        }
    }

    let _ = app.emit_to(CAPTURE, "capture-open", text);
    window.show().map_err(err)?;
    window.set_focus().map_err(err)?;
    Ok(())
}

fn hide_capture(app: &AppHandle, restore_focus: bool) {
    let Some(window) = app.get_webview_window(CAPTURE) else { return };
    if !window.is_visible().unwrap_or(false) {
        return;
    }
    let _ = window.hide();
    *lock(&app_state(app).editing) = None;
    #[cfg(windows)]
    if restore_focus {
        shell::set_foreground_window(app_state(app).previous_foreground.load(Ordering::SeqCst));
    }
}

fn focus_panel(app: &AppHandle) {
    #[cfg(windows)]
    remember_foreground(app);
    let st = app_state(app);
    if !st.visible.load(Ordering::SeqCst) {
        set_panel_visible(app, true);
    }
    if st.collapsed.load(Ordering::SeqCst) {
        apply_collapsed(app, false);
    }
    if let Some(panel) = app.get_webview_window(PANEL) {
        let _ = panel.set_focus();
    }
    let _ = app.emit_to(PANEL, "panel-focus", ());
}

fn set_panel_visible(app: &AppHandle, visible: bool) {
    let st = app_state(app);
    st.visible.store(visible, Ordering::SeqCst);
    if let Some(panel) = app.get_webview_window(PANEL) {
        if visible {
            let _ = panel.show();
            #[cfg(windows)]
            appbar::show();
        } else {
            #[cfg(windows)]
            appbar::hide();
            let _ = panel.hide();
        }
    }
    tray::sync(app, visible, st.collapsed.load(Ordering::SeqCst));
}

fn apply_collapsed(app: &AppHandle, collapsed: bool) {
    let st = app_state(app);
    st.collapsed.store(collapsed, Ordering::SeqCst);
    if let Err(e) = st.core.set_meta(COLLAPSED_KEY, if collapsed { "true" } else { "false" }) {
        eprintln!("[collapse] {e}");
    }
    #[cfg(windows)]
    appbar::set_collapsed(collapsed);
    tray::sync(app, st.visible.load(Ordering::SeqCst), collapsed);
    emit_snapshot(app);
}

pub(crate) fn toggle_panel(app: &AppHandle) {
    let visible = app_state(app).visible.load(Ordering::SeqCst);
    set_panel_visible(app, !visible);
}

pub(crate) fn toggle_collapsed(app: &AppHandle) {
    let collapsed = app_state(app).collapsed.load(Ordering::SeqCst);
    apply_collapsed(app, !collapsed);
}

pub(crate) fn open_settings(app: &AppHandle) {
    #[cfg(windows)]
    shell::open_file(&app_state(app).config_path);
}

fn on_shortcut(app: &AppHandle, shortcut: &Shortcut) {
    let config = lock(&app_state(app).config).clone();
    if config.hotkey.parse::<Shortcut>().ok().as_ref() == Some(shortcut) {
        if let Err(e) = show_capture(app, None) {
            eprintln!("[capture] {e}");
        }
    } else if config.focus_hotkey.parse::<Shortcut>().ok().as_ref() == Some(shortcut) {
        focus_panel(app);
    }
}

fn register_hotkeys(app: &AppHandle) {
    let config = lock(&app_state(app).config).clone();
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();
    let mut errors = Vec::new();
    for (field, key) in [("hotkey", &config.hotkey), ("focus_hotkey", &config.focus_hotkey)] {
        match key.parse::<Shortcut>() {
            Ok(shortcut) => {
                if let Err(e) = shortcuts.register(shortcut) {
                    eprintln!("[hotkey] {key}: {e}");
                    errors.push(format!("{key} is in use by another app. Change \"{field}\" in Settings."));
                }
            }
            Err(e) => errors.push(format!("\"{field}\": {key} is not a valid hotkey ({e}).")),
        }
    }
    *lock(&app_state(app).hotkey_errors) = errors;
}

fn apply_autostart(app: &AppHandle) {
    let want = lock(&app_state(app).config).autostart();
    let launcher = app.autolaunch();
    if launcher.is_enabled().ok() == Some(want) {
        return;
    }
    let result = if want { launcher.enable() } else { launcher.disable() };
    if let Err(e) = result {
        eprintln!("[autostart] {e}");
    }
}

fn reload_config(app: &AppHandle) {
    let st = app_state(app);
    match Config::load(&st.config_path) {
        Ok(config) => {
            eprintln!("[config] reloaded {config:?}");
            *lock(&st.config) = config.clone();
            *lock(&st.config_error) = None;
            start_server(app, config.port);
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || {
                #[cfg(windows)]
                appbar::update_config(config);
                register_hotkeys(&handle);
                apply_autostart(&handle);
                emit_snapshot(&handle);
            });
        }
        Err(e) => {
            eprintln!("[config] {e}");
            *lock(&st.config_error) = Some(format!("Invalid config, keeping the previous one. {e}"));
            emit_snapshot(app);
        }
    }
}

fn notify_due(app: &AppHandle, items: &[Item]) {
    let (title, body) = match items {
        [] => return,
        [item] => ("Due now".to_string(), item.title.clone()),
        _ => (
            format!("{} items due", items.len()),
            items.iter().take(5).map(|i| i.title.as_str()).collect::<Vec<_>>().join("\n"),
        ),
    };
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        eprintln!("[notify] {e}");
    }
}

fn notify_sessions(app: &AppHandle, sessions: &[Session]) {
    let describe = |s: &Session| match s.status {
        Status::NeedsInput => format!("{} needs input", s.folder),
        _ => format!("{} is done", s.folder),
    };
    let (title, body) = match sessions {
        [] => return,
        [s] => (describe(s), s.message.clone().or_else(|| s.prompt.clone()).unwrap_or_default()),
        _ => (
            format!("{} sessions waiting", sessions.len()),
            sessions.iter().take(5).map(describe).collect::<Vec<_>>().join("
"),
        ),
    };
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        eprintln!("[notify] {e}");
    }
}

fn start_server(app: &AppHandle, port: u16) {
    let st = app_state(app);
    if lock(&st.server).as_ref().is_some_and(|s| s.port == port) {
        return;
    }
    // Drop the old server first, so it releases the port.
    *lock(&st.server) = None;
    let core = st.core.clone();
    match tauri::async_runtime::block_on(server::start(core, port)) {
        Ok(server) => {
            *lock(&st.server) = Some(server);
            *lock(&st.server_error) = None;
        }
        Err(e) => {
            eprintln!("[server] {e}");
            *lock(&st.server_error) = Some(e);
        }
    }
}

pub(crate) fn install_hooks(app: &AppHandle, install: bool) {
    let port = lock(&app_state(app).config).port;
    let result = if install { hooks_install::install(port) } else { hooks_install::uninstall() };
    let (title, body) = match result {
        Ok(backup) => (
            if install { "Claude Code hooks installed" } else { "Claude Code hooks removed" }.to_string(),
            format!("New sessions use the change. Backup: {}", backup.display()),
        ),
        Err(e) => ("Claude Code hooks not changed".to_string(), e),
    };
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        eprintln!("[notify] {e}");
    }
}

fn start_background(app: &AppHandle) {
    let core = app_state(app).core.clone();

    let handle = app.clone();
    let mut events = core.subscribe();
    std::thread::spawn(move || loop {
        match events.blocking_recv() {
            Ok(Event::Changed) => emit_snapshot(&handle),
            Ok(Event::Due(items)) => notify_due(&handle, &items),
            Ok(Event::SessionsWaiting(sessions)) => notify_sessions(&handle, &sessions),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => emit_snapshot(&handle),
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    });

    // Compare wall-clock time each second, so due times stay correct after sleep or a clock change.
    let handle = app.clone();
    std::thread::spawn(move || {
        let config_path = app_state(&handle).config_path.clone();
        let modified = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let mut config_mtime = modified(&config_path);
        let mut last_purge: Option<SystemTime> = None;
        loop {
            let now = Local::now();
            if let Err(e) = core.tick(now) {
                eprintln!("[tick] {e}");
            }
            if let Err(e) = core.check_transcripts(now).and_then(|_| core.tick_sessions(now)) {
                eprintln!("[sessions] {e}");
            }
            if last_purge.is_none_or(|t| t.elapsed().unwrap_or_default() >= PURGE_EVERY) {
                if let Err(e) = core.purge(now) {
                    eprintln!("[purge] {e}");
                }
                last_purge = Some(SystemTime::now());
            }
            let mtime = modified(&config_path);
            if mtime != config_mtime {
                config_mtime = mtime;
                reload_config(&handle);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.first().map(String::as_str) == Some(appbar::WATCHDOG_ARG) {
            appbar::run_watchdog(&args[1..]);
            return;
        }
    }

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| focus_panel(app)))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        on_shortcut(app, shortcut);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            parse_capture,
            submit_capture,
            cancel_capture,
            open_capture,
            edit_item,
            done_item,
            reopen_item,
            snooze_item,
            delete_item,
            review_session,
            focus_session,
            set_collapsed,
            release_focus,
            quit,
            #[cfg(windows)]
            appbar_state,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let config_path = app.path().app_config_dir()?.join("config.json");
            let (config, config_error) = Config::load_or_create(&config_path);
            eprintln!("[config] {} {config:?}", config_path.display());
            let core = Core::open(&app.path().app_data_dir()?.join("margin.db"))?;
            let collapsed = core.meta(COLLAPSED_KEY)?.as_deref() == Some("true");

            app.manage(AppState {
                core,
                config_path,
                config: Mutex::new(config.clone()),
                config_error: Mutex::new(config_error),
                hotkey_errors: Mutex::new(Vec::new()),
                visible: AtomicBool::new(true),
                collapsed: AtomicBool::new(collapsed),
                previous_foreground: AtomicIsize::new(0),
                editing: Mutex::new(None),
                server: Mutex::new(None),
                server_error: Mutex::new(None),
            });
            start_server(&handle, config.port);

            let panel = app.get_webview_window(PANEL).expect("panel window");
            #[cfg(windows)]
            {
                let hwnd = windows::Win32::Foundation::HWND(panel.hwnd()?.0 as _);
                appbar::install(&handle, hwnd, config, collapsed);
            }
            panel.show()?;

            tray::create(&handle, true, collapsed)?;
            register_hotkeys(&handle);
            apply_autostart(&handle);
            start_background(&handle);
            Ok(())
        })
        .on_window_event(|window, event| match (window.label(), event) {
            (CAPTURE, WindowEvent::Focused(false)) => hide_capture(window.app_handle(), false),
            (CAPTURE, WindowEvent::CloseRequested { api, .. }) => {
                api.prevent_close();
                hide_capture(window.app_handle(), true);
            }
            (PANEL, WindowEvent::CloseRequested { .. }) => window.app_handle().exit(0),
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app, event| {
        #[cfg(windows)]
        if let tauri::RunEvent::Exit = event {
            appbar::remove();
        }
    });
}
