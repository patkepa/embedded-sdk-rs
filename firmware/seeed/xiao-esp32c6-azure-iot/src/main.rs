#![no_std]
#![no_main]
#![doc = "Experimental Azure IoT Hub firmware for the Seeed Studio XIAO ESP32C6."]

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_net::{
    Config as NetworkConfig, IpEndpoint, Runner as NetworkRunner, StackResources, tcp::TcpSocket,
};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_sdk_board_xiao_esp32c6::HARDWARE;
use embedded_sdk_cloud_azure_iot::{
    DIRECT_METHOD_TIMEOUT_STATUS, DeviceId, DirectMethodDispatch, DirectMethodQueue,
    HubCapabilities, HubClient, HubConfig, HubEvent, HubHostname, HubSession, HubSessionEvent,
    OutboundOperation, SessionDisposition, TelemetryDispatch, TelemetryQueue, TwinOperation,
};
use embedded_sdk_mqtt::{MqttSession, ReconnectPolicy};
use embedded_sdk_mqtt_v311::{
    Buffers as MqttBuffers, Client as MqttClient, ConnectEvent, Credentials, TransportSecurity,
};
use embedded_sdk_networking_embassy_net::EmbassyNetwork;
use embedded_sdk_platform_esp32c6::{
    security::{Esp32c6HardwareRandom, fill_getrandom_after_radio_started},
    start_embassy,
    wifi::{Esp32c6StationController, Esp32c6Wifi, StationInterface},
};
use embedded_sdk_security::{AnchoredTrustedTime, MonotonicClock, SecureRandom, UnixTime};
use embedded_sdk_tls_rustls::{
    ClientPrivateKey, TlsBuffers, TlsClientConfig, TlsRootStore, TlsStream,
};
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
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
const MQTT_OPERATION_TIMEOUT: Duration = Duration::from_secs(15);
const AZURE_KEEP_ALIVE_SECONDS: u16 = 60;
const MQTT_PACKET_CAPACITY: usize = 1024;
const MQTT_REPLAY_CAPACITY: usize = 1024;
const TCP_BUFFER_CAPACITY: usize = 2048;
const TLS_INCOMING_CAPACITY: usize = 8192;
const TLS_PLAINTEXT_CAPACITY: usize = 2048;
const TLS_OUTGOING_CAPACITY: usize = TLS_PLAINTEXT_CAPACITY + 64;
const TELEMETRY_QUEUE_DEPTH: usize = 4;
const TELEMETRY_PAYLOAD_CAPACITY: usize = 256;
const METHOD_QUEUE_DEPTH: usize = 4;
const METHOD_NAME_CAPACITY: usize = 64;
const METHOD_PAYLOAD_CAPACITY: usize = 256;
const METHOD_TIMEOUT_MS: u32 = 20_000;
const TELEMETRY_INTERVAL: Duration = Duration::from_secs(30);
const MAX_ROOT_CERTIFICATE_DER_SIZE: usize = 1452;
const MAX_CLIENT_CERTIFICATE_DER_SIZE: usize = 4096;
const MAX_CLIENT_PRIVATE_KEY_DER_SIZE: usize = 4096;
const HEARTBEAT_PAYLOAD: &[u8] = br#"{"version":1,"kind":"heartbeat"}"#;
const EMPTY_REPORTED_PROPERTIES: &[u8] = b"{}";
const METHOD_NOT_IMPLEMENTED_STATUS: u16 = 501;
const METHOD_NOT_IMPLEMENTED_RESPONSE: &[u8] = br#"{"error":"not implemented"}"#;
const METHOD_TIMEOUT_RESPONSE: &[u8] = br#"{"error":"method timed out"}"#;
const AZURE_SERVER_ROOTS: [&[u8]; 2] = [
    include_bytes!("../certificates/digicert-global-root-g2.pem"),
    include_bytes!("../certificates/microsoft-rsa-root-2017.pem"),
];

getrandom::register_custom_getrandom!(fill_getrandom_after_radio_started);
esp_bootloader_esp_idf::esp_app_desc!();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AzureAuthMode {
    DevelopmentX509,
}

#[derive(Clone, Copy)]
struct AzurePublicConfig {
    hub: HubConfig,
    auth_mode: AzureAuthMode,
    client_certificate_pem: &'static str,
    client_private_key_pem: &'static str,
    trusted_unix_time: UnixTime,
}

