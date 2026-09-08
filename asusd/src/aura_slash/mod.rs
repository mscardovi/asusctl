use std::sync::Arc;

use config::SlashConfig;
use rog_platform::slash_led::SlashLed;
use rog_platform::usb_raw::USBRaw;
use rog_slash::usb::{slash_pkt_enable, slash_pkt_init, slash_pkt_options, slash_pkt_set_mode};
use tokio::sync::{Mutex, MutexGuard};

use crate::error::RogError;

pub mod config;
pub mod trait_impls;

#[derive(Debug, Clone)]
pub struct Slash {
    led: Option<SlashLed>,
    usb: Option<Arc<Mutex<USBRaw>>>,
    config: Arc<Mutex<SlashConfig>>,
}

impl Slash {
    pub fn new(
        led: Option<SlashLed>,
        usb: Option<Arc<Mutex<USBRaw>>>,
        config: Arc<Mutex<SlashConfig>>,
    ) -> Self {
        Self { led, usb, config }
    }

    pub fn led(&self) -> Option<&SlashLed> {
        self.led.as_ref()
    }

    pub async fn lock_config(&self) -> MutexGuard<'_, SlashConfig> {
        self.config.lock().await
    }

    pub async fn write_bytes(&self, message: &[u8]) -> Result<(), RogError> {
        if let Some(usb) = &self.usb {
            usb.lock().await.write_bytes(message)?;
        }
        Ok(())
    }

    /// Initialise the device if required. Locks the internal config so be wary
    /// of deadlocks.
    pub async fn do_initialization(&self) -> Result<(), RogError> {
        let config = self.config.lock().await;

        if let Some(led) = &self.led {
            let brightness = if config.enabled { config.brightness } else { 0 };
            led.set_brightness(brightness)?;
            led.set_slash_interval(config.display_interval)?;
            led.set_slash_mode(&config.display_mode.to_string())?;
            return Ok(());
        }

        if let Some(usb) = &self.usb {
            for pkt in &slash_pkt_init(config.slash_type) {
                usb.lock().await.write_bytes(pkt)?;
            }
            usb.lock()
                .await
                .write_bytes(&slash_pkt_enable(config.slash_type, config.enabled))?;

            // Apply config upon initialization
            let option_packets = slash_pkt_options(
                config.slash_type,
                config.enabled,
                config.brightness,
                config.display_interval,
            );
            usb.lock().await.write_bytes(&option_packets)?;

            let mode_packets = slash_pkt_set_mode(config.slash_type, config.display_mode);
            usb.lock().await.write_bytes(&mode_packets[1])?;
        }

        Ok(())
    }
}
