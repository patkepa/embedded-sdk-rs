#![no_std]
#![no_main]
#![doc = "Experimental Azure IoT Hub firmware for the Seeed Studio XIAO ESP32C6."]

use embassy_executor::Spawner;
use embassy_net::{Config as NetworkConfig, Runner as NetworkRunner, StackResources};
use embassy_time::{Duration, Timer, with_timeout};
use embedded_sdk_board_xiao_esp32c6::HARDWARE;
use embedded_sdk_cloud_azure_iot::{
    DeviceId, HubCapabilities, HubClient, HubConfig, HubHostname, TelemetryQueue,
};
use embedded_sdk_mqtt_v311::{Buffers as MqttBuffers, Client as MqttClient};
use embedded_sdk_networking_embassy_net::EmbassyNetwork;
use embedded_sdk_platform_esp32c6::{
    security::{Esp32c6HardwareRandom, fill_getrandom_after_radio_started},
    start_embassy,
    wifi::{Esp32c6StationController, Esp32c6Wifi, StationInterface},
};
use embedded_sdk_security::SecureRandom;
use embedded_sdk_wifi::{
    Authentication, ConfigError as WifiConfigError, Passphrase, ReconnectBackoff, Ssid,
    StationConfig,
};
use esp_backtrace as _;
use esp_hal::{
    gpio::{Level, Output, OutputConfig},
    rng::Rng,
};
use static_cell::StaticCell;

// DHCP/DNS, the future trusted-time exchange, Azure TCP, and one recovery
// overlap are allowed to coexist. Hardware measurements must validate this.
const NETWORK_SOCKET_COUNT: usize = 4;
const NETWORK_OPERATION_TIMEOUT: Duration = Duration::from_secs(10);
const AZURE_KEEP_ALIVE_SECONDS: u16 = 60;
const MQTT_PACKET_CAPACITY: usize = 1024;
const MQTT_REPLAY_CAPACITY: usize = 1024;
const TELEMETRY_QUEUE_DEPTH: usize = 4;
const TELEMETRY_PAYLOAD_CAPACITY: usize = 256;
const HEARTBEAT_PAYLOAD: &[u8] = br#"{"version":1,"kind":"heartbeat"}"#;

getrandom::register_custom_getrandom!(fill_getrandom_after_radio_started);
esp_bootloader_esp_idf::esp_app_desc!();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AzureAuthMode {
    RuntimeSas,
}

#[derive(Clone, Copy)]
struct AzurePublicConfig {
    hub: HubConfig,
    auth_mode: AzureAuthMode,
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 96 * 1024);
    start_embassy(peripherals.TIMG0, peripherals.SW_INTERRUPT);

    let user_led = Output::new(peripherals.GPIO15, Level::High, OutputConfig::default());
    // Retain the XIAO RF-switch controls for the lifetime of the radio.
    let _rf_switch_enable = Output::new(peripherals.GPIO3, Level::Low, OutputConfig::default());
    let _rf_switch_select = Output::new(peripherals.GPIO14, Level::Low, OutputConfig::default());

    esp_println::println!(
        "azure-iot boot: board={}, chip={}, support=experimental",
        HARDWARE.board,
        HARDWARE.chip
    );
    match heartbeat(user_led) {
        Ok(task) => spawner.spawn(task),
        Err(_) => esp_println::println!("azure-iot heartbeat task allocation failed"),
    }

    let station = match development_station_config() {
        Ok(Some(station)) => station,
        Ok(None) => {
            esp_println::println!("azure-iot disabled: Wi-Fi configuration is absent");
            wait_forever().await;
        }
        Err(error) => {
            esp_println::println!("azure-iot Wi-Fi configuration failed: {error}");
            wait_forever().await;
        }
    };
    let azure = match azure_public_config() {
        Ok(Some(config)) => config,
        Ok(None) => {
            esp_println::println!("azure-iot disabled: public hub configuration is absent");
            wait_forever().await;
        }
        Err(error) => {
            esp_println::println!("azure-iot public configuration failed: {error}");
            wait_forever().await;
        }
    };

    let mut wifi = match Esp32c6Wifi::new(peripherals.WIFI) {
        Ok(wifi) => wifi,
        Err(error) => {
            esp_println::println!("azure-iot Wi-Fi initialization failed: {error}");
            wait_forever().await;
        }
    };
    if let Err(error) = wifi.configure_station(&station) {
        esp_println::println!("azure-iot Wi-Fi station configuration failed: {error}");
        wait_forever().await;
    }

    let (controller, station_interface) = wifi.into_station_parts();
    start_networking(&spawner, controller, station_interface, azure);
    wait_forever().await;
}

