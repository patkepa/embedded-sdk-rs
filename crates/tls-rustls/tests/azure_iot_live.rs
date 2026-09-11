//! Opt-in end-to-end telemetry test against an isolated Azure IoT Hub device.

use std::{
    future::Future,
    io::{Read as _, Write as _},
    net::{TcpStream, ToSocketAddrs},
    task::{Context, Poll, Waker},
    time::{Duration, SystemTime},
};

use embedded_io_async::{ErrorKind, ErrorType, Read, Write};
use embedded_sdk_cloud_azure_iot::{
    DeviceId, DeviceSasProvider, HubCapabilities, HubClient, HubConfig, HubHostname, HubSession,
    HubSessionEvent, OutboundOperation, SasCredentialProvider, SasKeySlot, SasKeySource,
    SessionDisposition,
};
use embedded_sdk_mqtt_v311::{
    Buffers as MqttBuffers, Client as MqttClient, ConnectEvent, Credentials, TransportSecurity,
};
use embedded_sdk_security::{TimeError, TrustedTime, UnixTime};
use embedded_sdk_tls_rustls::{TlsBuffers, TlsClientConfig, TlsRootStore, TlsStream};
use zeroize::Zeroizing;

const MQTT_PACKET_CAPACITY: usize = 2_048;
const TLS_RECORD_CAPACITY: usize = 16_384;
const TLS_PLAINTEXT_CAPACITY: usize = 2_048;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const IO_TIMEOUT: Duration = Duration::from_secs(20);
const AZURE_ROOTS: [&[u8]; 2] = [
    include_bytes!(
        "../../../firmware/seeed/xiao-esp32c6-azure-iot/certificates/digicert-global-root-g2.pem"
    ),
    include_bytes!(
        "../../../firmware/seeed/xiao-esp32c6-azure-iot/certificates/microsoft-rsa-root-2017.pem"
    ),
];

struct BlockingTcp(TcpStream);

impl ErrorType for BlockingTcp {
    type Error = ErrorKind;
}

impl Read for BlockingTcp {
    async fn read(&mut self, output: &mut [u8]) -> Result<usize, Self::Error> {
        self.0.read(output).map_err(map_io_error)
    }
}

impl Write for BlockingTcp {
    async fn write(&mut self, input: &[u8]) -> Result<usize, Self::Error> {
        self.0.write(input).map_err(map_io_error)
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.0.flush().map_err(map_io_error)
    }
}

fn map_io_error(error: std::io::Error) -> ErrorKind {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ErrorKind::TimedOut,
        std::io::ErrorKind::ConnectionRefused => ErrorKind::ConnectionRefused,
        std::io::ErrorKind::ConnectionReset => ErrorKind::ConnectionReset,
        std::io::ErrorKind::NotConnected => ErrorKind::NotConnected,
        _ => ErrorKind::Other,
    }
}

struct HostTrustedTime;

impl TrustedTime for HostTrustedTime {
    fn now(&self) -> Result<UnixTime, TimeError> {
        let seconds = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| TimeError::Unavailable)?
            .as_secs();
        Ok(UnixTime::from_seconds(seconds))
    }
}

struct EnvironmentKey(Zeroizing<String>);

impl SasKeySource for EnvironmentKey {
    type Error = KeySourceError;

    async fn load_base64_key(
        &mut self,
        slot: SasKeySlot,
        output: &mut [u8],
    ) -> Result<usize, Self::Error> {
        if slot != SasKeySlot::Primary {
            return Err(KeySourceError::UnavailableSlot);
        }
        let key = self.0.as_bytes();
        let destination = output
            .get_mut(..key.len())
            .ok_or(KeySourceError::OutputTooSmall)?;
        destination.copy_from_slice(key);
        Ok(key.len())
    }
}

