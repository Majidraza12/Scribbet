//! Tier 1 + probe: UI Automation.
//!
//! UIA cannot insert at an arbitrary caret (TextPattern is read-oriented),
//! so tier 1 is deliberately narrow: an *empty, writable, value-patterned*
//! field gets an atomic `SetValue` — the common "fresh input box" case —
//! and everything else falls through to SendInput. The probe additionally
//! supplies the password-field flag that gates the clipboard tier, and will
//! grow range operations for voice editing in M6.

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationValuePattern,
    UIA_ValuePatternId,
};
use windows::core::Interface;

use crate::InsertError;

/// UIA session bound to the constructing thread's COM apartment.
pub struct Uia {
    automation: IUIAutomation,
}

/// What the probe learned about the focused element.
pub struct FocusProbe {
    /// Focused element reports itself as a password field.
    pub is_password: bool,
    /// Value pattern of the focused element, when writable and currently
    /// empty (the tier-1 fast path).
    empty_value: Option<IUIAutomationValuePattern>,
}

impl Uia {
    /// Initializes COM (apartment-threaded; a pre-existing incompatible
    /// apartment is tolerated) and creates the automation object.
    pub fn new() -> Result<Self, InsertError> {
        unsafe {
            // RPC_E_CHANGED_MODE means the thread already has an apartment;
            // UIA client calls still work, so ignore it.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| InsertError::Platform(format!("UIA init: {e}")))?;
        Ok(Self { automation })
    }

    /// Probes the currently focused element. Failures degrade to a probe
    /// that reports nothing (tiers 2/3 still work without UIA).
    pub fn probe_focused(&self) -> FocusProbe {
        let mut probe = FocusProbe {
            is_password: false,
            empty_value: None,
        };
        let Ok(element) = (unsafe { self.automation.GetFocusedElement() }) else {
            return probe;
        };

        probe.is_password = unsafe { element.CurrentIsPassword() }
            .map(|b| b.as_bool())
            .unwrap_or(false);

        if let Some(vp) = value_pattern(&element)
            && let Ok(readonly) = unsafe { vp.CurrentIsReadOnly() }
            && !readonly.as_bool()
            && let Ok(value) = unsafe { vp.CurrentValue() }
            && value.is_empty()
        {
            probe.empty_value = Some(vp);
        }
        probe
    }

    /// Tier 1: atomically sets the text of an empty writable field.
    pub fn try_append_empty_value(&self, probe: &FocusProbe, text: &str) -> Result<(), String> {
        let Some(vp) = &probe.empty_value else {
            return Err("no empty writable value-patterned element focused".into());
        };
        unsafe { vp.SetValue(&windows::core::BSTR::from(text)) }
            .map_err(|e| format!("ValuePattern::SetValue: {e}"))
    }
}

fn value_pattern(element: &IUIAutomationElement) -> Option<IUIAutomationValuePattern> {
    let unknown = unsafe { element.GetCurrentPattern(UIA_ValuePatternId) }.ok()?;
    unknown.cast::<IUIAutomationValuePattern>().ok()
}
