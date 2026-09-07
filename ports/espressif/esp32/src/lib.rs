#![no_std]
#![forbid(unsafe_code)]
#![doc = "Chip-level integration for the original Xtensa ESP32 used by ESP32-WROOM modules."]

use embedded_sdk_core::{Architecture, Capabilities};

/// Stable chip identifier used by manifests and telemetry.
pub const CHIP: &str = "esp32";

/// Architecture implemented by the original ESP32's application cores.
pub const ARCHITECTURE: Architecture = Architecture::Xtensa;

/// Capabilities provided by the original ESP32 silicon.
pub const CAPABILITIES: Capabilities = Capabilities::WIFI
    .union(Capabilities::BLE)
    .union(Capabilities::HARDWARE_RNG)
    .union(Capabilities::CRYPTO_ACCELERATION);

/// Original ESP32 I2S-parallel and DMA integration for HUB75 RGB matrices.
#[cfg(all(target_arch = "xtensa", feature = "hub75"))]
pub mod hub75;

#[cfg(test)]
mod tests {
    use embedded_sdk_core::{Architecture, Capabilities};

    use super::{ARCHITECTURE, CAPABILITIES, CHIP};

    #[test]
    fn describes_original_esp32_wroom_silicon() {
        assert_eq!(CHIP, "esp32");
        assert_eq!(ARCHITECTURE, Architecture::Xtensa);
        assert!(CAPABILITIES.contains(Capabilities::WIFI));
        assert!(CAPABILITIES.contains(Capabilities::BLE));
        assert!(!CAPABILITIES.contains(Capabilities::IEEE_802_15_4));
    }
}
