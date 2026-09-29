//! Small Win32 helpers for focus, monitor geometry and opening files.

use std::path::Path;
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, IsWindow, SetForegroundWindow, SW_SHOWNORMAL,
};

/// True if the screen is locked, or nobody touched the keyboard or mouse for `idle_ms`.
pub fn user_is_away(idle_ms: u32) -> bool {
    let fg = foreground_window();
    if fg == 0 || crate::focus::process_name(HWND(fg as _)).eq_ignore_ascii_case("LockApp.exe") {
        return true;
    }
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    unsafe { GetLastInputInfo(&mut info) }.as_bool() && unsafe { GetTickCount() }.wrapping_sub(info.dwTime) > idle_ms
}

pub fn foreground_window() -> isize {
    unsafe { GetForegroundWindow() }.0 as isize
}

pub fn set_foreground_window(hwnd: isize) {
    let hwnd = HWND(hwnd as _);
    unsafe {
        if hwnd.0.is_null() || !IsWindow(Some(hwnd)).as_bool() {
            return;
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Work area of the monitor that holds `hwnd`, or of the monitor under the cursor.
pub fn work_area_for(hwnd: isize) -> Option<RECT> {
    unsafe {
        let monitor = if hwnd != 0 {
            MonitorFromWindow(HWND(hwnd as _), MONITOR_DEFAULTTONEAREST)
        } else {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
        };
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetMonitorInfoW(monitor, &mut info).as_bool().then_some(info.rcWork)
    }
}

/// Opens a web URL in the default browser. Other schemes are ignored.
pub fn open_url(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(url), None, None, SW_SHOWNORMAL);
    }
}

/// Opens a file with its default app, or with Notepad if no app is associated.
pub fn open_file(path: &Path) {
    let result = unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(path.as_os_str()), None, None, SW_SHOWNORMAL)
    };
    // ShellExecute returns a value of 32 or less on failure.
    if result.0 as isize <= 32 {
        let _ = std::process::Command::new("notepad.exe").arg(path).spawn();
    }
}
