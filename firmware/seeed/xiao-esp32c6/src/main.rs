#![no_std]
#![no_main]
#![doc = "Reference Embassy firmware for the Seeed Studio XIAO ESP32C6."]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embedded_sdk_board_xiao_esp32c6::HARDWARE;
use embedded_sdk_platform_esp32c6::{start_embassy, wifi::Esp32c6Wifi};
use embedded_sdk_wifi::{
    Authentication, ConfigError, Passphrase, ScanSummary, Ssid, StationConfig,
};
use esp_backtrace as _;
use esp_hal::gpio::{Level, Output, OutputConfig};

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 72 * 1024);
    start_embassy(peripherals.TIMG0, peripherals.SW_INTERRUPT);

    // The XIAO user LED is wired active-low, so High is the initial off state.
    let mut user_led = Output::new(peripherals.GPIO15, Level::High, OutputConfig::default());
    // GPIO3 enables the RF switch and GPIO14 selects the on-board antenna.
    // Keep both outputs alive for the lifetime of the radio.
    let _rf_switch_enable = Output::new(peripherals.GPIO3, Level::Low, OutputConfig::default());
    let _rf_switch_select = Output::new(peripherals.GPIO14, Level::Low, OutputConfig::default());

    esp_println::println!(
        "embedded-sdk boot: board={}, chip={}",
        HARDWARE.board,
        HARDWARE.chip
    );

    let mut wifi = match Esp32c6Wifi::new(peripherals.WIFI) {
        Ok(wifi) => wifi,
        Err(error) => {
            esp_println::println!("embedded-sdk wifi init failed: {error}");
            loop {
                Timer::after(Duration::from_secs(30)).await;
            }
        }
    };

    match wifi.scan(20).await {
        Ok(access_points) => {
            let summary = ScanSummary::from_access_points(&access_points);
            esp_println::println!(
                "embedded-sdk wifi scan: access_points={}, strongest_signal_dbm={:?}",
                summary.access_points,
                summary.strongest_signal_dbm
            );
        }
        Err(error) => esp_println::println!("embedded-sdk wifi scan failed: {error}"),
    }

    match development_station_config() {
        Ok(Some(station)) => {
            if let Err(error) = wifi.configure_station(&station) {
                esp_println::println!("embedded-sdk wifi configuration failed: {error}");
            } else {
                match wifi.connect().await {
                    Ok(connected) => esp_println::println!(
                        "embedded-sdk wifi associated: channel={}, security={:?}",
                        connected.channel,
                        connected.security
                    ),
                    Err(error) => {
                        esp_println::println!("embedded-sdk wifi association failed: {error}")
                    }
                }
            }
        }
        Ok(None) => esp_println::println!(
            "embedded-sdk wifi station: credentials not configured; scan-only mode"
        ),
        Err(error) => esp_println::println!("embedded-sdk wifi credential error: {error}"),
    }

    let mut heartbeat = 0_u64;
    loop {
        user_led.toggle();
        esp_println::println!(
            "embedded-sdk heartbeat={heartbeat}, wifi_state={:?}",
            wifi.state()
        );
        heartbeat = heartbeat.wrapping_add(1);
        Timer::after(Duration::from_secs(1)).await;
    }
}

fn development_station_config() -> Result<Option<StationConfig>, ConfigError> {
    match (option_env!("WIFI_SSID"), option_env!("WIFI_PASSWORD")) {
        (None, None) => Ok(None),
        (Some(ssid), None) => {
            let ssid = Ssid::try_from(ssid)?;
            StationConfig::open(ssid).map(Some)
        }
        (Some(ssid), Some(password)) => {
            let ssid = Ssid::try_from(ssid)?;
            let passphrase = Passphrase::new(password)?;
            StationConfig::personal(ssid, passphrase, Authentication::Wpa2Wpa3Personal).map(Some)
        }
        (None, Some(_)) => Err(ConfigError::EmptySsid),
    }
}
