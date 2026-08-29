#![no_std]
#![forbid(unsafe_code)]
#![doc = "Chip-level integration between the ESP32-C6 HAL and Embassy runtime."]

use embedded_sdk_core::{Architecture, Capabilities};

/// Stable chip identifier used by manifests and telemetry.
pub const CHIP: &str = "esp32c6";

/// Architecture implemented by the ESP32-C6 high-performance core.
pub const ARCHITECTURE: Architecture = Architecture::RiscV32;

/// Capabilities provided by ESP32-C6 silicon.
pub const CAPABILITIES: Capabilities = Capabilities::WIFI
    .union(Capabilities::BLE)
    .union(Capabilities::IEEE_802_15_4)
    .union(Capabilities::HARDWARE_RNG)
    .union(Capabilities::CRYPTO_ACCELERATION);

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
