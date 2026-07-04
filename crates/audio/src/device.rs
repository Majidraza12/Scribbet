//! Input-device enumeration and selection.
//!
//! Devices are identified by their human-readable name (what WASAPI exposes
//! stably enough for a settings dropdown). Hot-swap is session-granular:
//! stop the current [`CaptureSession`](crate::CaptureSession), start a new
//! one with a different [`DeviceSelector`]. cpal has no device-change
//! notifications, so unplugging the active device surfaces as the session's
//! `is_disconnected()` flag (driven by the stream error callback) and the
//! owner restarts on the new default.

use cpal::traits::{DeviceTrait, HostTrait};

use crate::error::AudioError;

/// Which input device a capture session should open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DeviceSelector {
    /// The system default input device (follows the OS setting at open time).
    #[default]
    SystemDefault,
    /// A specific device, matched by exact name from [`list_input_devices`].
    ByName(String),
}

/// A capture device as shown in the settings UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDeviceInfo {
    /// Human-readable device name; also the identity used by
    /// [`DeviceSelector::ByName`].
    pub name: String,
    /// Whether this is currently the system default input device.
    pub is_default: bool,
}

/// Enumerates the system's audio input devices.
pub fn list_input_devices() -> Result<Vec<InputDeviceInfo>, AudioError> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|d| d.name().ok());

    let devices = host
        .input_devices()
        .map_err(|e| AudioError::Enumerate(e.to_string()))?;

    let mut infos = Vec::new();
    for device in devices {
        // Devices that can't report a name can't be selected meaningfully;
        // skip them rather than showing "<unknown>" entries.
        let Ok(name) = device.name() else { continue };
        let is_default = Some(&name) == default_name.as_ref();
        infos.push(InputDeviceInfo { name, is_default });
    }
    Ok(infos)
}

/// Resolves a selector to a concrete cpal device.
pub(crate) fn find_input_device(selector: &DeviceSelector) -> Result<cpal::Device, AudioError> {
    let host = cpal::default_host();
    match selector {
        DeviceSelector::SystemDefault => host.default_input_device().ok_or(AudioError::NoDevice),
        DeviceSelector::ByName(wanted) => {
            let devices = host
                .input_devices()
                .map_err(|e| AudioError::Enumerate(e.to_string()))?;
            for device in devices {
                if device.name().is_ok_and(|n| &n == wanted) {
                    return Ok(device);
                }
            }
            Err(AudioError::DeviceNotFound(wanted.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_default_is_system_default() {
        assert_eq!(DeviceSelector::default(), DeviceSelector::SystemDefault);
    }

    #[test]
    fn unknown_device_name_errors() {
        // Enumeration itself may legitimately fail on CI runners with no
        // audio subsystem; only assert the not-found path when it works.
        let selector = DeviceSelector::ByName("od-audio nonexistent test device 3f9a".into());
        match find_input_device(&selector) {
            Err(AudioError::DeviceNotFound(name)) => {
                assert!(name.contains("nonexistent"));
            }
            Err(AudioError::Enumerate(_)) | Err(AudioError::NoDevice) => {}
            Err(other) => panic!("expected DeviceNotFound, got {other:?}"),
            Ok(_) => panic!("expected DeviceNotFound, got a device"),
        }
    }
}
