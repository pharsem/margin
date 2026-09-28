//! Registers the main window as a Windows AppBar so the shell reserves screen space for it.
//!
//! All coordinates are physical pixels. The process is per-monitor DPI aware (v2) through tao.

use crate::config::{Config, Edge, MonitorSelector};
use serde::Serialize;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter};
use windows::core::{w, BOOL};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::Diagnostics::Debug::{SetUnhandledExceptionFilter, EXCEPTION_POINTERS};
use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::{
    DefSubclassProc, RemoveWindowSubclass, SHAppBarMessage, SetWindowSubclass, ABE_LEFT, ABE_RIGHT,
    ABM_ACTIVATE, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, ABM_WINDOWPOSCHANGED,
    ABN_FULLSCREENAPP, ABN_POSCHANGED, APPBARDATA,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, RegisterWindowMessageW, SetWindowPos, HWND_NOTOPMOST, HWND_TOPMOST,
    SWP_NOACTIVATE, WM_ACTIVATE, WM_APP, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_ENDSESSION, WM_WINDOWPOSCHANGED,
};

const CALLBACK_MSG: u32 = WM_APP + 0x42;
const SUBCLASS_ID: usize = 1;
const COLLAPSED_WIDTH: u32 = 48;
pub const WATCHDOG_ARG: &str = "--appbar-watchdog";

static HWND_RAW: AtomicIsize = AtomicIsize::new(0);
static REGISTERED: AtomicBool = AtomicBool::new(false);
static FULLSCREEN: AtomicBool = AtomicBool::new(false);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static VISIBLE: AtomicBool = AtomicBool::new(true);
static COLLAPSED: AtomicBool = AtomicBool::new(false);
static CONFIG: Mutex<Option<Config>> = Mutex::new(None);
static APP: OnceLock<AppHandle> = OnceLock::new();
static STATE: Mutex<Option<AppBarState>> = Mutex::new(None);

thread_local! {
    static IN_REPOSITION: Cell<bool> = const { Cell::new(false) };
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppBarState {
    pub monitor: String,
    pub edge: Edge,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub dpi: u32,
}

pub fn state() -> Option<AppBarState> {
    STATE.lock().ok().and_then(|s| s.clone())
}

struct Monitor {
    handle: HMONITOR,
    name: String,
    rect: RECT,
    work: RECT,
    primary: bool,
}

fn appbar_data(hwnd: HWND) -> APPBARDATA {
    APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        hWnd: hwnd,
        ..Default::default()
    }
}

fn current_hwnd() -> HWND {
    HWND(HWND_RAW.load(Ordering::SeqCst) as _)
}

pub fn install(app: &AppHandle, hwnd: HWND, config: Config, collapsed: bool) {
    set_config_value(config);
    COLLAPSED.store(collapsed, Ordering::SeqCst);
    let _ = APP.set(app.clone());
    HWND_RAW.store(hwnd.0 as isize, Ordering::SeqCst);
    TASKBAR_CREATED.store(unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }, Ordering::SeqCst);

    install_exit_guards();
    for m in enumerate_monitors() {
        eprintln!(
            "[appbar] monitor {} primary={} rect=({},{})-({},{})",
            m.name, m.primary, m.rect.left, m.rect.top, m.rect.right, m.rect.bottom
        );
    }

    unsafe {
        if !SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0).as_bool() {
            eprintln!("[appbar] SetWindowSubclass failed");
        }
    }
    register(hwnd);
    reposition(hwnd);
    spawn_watchdog(hwnd);
}

fn set_config_value(config: Config) {
    if let Ok(mut c) = CONFIG.lock() {
        *c = Some(config);
    }
}

// The functions below must run on the main thread, which owns the window.

pub fn update_config(config: Config) {
    set_config_value(config);
    reposition(current_hwnd());
}

pub fn set_collapsed(collapsed: bool) {
    COLLAPSED.store(collapsed, Ordering::SeqCst);
    reposition(current_hwnd());
}

/// Releases the reserved space. The caller hides the window.
pub fn hide() {
    VISIBLE.store(false, Ordering::SeqCst);
    remove();
}

/// Reserves the space again. The caller shows the window.
pub fn show() {
    VISIBLE.store(true, Ordering::SeqCst);
    if !REGISTERED.load(Ordering::SeqCst) {
        register(current_hwnd());
    }
    reposition(current_hwnd());
}

