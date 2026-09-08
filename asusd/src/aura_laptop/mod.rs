use std::sync::Arc;

use config::AuraConfig;
use config_traits::StdConfig;
use log::info;
use log::{debug, warn};
use rog_aura::keyboard::AuraLaptopUsbPackets;
use rog_aura::{AuraDeviceType, AuraEffect, LedBrightness};
use rog_platform::DynamicLed;
use rog_platform::keyboard_led::KeyboardBacklight;
use tokio::sync::{Mutex, MutexGuard};

use crate::error::RogError;

pub mod config;
pub mod trait_impls;

#[derive(Debug, Clone)]
pub struct Aura {
    pub dynamic_global: Option<Arc<Mutex<DynamicLed>>>,
    pub dynamic_kbd: Option<Arc<Mutex<DynamicLed>>>,
    pub dynamic_lightbar: Option<Arc<Mutex<DynamicLed>>>,
    pub backlight: Option<Arc<Mutex<KeyboardBacklight>>>,
    pub config: Arc<Mutex<AuraConfig>>,
}

impl Aura {
    #[must_use]
    pub fn has_dynamic_lighting(&self) -> bool {
        self.dynamic_kbd.is_some() || self.dynamic_global.is_some()
    }

    /// Initialise the device if required.
    pub async fn do_initialization(&self) -> Result<(), RogError> {
        Ok(())
    }