#[derive(Clone, Copy)]
struct EmbassyMonotonicClock;

impl MonotonicClock for EmbassyMonotonicClock {
    fn now_millis(&self) -> u64 {
        Instant::now().as_millis()
    }
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
    match azure_iot_task(network, azure) {
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

type FirmwareTelemetryQueue = TelemetryQueue<TELEMETRY_QUEUE_DEPTH, TELEMETRY_PAYLOAD_CAPACITY>;
type FirmwareMethodQueue =
    DirectMethodQueue<METHOD_QUEUE_DEPTH, METHOD_NAME_CAPACITY, METHOD_PAYLOAD_CAPACITY>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FirmwareSessionEvent {
    SubscriptionAccepted,
    OutboundAcknowledged(OutboundOperation),
    TwinResponse(TwinOperation),
    InboundAccepted,
    Progress,
}

#[embassy_executor::task]
async fn azure_iot_task(network: EmbassyNetwork<'static>, azure: AzurePublicConfig) {
    let mut root_decode_scratch = [0_u8; MAX_ROOT_CERTIFICATE_DER_SIZE];
    let trust_roots =
        match TlsRootStore::from_pem_roots(AZURE_SERVER_ROOTS, &mut root_decode_scratch) {
            Ok(roots) => roots,
            Err(error) => {
                esp_println::println!("azure-iot TLS trust bundle failed: {error}");
                return;
            }
        };
    root_decode_scratch.fill(0);

    let trusted_time = AnchoredTrustedTime::new(EmbassyMonotonicClock, azure.trusted_unix_time);
    let mut client_certificate_der = [0_u8; MAX_CLIENT_CERTIFICATE_DER_SIZE];
    let mut client_private_key_der = [0_u8; MAX_CLIENT_PRIVATE_KEY_DER_SIZE];
    let tls_config = match build_x509_tls_config(
        trust_roots,
        &trusted_time,
        azure.client_certificate_pem,
        azure.client_private_key_pem,
        &mut client_certificate_der,
        &mut client_private_key_der,
    ) {
        Ok(config) => config,
        Err(error) => {
            esp_println::println!("azure-iot X.509 TLS configuration failed: {error}");
            return;
        }
    };
    client_certificate_der.fill(0);
    client_private_key_der.fill(0);

    let capabilities = HubCapabilities::TELEMETRY
        .union(HubCapabilities::DIRECT_METHODS)
        .union(HubCapabilities::TWINS);
    let mut hub = HubClient::new(azure.hub, capabilities);
    let mut telemetry = match FirmwareTelemetryQueue::new() {
        Ok(queue) => queue,
        Err(error) => {
            esp_println::println!("azure-iot telemetry queue configuration failed: {error}");
            return;
        }
    };
    let mut methods = match FirmwareMethodQueue::new(METHOD_TIMEOUT_MS) {
        Ok(queue) => queue,
        Err(error) => {
            esp_println::println!("azure-iot method queue configuration failed: {error}");
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

    let rng = Rng::new();
    let mut reconnect = ReconnectPolicy::default().backoff();
    let mut subscriptions_known_complete = false;
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
            Ok(Ok(count)) => {
                esp_println::println!(
                    "azure-iot endpoint resolved: addresses={count}, auth={:?}",
                    azure.auth_mode
                );
                let result = connect_and_run_azure(
                    network,
                    &azure,
                    &tls_config,
                    &mut hub,
                    &mut telemetry,
                    &mut methods,
                    &mut subscriptions_known_complete,
                    &topic,
                    &mut reconnect,
                    &addresses[..count],
                )
                .await;
                match result {
                    Ok(()) => {}
                    Err(error) => {
                        esp_println::println!("azure-iot session ended: {error}");
                        let delay_ms = reconnect.next_delay_ms(rng.random());
                        esp_println::println!(
                            "azure-iot reconnect: attempt={}, delay_ms={delay_ms}",
                            reconnect.attempts()
                        );
                        match select(
                            Timer::after(Duration::from_millis(u64::from(delay_ms))),
                            network.wait_ip_down(),
                        )
                        .await
                        {
                            Either::First(()) | Either::Second(_) => {}
                        }
                    }
                }
            }
            Ok(Err(error)) => {
                esp_println::println!("azure-iot DNS failed: {error}");
                let delay_ms = reconnect.next_delay_ms(rng.random());
                Timer::after(Duration::from_millis(u64::from(delay_ms))).await;
            }
            Err(_) => {
                esp_println::println!("azure-iot DNS timed out");
                let delay_ms = reconnect.next_delay_ms(rng.random());
                Timer::after(Duration::from_millis(u64::from(delay_ms))).await;
            }
        }
    }
}

fn build_x509_tls_config(
    trust_roots: TlsRootStore,
    trusted_time: &impl embedded_sdk_security::TrustedTime,
    client_certificate_pem: &str,
    client_private_key_pem: &str,
    certificate_scratch: &mut [u8],
    private_key_scratch: &mut [u8],
) -> Result<TlsClientConfig, &'static str> {
    let (certificate_label, certificate_der) =
        pem_rfc7468::decode(client_certificate_pem.as_bytes(), certificate_scratch)
            .map_err(|_| "invalid client certificate PEM")?;
    if certificate_label != "CERTIFICATE" {
        return Err("client certificate PEM must use the CERTIFICATE label");
    }
    let (private_key_label, private_key_der) =
        pem_rfc7468::decode(client_private_key_pem.as_bytes(), private_key_scratch)
            .map_err(|_| "invalid client private-key PEM")?;
    let private_key = match private_key_label {
        "PRIVATE KEY" => ClientPrivateKey::Pkcs8(private_key_der),
        "RSA PRIVATE KEY" => ClientPrivateKey::Pkcs1(private_key_der),
        "EC PRIVATE KEY" => ClientPrivateKey::Sec1(private_key_der),
        _ => return Err("unsupported client private-key PEM label"),
    };
    TlsClientConfig::from_trust_roots_with_client_auth(
        trust_roots,
        trusted_time,
        TLS_PLAINTEXT_CAPACITY,
        &[certificate_der],
        private_key,
    )
    .map_err(|_| "client identity or TLS policy rejected")
}

async fn connect_and_run_azure(
    network: EmbassyNetwork<'static>,
    azure: &AzurePublicConfig,
    tls_config: &TlsClientConfig,
    hub: &mut HubClient,
    telemetry: &mut FirmwareTelemetryQueue,
    methods: &mut FirmwareMethodQueue,
    subscriptions_known_complete: &mut bool,
    telemetry_topic: &embedded_sdk_mqtt::TopicName,
    reconnect: &mut embedded_sdk_mqtt::ReconnectBackoff,
    addresses: &[core::net::Ipv4Addr],
) -> Result<(), &'static str> {
    let mut tcp_rx = [0_u8; TCP_BUFFER_CAPACITY];
    let mut tcp_tx = [0_u8; TCP_BUFFER_CAPACITY];
    let mut socket = TcpSocket::new(network.stack(), &mut tcp_rx, &mut tcp_tx);
    let Some(address) = addresses.first().copied() else {
        return Err("endpoint resolution returned no address");
    };
    let endpoint = IpEndpoint::new(address.into(), embedded_sdk_cloud_azure_iot::MQTT_TLS_PORT);
    match with_timeout(NETWORK_OPERATION_TIMEOUT, socket.connect(endpoint)).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => return Err("TCP connection failed"),
        Err(_) => return Err("TCP connection timed out"),
    }

    let mut incoming_tls = [0_u8; TLS_INCOMING_CAPACITY];
    let mut outgoing_tls = [0_u8; TLS_OUTGOING_CAPACITY];
    let mut plaintext = [0_u8; TLS_PLAINTEXT_CAPACITY];
    let tls = match with_timeout(
        TLS_HANDSHAKE_TIMEOUT,
        TlsStream::connect(
            socket,
            tls_config,
            azure.hub.mqtt().hostname().as_str(),
            TlsBuffers {
                incoming_tls: &mut incoming_tls,
                outgoing_tls: &mut outgoing_tls,
                plaintext: &mut plaintext,
            },
        ),
    )
    .await
    {
        Ok(Ok(tls)) => tls,
        Ok(Err(_)) => return Err("mutual TLS handshake failed"),
        Err(_) => return Err("mutual TLS handshake timed out"),
    };

    let mut mqtt_rx = [0_u8; MQTT_PACKET_CAPACITY];
    let mut mqtt_tx = [0_u8; MQTT_PACKET_CAPACITY];
    let mut mqtt_replay = [0_u8; MQTT_REPLAY_CAPACITY];
    let mut mqtt = MqttClient::new(
        azure.hub.mqtt(),
        MqttBuffers {
            rx: &mut mqtt_rx,
            tx: &mut mqtt_tx,
            replay: &mut mqtt_replay,
        },
    )
    .map_err(|_| "MQTT resource configuration failed")?;
    let mut username_scratch = [0_u8; 512];
    let username = azure
        .hub
        .write_mqtt_username(&mut username_scratch)
        .map_err(|_| "MQTT username encoding failed")?;
    let credentials = Credentials::username_only(username)
        .map_err(|_| "MQTT X.509 identity configuration failed")?;
    let connection = match with_timeout(
        MQTT_OPERATION_TIMEOUT,
        mqtt.connect(tls, TransportSecurity::Encrypted, Some(credentials)),
    )
    .await
    {
        Ok(Ok(connection)) => connection,
        Ok(Err(_)) => return Err("Azure MQTT CONNECT failed"),
        Err(_) => return Err("Azure MQTT CONNECT timed out"),
    };
    username_scratch.fill(0);

    let broker_disposition = match connection.connect_event() {
        ConnectEvent::FreshSession => SessionDisposition::Fresh,
        ConnectEvent::ResumedSession if *subscriptions_known_complete => {
            SessionDisposition::Resumed
        }
        ConnectEvent::ResumedSession => SessionDisposition::Fresh,
    };
    let mut session = HubSession::new(hub, connection, broker_disposition)
        .map_err(|_| "Azure session attachment failed")?;
    let result = select(
        run_azure_session(
            &mut session,
            telemetry,
            methods,
            subscriptions_known_complete,
            telemetry_topic,
            reconnect,
        ),
        network.wait_ip_down(),
    )
    .await;
    match result {
        Either::First(result) => result,
        Either::Second(_) => Err("network configuration lost"),
    }
}

