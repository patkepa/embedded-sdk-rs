#![no_std]
#![no_main]
#![doc = "Battery-monitoring reference firmware for the DFRobot Beetle ESP32-C6."]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embedded_sdk_board_beetle_esp32c6::{BeetleBatteryMonitor, HARDWARE};
use embedded_sdk_platform_esp32c6::start_embassy;
use embedded_sdk_power::{BatteryMonitor, VoltageCurve, VoltagePoint};
use esp_backtrace as _;

const BATTERY_REPORT_INTERVAL: Duration = Duration::from_secs(30);

// An intentionally generic single-cell Li-ion/LiPo profile. Products should
// replace it with points characterized for their cell, load, and temperature.
const BATTERY_PROFILE_POINTS: [VoltagePoint; 11] = [
    VoltagePoint::new(3_300, 0),
    VoltagePoint::new(3_600, 10),
    VoltagePoint::new(3_700, 20),
    VoltagePoint::new(3_750, 30),
    VoltagePoint::new(3_790, 40),
    VoltagePoint::new(3_830, 50),
    VoltagePoint::new(3_870, 60),
    VoltagePoint::new(3_920, 70),
    VoltagePoint::new(3_980, 80),
    VoltagePoint::new(4_070, 90),
    VoltagePoint::new(4_200, 100),
];

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 16 * 1024);
    start_embassy(peripherals.TIMG0, peripherals.SW_INTERRUPT);

    let mut battery = BeetleBatteryMonitor::new(peripherals.ADC1, peripherals.GPIO0);
    let profile = VoltageCurve::new(&BATTERY_PROFILE_POINTS)
        .expect("the static Beetle battery profile must be valid");

    esp_println::println!(
        "embedded-sdk boot: board={}, chip={}",
        HARDWARE.board,
        HARDWARE.chip
    );
    esp_println::println!(
        "battery state-of-charge is a terminal-voltage estimate; charge status is unavailable"
    );

    loop {
        match battery.measure() {
            Ok(measurement) => {
                let estimate = profile.estimate(measurement.voltage());
                esp_println::println!(
                    "battery: voltage_mv={}, estimated_percent={}, charge_state={:?}",
                    measurement.voltage().get(),
                    estimate.percentage().get(),
                    measurement.charge_state()
                );
            }
            Err(error) => esp_println::println!("battery measurement failed: {error}"),
        }

        Timer::after(BATTERY_REPORT_INTERVAL).await;
    }
}