fn start_networking(
    spawner: &Spawner,
    controller: Esp32c6StationController<'static>,
    station_interface: StationInterface<'static>,
    azure: AzurePublicConfig,
) {
    static RESOURCES: StaticCell<StackResources<NETWORK_SOCKET_COUNT>> = StaticCell::new();

    let rng = Rng::new();
    let random_seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let (stack, runner) = embassy_net::new(
        station_interface,
        NetworkConfig::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        random_seed,
    );
    let network = EmbassyNetwork::new(stack);

    match network_runner_task(runner) {
        Ok(task) => spawner.spawn(task),
        Err(_) => {
            esp_println::println!("azure-iot network runner task allocation failed");
            return;
        }
    }
    match wifi_station_task(controller) {
        Ok(task) => spawner.spawn(task),
        Err(_) => {
            esp_println::println!("azure-iot Wi-Fi task allocation failed");
            return;
        }
    }
    match azure_preflight_task(network, azure) {
        Ok(task) => spawner.spawn(task),
        Err(_) => esp_println::println!("azure-iot cloud task allocation failed"),
    }
}

#[embassy_executor::task]
async fn heartbeat(mut user_led: Output<'static>) {
    loop {
        user_led.toggle();
        Timer::after(Duration::from_secs(1)).await;
    }
}

#[embassy_executor::task]
async fn network_runner_task(mut runner: NetworkRunner<'static, StationInterface<'static>>) {
    runner.run().await;
}

#[embassy_executor::task]
async fn wifi_station_task(mut controller: Esp32c6StationController<'static>) {
    let rng = Rng::new();
    let mut backoff = ReconnectBackoff::default();
    loop {
        match controller.connect().await {
            Ok(_) => {
                backoff.reset();
                esp_println::println!("azure-iot Wi-Fi associated");
                let _ = controller.wait_for_disconnect().await;
                esp_println::println!("azure-iot Wi-Fi link lost");
            }
            Err(error) => esp_println::println!("azure-iot Wi-Fi association failed: {error}"),
        }
        let delay_ms = backoff.next_delay_ms(rng.random());
        Timer::after(Duration::from_millis(u64::from(delay_ms))).await;
    }
}

