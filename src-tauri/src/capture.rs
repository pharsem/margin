//! Reads what was on screen at the capture hotkey: the foreground window, the browser URL
//! and a recent clipboard entry. Nothing here is logged or stored.

use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use windows::core::w;
use windows::Win32::Foundation::{HGLOBAL, HWND};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationValuePattern, TreeScope_Descendants, UIA_ControlTypePropertyId,
    UIA_EditControlTypeId, UIA_ToolBarControlTypeId, UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;

const CF_UNICODETEXT: u32 = 13;
const CLIPBOARD_RECENT_MS: i64 = 60_000;
const BROWSERS: &[&str] = &["chrome.exe", "msedge.exe"];

pub fn window_title(hwnd: isize) -> String {
    let mut buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(HWND(hwnd as _), &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

pub fn is_browser(process: &str) -> bool {
    BROWSERS.iter().any(|b| b.eq_ignore_ascii_case(process))
}

/// Tracks when the clipboard last changed. The sequence number changes on every copy.
#[derive(Default)]
pub struct ClipboardWatch {
    sequence: AtomicU32,
    changed_at: AtomicI64,
}

impl ClipboardWatch {
    pub fn poll(&self, now_ms: i64) {
        let seq = unsafe { GetClipboardSequenceNumber() };
        if self.sequence.swap(seq, Ordering::SeqCst) != seq {
            // The first poll after startup is not a real change.
            let first = self.changed_at.load(Ordering::SeqCst) == 0;
            self.changed_at.store(if first { 1 } else { now_ms }, Ordering::SeqCst);
        }
    }

    /// Clipboard text if it changed within the last minute, and the source app allows monitoring.
    pub fn recent_text(&self, now_ms: i64) -> Option<String> {
        self.poll(now_ms);
        if now_ms - self.changed_at.load(Ordering::SeqCst) > CLIPBOARD_RECENT_MS {
            return None;
        }
        read_clipboard_text()
    }
}

fn read_clipboard_text() -> Option<String> {
    unsafe {
        OpenClipboard(None).ok()?;
        let text = (|| {
            // Password managers set these formats to keep their entries out of clipboard tools.
            let exclude = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
            if IsClipboardFormatAvailable(exclude).is_ok() {
                return None;
            }
            let history = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
            if IsClipboardFormatAvailable(history).is_ok() {
                let allowed = global_bytes(history).is_some_and(|b| b.len() >= 4 && b[..4] != [0, 0, 0, 0]);
                if !allowed {
                    return None;
                }
            }
            let bytes = global_bytes(CF_UNICODETEXT)?;
            let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
            Some(String::from_utf16_lossy(&units[..end]))
        })();
        let _ = CloseClipboard();
        text
    }
}

/// Copies the bytes of one clipboard format. The clipboard must be open.
unsafe fn global_bytes(format: u32) -> Option<Vec<u8>> {
    let handle = unsafe { GetClipboardData(format) }.ok()?;
    let global = HGLOBAL(handle.0);
    let size = unsafe { GlobalSize(global) };
    let ptr = unsafe { GlobalLock(global) } as *const u8;
    if ptr.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, size) }.to_vec();
    let _ = unsafe { GlobalUnlock(global) };
    Some(bytes)
}

/// Reads the address bar of a Chrome or Edge window. Call from a worker thread: it can be slow.
/// The address bar is the first edit field in the first toolbar. Its name is localised, so
/// it is not used.
pub fn browser_url(hwnd: isize) -> Option<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let window = uia.ElementFromHandle(HWND(hwnd as _)).ok()?;
        let toolbar_condition = uia
            .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_ToolBarControlTypeId.0))
            .ok()?;
        let edit_condition = uia
            .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_EditControlTypeId.0))
            .ok()?;
        let toolbar = window.FindFirst(TreeScope_Descendants, &toolbar_condition).ok()?;
        let edit = toolbar.FindFirst(TreeScope_Descendants, &edit_condition).ok()?;
        let value = edit.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId).ok()?;
        normalize_url(&value.CurrentValue().ok()?.to_string())
    }
}

/// Chrome hides the scheme. While the user types in the bar, the value is not a URL.
fn normalize_url(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.contains(char::is_whitespace) {
        return None;
    }
    if value.starts_with("https://") || value.starts_with("http://") {
        return Some(value.to_string());
    }
    let host = value.split(['/', '?', '#']).next().unwrap_or("");
    if !host.contains('.') || value.contains("://") {
        return None;
    }
    Some(format!("https://{value}"))
}

#[cfg(test)]
mod tests {
    use super::normalize_url;

    /// Needs a running Chrome or Edge window: `cargo test --lib -- --ignored browser_url_timing --nocapture`
    #[test]
    #[ignore]
    fn browser_url_timing() {
        let windows = crate::focus::top_level_windows(|h| super::is_browser(&crate::focus::process_name(h)));
        let hwnd = windows.first().expect("no browser window").0 as isize;
        for run in 0..3 {
            let started = std::time::Instant::now();
            let url = super::browser_url(hwnd);
            let host = url.as_deref().and_then(|u| u.split('/').nth(2)).unwrap_or("-").to_string();
            println!("run {run}: {} ms, host {host}", started.elapsed().as_millis());
        }
    }

    #[test]
    fn urls_from_the_address_bar() {
        assert_eq!(normalize_url("github.com/a/b/pull/1").as_deref(), Some("https://github.com/a/b/pull/1"));
        assert_eq!(normalize_url("http://localhost.test:1420/").as_deref(), Some("http://localhost.test:1420/"));
        assert_eq!(normalize_url("how to rust"), None);
        assert_eq!(normalize_url("chrome://settings"), None);
        assert_eq!(normalize_url("localhost"), None);
    }
}
