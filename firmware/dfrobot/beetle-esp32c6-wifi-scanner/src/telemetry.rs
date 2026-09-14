use core::{
    fmt::{self, Write},
    net::Ipv4Addr,
};

use embassy_executor::Spawner;
use embassy_net::{
    Config as NetworkConfig, IpAddress, IpEndpoint, Runner, StackResources, tcp::TcpSocket,
};
use embassy_time::{Duration, Instant, with_timeout};
use embedded_sdk_mqtt::{
    BrokerHostname, BrokerPort, ClientId, Config as MqttConfig, QoS, TopicName,
};
use embedded_sdk_mqtt_minimq::{Client as MqttClient, TransportSecurity};
use embedded_sdk_networking_embassy_net::EmbassyNetwork;
use embedded_sdk_wifi::diagnostics::Analyzer;
use esp_hal::rng::Rng;
use esp_radio::wifi::{
    AuthenticationMethod, Config, Interface, WifiController,
    sta::{ScanMethod, StationConfig},
};
use static_cell::StaticCell;

const BATCH_SIZE: usize = 13;
const LINE_SIZE: usize = 768;
const PREFIX: &[u8] = b"WIFI_TELEMETRY ";
const OPERATION_TIMEOUT: Duration = Duration::from_secs(15);
const BATCH_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Settings {
    ssid: &'static str,
    password: &'static str,
    station_channel: Option<u8>,
    station_bssid: Option<[u8; 6]>,
    broker_ip: Ipv4Addr,
    session: MqttConfig,
    topic: TopicName,
}

impl Settings {
    pub fn device_id(&self) -> &str {
        self.session.client_id().as_str()
    }

    pub fn from_env(station_mac: [u8; 6]) -> Result<Option<Self>, &'static str> {
        let ssid = option_env!("WIFI_SSID");
        let password = option_env!("WIFI_PASSWORD");
        let host = option_env!("MQTT_HOST");
        let client = option_env!("MQTT_CLIENT_ID");
        if ssid.is_none() && password.is_none() && host.is_none() && client.is_none() {
            return Ok(None);
        }
        let (Some(ssid), Some(password), Some(host)) = (ssid, password, host) else {
            return Err("WIFI_SSID, WIFI_PASSWORD and MQTT_HOST must be set together");
        };
        if ssid.is_empty() || ssid.len() > 32 || !(8..=63).contains(&password.len()) {
            return Err("invalid Wi-Fi SSID or WPA passphrase length");
        }
        if option_env!("MQTT_PLAINTEXT_FIXTURE") != Some("1") {
            return Err("set MQTT_PLAINTEXT_FIXTURE=1 for the local plaintext broker");
        }
        let mut generated = Line::new();
        write!(
            generated,
            "beetle-esp32c6-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            station_mac[0],
            station_mac[1],
            station_mac[2],
            station_mac[3],
            station_mac[4],
            station_mac[5]
        )
        .map_err(|_| "generated MQTT_CLIENT_ID is too long")?;
        let client = client.unwrap_or(generated.as_str());
        if !(1..=32).contains(&client.len())
            || !client
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err("MQTT_CLIENT_ID must use 1–32 ASCII letters, digits, _ or -");
        }
        let broker_ip = host
            .parse::<Ipv4Addr>()
            .map_err(|_| "MQTT_HOST must be the backend computer's IPv4 address")?;
        let station_channel = option_env!("WIFI_CHANNEL")
            .map(|channel| channel.parse::<u8>())
            .transpose()
            .map_err(|_| "WIFI_CHANNEL must be a channel from 1 to 13")?;
        if station_channel.is_some_and(|channel| !(1..=13).contains(&channel)) {
            return Err("WIFI_CHANNEL must be a channel from 1 to 13");
        }
        let station_bssid = option_env!("WIFI_BSSID").map(parse_bssid).transpose()?;
        let port = option_env!("MQTT_PORT")
            .unwrap_or("1883")
            .parse::<u16>()
            .map_err(|_| "MQTT_PORT must be a valid port")?;
        let hostname = BrokerHostname::new(host).map_err(|_| "MQTT_HOST is invalid")?;
        let port = BrokerPort::new(port).map_err(|_| "MQTT_PORT must be nonzero")?;
        let client_id = ClientId::new(client).map_err(|_| "MQTT_CLIENT_ID is invalid")?;
        let session = MqttConfig::new(hostname, port, client_id, 30, 0, 1024)
            .map_err(|_| "MQTT session configuration is invalid")?;
        let mut topic_text = Line::new();
        write!(
            topic_text,
            "embedded-sdk/beetle-wifi-scan/v1/{client}/telemetry"
        )
        .map_err(|_| "MQTT topic is too long")?;
        let topic =
            TopicName::new(topic_text.as_str()).map_err(|_| "MQTT telemetry topic is invalid")?;
        Ok(Some(Self {
            ssid,
            password,
            station_channel,
            station_bssid,
            broker_ip,
            session,
            topic,
        }))
    }

    pub fn configure_station(
        &self,
        controller: &mut WifiController<'_>,
    ) -> Result<(), esp_radio::wifi::WifiError> {
        let mut station = StationConfig::default()
            .with_ssid(self.ssid)
            .with_password(self.password.into())
            .with_scan_method(ScanMethod::AllChannels)
            .with_auth_method(AuthenticationMethod::Wpa2Personal);
        if let Some(channel) = self.station_channel {
            station = station.with_channel(channel);
        }
        if let Some(bssid) = self.station_bssid {
            station = station.with_bssid(bssid);
        }
        controller.set_config(&Config::Station(station))
    }
}