async fn run_azure_session<S: MqttSession>(
    session: &mut HubSession<'_, S>,
    telemetry: &mut FirmwareTelemetryQueue,
    methods: &mut FirmwareMethodQueue,
    subscriptions_known_complete: &mut bool,
    telemetry_topic: &embedded_sdk_mqtt::TopicName,
    reconnect: &mut embedded_sdk_mqtt::ReconnectBackoff,
) -> Result<(), &'static str> {
    let mut subscription_scratch = [0_u8; 256];
    while session
        .begin_next_subscription(&mut subscription_scratch)
        .await
        .map_err(|_| "Azure subscription start failed")?
        .is_some()
    {
        wait_for_session_event(session, methods, |event| {
            matches!(event, FirmwareSessionEvent::SubscriptionAccepted)
        })
        .await?;
    }
    *subscriptions_known_complete = true;

    let mut topic_scratch = [0_u8; 256];
    session
        .begin_twin_sync(&mut topic_scratch)
        .await
        .map_err(|_| "Azure twin synchronization start failed")?;
    let mut twin_publish_acknowledged = false;
    let mut twin_response_received = false;
    while !twin_publish_acknowledged || !twin_response_received {
        match next_session_event(session, methods).await? {
            FirmwareSessionEvent::OutboundAcknowledged(OutboundOperation::TwinGet) => {
                twin_publish_acknowledged = true;
            }
            FirmwareSessionEvent::TwinResponse(TwinOperation::Get) => {
                twin_response_received = true;
            }
            _ => {}
        }
    }
    esp_println::println!("azure-iot online: subscriptions and twin synchronized");
    reconnect.reset();

    session
        .publish_reported_properties(EMPTY_REPORTED_PROPERTIES, &mut topic_scratch)
        .await
        .map_err(|_| "Azure reported-properties publish failed")?;
    wait_for_session_event(session, methods, |event| {
        matches!(
            event,
            FirmwareSessionEvent::OutboundAcknowledged(OutboundOperation::ReportedProperties)
        )
    })
    .await?;

    let mut next_telemetry = Instant::now();
    loop {
        let now_ms = Instant::now().as_millis();
        let pending_method = if let Some(request) = methods.active() {
            Some(if request.is_expired(now_ms) {
                (
                    *request.request_id(),
                    DIRECT_METHOD_TIMEOUT_STATUS,
                    METHOD_TIMEOUT_RESPONSE,
                )
            } else {
                (
                    *request.request_id(),
                    METHOD_NOT_IMPLEMENTED_STATUS,
                    METHOD_NOT_IMPLEMENTED_RESPONSE,
                )
            })
        } else {
            methods.begin_next(now_ms).map(|dispatch| match dispatch {
                DirectMethodDispatch::Ready(request) => (
                    *request.request_id(),
                    METHOD_NOT_IMPLEMENTED_STATUS,
                    METHOD_NOT_IMPLEMENTED_RESPONSE,
                ),
                DirectMethodDispatch::TimedOut(request) => (
                    *request.request_id(),
                    DIRECT_METHOD_TIMEOUT_STATUS,
                    METHOD_TIMEOUT_RESPONSE,
                ),
            })
        };
        if let Some((request_id, status, response)) = pending_method {
            session
                .respond_direct_method(&request_id, status, response, &mut topic_scratch)
                .await
                .map_err(|_| "Azure direct-method response failed")?;
            wait_for_session_event(session, methods, |event| {
                matches!(
                    event,
                    FirmwareSessionEvent::OutboundAcknowledged(
                        OutboundOperation::DirectMethodResponse
                    )
                )
            })
            .await?;
            methods
                .complete_active()
                .map_err(|_| "Azure method queue completion failed")?;
            continue;
        }

        if Instant::now() >= next_telemetry {
            if telemetry.active().is_none() && telemetry.is_empty() {
                telemetry
                    .enqueue(telemetry_topic, HEARTBEAT_PAYLOAD, None)
                    .map_err(|_| "telemetry queue refill failed")?;
            }
            let active = if let Some(active) = telemetry.active() {
                active
            } else {
                match telemetry.begin_next(Instant::now().as_millis()) {
                    Some(TelemetryDispatch::Ready(entry)) => entry,
                    Some(TelemetryDispatch::Expired(_)) => {
                        telemetry
                            .complete_active()
                            .map_err(|_| "telemetry expiration completion failed")?;
                        continue;
                    }
                    None => return Err("telemetry dispatch unavailable"),
                }
            };
            session
                .publish_queued_telemetry(active)
                .await
                .map_err(|_| "Azure telemetry publish failed")?;
            wait_for_session_event(session, methods, |event| {
                matches!(
                    event,
                    FirmwareSessionEvent::OutboundAcknowledged(OutboundOperation::Telemetry)
                )
            })
            .await?;
            telemetry
                .complete_active()
                .map_err(|_| "telemetry PUBACK completion failed")?;
            esp_println::println!("azure-iot telemetry acknowledged");
            next_telemetry = Instant::now() + TELEMETRY_INTERVAL;
            continue;
        }

        match select(
            next_session_event(session, methods),
            Timer::at(next_telemetry),
        )
        .await
        {
            Either::First(Ok(_)) | Either::Second(()) => {}
            Either::First(Err(error)) => return Err(error),
        }
    }
}

