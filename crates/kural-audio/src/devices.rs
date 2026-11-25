use crate::error::AudioError;
use crate::types::AudioDeviceInfo;
use cpal::traits::{DeviceTrait, HostTrait};

/// Enumerate all available input and output audio devices on the default host.
pub fn list_audio_devices() -> Result<Vec<AudioDeviceInfo>, AudioError> {
    let host = cpal::default_host();
    let default_in = host.default_input_device();
    let default_out = host.default_output_device();

    let devices = host.devices().map_err(|err| {
        AudioError::DeviceUnavailable(format!("failed to enumerate audio devices: {err}"))
    })?;

    let mut result = Vec::new();
    for device in devices {
        let Ok(desc) = device.description() else {
            continue;
        };
        let is_input = device.supports_input();
        let is_output = device.supports_output();
        if !is_input && !is_output {
            continue;
        }

        let is_default = default_in.as_ref().is_some_and(|d| d == &device)
            || default_out.as_ref().is_some_and(|d| d == &device);

        let default_cfg = if is_input {
            device.default_input_config().ok()
        } else {
            None
        }
        .or_else(|| {
            if is_output {
                device.default_output_config().ok()
            } else {
                None
            }
        });

        result.push(AudioDeviceInfo {
            name: desc.name().to_string(),
            driver: desc.driver().map(str::to_string),
            is_input,
            is_output,
            is_default,
            default_sample_rate_hz: default_cfg.as_ref().map(|cfg| cfg.sample_rate()),
            default_channels: default_cfg.as_ref().map(|cfg| cfg.channels()),
        });
    }

    Ok(result)
}

/// Find an input device matching `pattern` (case-insensitive substring), or the host default.
///
/// `"default"` (or empty/whitespace) directly selects `default_input_device()`. If a non-default
/// pattern matches no input device, falls back to `default_input_device()`.
#[must_use]
pub fn find_input_device(pattern: &str) -> Option<cpal::Device> {
    let host = cpal::default_host();
    find_device_with(
        pattern,
        || host.input_devices(),
        || host.default_input_device(),
    )
}

/// Find an output device matching `pattern` (case-insensitive substring), or the host default.
///
/// `"default"` (or empty/whitespace) directly selects `default_output_device()`. If a non-default
/// pattern matches no output device, falls back to `default_output_device()`.
#[must_use]
pub fn find_output_device(pattern: &str) -> Option<cpal::Device> {
    let host = cpal::default_host();
    find_device_with(
        pattern,
        || host.output_devices(),
        || host.default_output_device(),
    )
}

fn find_device_with<I>(
    pattern: &str,
    enumerate: impl FnOnce() -> Result<I, cpal::Error>,
    default_device: impl FnOnce() -> Option<cpal::Device>,
) -> Option<cpal::Device>
where
    I: Iterator<Item = cpal::Device>,
{
    if is_default_pattern(pattern) {
        return default_device();
    }

    if let Ok(devices) = enumerate() {
        for device in devices {
            if let Ok(desc) = device.description() {
                if matches_device_name(desc.name(), pattern) {
                    return Some(device);
                }
            }
        }
    }

    default_device()
}

pub(crate) fn is_default_pattern(pattern: &str) -> bool {
    let trimmed = pattern.trim();
    trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default")
}

pub(crate) fn matches_device_name(device_name: &str, pattern: &str) -> bool {
    let needle = pattern.trim().to_ascii_lowercase();
    !needle.is_empty() && device_name.to_ascii_lowercase().contains(&needle)
}

#[cfg(test)]
fn select_matching_name_index(names: &[&str], pattern: &str) -> Option<usize> {
    if is_default_pattern(pattern) {
        return None;
    }
    names
        .iter()
        .position(|name| matches_device_name(name, pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pattern_recognizes_empty_whitespace_and_case() {
        assert!(is_default_pattern("default"));
        assert!(is_default_pattern("DEFAULT"));
        assert!(is_default_pattern("  Default  "));
        assert!(is_default_pattern(""));
        assert!(is_default_pattern("   "));
        assert!(!is_default_pattern("usb"));
    }

    #[test]
    fn substring_matching_is_case_insensitive_and_trimmed() {
        let devices = [
            "HDA Intel PCH: ALC256 Analog",
            "USB PnP Sound Device: Audio",
            "sof-hda-dsp: Digital Mic",
        ];

        assert_eq!(select_matching_name_index(&devices, "default"), None);
        assert_eq!(select_matching_name_index(&devices, "usb pnp"), Some(1));
        assert_eq!(
            select_matching_name_index(&devices, "  DIGITAL MIC "),
            Some(2)
        );
        assert_eq!(select_matching_name_index(&devices, "alc256"), Some(0));
        assert_eq!(
            select_matching_name_index(&devices, "nonexistent-card"),
            None
        );
    }

    #[test]
    fn device_enumeration_and_lookup_never_panic_even_without_hardware() {
        match list_audio_devices() {
            Ok(list) => {
                for dev in list {
                    assert!(!dev.name.is_empty());
                    assert!(dev.is_input || dev.is_output);
                }
            }
            Err(AudioError::DeviceUnavailable(_)) => {}
            Err(other) => panic!("unexpected enumeration error: {other:?}"),
        }

        let _ = find_input_device("default");
        let _ = find_input_device("__nonexistent_kural_test_device__");
        let _ = find_output_device("default");
        let _ = find_output_device("__nonexistent_kural_test_device__");
    }
}
