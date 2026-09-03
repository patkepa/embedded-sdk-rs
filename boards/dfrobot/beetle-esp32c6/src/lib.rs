#![no_std]
#![forbid(unsafe_code)]
#![doc = "Board support for the DFRobot Beetle ESP32-C6."]

use embedded_sdk_core::{BoardSupport, Capabilities, HardwareDescriptor};
use embedded_sdk_platform_esp32c6 as platform;

#[cfg(target_arch = "riscv32")]
mod battery;
#[cfg(target_arch = "riscv32")]
pub use battery::{BatteryMonitorError, BeetleBatteryMonitor};

/// Stable board support type for the DFRobot Beetle ESP32-C6.
pub struct BeetleEsp32c6;

impl BoardSupport for BeetleEsp32c6 {
    const HARDWARE: HardwareDescriptor = HardwareDescriptor {
        board: "beetle-esp32c6",
        chip: platform::CHIP,
        manufacturer: "DFRobot",
        architecture: platform::ARCHITECTURE,
        capabilities: platform::CAPABILITIES.union(Capabilities::BATTERY_VOLTAGE_MONITORING),
    };
}

/// Static descriptor for the DFRobot Beetle ESP32-C6.
pub const HARDWARE: HardwareDescriptor = BeetleEsp32c6::HARDWARE;

/// GPIO0 / ADC1 channel 0, wired to the battery divider midpoint.
pub const BATTERY_SENSE_GPIO: u8 = 0;
/// Numerator used to recover battery voltage from measured divider voltage.
pub const BATTERY_DIVIDER_NUMERATOR: u32 = 2;
/// Denominator used to recover battery voltage from measured divider voltage.
pub const BATTERY_DIVIDER_DENOMINATOR: u32 = 1;
/// Number of calibrated ADC readings averaged into one battery measurement.
pub const BATTERY_SAMPLE_COUNT: usize = 8;

#[cfg(test)]
mod tests {
    use embedded_sdk_core::Capabilities;

    use super::{
        BATTERY_DIVIDER_DENOMINATOR, BATTERY_DIVIDER_NUMERATOR, BATTERY_SENSE_GPIO, HARDWARE,
    };

    #[test]
    fn descriptor_exposes_voltage_monitoring() {
        assert_eq!(HARDWARE.board, "beetle-esp32c6");
        assert_eq!(HARDWARE.manufacturer, "DFRobot");
        assert!(
            HARDWARE
                .capabilities
                .contains(Capabilities::BATTERY_VOLTAGE_MONITORING)
        );
    }

    #[test]
    fn battery_wiring_matches_both_documented_board_revisions() {
        assert_eq!(BATTERY_SENSE_GPIO, 0);
        assert_eq!(
            (BATTERY_DIVIDER_NUMERATOR, BATTERY_DIVIDER_DENOMINATOR),
            (2, 1)
        );
    }
}
