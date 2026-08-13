//! M9 end-to-end suite: synthetic audio → full pipeline → real focused
//! window → assert the text that actually landed.
//!
//! This is the whole product loop minus the microphone and the hotkey:
//! WAV fixture → VAD → whisper → segmenter → cleanup chain → WindowsInserter
//! → Win32 EDIT control → read back.
//!
//! `#[ignore]`d: needs the local STT model (scripts/fetch-models.ps1) and an
//! interactive desktop (SendInput + foreground activation). Run serially:
//! `cargo test -p scribbet-desktop --release -- --ignored --test-threads=1 --nocapture`
//!
//! Foreground guard: same pattern as the od-insertion harness (docs/04 I-6)
//! — if this process cannot own the foreground, the test SKIPS rather than
//! typing into whatever the developer has focused.

#![cfg(windows)]

use std::time::{Duration, Instant};

use od_cleanup::Chain;
use od_core_types::{PipelineCtx, Segment, SegmentKind};
use od_insertion::{TextInserter, WindowsInserter};
use od_pipeline::{Transcriber, TranscriberConfig};
use od_stt::{WhisperConfig, WhisperEngine};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetWindowTextLengthW, GetWindowTextW, MSG,
    PM_REMOVE, PeekMessageW, SW_SHOW, SetForegroundWindow, ShowWindow, TranslateMessage,
    WINDOW_EX_STYLE, WS_BORDER, WS_CHILD, WS_POPUP, WS_VISIBLE,
};
use windows::core::w;

const SAMPLE_RATE: usize = 16_000;

// ---------- harness window (I-6 guard pattern) ----------

fn create_edit_window() -> (HWND, HWND) {
    unsafe {
        let hinstance = GetModuleHandleW(None).expect("module handle");
        let popup = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("scribbet e2e harness"),
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

fn ensure_foreground(popup: HWND, edit: HWND) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_MENU,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    for _ in 0..5 {
        unsafe {
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

// ---------- pipeline plumbing ----------

fn load_fixture(name: &str) -> Vec<f32> {
    let path = format!(
        "{}/../../../testdata/speech/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut reader = hound::WavReader::open(&path)
        .unwrap_or_else(|e| panic!("open fixture {path}: {e} (run scripts/gen-fixtures.ps1)"));
    reader
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / f32::from(i16::MAX))
        .collect()
}

fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.to_lowercase().chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
    }
    out.trim().to_owned()
}

/// Transcribes a fixture through the real pipeline and runs the cleanup
/// chain over every final. No window involved: decoding takes tens of
/// seconds, and holding the foreground across that gap is exactly the I-6
/// hazard — insertion happens *after*, against a freshly verified window.
fn transcribe_and_clean(samples: &[f32]) -> (Vec<Segment>, Option<Duration>) {
    let ctx = PipelineCtx::default();
    let chain = Chain::from_ctx(&ctx);
    let engine = WhisperEngine::new(WhisperConfig::default())
        .expect("model missing - run scripts/fetch-models.ps1");
    let mut t =
        Transcriber::new(&TranscriberConfig::default(), engine, ctx.clone()).expect("transcriber");

    let mut finals = Vec::new();
    let mut scratch = Vec::new();
    for chunk in samples.chunks(SAMPLE_RATE / 10) {
        scratch.clear();
        t.feed(chunk, &mut scratch).expect("feed");
        for seg in &mut scratch {
            if seg.kind == SegmentKind::Final {
                chain.run(seg, &ctx);
                finals.push(seg.clone());
            }
        }
    }
    scratch.clear();
    t.finish(&mut scratch).expect("finish");
    for seg in &mut scratch {
        if seg.kind == SegmentKind::Final {
            chain.run(seg, &ctx);
            finals.push(seg.clone());
        }
    }
    (finals, t.last_finalize_latency())
}

/// Inserts cleaned finals into the harness EDIT, re-verifying foreground
/// ownership and the captured focus process *before every insert* (I-6:
/// a test that cannot own the foreground refuses to type anywhere).
/// Returns false if foreground ownership was lost (caller should SKIP).
fn insert_finals(
    finals: &[Segment],
    popup: HWND,
    edit: HWND,
    inserter: &mut WindowsInserter,
) -> bool {
    let me = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_lowercase()));
    for seg in finals {
        if !ensure_foreground(popup, edit) {
            return false;
        }
        let target = inserter.capture_focus().expect("capture focus");
        if Some(target.process.clone()) != me {
            eprintln!("focus drifted to {}; refusing to type", target.process);
            return false;
        }
        let text = if seg.text.ends_with(char::is_whitespace) {
            seg.text.clone()
        } else {
            format!("{} ", seg.text)
        };
        inserter.insert(&text, &target).expect("insert final");
        pump(Duration::from_millis(300));
    }
    true
}

// ---------- tests ----------

#[test]
#[ignore = "requires local STT model + interactive desktop"]
fn dictation_lands_cleaned_text_in_a_real_window() {
    // Heavy lifting first, no window on screen yet.
    let mut samples = load_fixture("hello_world.wav");
    samples.extend(std::iter::repeat_n(0.0f32, SAMPLE_RATE / 2));
    let (finals, latency) = transcribe_and_clean(&samples);
    println!("finals: {finals:?}");
    if let Some(l) = latency {
        println!("finalize latency: {} ms", l.as_millis());
    }
    assert!(!finals.is_empty(), "no final segments produced");

    // Now the insertion half: own the foreground, verify, type, read back.
    let (popup, edit) = create_edit_window();
    pump(Duration::from_millis(300));
    let mut inserter = WindowsInserter::new().expect("inserter init");
    let delivered = insert_finals(&finals, popup, edit, &mut inserter);
    pump(Duration::from_millis(500));
    let landed = read_text(edit);
    unsafe {
        let _ = DestroyWindow(popup);
    }
    if !delivered {
        eprintln!("SKIP: e2e window could not keep foreground; refusing to type anywhere else");
        return;
    }

    println!("landed text: {landed:?}");
    let norm = normalize(&landed);
    assert!(norm.contains("hello world"), "landed: {landed:?}");
    assert!(
        norm.contains("test of the dictation system"),
        "landed: {landed:?}"
    );
    // Cleanup chain must have run: sentence-cased, terminally punctuated.
    assert!(
        landed.trim_end().ends_with('.'),
        "terminal punctuation missing: {landed:?}"
    );
    assert!(
        landed.starts_with('H'),
        "sentence capitalization missing: {landed:?}"
    );
}

#[test]
#[ignore = "requires local STT model + interactive desktop"]
fn two_utterance_fixture_lands_both_sentences() {
    let mut samples = load_fixture("with_pause.wav");
    samples.extend(std::iter::repeat_n(0.0f32, SAMPLE_RATE / 2));
    let (finals, _) = transcribe_and_clean(&samples);
    println!("finals: {finals:?}");
    assert!(finals.len() >= 2, "expected >= 2 finals, got {finals:?}");

    let (popup, edit) = create_edit_window();
    pump(Duration::from_millis(300));
    let mut inserter = WindowsInserter::new().expect("inserter init");
    let delivered = insert_finals(&finals, popup, edit, &mut inserter);
    pump(Duration::from_millis(500));
    let landed = read_text(edit);
    unsafe {
        let _ = DestroyWindow(popup);
    }
    if !delivered {
        eprintln!("SKIP: e2e window could not keep foreground; refusing to type anywhere else");
        return;
    }

    println!("landed text: {landed:?}");
    let norm = normalize(&landed);
    assert!(norm.contains("first part"), "landed: {landed:?}");
    assert!(norm.contains("second part"), "landed: {landed:?}");
}
