#![no_std]
#![no_main]
#![doc = "Reference Embassy firmware for the Seeed Studio XIAO ESP32C6."]
#![allow(
    clippy::needless_borrows_for_generic_args,
    reason = "TrouBLE 0.6 GATT derive macros emit this pattern"
)]

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_executor::Spawner;
use embassy_futures::{
    join::join,
    select::{Either, select},
};
use embassy_net::{
    Config as NetworkConfig, IpAddress, IpEndpoint, Runner as NetworkRunner, StackResources,
    tcp::TcpSocket,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Timer, with_timeout};
use embedded_io_async::{ErrorType, Read, Write};
use embedded_sdk_bluetooth::{DeviceName, PeripheralConfig as SdkBluetoothConfig};
use embedded_sdk_board_xiao_esp32c6::{BLUETOOTH_DEVICE_NAME, HARDWARE};
use embedded_sdk_mqtt::{
    BrokerHostname, BrokerPort, ClientId, Config as MqttConfig, ConnectionState as MqttState,
    ErrorKind as MqttErrorKind, QoS as MqttQos, ReconnectPolicy as MqttReconnectPolicy,
    TopicFilter, TopicName,
};
use embedded_sdk_mqtt_minimq::{
    Client as MqttClient, Connection as MqttConnection, TransportSecurity,
};
use embedded_sdk_networking_embassy_net::EmbassyNetwork;
use embedded_sdk_platform_esp32c6::{
    bluetooth::{BluetoothConnector, Esp32c6Bluetooth, static_random_address},
    start_embassy,
    wifi::{Esp32c6StationController, Esp32c6Wifi, StationInterface},
};
use embedded_sdk_wifi::{
    Authentication, ConfigError, Passphrase, ReconnectBackoff, ScanSummary, Ssid, StationConfig,
};
use esp_backtrace as _;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::rng::Rng;
use static_cell::StaticCell;
use trouble_host::prelude::*;

const BLUETOOTH_CONNECTIONS_MAX: usize = 1;
// One signaling channel and one ATT channel per connection.
const BLUETOOTH_L2CAP_CHANNELS_MAX: usize = 2;
// DHCP, DNS, the controlled probe, and long-lived MQTT may overlap.
const NETWORK_SOCKET_COUNT: usize = 4;
const NETWORK_TCP_RX_BUFFER_SIZE: usize = 512;
const NETWORK_TCP_TX_BUFFER_SIZE: usize = 512;
const NETWORK_DHCP_REPORT_INTERVAL: Duration = Duration::from_secs(30);
const NETWORK_OPERATION_TIMEOUT: Duration = Duration::from_secs(10);
const MQTT_RX_PACKET_BUFFER_SIZE: usize = 512;
const MQTT_TX_REPLAY_BUFFER_SIZE: usize = 1024;
const MQTT_TCP_RX_BUFFER_SIZE: usize = 1024;
const MQTT_TCP_TX_BUFFER_SIZE: usize = 1024;
const MQTT_OUTBOUND_CHANNEL_DEPTH: usize = 4;
const MQTT_TELEMETRY_INTERVAL: Duration = Duration::from_secs(30);

static MQTT_OUTBOUND: Channel<
    CriticalSectionRawMutex,
    OutboundMessage,
    MQTT_OUTBOUND_CHANNEL_DEPTH,
> = Channel::new();
static MQTT_QUEUE_DROPS: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy)]
struct NetworkProbe {
    host: &'static str,
    port: u16,
}

#[derive(Clone, Copy)]
struct MqttDevelopmentConfig {
    session: MqttConfig,
    telemetry_topic: TopicName,
    command_filter: TopicFilter,
}

#[derive(Clone, Copy)]
struct OutboundMessage {
    payload: &'static [u8],
    qos: MqttQos,
}

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
                let (controller, station_interface) = wifi.into_station_parts();
                let network_probe = match development_network_probe() {
                    Ok(probe) => probe,
                    Err(error) => {
                        esp_println::println!(
                            "embedded-sdk network probe configuration failed: {error}"
                        );
                        None
                    }
                };
                let mqtt = match development_mqtt_config() {
                    Ok(config) => config,
                    Err(error) => {
                        esp_println::println!("embedded-sdk MQTT configuration failed: {error}");
                        None
                    }
                };
                start_networking(&spawner, controller, station_interface, network_probe, mqtt);
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