    pub async fn lock_config(&self) -> MutexGuard<'_, AuraConfig> {
        self.config.lock().await
    }

    /// Will lock the internal config and update. If anything else has locked
    /// this in scope then a deadlock can occur.
    pub async fn update_config(&self) -> Result<(), RogError> {
        let mut config = self.config.lock().await;
        let bright = if let Some(dynamic) = self.dynamic_global.as_ref() {
            dynamic.lock().await.get_brightness().unwrap_or_default()
        } else if let Some(dynamic) = self.dynamic_kbd.as_ref() {
            dynamic.lock().await.get_brightness().unwrap_or_default()
        } else if let Some(bl) = self.backlight.as_ref() {
            bl.lock().await.get_brightness().unwrap_or_default()
        } else {
            config.brightness.into()
        };
        config.read();
        config.brightness = bright.into();
        config.write();
        Ok(())
    }

    pub async fn write_current_config_mode(&self, config: &mut AuraConfig) -> Result<(), RogError> {
        if config.multizone_on {
            let mode = config.current_mode;
            let mut create = false;
            // There is no multizone config for this mode so create one here
            // using the colours of rainbow if it exists, or first available
            // mode, or random
            if config.multizone.is_none() {
                create = true;
            } else if let Some(multizones) = config.multizone.as_ref()
                && !multizones.contains_key(&mode)
            {
                create = true;
            }
            if create {
                info!("No user-set config for zone founding, attempting a default");
                config.create_multizone_default()?;
            }

            if let Some(multizones) = config.multizone.as_mut()
                && let Some(set) = multizones.get(&mode)
            {
                for mode in set.clone() {
                    self.write_effect_and_apply(config.led_type, &mode).await?;
                }
            }
        } else {
            let mode = config.current_mode;
            if let Some(effect) = config.builtins.get(&mode).cloned() {
                self.write_effect_and_apply(config.led_type, &effect)
                    .await?;
            }
        }

        Ok(())
    }

    /// Write the AuraEffect to the device. Will lock `backlight` or `hid`.
    ///
    /// If per-key or software-mode is active it must be marked as disabled in
    /// config.
    pub async fn write_effect_and_apply(
        &self,
        dev_type: AuraDeviceType,
        mode: &AuraEffect,
    ) -> Result<(), RogError> {
        // Priority: Dynamic Lighting sysfs interface
        if self.has_dynamic_lighting()
            && let Some(eff_str) = mode.mode.to_dynamic_effect_str()
        {
            let speed = mode.speed.to_dynamic_speed();
            let dir_str = mode.direction.to_dynamic_direction_str();
            let palette = mode.to_dynamic_palette();

            let apply_to_led = |led: &DynamicLed| -> bool {
                if led.is_effect_supported(eff_str) {
                    let _ = led.set_speed(speed);
                    let _ = led.set_direction(dir_str);
                    let _ = led.set_palette_colors(&palette);
                    if let Err(e) = led.set_effect(eff_str) {
                        warn!("Failed to set dynamic lighting effect '{eff_str}': {e}");
                        false
                    } else {
                        true
                    }
                } else {
                    debug!(
                        "Dynamic lighting effect '{eff_str}' not supported by kernel, trying fallback"
                    );
                    false
                }
            };

            match mode.zone {
                rog_aura::AuraZone::BarLeft | rog_aura::AuraZone::BarRight => {
                    if let Some(lb) = &self.dynamic_lightbar {
                        let led = lb.lock().await;
                        if apply_to_led(&led) {
                            return Ok(());
                        }
                    } else {
                        debug!(
                            "Skipping unsupported lightbar zone in dynamic lighting unified mode"
                        );
                        return Ok(());
                    }
                }
                rog_aura::AuraZone::None => {
                    if let Some(global) = &self.dynamic_global {
                        let global_led = global.lock().await;
                        if apply_to_led(&global_led) {
                            return Ok(());
                        }
                    }
                    if let Some(kbd) = &self.dynamic_kbd {
                        let kbd_led = kbd.lock().await;
                        let kbd_ok = apply_to_led(&kbd_led);
                        if let Some(lb) = &self.dynamic_lightbar {
                            let lb_led = lb.lock().await;
                            let _ = apply_to_led(&lb_led);
                        }
                        if kbd_ok {
                            return Ok(());
                        }
                    }
                }
                _ => {
                    if let Some(kbd) = &self.dynamic_kbd {
                        let kbd_led = kbd.lock().await;
                        if apply_to_led(&kbd_led) {
                            return Ok(());
                        }
                    } else if let Some(global) = &self.dynamic_global {
                        let global_led = global.lock().await;
                        if apply_to_led(&global_led) {
                            return Ok(());
                        }
                    }
                }
            }
        }

        // When Dynamic Lighting is active, do not fall back to raw hidraw or TUF platform
        if self.has_dynamic_lighting() {
            return Err(RogError::MissingFunction(
                "Dynamic lighting mode or zone not supported by kernel".to_string(),
            ));
        }

        // Fallback: TUF platform sysfs backlight
        if matches!(dev_type, AuraDeviceType::LaptopKeyboardTuf)
            && let Some(platform) = &self.backlight
        {
            let buf = [
                1, mode.mode as u8, mode.colour1.r, mode.colour1.g, mode.colour1.b,
                mode.speed as u8,
            ];
            platform.lock().await.set_kbd_rgb_mode(&buf)?;
            return Ok(());
        }

        Err(RogError::NoAuraKeyboard)
    }

    pub async fn set_brightness(&self, value: u8) -> Result<(), RogError> {
        let mut updated = false;
        if let Some(dynamic_global) = &self.dynamic_global
            && dynamic_global.lock().await.set_brightness(value).is_ok()
        {
            updated = true;
        }
        if let Some(dynamic_kbd) = &self.dynamic_kbd
            && dynamic_kbd.lock().await.set_brightness(value).is_ok()
        {
            updated = true;
        }
        if let Some(dynamic_lb) = &self.dynamic_lightbar {
            let _ = dynamic_lb.lock().await.set_brightness(value);
        }
        if updated {
            return Ok(());
        }

        if let Some(backlight) = &self.backlight {
            backlight.lock().await.set_brightness(value)?;
            return Ok(());
        }
        Err(RogError::MissingFunction(
            "No LED backlight control available".to_string(),
        ))
    }

    /// Set combination state for boot animation/sleep animation/all leds/keys
    /// leds/side leds LED active
    pub async fn set_power_states(&self, config: &AuraConfig) -> Result<(), RogError> {
        if let Some(dynamic_led) = self.dynamic_global.as_ref().or(self.dynamic_kbd.as_ref()) {
            let kbd = dynamic_led.lock().await;
            if kbd.has_power_states() {
                let mut states = Vec::new();
                for state in &config.enabled.states {
                    if state.boot {
                        states.push("boot");
                    }
                    if state.awake {
                        states.push("awake");
                    }
                    if state.sleep {
                        states.push("sleep");
                    }
                    if state.shutdown {
                        states.push("shutdown");
                    }
                }
                let states_str = states.join(" ");
                if let Err(e) = kbd.set_power_states(&states_str) {
                    warn!("Failed to set power states via dynamic lighting: {e}");
                } else {
                    return Ok(());
                }
            }
        }

        if matches!(config.led_type, rog_aura::AuraDeviceType::LaptopKeyboardTuf)
            && let Some(backlight) = &self.backlight
        {
            // TODO: tuf bool array
            let buf = config.enabled.to_bytes(config.led_type);
            backlight.lock().await.set_kbd_rgb_state(&buf)?;
        }
        Ok(())
    }

    /// Write an effect block. This is for per-key, but can be repurposed to
    /// write the raw factory mode packets - when doing this it is expected that
    /// only the first `Vec` (`effect[0]`) is valid.
    pub async fn write_effect_block(
        &self,
        config: &mut AuraConfig,
        effect: &AuraLaptopUsbPackets,
    ) -> Result<(), RogError> {
        if config.brightness == LedBrightness::Off {
            config.brightness = LedBrightness::Med;
            config.write();
        }

        if matches!(config.led_type, rog_aura::AuraDeviceType::LaptopKeyboardTuf)
            && let Some(tuf) = &self.backlight
        {
            for row in effect.iter() {
                let r = row[9];
                let g = row[10];
                let b = row[11];
                tuf.lock().await.set_kbd_rgb_mode(&[
                    0, 0, r, g, b, 0,
                ])?;
            }
            return Ok(());
        }

        let dynamic_led = self.dynamic_kbd.as_ref().or(self.dynamic_global.as_ref());
        if let Some(dynamic) = dynamic_led {
            let dynamic = dynamic.lock().await;
            let led_count = dynamic.get_led_count().unwrap_or(168) as usize;
            let expected_len = led_count * 3;
            let mut rgb_buf = Vec::with_capacity(expected_len);

            if effect.len() == 1 && led_count == 4 {
                // Zoned keyboard (4 zones: left, left-mid, right-mid, right)
                if let Some(row) = effect.first()
                    && let Some(payload) = row.get(9..21)
                {
                    rgb_buf.extend_from_slice(payload);
                }
            } else {
                // Per-key keyboard
                for row in effect.iter() {
                    if row.len() >= 9 {
                        let num_leds = row.get(7).copied().unwrap_or(16) as usize;
                        let payload_len = num_leds * 3;
                        if let Some(payload) = row.get(9..9 + payload_len) {
                            rgb_buf.extend_from_slice(payload);
                        }
                    }
                }
            }

            if !rgb_buf.is_empty() {
                rgb_buf.resize(expected_len, 0);
                dynamic.write_direct(&rgb_buf)?;
                config.per_key_mode_active = true;
            }
            return Ok(());
        }

        Err(RogError::NoAuraKeyboard)
    }

    pub async fn fix_ally_power(&mut self) -> Result<(), RogError> {
        Ok(())
    }
}