#[embassy_executor::task]
async fn azure_preflight_task(network: EmbassyNetwork<'static>, azure: AzurePublicConfig) {
    // These arrays make the firmware, rather than the MQTT crate, own the
    // exact packet and reconnect-replay budget.
    let mut mqtt_rx = [0; MQTT_PACKET_CAPACITY];
    let mut mqtt_tx = [0; MQTT_PACKET_CAPACITY];
    let mut mqtt_replay = [0; MQTT_REPLAY_CAPACITY];
    let _mqtt = match MqttClient::new(
        azure.hub.mqtt(),
        MqttBuffers {
            rx: &mut mqtt_rx,
            tx: &mut mqtt_tx,
            replay: &mut mqtt_replay,
        },
    ) {
        Ok(client) => client,
        Err(error) => {
            esp_println::println!("azure-iot MQTT resource configuration failed: {error}");
            return;
        }
    };
    let _hub = HubClient::new(azure.hub, HubCapabilities::TELEMETRY);
    let mut telemetry =
        match TelemetryQueue::<TELEMETRY_QUEUE_DEPTH, TELEMETRY_PAYLOAD_CAPACITY>::new() {
            Ok(queue) => queue,
            Err(error) => {
                esp_println::println!("azure-iot telemetry queue configuration failed: {error}");
                return;
            }
        };
    let mut topic_scratch = [0; embedded_sdk_cloud_azure_iot::MAX_DEVICE_ID_LEN + 32];
    let topic = match azure.hub.telemetry_topic(&mut topic_scratch) {
        Ok(topic) => topic,
        Err(error) => {
            esp_println::println!("azure-iot telemetry topic configuration failed: {error}");
            return;
        }
    };
    if telemetry.enqueue(&topic, HEARTBEAT_PAYLOAD, None).is_err() {
        esp_println::println!("azure-iot initial telemetry enqueue failed");
        return;
    }

    // Exercise the same post-radio entropy path rustls uses. Never print it.
    let mut entropy = [0_u8; 32];
    if Esp32c6HardwareRandom::after_radio_started()
        .fill_bytes(&mut entropy)
        .is_err()
    {
        esp_println::println!("azure-iot hardware entropy unavailable");
        return;
    }
    entropy.fill(0);

    loop {
        let snapshot = match network.wait_ip_ready().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                esp_println::println!("azure-iot network state failed: {error}");
                Timer::after(Duration::from_secs(1)).await;
                continue;
            }
        };
        if !snapshot.is_dns_ready() {
            esp_println::println!("azure-iot waiting for DNS");
            let _ = network.wait_ip_down().await;
            continue;
        }

        let mut addresses = [core::net::Ipv4Addr::UNSPECIFIED; 4];
        match with_timeout(
            NETWORK_OPERATION_TIMEOUT,
            network.resolve_ipv4(azure.hub.mqtt().hostname().as_str(), &mut addresses),
        )
        .await
        {
            Ok(Ok(count)) => esp_println::println!(
                "azure-iot preflight ready: addresses={count}, queued={}, auth={:?}; authenticated TLS is gated on trusted time, trust roots, and runtime credentials",
                telemetry.len(),
                azure.auth_mode
            ),
            Ok(Err(error)) => esp_println::println!("azure-iot DNS failed: {error}"),
            Err(_) => esp_println::println!("azure-iot DNS timed out"),
        }
        let _ = network.wait_ip_down().await;
    }
}

fn development_station_config() -> Result<Option<StationConfig>, WifiConfigError> {
    match (option_env!("WIFI_SSID"), option_env!("WIFI_PASSWORD")) {
        (None, None) => Ok(None),
        (Some(ssid), None) => StationConfig::open(Ssid::try_from(ssid)?).map(Some),
        (Some(ssid), Some(password)) => StationConfig::personal(
            Ssid::try_from(ssid)?,
            Passphrase::new(password)?,
            Authentication::Wpa2Wpa3Personal,
        )
        .map(Some),
        (None, Some(_)) => Err(WifiConfigError::EmptySsid),
    }
}

fn azure_public_config() -> Result<Option<AzurePublicConfig>, &'static str> {
    let hostname = option_env!("AZURE_IOT_HUB_HOSTNAME");
    let device_id = option_env!("AZURE_IOT_DEVICE_ID");
    let auth_mode = option_env!("AZURE_IOT_AUTH_MODE");
    if hostname.is_none() && device_id.is_none() && auth_mode.is_none() {
        return Ok(None);
    }
    let (Some(hostname), Some(device_id), Some(auth_mode)) = (hostname, device_id, auth_mode)
    else {
        return Err(
            "AZURE_IOT_HUB_HOSTNAME, AZURE_IOT_DEVICE_ID, and AZURE_IOT_AUTH_MODE must be set together",
        );
    };
    let auth_mode = match auth_mode {
        "runtime-sas" => AzureAuthMode::RuntimeSas,
        _ => return Err("AZURE_IOT_AUTH_MODE currently accepts only runtime-sas"),
    };
    let hostname = HubHostname::new(hostname).map_err(|_| "invalid Azure IoT Hub hostname")?;
    let device_id = DeviceId::new(device_id).map_err(|_| "invalid Azure IoT device ID")?;
    let hub = HubConfig::new(
        hostname,
        device_id,
        AZURE_KEEP_ALIVE_SECONDS,
        MQTT_PACKET_CAPACITY as u32,
    )
    .map_err(|_| "invalid Azure IoT Hub MQTT limits")?;
    Ok(Some(AzurePublicConfig { hub, auth_mode }))
}

async fn wait_forever() -> ! {
    loop {
        Timer::after(Duration::from_secs(30)).await;
    }
}
