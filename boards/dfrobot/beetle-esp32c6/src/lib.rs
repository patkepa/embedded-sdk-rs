#![no_std]
#![forbid(unsafe_code)]
#![doc = "Board metadata for DFRobot Beetle ESP32-C6, SKU DFR1117."]

use embedded_sdk_core::{BoardSupport, HardwareDescriptor};
use embedded_sdk_platform_esp32c6 as platform;

/// DFRobot Beetle ESP32-C6 board.
pub struct BeetleEsp32c6;

impl BoardSupport for BeetleEsp32c6 {
    const HARDWARE: HardwareDescriptor = HardwareDescriptor {
        board: "beetle-esp32c6",
        chip: platform::CHIP,
        manufacturer: "DFRobot",
        architecture: platform::ARCHITECTURE,
        capabilities: platform::CAPABILITIES,
    };
}

/// Static hardware descriptor.
pub const HARDWARE: HardwareDescriptor = BeetleEsp32c6::HARDWARE;
/// Onboard user LED GPIO, per DFR1117 pinout.
pub const USER_LED_GPIO: u8 = 15;
/// Active-low BOOT button GPIO.
pub const BOOT_BUTTON_GPIO: u8 = 9;
/// Integrated flash capacity in bytes.
pub const FLASH_BYTES: usize = 4 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_sdk_core::Capabilities;

    #[test]
    fn beetle_hardware_contract() {
        assert_eq!(HARDWARE.board, "beetle-esp32c6");
        assert!(HARDWARE.capabilities.contains(Capabilities::WIFI));
        assert_eq!(
            (USER_LED_GPIO, BOOT_BUTTON_GPIO, FLASH_BYTES),
            (15, 9, 4_194_304)
        );
    }
}