fn start_networking(
    spawner: &Spawner,
    controller: Esp32c6StationController<'static>,
    station_interface: StationInterface<'static>,
    probe: Option<NetworkProbe>,
    mqtt: Option<MqttDevelopmentConfig>,
) {
    static RESOURCES: StaticCell<StackResources<NETWORK_SOCKET_COUNT>> = StaticCell::new();

    let rng = Rng::new();
    let random_seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let network_config = NetworkConfig::dhcpv4(Default::default());
    let (stack, runner) = embassy_net::new(
        station_interface,
        network_config,
        RESOURCES.init(StackResources::new()),
        random_seed,
    );
    let network = EmbassyNetwork::new(stack);

    match network_runner_task(runner) {
        Ok(task) => spawner.spawn(task),
        Err(_) => {
            esp_println::println!("embedded-sdk network runner task allocation failed");
            return;
        }
    }
    match wifi_station_task(controller) {
        Ok(task) => spawner.spawn(task),
        Err(_) => {
            esp_println::println!("embedded-sdk wifi station task allocation failed");
            return;
        }
    }
    match network_monitor_task(network, probe) {
        Ok(task) => spawner.spawn(task),
        Err(_) => esp_println::println!("embedded-sdk network monitor task allocation failed"),
    }
    if let Some(config) = mqtt {
        match mqtt_task(network, config) {
            Ok(task) => spawner.spawn(task),
            Err(_) => esp_println::println!("embedded-sdk MQTT task allocation failed"),
        }
        match mqtt_telemetry_producer_task() {
            Ok(task) => spawner.spawn(task),
            Err(_) => esp_println::println!("embedded-sdk MQTT producer task allocation failed"),
        }
    } else {
        esp_println::println!("embedded-sdk MQTT disabled");
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

#[embassy_executor::task]
async fn wifi_station_task(mut controller: Esp32c6StationController<'static>) {
    supervise_station(&mut controller).await;
}

#[embassy_executor::task]
async fn network_runner_task(mut runner: NetworkRunner<'static, StationInterface<'static>>) {
    runner.run().await;
}

#[embassy_executor::task]
async fn network_monitor_task(network: EmbassyNetwork<'static>, probe: Option<NetworkProbe>) {
    let stack = network.stack();

    loop {
        stack.wait_link_up().await;
        esp_println::println!("embedded-sdk network link up: ipv4=pending");

        let configured = loop {
            match select(
                with_timeout(NETWORK_DHCP_REPORT_INTERVAL, stack.wait_config_up()),
                stack.wait_link_down(),
            )
            .await
            {
                Either::First(Ok(())) => match network.snapshot() {
                    Ok(snapshot) if snapshot.is_ip_ready() => break true,
                    Ok(_) => continue,
                    Err(error) => {
                        esp_println::println!(
                            "embedded-sdk network state conversion failed: {error}"
                        );
                        break false;
                    }
                },
                Either::First(Err(_)) => {
                    esp_println::println!("embedded-sdk network DHCP pending");
                }
                Either::Second(()) => {
                    esp_println::println!("embedded-sdk network link down");
                    break false;
                }
            }
        };

        if !configured {
            continue;
        }

        let snapshot = match network.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                esp_println::println!("embedded-sdk network state conversion failed: {error}");
                continue;
            }
        };
        let dns_servers = snapshot
            .ipv4()
            .map_or(0, |configuration| configuration.dns_servers().len());
        esp_println::println!("embedded-sdk network IPv4 configured: dns_servers={dns_servers}");

        if let Some(probe) = probe {
            if snapshot.is_dns_ready() {
                run_network_probe(network, probe).await;
            } else {
                esp_println::println!("embedded-sdk network probe skipped: DNS unavailable");
            }
        }

        match network.wait_ip_down().await {
            Ok(_) => esp_println::println!("embedded-sdk network IPv4 configuration lost"),
            Err(error) => {
                esp_println::println!("embedded-sdk network state conversion failed: {error}")
            }
        }
    }
}

