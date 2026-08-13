//! Automated insertion harness: creates a real Win32 EDIT window, focuses
//! it, runs the full WindowsInserter tier stack against it, and reads the
//! text back.
//!
//! `#[ignore]`d: needs an interactive desktop (SendInput and foreground
//! activation do not work in service/CI sessions). Run locally:
//! `cargo test -p od-insertion -- --ignored --nocapture --test-threads=1`

#![cfg(windows)]

use std::time::{Duration, Instant};

use od_insertion::{TextInserter, WindowsInserter};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetWindowTextLengthW, GetWindowTextW, MSG,
    PM_REMOVE, PeekMessageW, SW_SHOW, SetForegroundWindow, ShowWindow, TranslateMessage,
    WINDOW_EX_STYLE, WS_BORDER, WS_CHILD, WS_POPUP, WS_VISIBLE,
};
use windows::core::w;

/// Creates a popup window hosting a multiline EDIT control and returns
/// (popup, edit).
fn create_edit_window() -> (HWND, HWND) {
    unsafe {
        let hinstance = GetModuleHandleW(None).expect("module handle");
        let popup = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("od-insertion harness"),
            WS_POPUP | WS_VISIBLE,
            100,
            100,
            420,
            240,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
        .expect("create popup");
        // ES_MULTILINE (0x0004) | ES_AUTOVSCROLL (0x0040) via style bits.
        let edit = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            w!(""),
            WS_CHILD
                | WS_VISIBLE
                | WS_BORDER
                | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(0x0044),
            10,
            10,
            400,
            220,
            Some(popup),
            None,
            Some(hinstance.into()),
            None,
        )
        .expect("create edit");

        let _ = ShowWindow(popup, SW_SHOW);
        let _ = SetForegroundWindow(popup);
        let _ = SetFocus(Some(edit));
        (popup, edit)
    }
}

/// SAFETY GUARD (docs/04 I-6): Windows may refuse foreground activation to a
/// background process, leaving focus on whatever the developer had open —
/// and the inserter would then type test strings into *that* window. Nudge
/// the foreground lock (Alt tap, retry), then verify our window really is
/// the foreground + focused target; otherwise the caller must SKIP, never
/// insert.
fn ensure_foreground(popup: HWND, edit: HWND) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_MENU,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    for _ in 0..5 {
        unsafe {
            // The documented workaround: a synthetic Alt tap makes this
            // process "input-active" so SetForegroundWindow is honored.
            let alt = |flags| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_MENU,
                        wScan: 0,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            let taps = [alt(Default::default()), alt(KEYEVENTF_KEYUP)];
            SendInput(&taps, std::mem::size_of::<INPUT>() as i32);
            let _ = SetForegroundWindow(popup);
            let _ = SetFocus(Some(edit));
        }
        pump(Duration::from_millis(150));
        if unsafe { GetForegroundWindow() } == popup {
            return true;
        }
    }
    false
}

/// Pumps this thread's message queue for `duration` so synthesized input
/// reaches the EDIT control.
fn pump(duration: Duration) {
    let deadline = Instant::now() + duration;
    let mut msg = MSG::default();
    while Instant::now() < deadline {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn read_text(edit: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(edit) as usize;
        let mut buf = vec![0u16; len + 1];
        let copied = GetWindowTextW(edit, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..copied])
    }
}

