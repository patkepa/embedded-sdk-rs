#![no_std]
#![no_main]
#![doc = "Reference Embassy firmware for the Seeed Studio XIAO ESP32C6."]
#![allow(
    clippy::needless_borrows_for_generic_args,
    reason = "TrouBLE 0.6 GATT derive macros emit this pattern"
)]

use embassy_executor::Spawner;
use embassy_futures::{join::join, select::select};
use embassy_time::{Duration, Timer};
use embedded_sdk_bluetooth::{DeviceName, PeripheralConfig as SdkBluetoothConfig};
use embedded_sdk_board_xiao_esp32c6::{BLUETOOTH_DEVICE_NAME, HARDWARE};
use embedded_sdk_platform_esp32c6::{
    bluetooth::{BluetoothConnector, Esp32c6Bluetooth, static_random_address},
    start_embassy,
    wifi::Esp32c6Wifi,
};
use embedded_sdk_wifi::{
    Authentication, ConfigError, Passphrase, ReconnectBackoff, ScanSummary, Ssid, StationConfig,
};
use esp_backtrace as _;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::rng::Rng;
use trouble_host::prelude::*;

const BLUETOOTH_CONNECTIONS_MAX: usize = 1;
// One signaling channel and one ATT channel per connection.
const BLUETOOTH_L2CAP_CHANNELS_MAX: usize = 2;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 96 * 1024);
    start_embassy(peripherals.TIMG0, peripherals.SW_INTERRUPT);

    // The XIAO user LED is wired active-low, so High is the initial off state.
    let user_led = Output::new(peripherals.GPIO15, Level::High, OutputConfig::default());
    // GPIO3 enables the RF switch and GPIO14 selects the on-board antenna.
    // Keep both outputs alive for the lifetime of the radio.
    let _rf_switch_enable = Output::new(peripherals.GPIO3, Level::Low, OutputConfig::default());
    let _rf_switch_select = Output::new(peripherals.GPIO14, Level::Low, OutputConfig::default());

    esp_println::println!(
        "embedded-sdk boot: board={}, chip={}",
        HARDWARE.board,
        HARDWARE.chip
    );
    match heartbeat(user_led) {
        Ok(task) => spawner.spawn(task),
        Err(_) => esp_println::println!("embedded-sdk heartbeat task allocation failed"),
    }

    let bluetooth_name = match DeviceName::new(BLUETOOTH_DEVICE_NAME) {
        Ok(name) => name,
        Err(error) => {
            esp_println::println!("embedded-sdk bluetooth configuration failed: {error}");
            loop {
                Timer::after(Duration::from_secs(30)).await;
            }
        }
    };
    let bluetooth_config = SdkBluetoothConfig::new(bluetooth_name, static_random_address());
    match Esp32c6Bluetooth::new(peripherals.BT) {
        Ok(bluetooth) => match bluetooth_task(bluetooth.into_connector(), bluetooth_config) {
            Ok(task) => spawner.spawn(task),
            Err(_) => esp_println::println!("embedded-sdk bluetooth task allocation failed"),
        },
        Err(error) => esp_println::println!("embedded-sdk bluetooth init failed: {error}"),
    }

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
                supervise_station(&mut wifi).await;
            }
        }
        Ok(None) => esp_println::println!(
            "embedded-sdk wifi station: credentials not configured; scan-only mode"
        ),
        Err(error) => esp_println::println!("embedded-sdk wifi credential error: {error}"),
    }

    loop {
        Timer::after(Duration::from_secs(30)).await;
    }
}

#[embassy_executor::task]
async fn heartbeat(mut user_led: Output<'static>) {
    let mut heartbeat = 0_u64;
    loop {
        user_led.toggle();
        esp_println::println!("embedded-sdk heartbeat={heartbeat}");
        heartbeat = heartbeat.wrapping_add(1);
        Timer::after(Duration::from_secs(1)).await;
    }
}

#[gatt_server]
struct BluetoothServer {
    sdk: SdkService,
}

#[gatt_service(uuid = "7a1e1000-4c2a-4f66-a1d4-3f55b55a1000")]
struct SdkService {
    /// Monotonic wrapping status value proving GATT reads and notifications.
    #[characteristic(uuid = "7a1e1001-4c2a-4f66-a1d4-3f55b55a1000", read, notify, value = 0)]
    status: u8,
}