#[derive(Debug)]
enum KeySourceError {
    UnavailableSlot,
    OutputTooSmall,
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

/// Required environment:
///
/// - `AZURE_IOT_LIVE_HUB_HOSTNAME`
/// - `AZURE_IOT_LIVE_DEVICE_ID`
/// - `AZURE_IOT_LIVE_DEVICE_KEY`
///
/// Use only a dedicated test identity. Inject the device key from a secret
/// manager or a scoped process environment; never commit or print it.
#[test]
#[ignore = "requires an isolated Azure IoT Hub device identity and network access"]
fn publishes_qos1_telemetry_over_verified_tls() {
    let hostname = std::env::var("AZURE_IOT_LIVE_HUB_HOSTNAME")
        .expect("AZURE_IOT_LIVE_HUB_HOSTNAME must name an isolated test hub");
    let device_id = std::env::var("AZURE_IOT_LIVE_DEVICE_ID")
        .expect("AZURE_IOT_LIVE_DEVICE_ID must name an isolated test device");
    let device_key = Zeroizing::new(
        std::env::var("AZURE_IOT_LIVE_DEVICE_KEY")
            .expect("AZURE_IOT_LIVE_DEVICE_KEY must be injected securely"),
    );

    let config = HubConfig::new(
        HubHostname::new(&hostname).expect("valid hub hostname"),
        DeviceId::new(&device_id).expect("valid device ID"),
        60,
        MQTT_PACKET_CAPACITY as u32,
    )
    .expect("valid live test configuration");
    let time = HostTrustedTime;
    let mut credential_provider =
        DeviceSasProvider::new(EnvironmentKey(device_key), HostTrustedTime, 3_600, 300)
            .expect("valid SAS lifetime");
    let token = block_on(credential_provider.acquire(&config)).expect("generate device SAS");

    let mut root_scratch = [0_u8; 1_452];
    let roots = TlsRootStore::from_pem_roots(AZURE_ROOTS, &mut root_scratch)
        .expect("parse Azure IoT Hub roots");
    let tls_config = TlsClientConfig::from_trust_roots(roots, &time, TLS_PLAINTEXT_CAPACITY)
        .expect("build verified TLS policy");

    let stream = (
        hostname.as_str(),
        embedded_sdk_cloud_azure_iot::MQTT_TLS_PORT,
    )
        .to_socket_addrs()
        .expect("resolve IoT Hub endpoint")
        .find_map(|address| TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).ok())
        .expect("connect to an IoT Hub TCP endpoint");
    stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(IO_TIMEOUT)).unwrap();

    let mut incoming_tls = [0_u8; TLS_RECORD_CAPACITY];
    let mut outgoing_tls = [0_u8; TLS_PLAINTEXT_CAPACITY + 64];
    let mut plaintext = [0_u8; TLS_PLAINTEXT_CAPACITY];
    let tls = block_on(TlsStream::connect(
        BlockingTcp(stream),
        &tls_config,
        &hostname,
        TlsBuffers {
            incoming_tls: &mut incoming_tls,
            outgoing_tls: &mut outgoing_tls,
            plaintext: &mut plaintext,
        },
    ))
    .expect("authenticate IoT Hub TLS endpoint");

    let mut mqtt_rx = [0_u8; MQTT_PACKET_CAPACITY];
    let mut mqtt_tx = [0_u8; MQTT_PACKET_CAPACITY];
    let mut mqtt_replay = [0_u8; MQTT_PACKET_CAPACITY];
    let mut mqtt = MqttClient::new(
        config.mqtt(),
        MqttBuffers {
            rx: &mut mqtt_rx,
            tx: &mut mqtt_tx,
            replay: &mut mqtt_replay,
        },
    )
    .expect("build bounded MQTT client");
    let mut username_scratch = [0_u8; 512];
    let username = config
        .write_mqtt_username(&mut username_scratch)
        .expect("encode MQTT username");
    let password = token.expose_password();
    let credentials = Credentials::new(username, password.as_bytes()).expect("borrow credentials");
    let connection = block_on(mqtt.connect(tls, TransportSecurity::Encrypted, Some(credentials)))
        .expect("connect MQTT 3.1.1 session");
    let disposition = match connection.connect_event() {
        ConnectEvent::FreshSession => SessionDisposition::Fresh,
        ConnectEvent::ResumedSession => SessionDisposition::Resumed,
    };

    let mut hub = HubClient::new(config, HubCapabilities::TELEMETRY);
    let mut session =
        HubSession::new(&mut hub, connection, disposition).expect("attach Azure session");
    let mut topic_scratch = [0_u8; 192];
    let operation = block_on(session.publish_telemetry(
        br#"{"version":1,"source":"embedded-sdk-live-test"}"#,
        &mut topic_scratch,
    ))
    .expect("publish QoS 1 telemetry");
    loop {
        match block_on(session.poll()).expect("wait for IoT Hub PUBACK") {
            HubSessionEvent::OutboundAcknowledged {
                operation: acknowledged,
                purpose: OutboundOperation::Telemetry,
            } => {
                assert_eq!(acknowledged, operation);
                break;
            }
            HubSessionEvent::Progress => {}
            event => panic!("unexpected Azure session event: {event:?}"),
        }
    }
    block_on(session.disconnect()).expect("disconnect MQTT session");
}