#[test]
#[ignore = "requires an interactive desktop session"]
fn inserts_into_real_edit_control_and_reads_back() {
    let (popup, edit) = create_edit_window();
    pump(Duration::from_millis(300)); // let the window settle

    if !ensure_foreground(popup, edit) {
        unsafe {
            let _ = DestroyWindow(popup);
        }
        eprintln!("SKIP: harness window could not take foreground; refusing to type anywhere else");
        return;
    }

    let mut inserter = WindowsInserter::new().expect("inserter init");
    let target = inserter.capture_focus().expect("capture focus");
    println!("captured focus: {} ({})", target.process, target.title);
    // Belt and braces: the captured target must be THIS test process.
    let me = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_lowercase()));
    assert_eq!(
        Some(target.process.clone()),
        me,
        "focus is not on the harness window"
    );

    // First insertion: field is empty → the UIA ValuePattern fast path is
    // eligible (falls back to SendInput transparently if the classic EDIT
    // control's UIA proxy declines).
    let first = "Hello from Scribbet. ";
    let outcome1 = inserter.insert(first, &target).expect("insert #1");
    println!(
        "insert #1 via {:?} in {:?}",
        outcome1.tier, outcome1.duration
    );
    pump(Duration::from_millis(400));

    // Second insertion: field is non-empty → must append at the caret via
    // SendInput (UIA fast path not applicable), including unicode + newline.
    let second = "Ünïcode ✓ and\na new line";
    let outcome2 = inserter.insert(second, &target).expect("insert #2");
    println!(
        "insert #2 via {:?} in {:?}",
        outcome2.tier, outcome2.duration
    );
    pump(Duration::from_millis(600));

    let text = read_text(edit);
    println!("edit content: {text:?}");
    unsafe {
        let _ = DestroyWindow(popup);
    }

    // EDIT stores newlines as CRLF; normalize for comparison.
    let normalized = text.replace("\r\n", "\n");
    assert_eq!(normalized, format!("{first}{second}"));
}

#[test]
#[ignore = "requires an interactive desktop session"]
fn clipboard_tier_round_trips_and_restores() {
    use windows::Win32::Foundation::HANDLE;

    let (popup, edit) = create_edit_window();
    pump(Duration::from_millis(300));

    if !ensure_foreground(popup, edit) {
        unsafe {
            let _ = DestroyWindow(popup);
        }
        eprintln!("SKIP: harness window could not take foreground; refusing to type anywhere else");
        return;
    }

    // Seed the clipboard with sentinel text we expect back afterwards.
    let sentinel = "clipboard-sentinel-1793";
    set_clipboard_text(sentinel);

    // Force the clipboard tier by faking a terminal process quirk: we can't
    // rename our test binary, so call the tier through the public API by
    // inserting into the edit — then verify the restore path with a direct
    // sentinel check. (Tier forcing per-target arrives with user quirk
    // overrides in M7.)
    let mut inserter = WindowsInserter::new().expect("inserter init");
    let target = inserter.capture_focus().expect("capture focus");
    inserter.insert("pasted text", &target).expect("insert");
    pump(Duration::from_millis(600));

    let clip = get_clipboard_text();
    unsafe {
        let _ = DestroyWindow(popup);
    }
    assert_eq!(
        clip.as_deref(),
        Some(sentinel),
        "clipboard content must survive insertion regardless of tier"
    );

    // Helpers ---------------------------------------------------------
    fn set_clipboard_text(text: &str) {
        use windows::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
        };
        use windows::Win32::System::Memory::{
            GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
        };
        unsafe {
            OpenClipboard(None).expect("open clipboard");
            EmptyClipboard().expect("empty clipboard");
            let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let h = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2).expect("alloc");
            let p = GlobalLock(h) as *mut u16;
            std::ptr::copy_nonoverlapping(units.as_ptr(), p, units.len());
            let _ = GlobalUnlock(h);
            SetClipboardData(13, Some(HANDLE(h.0))).expect("set clipboard");
            let _ = CloseClipboard();
        }
    }

    fn get_clipboard_text() -> Option<String> {
        use windows::Win32::Foundation::HGLOBAL;
        use windows::Win32::System::DataExchange::{
            CloseClipboard, GetClipboardData, OpenClipboard,
        };
        use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
        unsafe {
            OpenClipboard(None).ok()?;
            let result = GetClipboardData(13).ok().and_then(|handle| {
                let hglobal = HGLOBAL(handle.0);
                let ptr = GlobalLock(hglobal) as *const u16;
                if ptr.is_null() {
                    return None;
                }
                let mut len = 0usize;
                while *ptr.add(len) != 0 {
                    len += 1;
                }
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
                let _ = GlobalUnlock(hglobal);
                Some(s)
            });
            let _ = CloseClipboard();
            result
        }
    }
}