fn register(hwnd: HWND) {
    let mut abd = appbar_data(hwnd);
    abd.uCallbackMessage = CALLBACK_MSG;
    if unsafe { SHAppBarMessage(ABM_NEW, &mut abd) } != 0 {
        REGISTERED.store(true, Ordering::SeqCst);
    } else {
        eprintln!("[appbar] ABM_NEW failed");
    }
}

/// Idempotent and safe to call from any thread, including panic and console handlers.
pub fn remove() {
    if REGISTERED.swap(false, Ordering::SeqCst) {
        remove_hwnd(current_hwnd());
    }
}

fn remove_hwnd(hwnd: HWND) {
    let mut abd = appbar_data(hwnd);
    unsafe { SHAppBarMessage(ABM_REMOVE, &mut abd) };
}

fn reposition(hwnd: HWND) {
    if IN_REPOSITION.get() || !REGISTERED.load(Ordering::SeqCst) {
        return;
    }
    IN_REPOSITION.set(true);
    reposition_inner(hwnd);
    IN_REPOSITION.set(false);
}

fn reposition_inner(hwnd: HWND) {
    let Some(config) = CONFIG.lock().ok().and_then(|c| c.clone()) else { return };
    let monitors = enumerate_monitors();
    let Some(monitor) = resolve_monitor(&config.monitor, &monitors) else {
        eprintln!("[appbar] no monitors found");
        return;
    };

    let dpi = monitor_dpi(monitor.handle);
    let logical = if COLLAPSED.load(Ordering::SeqCst) { COLLAPSED_WIDTH } else { config.width };
    let width = (logical * dpi).div_ceil(96) as i32;

    let mut abd = appbar_data(hwnd);
    abd.uEdge = match config.edge {
        Edge::Left => ABE_LEFT,
        Edge::Right => ABE_RIGHT,
    };
    // Take the height from the work area so a bottom or top taskbar is not covered.
    abd.rc = RECT {
        left: monitor.rect.left,
        right: monitor.rect.right,
        top: monitor.work.top,
        bottom: monitor.work.bottom,
    };
    anchor(&mut abd.rc, config.edge, width);
    unsafe { SHAppBarMessage(ABM_QUERYPOS, &mut abd) };
    // QUERYPOS moves the anchored side past other AppBars, so the width must be restored.
    anchor(&mut abd.rc, config.edge, width);
    unsafe { SHAppBarMessage(ABM_SETPOS, &mut abd) };

    let rc = abd.rc;
    // Crossing into a monitor with a different DPI makes tao resize the window
    // from WM_DPICHANGED, so check the result and move once more if needed.
    for _ in 0..2 {
        move_window(hwnd, rc);
        let mut actual = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut actual) }.is_err() || actual == rc {
            break;
        }
    }

    let state = AppBarState {
        monitor: monitor.name.clone(),
        edge: config.edge,
        left: rc.left,
        top: rc.top,
        right: rc.right,
        bottom: rc.bottom,
        dpi,
    };
    let changed = match STATE.lock() {
        Ok(mut s) if s.as_ref() != Some(&state) => {
            *s = Some(state.clone());
            true
        }
        _ => false,
    };
    if !changed {
        return;
    }
    eprintln!("[appbar] positioned {state:?}");
    if let Some(app) = APP.get() {
        let _ = app.emit("appbar-state", state);
    }
}

fn anchor(rc: &mut RECT, edge: Edge, width: i32) {
    match edge {
        Edge::Left => rc.right = rc.left + width,
        Edge::Right => rc.left = rc.right - width,
    }
}

fn move_window(hwnd: HWND, rc: RECT) {
    let z = if FULLSCREEN.load(Ordering::SeqCst) { HWND_NOTOPMOST } else { HWND_TOPMOST };
    let _ = unsafe {
        SetWindowPos(hwnd, Some(z), rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top, SWP_NOACTIVATE)
    };
}

fn monitor_dpi(handle: HMONITOR) -> u32 {
    let (mut x, mut y) = (0u32, 0u32);
    match unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut x, &mut y) } {
        Ok(()) if x > 0 => x,
        _ => 96,
    }
}