fn parse_bssid(value: &str) -> Result<[u8; 6], &'static str> {
    if value.len() != 12 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("WIFI_BSSID must be 12 hexadecimal digits");
    }
    let mut bssid = [0; 6];
    for (index, byte) in bssid.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "WIFI_BSSID must be 12 hexadecimal digits")?;
    }
    Ok(bssid)
}

pub struct Line {
    bytes: [u8; LINE_SIZE],
    len: usize,
}

impl Line {
    const fn new() -> Self {
        Self {
            bytes: [0; LINE_SIZE],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
    fn payload(&self) -> Option<&[u8]> {
        let bytes = &self.bytes[..self.len];
        bytes.strip_prefix(PREFIX)?.strip_suffix(b"\n")
    }
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self.len + text.len();
        if end > self.bytes.len() {
            return Err(fmt::Error);
        }
        self.bytes[self.len..end].copy_from_slice(text.as_bytes());
        self.len = end;
        Ok(())
    }
}

pub struct Batch {
    lines: [Line; BATCH_SIZE],
    count: usize,
}

impl Batch {
    pub const fn new() -> Self {
        Self {
            lines: [const { Line::new() }; BATCH_SIZE],
            count: 0,
        }
    }
    pub fn push<const P: usize, const A: usize>(
        &mut self,
        analyzer: &Analyzer<P, A>,
        now_ms: u64,
        dropped: u32,
        gap_ms: u64,
    ) {
        let line = &mut self.lines[self.count];
        line.len = 0;
        if analyzer
            .write_telemetry(line, now_ms, dropped, gap_ms)
            .is_ok()
        {
            self.count += 1;
        } else {
            esp_println::println!("telemetry line exceeded bounded storage");
        }
    }
    pub fn is_full(&self) -> bool {
        self.count == BATCH_SIZE
    }
    pub fn clear(&mut self) {
        self.count = 0;
    }
}

#[embassy_executor::task]
async fn network_runner(mut runner: Runner<'static, Interface<'static>>) {
    runner.run().await;
}

pub fn start_network(spawner: &Spawner, station: Interface<'static>) -> EmbassyNetwork<'static> {
    // DHCP occupies a socket slot; leave room for the MQTT TCP connection.
    static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let (stack, runner) = embassy_net::new(
        station,
        NetworkConfig::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    );
    spawner.spawn(network_runner(runner).expect("network runner task allocation"));
    EmbassyNetwork::new(stack)
}

pub struct Buffers {
    mqtt_rx: [u8; 1024],
    mqtt_tx: [u8; 2048],
    tcp_rx: [u8; 1024],
    tcp_tx: [u8; 1024],
}

impl Buffers {
    const fn new() -> Self {
        Self {
            mqtt_rx: [0; 1024],
            mqtt_tx: [0; 2048],
            tcp_rx: [0; 1024],
            tcp_tx: [0; 1024],
        }
    }
}

pub fn buffers() -> &'static mut Buffers {
    static BUFFERS: StaticCell<Buffers> = StaticCell::new();
    BUFFERS.init_with(Buffers::new)
}

pub async fn upload(
    controller: &mut WifiController<'_>,
    network: EmbassyNetwork<'static>,
    settings: &Settings,
    batch: &Batch,
    buffers: &mut Buffers,
) {
    let result = match with_timeout(BATCH_TIMEOUT, async {
        match with_timeout(Duration::from_secs(35), controller.connect_async()).await {
            Ok(Ok(info)) => {
                esp_println::println!("telemetry Wi-Fi associated on channel {}", info.channel)
            }
            Ok(Err(error)) => {
                esp_println::println!("telemetry Wi-Fi association failed: {error:?}");
                return false;
            }
            Err(_) => {
                esp_println::println!("telemetry Wi-Fi association timed out");
                return false;
            }
        }
        match with_timeout(Duration::from_secs(20), network.wait_ip_ready()).await {
            Ok(Ok(_)) => {}
            _ => {
                esp_println::println!("telemetry DHCP timed out or failed");
                return false;
            }
        }
        let mut socket = TcpSocket::new(network.stack(), &mut buffers.tcp_rx, &mut buffers.tcp_tx);
        let endpoint = IpEndpoint::new(
            IpAddress::Ipv4(settings.broker_ip),
            settings.session.port().get(),
        );
        match with_timeout(OPERATION_TIMEOUT, socket.connect(endpoint)).await {
            Ok(Ok(())) => {}
            other => {
                esp_println::println!("telemetry TCP connect failed: {other:?}");
                return false;
            }
        }
        let mut client = match MqttClient::new(
            &settings.session,
            &mut buffers.mqtt_rx,
            &mut buffers.mqtt_tx,
            TransportSecurity::PlaintextFixture,
            None,
        ) {
            Ok(client) => client,
            Err(error) => {
                esp_println::println!("telemetry MQTT config failed: {error}");
                return false;
            }
        };
        let mut connection = match with_timeout(OPERATION_TIMEOUT, client.connect(socket)).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) => {
                esp_println::println!("telemetry MQTT connect failed: {:?}", error.kind());
                return false;
            }
            Err(_) => {
                esp_println::println!("telemetry MQTT connect timed out");
                return false;
            }
        };
        for (index, line) in batch.lines[..batch.count].iter().enumerate() {
            let Some(payload) = line.payload() else {
                return false;
            };
            // Anchor the window's uptime to this publish attempt. The backend can
            // reconstruct an approximate capture time even though the 13 windows
            // are delivered together after the sweep.
            let Ok(payload) = core::str::from_utf8(payload) else {
                return false;
            };
            let Some(prefix) = payload.strip_suffix('}') else {
                return false;
            };
            let mut published = Line::new();
            if write!(
                published,
                "{prefix},\"upload_uptime_ms\":{}}}",
                Instant::now().as_millis()
            )
            .is_err()
            {
                return false;
            }
            match with_timeout(
                OPERATION_TIMEOUT,
                connection.publish(
                    &settings.topic,
                    published.as_str().as_bytes(),
                    QoS::AtMostOnce,
                ),
            )
            .await
            {
                Ok(Ok(())) => {
                    esp_println::println!("telemetry MQTT sent {}/{}", index + 1, batch.count)
                }
                other => {
                    esp_println::println!("telemetry MQTT publish failed: {other:?}");
                    return false;
                }
            }
        }
        let _ = with_timeout(Duration::from_secs(3), connection.disconnect()).await;
        true
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            esp_println::println!("telemetry batch upload timed out");
            false
        }
    };
    match with_timeout(Duration::from_secs(5), controller.disconnect_async()).await {
        Ok(Ok(_)) => {}
        other => esp_println::println!("telemetry Wi-Fi disconnect: {other:?}"),
    }
    esp_println::println!(
        "telemetry upload complete: sent={} success={result}",
        batch.count
    );
}