#[embassy_executor::task]
async fn mqtt_telemetry_producer_task() {
    const PAYLOAD: &[u8] = br#"{"version":1,"kind":"heartbeat"}"#;
    loop {
        if MQTT_OUTBOUND
            .try_send(OutboundMessage {
                payload: PAYLOAD,
                qos: MqttQos::AtLeastOnce,
            })
            .is_err()
        {
            MQTT_QUEUE_DROPS.fetch_add(1, Ordering::Relaxed);
        }
        Timer::after(MQTT_TELEMETRY_INTERVAL).await;
    }
}

#[embassy_executor::task]
async fn mqtt_task(network: EmbassyNetwork<'static>, config: MqttDevelopmentConfig) {
    let mut mqtt_rx = [0; MQTT_RX_PACKET_BUFFER_SIZE];
    let mut mqtt_tx = [0; MQTT_TX_REPLAY_BUFFER_SIZE];
    let mut client = match MqttClient::new(
        &config.session,
        &mut mqtt_rx,
        &mut mqtt_tx,
        TransportSecurity::PlaintextFixture,
        None,
    ) {
        Ok(client) => client,
        Err(error) => {
            esp_println::println!("embedded-sdk MQTT adapter configuration failed: {error}");
            return;
        }
    };
    let mut backoff = MqttReconnectPolicy::default().backoff();
    let rng = Rng::new();

    loop {
        client.transition(MqttState::WaitingForNetwork);
        let snapshot = match network.wait_ip_ready().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                esp_println::println!("embedded-sdk MQTT network state failed: {error}");
                Timer::after(Duration::from_secs(1)).await;
                continue;
            }
        };
        if !snapshot.is_dns_ready() {
            esp_println::println!("embedded-sdk MQTT waiting for DNS");
            let _ = network.wait_ip_down().await;
            continue;
        }

        client.transition(MqttState::ResolvingBroker);
        let mut addresses = [core::net::Ipv4Addr::UNSPECIFIED; 4];
        let address_count = match with_timeout(
            NETWORK_OPERATION_TIMEOUT,
            network.resolve_ipv4(config.session.hostname().as_str(), &mut addresses),
        )
        .await
        {
            Ok(Ok(count)) => count,
            Ok(Err(error)) => {
                esp_println::println!("embedded-sdk MQTT DNS failed: {error}");
                client.record_external_failure(MqttErrorKind::Transport);
                mqtt_backoff(&mut backoff, &rng).await;
                continue;
            }
            Err(_) => {
                esp_println::println!("embedded-sdk MQTT DNS timed out");
                client.record_external_failure(MqttErrorKind::Transport);
                mqtt_backoff(&mut backoff, &rng).await;
                continue;
            }
        };

        client.transition(MqttState::ConnectingTransport);
        let mut tcp_rx = [0; MQTT_TCP_RX_BUFFER_SIZE];
        let mut tcp_tx = [0; MQTT_TCP_TX_BUFFER_SIZE];
        let mut socket = TcpSocket::new(network.stack(), &mut tcp_rx, &mut tcp_tx);
        let endpoint = IpEndpoint::new(IpAddress::Ipv4(addresses[0]), config.session.port().get());
        match with_timeout(NETWORK_OPERATION_TIMEOUT, socket.connect(endpoint)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                esp_println::println!("embedded-sdk MQTT TCP failed: {error:?}");
                client.record_external_failure(MqttErrorKind::Transport);
                mqtt_backoff(&mut backoff, &rng).await;
                continue;
            }
            Err(_) => {
                esp_println::println!("embedded-sdk MQTT TCP timed out");
                client.record_external_failure(MqttErrorKind::Transport);
                mqtt_backoff(&mut backoff, &rng).await;
                continue;
            }
        }

        let mut connection =
            match with_timeout(NETWORK_OPERATION_TIMEOUT, client.connect(socket)).await {
                Ok(Ok(connection)) => connection,
                Ok(Err(error)) => {
                    esp_println::println!("embedded-sdk MQTT CONNECT failed: {:?}", error.kind());
                    mqtt_backoff(&mut backoff, &rng).await;
                    continue;
                }
                Err(_) => {
                    esp_println::println!("embedded-sdk MQTT CONNECT timed out");
                    client.record_external_failure(MqttErrorKind::Transport);
                    mqtt_backoff(&mut backoff, &rng).await;
                    continue;
                }
            };
        backoff.reset();
        esp_println::println!(
            "embedded-sdk MQTT connected: resumed={}, resolved_addresses={address_count}",
            connection.resumed()
        );

        let result = select(
            run_mqtt_connection(
                &mut connection,
                &config.telemetry_topic,
                &config.command_filter,
            ),
            network.wait_ip_down(),
        )
        .await;
        drop(connection);
        match result {
            Either::First(Ok(())) => {}
            Either::First(Err(error)) => {
                esp_println::println!("embedded-sdk MQTT session failed: {error:?}");
                mqtt_backoff(&mut backoff, &rng).await;
            }
            Either::Second(_) => {
                client.transition(MqttState::WaitingForNetwork);
                esp_println::println!("embedded-sdk MQTT stopped after IPv4 loss");
            }
        }
    }
}