fn enumerate_monitors() -> Vec<Monitor> {
    unsafe extern "system" fn callback(handle: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(handle, &mut info as *mut _ as *mut MONITORINFO) }.as_bool() {
            let len = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
            list.push(Monitor {
                handle,
                name: String::from_utf16_lossy(&info.szDevice[..len]),
                rect: info.monitorInfo.rcMonitor,
                work: info.monitorInfo.rcWork,
                primary: info.monitorInfo.dwFlags & 1 != 0,
            });
        }
        BOOL(1)
    }

    let mut list: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(callback), LPARAM(&mut list as *mut _ as isize));
    }
    list.sort_by_key(|m| (m.rect.left, m.rect.top));
    list
}

fn resolve_monitor<'a>(selector: &MonitorSelector, monitors: &'a [Monitor]) -> Option<&'a Monitor> {
    let primary = || monitors.iter().find(|m| m.primary).or(monitors.first());
    let found = match selector {
        MonitorSelector::Index(i) => monitors.get(*i),
        MonitorSelector::Name(n) if n.eq_ignore_ascii_case("primary") => return primary(),
        MonitorSelector::Name(n) => monitors.iter().find(|m| m.name.eq_ignore_ascii_case(n)),
    };
    if found.is_none() {
        eprintln!("[appbar] monitor {selector:?} not connected, falling back to primary");
    }
    found.or_else(primary)
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        CALLBACK_MSG => {
            match wparam.0 as u32 {
                ABN_POSCHANGED => reposition(hwnd),
                ABN_FULLSCREENAPP => {
                    FULLSCREEN.store(lparam.0 != 0, Ordering::SeqCst);
                    let mut rc = RECT::default();
                    if unsafe { GetWindowRect(hwnd, &mut rc) }.is_ok() {
                        move_window(hwnd, rc);
                    }
                }
                _ => {}
            }
            return LRESULT(0);
        }
        WM_ACTIVATE | WM_WINDOWPOSCHANGED if REGISTERED.load(Ordering::SeqCst) => {
            let mut abd = appbar_data(hwnd);
            let m = if msg == WM_ACTIVATE { ABM_ACTIVATE } else { ABM_WINDOWPOSCHANGED };
            unsafe { SHAppBarMessage(m, &mut abd) };
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            reposition(hwnd);
            return result;
        }
        WM_ENDSESSION if wparam.0 != 0 => remove(),
        WM_DESTROY => {
            remove();
            unsafe {
                let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
            }
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::SeqCst) => {
            // Explorer restarted and forgot every AppBar.
            REGISTERED.store(false, Ordering::SeqCst);
            if VISIBLE.load(Ordering::SeqCst) {
                register(hwnd);
                reposition(hwnd);
            }
        }
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

fn install_exit_guards() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        remove();
        previous(info);
    }));

    unsafe extern "system" fn console_handler(_ctrl_type: u32) -> BOOL {
        remove();
        // FALSE lets the default handler terminate the process.
        BOOL(0)
    }
    unsafe extern "system" fn exception_filter(_: *const EXCEPTION_POINTERS) -> i32 {
        remove();
        0 // EXCEPTION_CONTINUE_SEARCH
    }
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(console_handler), true);
        SetUnhandledExceptionFilter(Some(exception_filter));
    }
}

/// A detached copy of this executable waits for the app to exit and releases the space.
/// It covers what in-process guards cannot: TerminateProcess, Task Manager, `tauri dev` restarts.
fn spawn_watchdog(hwnd: HWND) {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    let Ok(exe) = std::env::current_exe() else { return };
    let args = [WATCHDOG_ARG.to_string(), std::process::id().to_string(), (hwnd.0 as isize).to_string()];
    let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    let spawned = Command::new(&exe)
        .args(&args)
        .creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
        .or_else(|_| Command::new(&exe).args(&args).creation_flags(flags).spawn());
    if let Err(e) = spawned {
        eprintln!("[appbar] watchdog failed to start: {e}");
    }
}

pub fn run_watchdog(args: &[String]) {
    let (Some(pid), Some(hwnd)) = (
        args.first().and_then(|a| a.parse::<u32>().ok()),
        args.get(1).and_then(|a| a.parse::<isize>().ok()),
    ) else {
        return;
    };
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else { return };
        WaitForSingleObject(process, INFINITE);
    }
    remove_hwnd(HWND(hwnd as _));
}
