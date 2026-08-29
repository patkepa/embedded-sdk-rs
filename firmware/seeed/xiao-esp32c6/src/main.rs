#![no_std]
#![no_main]
#![doc = "Reference Embassy firmware for the Seeed Studio XIAO ESP32C6."]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embedded_sdk_board_xiao_esp32c6::HARDWARE;
use embedded_sdk_platform_esp32c6::start_embassy;
use esp_backtrace as _;
use esp_hal::gpio::{Level, Output, OutputConfig};

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    start_embassy(peripherals.TIMG0, peripherals.SW_INTERRUPT);

    // The XIAO user LED is wired active-low, so High is the initial off state.
    let mut user_led = Output::new(peripherals.GPIO15, Level::High, OutputConfig::default());

    esp_println::println!(
        "embedded-sdk boot: board={}, chip={}",
        HARDWARE.board,
        HARDWARE.chip
    );

    let mut heartbeat = 0_u64;
    loop {
        user_led.toggle();
        esp_println::println!("embedded-sdk heartbeat={heartbeat}");
        heartbeat = heartbeat.wrapping_add(1);
        Timer::after(Duration::from_secs(1)).await;
    }
}