async fn run_mqtt_connection<IO>(
    connection: &mut MqttConnection<'_, '_, IO>,
    telemetry_topic: &TopicName,
    command_filter: &TopicFilter,
) -> Result<(), MqttErrorKind>
where
    IO: Read + Write + ErrorType,
{
    if !connection.resumed() {
        connection
            .subscribe(command_filter, MqttQos::AtLeastOnce)
            .await
            .map_err(|error| error.kind())?;
    }

    loop {
        let dropped = MQTT_QUEUE_DROPS.swap(0, Ordering::Relaxed);
        connection.record_queue_drops(dropped);
        match select(connection.receive(), MQTT_OUTBOUND.receive()).await {
            Either::First(Ok(message)) => {
                let _ = message;
                esp_println::println!("embedded-sdk MQTT command received");
            }
            Either::First(Err(error)) => return Err(error.kind()),
            Either::Second(message) => {
                connection
                    .publish(telemetry_topic, message.payload, message.qos)
                    .await
                    .map_err(|error| error.kind())?;
            }
        }
    }
}

async fn mqtt_backoff(backoff: &mut embedded_sdk_mqtt::ReconnectBackoff, rng: &Rng) {
    let delay_ms = backoff.next_delay_ms(rng.random());
    esp_println::println!(
        "embedded-sdk MQTT retry: attempt={}, delay_ms={delay_ms}",
        backoff.attempts()
    );
    Timer::after(Duration::from_millis(u64::from(delay_ms))).await;
}

