use std::path::{Path, PathBuf};

use log::{info, warn};

use crate::error::{PlatformError, Result};
use crate::{attr_num, attr_string, to_device};

/// Dynamic Lighting class device under `/sys/class/leds/`.
///
/// Wraps a kernel `led-class-dynamic` sysfs node exposing effects, palette,
/// speed, direction, power states, direct buffer streaming, and standard
/// brightness attributes.
#[derive(Debug, PartialEq, Eq, PartialOrd, Clone)]
pub struct DynamicLed {
    path: PathBuf,
}

impl DynamicLed {
    attr_string!("effect", path);
    attr_string!("effect_index", path);
    attr_string!("direction", path);
    attr_string!("direction_index", path);
    attr_string!("effects_palette", path);
    attr_string!("speed_range", path);
    attr_string!("zone_type", path);
    attr_string!("matrix_dimensions", path);
    attr_string!("power_states", path);
    attr_string!("power_states_index", path);

    attr_num!("speed", path, u32);
    attr_num!("max_palette_entries", path, u32);
    attr_num!("led_count", path, u32);
    attr_num!("brightness", path, u8);
    attr_num!("max_brightness", path, u8);

    /// Create a new `DynamicLed` by matching the exact sysfs name (e.g. `"aura:keyboard"`).
    pub fn new(name: &str) -> Result<Self> {
        let mut enumerator = udev::Enumerator::new().map_err(|err| {
            warn!("DynamicLed udev enumerator failed: {err}");
            PlatformError::Udev("enumerator failed".into(), err)
        })?;
        enumerator.match_subsystem("leds").map_err(|err| {
            warn!("DynamicLed match_subsystem failed: {err}");
            PlatformError::Udev("match_subsystem failed".into(), err)
        })?;

        for device in enumerator.scan_devices().map_err(|err| {
            warn!("DynamicLed scan_devices failed: {err}");
            PlatformError::Udev("scan_devices failed".into(), err)
        })? {
            let sysname = device.sysname().to_string_lossy();
            if sysname == name {
                info!("Found Dynamic Lighting LED device at {:?}", sysname);
                return Ok(Self {
                    path: device.syspath().to_path_buf(),
                });
            }
        }

        Err(PlatformError::MissingFunction(format!(
            "DynamicLed::new(): no dynamic LED named '{name}' found"
        )))
    }

    /// Helper to find a dynamic LED by name.
    pub fn find(name: &str) -> Result<Self> {
        Self::new(name)
    }

    /// Check if a dynamic LED is present on the system.
    pub fn is_available(name: &str) -> bool {
        Self::find(name).is_ok()
    }

    /// Return the sysfs path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read and parse space-separated list of supported effects from `effect_index`.
    pub fn get_supported_effects_list(&self) -> Result<Vec<String>> {
        let raw = self.get_effect_index()?;
        Ok(raw.split_whitespace().map(String::from).collect())
    }

    /// Check if a given effect mode string is supported by the kernel driver.
    pub fn is_effect_supported(&self, effect: &str) -> bool {
        self.get_supported_effects_list()
            .map(|list| list.iter().any(|e| e == effect))
            .unwrap_or(false)
    }

    /// Read and parse space-separated list of supported directions from `direction_index`.
    pub fn get_supported_directions_list(&self) -> Result<Vec<String>> {
        let raw = self.get_direction_index()?;
        Ok(raw.split_whitespace().map(String::from).collect())
    }

    /// Write raw RGB byte buffer directly to the `direct_buffer` (or `direct`) binary attribute.
    pub fn write_direct(&self, data: &[u8]) -> Result<()> {
        let direct_path = if self.path.join("direct_buffer").exists() {
            self.path.join("direct_buffer")
        } else {
            self.path.join("direct")
        };
        std::fs::write(&direct_path, data)
            .map_err(|e| PlatformError::IoPath(direct_path.to_string_lossy().into_owned(), e))
    }

    /// Write palette colors as formatted `"#RRGGBB #RRGGBB ..."` string to `effects_palette`.
    pub fn set_palette_colors(&self, colors: &[(u8, u8, u8)]) -> Result<()> {
        let formatted: Vec<String> = colors
            .iter()
            .map(|(r, g, b)| format!("#{r:02x}{g:02x}{b:02x}"))
            .collect();
        let palette_str = formatted.join(" ");
        self.set_effects_palette(&palette_str)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_palette_colors_formatting() {
        let colors = [
            (255, 0, 128),
            (0, 255, 64),
        ];
        let formatted: Vec<String> = colors
            .iter()
            .map(|(r, g, b)| format!("#{r:02x}{g:02x}{b:02x}"))
            .collect();
        let palette_str = formatted.join(" ");
        assert_eq!(palette_str, "#ff0080 #00ff40");
    }
}
