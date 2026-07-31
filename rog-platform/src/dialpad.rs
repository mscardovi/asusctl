use std::fs;
use std::path::PathBuf;

use log::{info, warn};
use serde::{Deserialize, Serialize};
use zbus::zvariant::{OwnedValue, Type, Value};

use crate::error::{PlatformError, Result};
use crate::{attr_num, to_device};

/// ASUS WMI Device ID for DialPad hardware toggle (`IIA0 == 0x00100063`)
pub const ASUS_WMI_DEVID_DIALPAD: u32 = 0x00100063;

/// Operating mode for the DialPad controller.
#[derive(
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Clone,
    Copy,
    Serialize,
    Deserialize,
    Type,
    Value,
    OwnedValue,
)]
#[repr(u8)]
pub enum DialpadMode {
    Hardware = 0,
    VirtualSoftware = 1,
    #[default]
    Auto = 2,
}

impl std::fmt::Display for DialpadMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hardware => write!(f, "Hardware"),
            Self::VirtualSoftware => write!(f, "VirtualSoftware"),
            Self::Auto => write!(f, "Auto"),
        }
    }
}

impl std::str::FromStr for DialpadMode {
    type Err = PlatformError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "hardware" | "hw" => Ok(Self::Hardware),
            "virtualsoftware" | "virtual" | "sw" => Ok(Self::VirtualSoftware),
            "auto" => Ok(Self::Auto),
            _ => Err(PlatformError::MissingFunction(format!(
                "Invalid DialPad mode: {s}"
            ))),
        }
    }
}

/// The Dialpad device provides access to ASUS DialPad backlight and hardware/software status.
#[derive(Debug, PartialEq, Eq, PartialOrd, Clone)]
pub struct Dialpad {
    path: PathBuf,
    wmi_dev_id_path: Option<PathBuf>,
    mode: DialpadMode,
    is_hardware_capable: bool,
}

impl Dialpad {
    attr_num!("brightness", path, u8);
    attr_num!("max_brightness", path, u8);

    pub fn new() -> Result<Self> {
        let wmi_path = PathBuf::from("/sys/devices/platform/asus-wmi/dev_id");
        let wmi_dev_id_path = if wmi_path.exists() {
            Some(wmi_path)
        } else {
            None
        };

        // Scan for physical LED device
        let mut enumerator = udev::Enumerator::new().map_err(|err| {
            warn!("{}", err);
            PlatformError::Udev("enumerator failed".into(), err)
        })?;
        enumerator.match_subsystem("leds").map_err(|err| {
            warn!("{}", err);
            PlatformError::Udev("match_subsystem failed".into(), err)
        })?;

        for device in enumerator.scan_devices().map_err(|err| {
            warn!("{}", err);
            PlatformError::Udev("scan_devices failed".into(), err)
        })? {
            let name = device.sysname().to_string_lossy();
            if name == "asus::dialpad" || name == "asus_dialpad" || name.contains("dialpad") {
                info!(
                    "Found hardware DialPad LED device at {:?}",
                    device.syspath()
                );
                return Ok(Self {
                    path: device.syspath().to_path_buf(),
                    wmi_dev_id_path,
                    mode: DialpadMode::Hardware,
                    is_hardware_capable: true,
                });
            }
        }

        let fallback_path = PathBuf::from("/sys/class/leds/asus::dialpad");
        if fallback_path.exists() {
            info!(
                "Found hardware DialPad LED at fallback path {:?}",
                fallback_path
            );
            return Ok(Self {
                path: fallback_path,
                wmi_dev_id_path,
                mode: DialpadMode::Hardware,
                is_hardware_capable: true,
            });
        }

        // If hardware LED is missing, check if an ASUS Touchpad input device exists
        if Self::has_asus_touchpad()? {
            info!("Physical DialPad LED not found, but ASUS Touchpad detected. Initializing VirtualSoftware mode.");
            return Ok(Self {
                path: PathBuf::new(),
                wmi_dev_id_path,
                mode: DialpadMode::VirtualSoftware,
                is_hardware_capable: false,
            });
        }

        Err(PlatformError::MissingFunction(
            "Neither hardware DialPad nor ASUS touchpad found".into(),
        ))
    }

    /// Check if any ASUS/ELAN/Synaptics touchpad device exists on the system.
    pub fn has_asus_touchpad() -> Result<bool> {
        let mut enumerator = udev::Enumerator::new().map_err(|err| {
            warn!("{}", err);
            PlatformError::Udev("enumerator failed".into(), err)
        })?;
        enumerator.match_subsystem("input").map_err(|err| {
            warn!("{}", err);
            PlatformError::Udev("match_subsystem failed".into(), err)
        })?;

        for device in enumerator.scan_devices().map_err(|err| {
            warn!("{}", err);
            PlatformError::Udev("scan_devices failed".into(), err)
        })? {
            let sysname = device.sysname().to_string_lossy();
            if sysname.starts_with("event") {
                if let Some(parent) = device.parent() {
                    let name = parent.sysname().to_string_lossy().to_lowercase();
                    if name.contains("touchpad")
                        || name.contains("elan")
                        || name.contains("synaptics")
                        || name.contains("asue")
                    {
                        info!("Found ASUS touchpad device: {:?}", parent.syspath());
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn mode(&self) -> DialpadMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: DialpadMode) {
        self.mode = mode;
    }

    pub fn is_hardware_capable(&self) -> bool {
        self.is_hardware_capable
    }

    /// Calculate the radial rotational delta Δθ (in radians) given consecutive touch points
    /// (x1, y1) and (x2, y2) relative to a center point (cx, cy).
    pub fn calculate_radial_delta(x1: f64, y1: f64, x2: f64, y2: f64, cx: f64, cy: f64) -> f64 {
        let angle1 = (y1 - cy).atan2(x1 - cx);
        let angle2 = (y2 - cy).atan2(x2 - cx);
        let mut delta = angle2 - angle1;
        if delta > std::f64::consts::PI {
            delta -= 2.0 * std::f64::consts::PI;
        } else if delta < -std::f64::consts::PI {
            delta += 2.0 * std::f64::consts::PI;
        }
        delta
    }

    /// Send WMI command `0x00100063` to toggle hardware DialPad ACPI state.
    pub fn set_wmi_hardware_state(&self, enabled: bool) -> Result<()> {
        if let Some(ref wmi_path) = self.wmi_dev_id_path {
            let val = if enabled { 1 } else { 0 };
            let cmd = format!("{ASUS_WMI_DEVID_DIALPAD:#x} {val}");
            info!("Sending WMI command to {wmi_path:?}: {cmd}");
            fs::write(wmi_path, &cmd).map_err(|e| {
                warn!("WMI DialPad toggle write failed: {e}");
                PlatformError::IoPath(wmi_path.to_string_lossy().into(), e)
            })?;
        }
        Ok(())
    }
}
