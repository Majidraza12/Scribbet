//! Foreground-window capture: HWND, pid, process name, title.

use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
};
use windows::core::PWSTR;

use crate::{FocusInfo, InsertError, WindowId};

/// Snapshots the current foreground window.
pub fn capture() -> Result<FocusInfo, InsertError> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return Err(InsertError::NoFocus);
    }

    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return Err(InsertError::NoFocus);
    }

    let mut title_buf = [0u16; 512];
    let title_len = unsafe { GetWindowTextW(hwnd, &mut title_buf) } as usize;
    let title = String::from_utf16_lossy(&title_buf[..title_len.min(title_buf.len())]);

    Ok(FocusInfo {
        window: WindowId(hwnd.0 as isize),
        pid,
        process: process_name(pid).unwrap_or_default(),
        title,
    })
}

/// Lowercase executable name for a pid (`"notepad.exe"`).
fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        result.ok()?;
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        Some(
            full.rsplit(['\\', '/'])
                .next()
                .unwrap_or(&full)
                .to_lowercase(),
        )
    }
}

/// Re-exposed for the harness test: is this HWND still alive?
#[allow(dead_code)]
pub fn window_alive(window: WindowId) -> bool {
    let hwnd = HWND(window.0 as *mut core::ffi::c_void);
    unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(hwnd)) }.as_bool()
}
