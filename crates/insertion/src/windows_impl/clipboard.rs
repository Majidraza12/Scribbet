//! Tier 3: clipboard swap → Ctrl+V → restore.
//!
//! Restore policy (docs/06 T4): the previous CF_UNICODETEXT content is
//! snapshotted and restored best-effort. Non-text clipboard content (images,
//! files) cannot be round-tripped through this tier; if present it is lost
//! and a warning is logged — accepted, documented behavior for the *last
//! resort* tier only.

use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, CountClipboardFormats, EmptyClipboard, GetClipboardData,
    IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_CONTROL, VK_V,
};

use super::sendinput;

const CF_UNICODETEXT: u32 = 13;

/// Pastes `text` into the focused window via the clipboard, then restores
/// the previous text clipboard content.
pub fn paste_with_restore(text: &str, settle: Duration) -> Result<(), String> {
    let backup = with_clipboard(|| {
        let backup = read_text();
        let formats = unsafe { CountClipboardFormats() };
        if backup.is_none() && formats > 0 {
            tracing::warn!(
                formats,
                "clipboard held non-text content; it will not be restored after paste"
            );
        }
        write_text(text)?;
        Ok(backup)
    })??;

    // Ctrl+V into the focused window (modifier remnants released first).
    sendinput::release_held_modifiers();
    send_paste_chord()?;

    // Give the target time to consume WM_PASTE before we swap back.
    std::thread::sleep(settle);

    with_clipboard(|| match &backup {
        Some(prev) => write_text(prev),
        None => {
            // Nothing to restore; leave our text rather than clearing, so a
            // failed paste is still recoverable by the user with Ctrl+V.
            Ok(())
        }
    })??;
    Ok(())
}

/// Places `text` on the clipboard without pasting or restoring — the
/// recovery path when every insertion tier fails, so a session's text is
/// never lost (the user can paste it manually).
pub fn copy_only(text: &str) -> Result<(), String> {
    with_clipboard(|| write_text(text))?
}

/// Opens the clipboard with retries (other apps hold it in bursts), runs
/// `f`, always closes.
fn with_clipboard<T>(f: impl FnOnce() -> Result<T, String>) -> Result<Result<T, String>, String> {
    let mut opened = false;
    for _ in 0..10 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            opened = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    if !opened {
        return Err("clipboard is locked by another application".into());
    }
    let result = f();
    unsafe {
        let _ = CloseClipboard();
    }
    Ok(result)
}

/// Reads CF_UNICODETEXT from the (already open) clipboard.
fn read_text() -> Option<String> {
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT).ok()?;
        let hglobal = HGLOBAL(handle.0);
        let ptr = GlobalLock(hglobal) as *const u16;
        if ptr.is_null() {
            return None;
        }
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
        let _ = GlobalUnlock(hglobal);
        Some(text)
    }
}

/// Writes CF_UNICODETEXT to the (already open) clipboard.
fn write_text(text: &str) -> Result<(), String> {
    unsafe {
        EmptyClipboard().map_err(|e| format!("EmptyClipboard: {e}"))?;
        let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = units.len() * 2;
        let hglobal = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| format!("GlobalAlloc: {e}"))?;
        let ptr = GlobalLock(hglobal) as *mut u16;
        if ptr.is_null() {
            return Err("GlobalLock returned null".into());
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), ptr, units.len());
        let _ = GlobalUnlock(hglobal);
        // On success the system owns the allocation.
        SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hglobal.0)))
            .map_err(|e| format!("SetClipboardData: {e}"))?;
        Ok(())
    }
}

/// Ctrl down, V down, V up, Ctrl up.
fn send_paste_chord() -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, SendInput,
    };
    let key = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
               flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let chord = [
        key(VK_CONTROL, KEYBD_EVENT_FLAGS(0)),
        key(VK_V, KEYBD_EVENT_FLAGS(0)),
        key(VK_V, KEYEVENTF_KEYUP),
        key(VK_CONTROL, KEYEVENTF_KEYUP),
    ];
    let sent = unsafe { SendInput(&chord, std::mem::size_of::<INPUT>() as i32) };
    if sent != chord.len() as u32 {
        return Err(format!("paste chord: SendInput delivered {sent}/4 events"));
    }
    Ok(())
}
