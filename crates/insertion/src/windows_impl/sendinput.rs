//! Tier 2: synthesized unicode keyboard events.

use std::time::Duration;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU,
    VK_RETURN, VK_RWIN, VK_SHIFT,
};

/// Events per SendInput batch. Batching keeps syscall count low; the pacing
/// sleep between batches (quirk-driven) throttles apps that drop rapid
/// synthetic input.
const BATCH: usize = 64;

/// Types `text` into the focused window via `KEYEVENTF_UNICODE` events.
/// `\n` becomes a real Return press; `\r` is skipped; UTF-16 surrogate pairs
/// are sent as consecutive unicode events per the SendInput contract.
pub fn type_text(text: &str, pacing: Duration) -> Result<(), String> {
    // The user may still be holding the hotkey chord (push-to-talk): release
    // any real modifiers first so "v" doesn't become "Ctrl+Shift+V" etc.
    release_held_modifiers();

    let mut events: Vec<INPUT> = Vec::with_capacity(BATCH + 4);
    for ch in text.chars() {
        match ch {
            '\r' => continue,
            '\n' => {
                events.push(key_event(VK_RETURN, 0, KEYBD_EVENT_FLAGS(0)));
                events.push(key_event(VK_RETURN, 0, KEYEVENTF_KEYUP));
            }
            _ => {
                let mut units = [0u16; 2];
                for &unit in ch.encode_utf16(&mut units).iter() {
                    events.push(key_event(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE));
                    events.push(key_event(
                        VIRTUAL_KEY(0),
                        unit,
                        KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    ));
                }
            }
        }
        if events.len() >= BATCH {
            send(&events)?;
            events.clear();
            if !pacing.is_zero() {
                std::thread::sleep(pacing);
            }
        }
    }
    if !events.is_empty() {
        send(&events)?;
    }
    Ok(())
}

/// Sends key-up events for any currently-down modifier keys (hotkey chord
/// remnants). Only fires for modifiers that are actually down.
pub fn release_held_modifiers() {
    let modifiers = [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN];
    let mut ups: Vec<INPUT> = Vec::with_capacity(modifiers.len());
    for vk in modifiers {
        let down = unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0;
        if down {
            ups.push(key_event(vk, 0, KEYEVENTF_KEYUP));
        }
    }
    if !ups.is_empty() {
        let _ = send(&ups);
    }
}

/// Synthetic Alt tap. Marks this process "input-active" so a following
/// `SetForegroundWindow` is honored despite the Windows foreground lock, which
/// otherwise silently denies activation to a background process (docs/04 I-6 —
/// the same workaround the test harness uses). Balanced down+up, so it never
/// leaves Alt held.
pub fn nudge_foreground_lock() {
    let taps = [
        key_event(VK_MENU, 0, KEYBD_EVENT_FLAGS(0)),
        key_event(VK_MENU, 0, KEYEVENTF_KEYUP),
    ];
    let _ = send(&taps);
}

/// Sends Ctrl+End: caret to end of document. Used after a UIA `SetValue`,
/// which places the caret at position 0 (WM_SETTEXT semantics) — without
/// this, the *next* insertion would land in front of the text just set
/// (caught by the harness test; docs/04 I-5).
pub fn caret_to_end() -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_END;
    let chord = [
        key_event(VK_CONTROL, 0, KEYBD_EVENT_FLAGS(0)),
        key_event(VK_END, 0, KEYBD_EVENT_FLAGS(0)),
        key_event(VK_END, 0, KEYEVENTF_KEYUP),
        key_event(VK_CONTROL, 0, KEYEVENTF_KEYUP),
    ];
    send(&chord)
}

fn key_event(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send(events: &[INPUT]) -> Result<(), String> {
    let sent = unsafe { SendInput(events, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != events.len() {
        return Err(format!(
            "SendInput delivered {sent}/{} events (input blocked?)",
            events.len()
        ));
    }
    Ok(())
}
