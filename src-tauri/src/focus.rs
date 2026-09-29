//! Brings the window that holds a Claude Code session to the front.
//!
//! Claude Code sets the terminal title to the session title, with a status symbol in front.
//! Windows Terminal exposes its tabs through UI Automation, so the tab is found by that title.

use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, MAX_PATH};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationSelectionItemPattern, TreeScope_Descendants,
    UIA_ControlTypePropertyId, UIA_SelectionItemPatternId, UIA_TabItemControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SetForegroundWindow, ShowWindow, SW_RESTORE,
};

const TERMINAL_CLASS: &str = "CASCADIA_HOSTING_WINDOW_CLASS";

/// Call from a worker thread. UI Automation calls into other processes and can block.
pub fn focus_session(title: Option<&str>, entrypoint: Option<&str>) -> Result<(), String> {
    if let Some(title) = title.map(str::trim).filter(|t| !t.is_empty()) {
        if focus_terminal_tab(title)? {
            return Ok(());
        }
        if entrypoint == Some("cli") {
            return Err(format!("No Windows Terminal tab has the title \"{title}\"."));
        }
    } else if entrypoint == Some("cli") {
        return Err("The session has no title yet, so Margin cannot find its tab.".into());
    }
    let windows = top_level_windows(|hwnd| process_name(hwnd).eq_ignore_ascii_case("claude.exe"));
    match windows.first() {
        Some(&hwnd) => {
            bring_to_front(hwnd);
            Ok(())
        }
        None => Err("Margin cannot find the window of this session.".into()),
    }
}

fn focus_terminal_tab(title: &str) -> Result<bool, String> {
    let terminals = top_level_windows(|hwnd| class_name(hwnd) == TERMINAL_CLASS);
    if terminals.is_empty() {
        return Ok(false);
    }
    unsafe {
        // The worker thread is new, so this cannot clash with an existing apartment.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        let is_tab = uia
            .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_TabItemControlTypeId.0))
            .map_err(|e| e.to_string())?;
        for hwnd in terminals {
            let Ok(window) = uia.ElementFromHandle(hwnd) else { continue };
            let Ok(tabs) = window.FindAll(TreeScope_Descendants, &is_tab) else { continue };
            for i in 0..tabs.Length().unwrap_or(0) {
                let Ok(tab) = tabs.GetElement(i) else { continue };
                let name = tab.CurrentName().map(|n| n.to_string()).unwrap_or_default();
                if strip_status(&name) != title {
                    continue;
                }
                if let Ok(pattern) = tab.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId) {
                    let _ = pattern.Select();
                }
                bring_to_front(hwnd);
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Removes the status symbol (spinner, ✳, ◐) that Claude Code puts before the title.
fn strip_status(name: &str) -> &str {
    name.trim_start_matches(|c: char| !c.is_alphanumeric() && !"([{\"'`#/.~".contains(c)).trim()
}

fn bring_to_front(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

fn top_level_windows(filter: impl Fn(HWND) -> bool) -> Vec<HWND> {
    unsafe extern "system" fn callback(hwnd: HWND, data: LPARAM) -> BOOL {
        let list = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
        if unsafe { IsWindowVisible(hwnd) }.as_bool() && unsafe { GetWindowTextLengthW(hwnd) } > 0 {
            list.push(hwnd);
        }
        BOOL(1)
    }
    let mut all: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(callback), LPARAM(&mut all as *mut _ as isize));
    }
    all.into_iter().filter(|&h| filter(h)).collect()
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

fn process_name(hwnd: HWND) -> String {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return String::new();
    };
    let mut buf = [0u16; MAX_PATH as usize];
    let mut len = buf.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len)
    };
    unsafe {
        let _ = CloseHandle(process);
    }
    if ok.is_err() {
        return String::new();
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    path.rsplit('\\').next().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::strip_status;

    #[test]
    fn strips_status_symbols() {
        assert_eq!(strip_status("✳ SPEC.md step 0 implementation"), "SPEC.md step 0 implementation");
        assert_eq!(strip_status("◐ Fix login"), "Fix login");
        assert_eq!(strip_status("⠂ Fix login"), "Fix login");
        assert_eq!(strip_status("Fix login"), "Fix login");
    }
}
