//! Foreground-window capture: HWND, pid, process name, title.

use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GW_HWNDNEXT, GWL_EXSTYLE, GetForegroundWindow, GetWindow, GetWindowLongW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow, WS_EX_TOOLWINDOW,
};
use windows::core::PWSTR;

use crate::{FocusInfo, InsertError, WindowId};

/// Snapshots the current foreground window. Our own windows (overlay pill,
/// settings) are never a dictation target: when the user starts or stops a
/// session by clicking the pill, the pill itself is foreground — the real
/// target is the window underneath, found by walking down the z-order.
pub fn capture() -> Result<FocusInfo, InsertError> {
    let mut hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return Err(InsertError::NoFocus);
    }
    let own_pid = unsafe { GetCurrentProcessId() };
    if pid_of(hwnd) == Some(own_pid) {
        hwnd = next_foreign_window(hwnd, own_pid)?;
        tracing::debug!("own window was foreground; targeting the window beneath it");
    }
    snapshot(hwnd)
}

/// Brings `window` to the foreground (used to hand focus back to the target
/// when the user's stop-click left our pill focused). Succeeds only when the
/// caller currently owns the foreground, which is exactly that case.
pub fn activate(window: WindowId) -> bool {
    let hwnd = HWND(window.0 as *mut core::ffi::c_void);
    unsafe { SetForegroundWindow(hwnd) }.as_bool()
}

/// First visible, titled, non-tool window below `from` in the z-order that
/// does not belong to `own_pid`.
fn next_foreign_window(from: HWND, own_pid: u32) -> Result<HWND, InsertError> {
    let mut hwnd = from;
    loop {
        hwnd = match unsafe { GetWindow(hwnd, GW_HWNDNEXT) } {
            Ok(next) if !next.is_invalid() => next,
            _ => return Err(InsertError::NoFocus),
        };
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            continue;
        }
        if pid_of(hwnd) == Some(own_pid) {
            continue;
        }
        let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
        if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
            continue;
        }
        let mut title_buf = [0u16; 8];
        if unsafe { GetWindowTextW(hwnd, &mut title_buf) } == 0 {
            continue;
        }
        return Ok(hwnd);
    }
}

fn pid_of(hwnd: HWND) -> Option<u32> {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

fn snapshot(hwnd: HWND) -> Result<FocusInfo, InsertError> {
    let pid = pid_of(hwnd).ok_or(InsertError::NoFocus)?;
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
