#![no_std]
#![forbid(unsafe_code)]
#![doc = "Chip-level integration between the ESP32-C6 HAL and Embassy runtime."]

use embedded_sdk_core::{Architecture, Capabilities};

/// Stable chip identifier used by manifests and telemetry.
pub const CHIP: &str = "esp32c6";

/// Architecture implemented by the ESP32-C6 high-performance core.
pub const ARCHITECTURE: Architecture = Architecture::RiscV32;

/// Sign-extends an eight-bit RSSI/noise field from the pinned radio bindings.
///
/// The generated C6 bitfield getters return zero-extended values in an `i32`;
/// already sign-extended values are accepted too. Apply before aggregation.
pub const fn signed_rx_dbm(raw: i32) -> i32 {
    raw as i8 as i32
}

/// Capabilities provided by ESP32-C6 silicon.
pub const CAPABILITIES: Capabilities = Capabilities::WIFI
    .union(Capabilities::BLE)
    .union(Capabilities::IEEE_802_15_4)
    .union(Capabilities::HARDWARE_RNG)
    .union(Capabilities::CRYPTO_ACCELERATION);

/// ESP32-C6 implementation of the portable Wi-Fi contracts.
#[cfg(all(target_arch = "riscv32", feature = "wifi"))]
pub mod wifi;

/// ESP32-C6 Bluetooth Low Energy controller adapter.
#[cfg(all(target_arch = "riscv32", feature = "bluetooth"))]
pub mod bluetooth;

/// Initializes the Embassy executor and time driver on ESP32-C6.
///
/// The caller owns chip initialization and passes the two peripheral tokens
/// reserved by the runtime. Keeping that ownership visible prevents a platform
/// library from silently taking peripherals needed by an application.
#[cfg(target_arch = "riscv32")]
pub fn start_embassy(
    timer_group: esp_hal::peripherals::TIMG0<'static>,
    software_interrupt: esp_hal::peripherals::SW_INTERRUPT<'static>,
) {
    use esp_hal::{interrupt::software::SoftwareInterruptControl, timer::timg::TimerGroup};

    let software_interrupts = SoftwareInterruptControl::new(software_interrupt);
    let timers = TimerGroup::new(timer_group);
    esp_rtos::start(timers.timer0, software_interrupts.software_interrupt0);
}

#[cfg(test)]
mod tests {
    use super::signed_rx_dbm;

    #[test]
    fn receive_signal_fields_are_signed_before_averaging() {
        assert_eq!(signed_rx_dbm(185), -71);
        assert_eq!(signed_rx_dbm(195), -61);
        assert_eq!(signed_rx_dbm(202), -54);
        assert_eq!(signed_rx_dbm(164), -92);
        for value in -128..=127 {
            assert_eq!(signed_rx_dbm(value), value);
            assert_eq!(signed_rx_dbm(value as u8 as i32), value);
        }
    }
}