async fn wait_for_session_event<S: MqttSession>(
    session: &mut HubSession<'_, S>,
    methods: &mut FirmwareMethodQueue,
    predicate: impl Fn(FirmwareSessionEvent) -> bool,
) -> Result<(), &'static str> {
    match with_timeout(MQTT_OPERATION_TIMEOUT, async {
        loop {
            let event = next_session_event(session, methods).await?;
            if predicate(event) {
                return Ok(());
            }
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Err("Azure operation timed out"),
    }
}

async fn next_session_event<S: MqttSession>(
    session: &mut HubSession<'_, S>,
    methods: &mut FirmwareMethodQueue,
) -> Result<FirmwareSessionEvent, &'static str> {
    let event = session
        .poll()
        .await
        .map_err(|_| "Azure MQTT processing failed")?;
    let (result, accept_inbound) = match event {
        HubSessionEvent::SubscriptionAccepted { .. } => {
            (FirmwareSessionEvent::SubscriptionAccepted, false)
        }
        HubSessionEvent::OutboundAcknowledged { purpose, .. } => {
            (FirmwareSessionEvent::OutboundAcknowledged(purpose), false)
        }
        HubSessionEvent::Progress => (FirmwareSessionEvent::Progress, false),
        HubSessionEvent::Inbound(HubEvent::DirectMethod(request)) => {
            if methods
                .enqueue(request, Instant::now().as_millis())
                .is_err()
            {
                session
                    .reject_inbound_capacity()
                    .await
                    .map_err(|_| "Azure direct-method capacity rejection failed")?;
                esp_println::println!("azure-iot direct-method queue rejected request");
                return Ok(FirmwareSessionEvent::Progress);
            }
            (FirmwareSessionEvent::InboundAccepted, true)
        }
        HubSessionEvent::Inbound(HubEvent::TwinResponse { operation, .. }) => {
            (FirmwareSessionEvent::TwinResponse(operation), true)
        }
        HubSessionEvent::Inbound(HubEvent::DesiredPropertiesPatch(patch)) => {
            esp_println::println!(
                "azure-iot desired-properties accepted: version={}",
                patch.version()
            );
            (FirmwareSessionEvent::InboundAccepted, true)
        }
        HubSessionEvent::Inbound(HubEvent::CloudToDevice(_)) => {
            (FirmwareSessionEvent::InboundAccepted, true)
        }
    };
    if accept_inbound {
        session
            .accept_inbound()
            .await
            .map_err(|_| "Azure inbound acceptance failed")?;
    }
    Ok(result)
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
    let development_gate = option_env!("AZURE_IOT_ALLOW_EMBEDDED_DEVELOPMENT_CREDENTIALS");
    let client_certificate_pem = option_env!("AZURE_IOT_CLIENT_CERTIFICATE_PEM");
    let client_private_key_pem = option_env!("AZURE_IOT_CLIENT_PRIVATE_KEY_PEM");
    let trusted_unix_time = option_env!("AZURE_IOT_TRUSTED_UNIX_TIME");
    if hostname.is_none()
        && device_id.is_none()
        && auth_mode.is_none()
        && development_gate.is_none()
        && client_certificate_pem.is_none()
        && client_private_key_pem.is_none()
        && trusted_unix_time.is_none()
    {
        return Ok(None);
    }
    let (
        Some(hostname),
        Some(device_id),
        Some(auth_mode),
        Some(development_gate),
        Some(client_certificate_pem),
        Some(client_private_key_pem),
        Some(trusted_unix_time),
    ) = (
        hostname,
        device_id,
        auth_mode,
        development_gate,
        client_certificate_pem,
        client_private_key_pem,
        trusted_unix_time,
    )
    else {
        return Err("Azure X.509 development configuration must be complete");
    };
    if development_gate != "1" {
        return Err("embedded development credentials require an explicit value of 1");
    }
    let auth_mode = match auth_mode {
        "development-x509" => AzureAuthMode::DevelopmentX509,
        _ => return Err("AZURE_IOT_AUTH_MODE currently accepts only development-x509"),
    };
    if client_certificate_pem.is_empty() || client_private_key_pem.is_empty() {
        return Err("Azure X.509 development identity must not be empty");
    }
    let trusted_unix_time = trusted_unix_time
        .parse::<u64>()
        .map(UnixTime::from_seconds)
        .map_err(|_| "AZURE_IOT_TRUSTED_UNIX_TIME must be an unsigned Unix timestamp")?;
    let hostname = HubHostname::new(hostname).map_err(|_| "invalid Azure IoT Hub hostname")?;
    let device_id = DeviceId::new(device_id).map_err(|_| "invalid Azure IoT device ID")?;
    let hub = HubConfig::new(
        hostname,
        device_id,
        AZURE_KEEP_ALIVE_SECONDS,
        MQTT_PACKET_CAPACITY as u32,
    )
    .map_err(|_| "invalid Azure IoT Hub MQTT limits")?;
    Ok(Some(AzurePublicConfig {
        hub,
        auth_mode,
        client_certificate_pem,
        client_private_key_pem,
        trusted_unix_time,
    }))
}

async fn wait_forever() -> ! {
    loop {
        Timer::after(Duration::from_secs(30)).await;
    }
}
