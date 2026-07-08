//! Windows insertion backend: tier orchestration.

mod clipboard;
mod focus;
mod sendinput;
mod uia;

use std::time::Instant;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsWindow};

use crate::{FocusInfo, InsertError, InsertOutcome, InsertionTier, TextInserter, builtin_quirk};

/// Places `text` on the clipboard (no paste, no restore). Recovery path for
/// callers when insertion fails outright: the text stays one Ctrl+V away.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    clipboard::copy_only(text)
}

/// Above this many chars, keystroke simulation is user-perceptible (seconds
/// for a paragraph) and one clipboard paste is preferred.
const LONG_TEXT_PASTE_CHARS: usize = 120;

/// [`TextInserter`] for Windows.
///
/// Not `Send` by design (holds an apartment-threaded UIA COM object):
/// construct it on the thread that will use it — the session controller
/// thread — via the factory passed to `od_pipeline::spawn`.
pub struct WindowsInserter {
    uia: uia::Uia,
}

impl WindowsInserter {
    /// Initializes COM (apartment-threaded) and the UIA automation object on
    /// the calling thread.
    pub fn new() -> Result<Self, InsertError> {
        Ok(Self {
            uia: uia::Uia::new()?,
        })
    }
}

impl TextInserter for WindowsInserter {
    fn capture_focus(&mut self) -> Result<FocusInfo, InsertError> {
        focus::capture()
    }

    fn insert(&mut self, text: &str, target: &FocusInfo) -> Result<InsertOutcome, InsertError> {
        if text.is_empty() {
            return Ok(InsertOutcome {
                tier: InsertionTier::SendInput,
                duration: std::time::Duration::ZERO,
            });
        }
        let t0 = Instant::now();

        // Re-verify the target. If the user deliberately moved focus since
        // capture, follow the caret — never type into a window the user left.
        // (focus::capture resolves our own windows to the one beneath them,
        // so a stop-click on the overlay pill never counts as a move.)
        let target_hwnd = HWND(target.window.0 as *mut core::ffi::c_void);
        let current = unsafe { GetForegroundWindow() };
        let effective: FocusInfo = if current == target_hwnd {
            target.clone()
        } else if unsafe { IsWindow(Some(current)) }.as_bool() && !current.is_invalid() {
            let refreshed = focus::capture()?;
            if refreshed.window != target.window {
                tracing::info!(
                    from = %target.process,
                    to = %refreshed.process,
                    "focus moved since capture; following current focus"
                );
            }
            refreshed
        } else if unsafe { IsWindow(Some(target_hwnd)) }.as_bool() {
            target.clone()
        } else {
            return Err(InsertError::TargetGone);
        };

        // The keystroke tiers deliver to whichever window owns the keyboard.
        // If that isn't the effective target (stop-click left our pill
        // foreground), hand focus back before typing.
        let effective_hwnd = HWND(effective.window.0 as *mut core::ffi::c_void);
        if unsafe { GetForegroundWindow() } != effective_hwnd {
            // Poll until the target really owns the foreground (nudging the
            // foreground lock on retry) instead of trusting a single fixed
            // delay — a throttled machine can miss an 80 ms deadline (docs/04
            // I-6). Returns as soon as focus lands, so fast machines pay ~0.
            if !focus::activate_and_wait(effective.window, std::time::Duration::from_millis(600)) {
                return Err(InsertError::Platform(
                    "dictation target did not accept focus".into(),
                ));
            }
            // The top-level window is ours, but an Electron/Chromium host still
            // needs a beat to route focus into its child control (e.g. the
            // terminal pane inside Cursor) before a keystroke or paste will
            // land (docs/04 I-10). Native Win32 targets are ready at once; the
            // extra idle only occurs on a click-to-talk stop and is imperceptible.
            std::thread::sleep(std::time::Duration::from_millis(120));
        }

        let quirk = builtin_quirk(&effective.process);
        let probe = self.uia.probe_focused();

        // Password fields must never transit the clipboard (docs/06 T4/T8).
        let allow_clipboard = !probe.is_password;

        let mut plan: Vec<InsertionTier> = Vec::with_capacity(3);
        match quirk.prefer {
            InsertionTier::Uia => {
                plan.push(InsertionTier::Uia);
                plan.push(InsertionTier::SendInput);
                plan.push(InsertionTier::Clipboard);
            }
            InsertionTier::SendInput => {
                plan.push(InsertionTier::SendInput);
                plan.push(InsertionTier::Clipboard);
            }
            InsertionTier::Clipboard => {
                plan.push(InsertionTier::Clipboard);
                plan.push(InsertionTier::SendInput);
            }
        }

        // Long text: per-character SendInput takes visible seconds for a
        // whole dictation session (v1.2 inserts once per session); a single
        // paste is instant. Promote the clipboard tier — but NOT for hosts
        // that prefer SendInput (Electron/Chromium, e.g. Cursor). There a
        // Ctrl+V into an embedded terminal pane is unreliable (the clipboard
        // is restored before the pane finishes pasting, so long text drops
        // while short typed text lands — docs/04 I-13); keep typing, which is
        // proven reliable for these hosts, and leave clipboard only as a
        // fallback. Never promote for a password field either.
        if allow_clipboard
            && quirk.prefer != InsertionTier::SendInput
            && text.chars().count() > LONG_TEXT_PASTE_CHARS
        {
            plan.retain(|t| *t != InsertionTier::Clipboard);
            plan.insert(0, InsertionTier::Clipboard);
        }

        let mut last_err = String::new();
        for tier in plan {
            let attempt = match tier {
                InsertionTier::Uia => self.uia.try_append_empty_value(&probe, text).map(|()| {
                    // SetValue parks the caret at 0; park it at the end so a
                    // following insertion appends instead of prepending.
                    sendinput::release_held_modifiers();
                    if let Err(e) = sendinput::caret_to_end() {
                        tracing::debug!("caret_to_end after SetValue failed: {e}");
                    }
                }),
                InsertionTier::SendInput => sendinput::type_text(text, quirk.keystroke_pacing),
                InsertionTier::Clipboard => {
                    if !allow_clipboard {
                        Err("clipboard tier disabled for password field".into())
                    } else {
                        clipboard::paste_with_restore(text, quirk.paste_settle)
                    }
                }
            };
            match attempt {
                Ok(()) => {
                    let outcome = InsertOutcome {
                        tier,
                        duration: t0.elapsed(),
                    };
                    tracing::info!(
                        tier = ?tier,
                        insert_ms = t0.elapsed().as_millis() as u64,
                        chars = text.chars().count(),
                        app = %effective.process,
                        "inserted"
                    );
                    return Ok(outcome);
                }
                Err(e) => {
                    tracing::debug!(tier = ?tier, "tier failed: {e}");
                    last_err = e;
                }
            }
        }
        Err(InsertError::AllTiersFailed(last_err))
    }
}