async fn run_network_probe(network: EmbassyNetwork<'_>, probe: NetworkProbe) {
    let mut addresses = [core::net::Ipv4Addr::UNSPECIFIED; 4];
    let address_count = match with_timeout(
        NETWORK_OPERATION_TIMEOUT,
        network.resolve_ipv4(probe.host, &mut addresses),
    )
    .await
    {
        Ok(Ok(count)) => count,
        Ok(Err(error)) => {
            esp_println::println!("embedded-sdk network DNS probe failed: {error}");
            return;
        }
        Err(_) => {
            esp_println::println!("embedded-sdk network DNS probe timed out");
            return;
        }
    };

    let mut rx_buffer = [0; NETWORK_TCP_RX_BUFFER_SIZE];
    let mut tx_buffer = [0; NETWORK_TCP_TX_BUFFER_SIZE];
    let mut socket = TcpSocket::new(network.stack(), &mut rx_buffer, &mut tx_buffer);
    socket.set_timeout(Some(NETWORK_OPERATION_TIMEOUT));
    let endpoint = IpEndpoint::new(IpAddress::Ipv4(addresses[0]), probe.port);
    match with_timeout(NETWORK_OPERATION_TIMEOUT, socket.connect(endpoint)).await {
        Ok(Ok(())) => {
            esp_println::println!(
                "embedded-sdk network probe succeeded: resolved_addresses={address_count}"
            );
            socket.close();
            let _ = with_timeout(NETWORK_OPERATION_TIMEOUT, socket.flush()).await;
        }
        Ok(Err(error)) => {
            esp_println::println!("embedded-sdk network TCP probe failed: {error:?}");
        }
        Err(_) => esp_println::println!("embedded-sdk network TCP probe timed out"),
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

async fn supervise_station(wifi: &mut Esp32c6StationController<'_>) -> ! {
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

fn development_network_probe() -> Result<Option<NetworkProbe>, &'static str> {
    match (
        option_env!("NETWORK_TEST_HOST"),
        option_env!("NETWORK_TEST_PORT"),
    ) {
        (None, None) => Ok(None),
        (Some(host), Some(port)) if !host.is_empty() => {
            let port = port
                .parse::<u16>()
                .map_err(|_| "NETWORK_TEST_PORT must be an integer from 1 through 65535")?;
            if port == 0 {
                return Err("NETWORK_TEST_PORT must be an integer from 1 through 65535");
            }
            Ok(Some(NetworkProbe { host, port }))
        }
        (Some(_), Some(_)) => Err("NETWORK_TEST_HOST must not be empty"),
        _ => Err("NETWORK_TEST_HOST and NETWORK_TEST_PORT must be set together"),
    }
}

fn development_mqtt_config() -> Result<Option<MqttDevelopmentConfig>, &'static str> {
    let host = option_env!("MQTT_HOST");
    let port = option_env!("MQTT_PORT");
    let client_id = option_env!("MQTT_CLIENT_ID");
    let plaintext_fixture = option_env!("MQTT_PLAINTEXT_FIXTURE");
    let username = option_env!("MQTT_USERNAME");
    let password = option_env!("MQTT_PASSWORD");

    if host.is_none()
        && port.is_none()
        && client_id.is_none()
        && plaintext_fixture.is_none()
        && username.is_none()
        && password.is_none()
    {
        return Ok(None);
    }
    if username.is_some() || password.is_some() {
        return Err("credentials are unavailable until verified TLS is implemented");
    }
    let (Some(host), Some(port), Some(client_id)) = (host, port, client_id) else {
        return Err("MQTT_HOST, MQTT_PORT, and MQTT_CLIENT_ID must be set together");
    };
    if plaintext_fixture != Some("1") {
        return Err("set MQTT_PLAINTEXT_FIXTURE=1 for an isolated local test broker");
    }

    let hostname = BrokerHostname::new(host).map_err(|_| "MQTT_HOST is invalid")?;
    let port = port
        .parse::<u16>()
        .map_err(|_| "MQTT_PORT must be an integer from 1 through 65535")?;
    let port =
        BrokerPort::new(port).map_err(|_| "MQTT_PORT must be an integer from 1 through 65535")?;
    let client_id = ClientId::new(client_id).map_err(|_| "MQTT_CLIENT_ID is invalid")?;

    let mut telemetry_storage = [0; embedded_sdk_mqtt::MAX_TOPIC_LEN];
    let telemetry_topic = TopicName::new(fixture_topic_text(
        &mut telemetry_storage,
        &client_id,
        "/telemetry",
    )?)
    .map_err(|_| "MQTT telemetry topic is invalid")?;
    let mut command_storage = [0; embedded_sdk_mqtt::MAX_TOPIC_LEN];
    let command_filter = TopicFilter::new(fixture_topic_text(
        &mut command_storage,
        &client_id,
        "/commands",
    )?)
    .map_err(|_| "MQTT command topic is invalid")?;
    let session = MqttConfig::new_v5(
        hostname,
        port,
        client_id,
        30,
        300,
        MQTT_RX_PACKET_BUFFER_SIZE as u32,
    )
    .map_err(|_| "MQTT session limits are invalid")?;

    Ok(Some(MqttDevelopmentConfig {
        session,
        telemetry_topic,
        command_filter,
    }))
}

fn fixture_topic_text<'a>(
    storage: &'a mut [u8; embedded_sdk_mqtt::MAX_TOPIC_LEN],
    client_id: &ClientId,
    suffix: &str,
) -> Result<&'a str, &'static str> {
    const PREFIX: &[u8] = b"embedded-sdk/test/";
    let client = client_id.as_str().as_bytes();
    let suffix = suffix.as_bytes();
    let len = PREFIX.len() + client.len() + suffix.len();
    if len > storage.len() {
        return Err("MQTT fixture topic exceeds its bounded capacity");
    }
    storage[..PREFIX.len()].copy_from_slice(PREFIX);
    storage[PREFIX.len()..PREFIX.len() + client.len()].copy_from_slice(client);
    storage[PREFIX.len() + client.len()..len].copy_from_slice(suffix);
    core::str::from_utf8(&storage[..len]).map_err(|_| "MQTT fixture topic is not UTF-8")
}