#[embassy_executor::task]
async fn bluetooth_task(connector: BluetoothConnector<'static>, config: SdkBluetoothConfig) {
    let controller: ExternalController<_, 20> = ExternalController::new(connector);
    let mut resources: HostResources<
        DefaultPacketPool,
        BLUETOOTH_CONNECTIONS_MAX,
        BLUETOOTH_L2CAP_CHANNELS_MAX,
    > = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(Address::random(config.address().to_hci_bytes()));
    let Host {
        mut peripheral,
        runner,
        ..
    } = stack.build();

    let server = match BluetoothServer::new_with_config(GapConfig::Peripheral(
        trouble_host::gap::PeripheralConfig {
            name: config.name().as_str(),
            appearance: &appearance::sensor::GENERIC_SENSOR,
        },
    )) {
        Ok(server) => server,
        Err(_) => {
            esp_println::println!("embedded-sdk bluetooth GATT server configuration failed");
            return;
        }
    };

    esp_println::println!(
        "embedded-sdk bluetooth ready: name={}",
        config.name().as_str()
    );
    let _ = join(bluetooth_runner(runner), async {
        loop {
            match advertise_bluetooth(config.name().as_str(), &mut peripheral, &server).await {
                Ok(connection) => {
                    esp_println::println!("embedded-sdk bluetooth connected");
                    let _ = select(
                        bluetooth_gatt_events(&server, &connection),
                        bluetooth_status_notifications(&server, &connection),
                    )
                    .await;
                    esp_println::println!("embedded-sdk bluetooth disconnected");
                }
                Err(_) => {
                    esp_println::println!("embedded-sdk bluetooth advertising failed; retrying");
                    Timer::after(Duration::from_secs(1)).await;
                }
            }
        }
    })
    .await;
}

async fn bluetooth_runner<C: Controller, P: PacketPool>(mut runner: Runner<'_, C, P>) {
    loop {
        if runner.run().await.is_err() {
            esp_println::println!("embedded-sdk bluetooth host runner failed; retrying");
            Timer::after(Duration::from_secs(1)).await;
        }
    }
}

async fn advertise_bluetooth<'values, 'server, C: Controller>(
    name: &'values str,
    peripheral: &mut Peripheral<'values, C, DefaultPacketPool>,
    server: &'server BluetoothServer<'values>,
) -> Result<GattConnection<'values, 'server, DefaultPacketPool>, BleHostError<C::Error>> {
    let mut advertising_data = [0; 31];
    let length = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(name.as_bytes()),
        ],
        &mut advertising_data,
    )?;
    let advertiser = peripheral
        .advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected {
                adv_data: &advertising_data[..length],
                scan_data: &[],
            },
        )
        .await?;

    esp_println::println!("embedded-sdk bluetooth advertising");
    Ok(advertiser.accept().await?.with_attribute_server(server)?)
}

async fn bluetooth_gatt_events<P: PacketPool>(
    server: &BluetoothServer<'_>,
    connection: &GattConnection<'_, '_, P>,
) {
    let status = server.sdk.status;
    loop {
        match connection.next().await {
            GattConnectionEvent::Disconnected { .. } => return,
            GattConnectionEvent::Gatt { event } => {
                if let GattEvent::Read(read) = &event
                    && read.handle() == status.handle
                {
                    esp_println::println!("embedded-sdk bluetooth status read");
                }

                match event.accept() {
                    Ok(reply) => reply.send().await,
                    Err(_) => esp_println::println!("embedded-sdk bluetooth GATT reply failed"),
                }
            }
            _ => {}
        }
    }
}

async fn bluetooth_status_notifications<P: PacketPool>(
    server: &BluetoothServer<'_>,
    connection: &GattConnection<'_, '_, P>,
) {
    let status = server.sdk.status;
    let mut value = 0_u8;
    loop {
        value = value.wrapping_add(1);
        if status.notify(connection, &value).await.is_err() {
            return;
        }
        Timer::after(Duration::from_secs(5)).await;
    }
}

async fn supervise_station(wifi: &mut Esp32c6Wifi<'_>) -> ! {
    let rng = Rng::new();
    let mut backoff = ReconnectBackoff::default();

    loop {
        match wifi.connect().await {
            Ok(connected) => {
                backoff.reset();
                esp_println::println!(
                    "embedded-sdk wifi associated: channel={}, security={:?}",
                    connected.channel,
                    connected.security
                );

                match wifi.wait_for_disconnect().await {
                    Ok(()) => esp_println::println!("embedded-sdk wifi link lost"),
                    Err(error) => {
                        esp_println::println!("embedded-sdk wifi disconnect wait failed: {error}")
                    }
                }
            }
            Err(error) => esp_println::println!("embedded-sdk wifi association failed: {error}"),
        }

        let retry_delay_ms = backoff.next_delay_ms(rng.random());
        esp_println::println!(
            "embedded-sdk wifi retry: attempt={}, delay_ms={retry_delay_ms}",
            backoff.attempts()
        );
        Timer::after(Duration::from_millis(u64::from(retry_delay_ms))).await;
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
