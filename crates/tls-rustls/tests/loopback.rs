//! In-process TLS 1.2 interoperability and verification tests.

use std::{
    collections::VecDeque,
    future::Future,
    io::{Cursor, Read as _, Write as _},
    sync::Arc,
    task::{Context, Poll, Waker},
};

use embedded_io_async::{ErrorKind, ErrorType, Read, Write};
use embedded_sdk_cloud_azure_iot::{
    DeviceId, HubCapabilities, HubClient, HubConfig, HubHostname, HubSession, HubSessionEvent,
    OutboundOperation, SessionDisposition, SymmetricKey, generate_device_sas,
};
use embedded_sdk_mqtt::{BrokerHostname, BrokerPort, ClientId, Config, QoS, TopicName};
use embedded_sdk_mqtt_v311::{
    Buffers as MqttBuffers, Client as MqttClient, ConnectEvent, Credentials, Event,
    TransportSecurity,
};
use embedded_sdk_security::{TimeError, TrustedTime, UnixTime};
use embedded_sdk_tls_rustls::{Error, TlsBuffers, TlsClientConfig, TlsStream};
use rcgen::{
    CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_RSA_SHA256, date_time_ymd,
};
use rustls::{
    ServerConfig, SupportedCipherSuite,
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
    version::TLS12,
};

const HOSTNAME: &str = "unit.azure-devices.net";
const NOW: UnixTime = UnixTime::from_seconds(1_788_480_000);
const REQUEST: &[u8] = b"mqtt client bytes";
const RESPONSE: &[u8] = b"mqtt server bytes";

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

struct TestIdentity {
    certificate: rustls::pki_types::CertificateDer<'static>,
    private_key: PrivatePkcs8KeyDer<'static>,
}

fn test_identity(hostname: &str) -> TestIdentity {
    let key = KeyPair::from_pkcs8_pem_and_sign_algo(TEST_RSA_KEY, &PKCS_RSA_SHA256).unwrap();
    let mut params = CertificateParams::new(vec![hostname.to_owned()]).unwrap();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, hostname);
    params.not_before = date_time_ymd(2025, 1, 1);
    params.not_after = date_time_ymd(2035, 1, 1);
    let certificate = params.self_signed(&key).unwrap();
    TestIdentity {
        certificate: certificate.der().clone(),
        private_key: PrivatePkcs8KeyDer::from(key.serialize_der()),
    }
}

fn test_ecdsa_identity(hostname: &str) -> TestIdentity {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec![hostname.to_owned()]).unwrap();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, hostname);
    params.not_before = date_time_ymd(2025, 1, 1);
    params.not_after = date_time_ymd(2035, 1, 1);
    let certificate = params.self_signed(&key).unwrap();
    TestIdentity {
        certificate: certificate.der().clone(),
        private_key: PrivatePkcs8KeyDer::from(key.serialize_der()),
    }
}

struct LoopbackServer {
    server: rustls::ServerConnection,
    encrypted_to_client: VecDeque<u8>,
    received_plaintext: Vec<u8>,
    application: TestApplication,
    processed_plaintext: usize,
    corrupt_next_read: bool,
}

enum TestApplication {
    FixedResponse { replied: bool },
    Mqtt311,
}

impl LoopbackServer {
    fn new(identity: &TestIdentity) -> Self {
        Self::with_application(identity, TestApplication::FixedResponse { replied: false })
    }

    fn mqtt(identity: &TestIdentity) -> Self {
        Self::with_application(identity, TestApplication::Mqtt311)
    }

    fn with_application(identity: &TestIdentity, application: TestApplication) -> Self {
        Self::with_cipher_suites(
            identity,
            application,
            vec![
                rustls_rustcrypto::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
                rustls_rustcrypto::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
            ],
        )
    }

    fn with_cipher_suites(
        identity: &TestIdentity,
        application: TestApplication,
        cipher_suites: Vec<SupportedCipherSuite>,
    ) -> Self {
        let mut provider = rustls_rustcrypto::provider();
        provider.cipher_suites = cipher_suites;
        let config = ServerConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(&[&TLS12])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![identity.certificate.clone()],
                PrivateKeyDer::Pkcs8(identity.private_key.clone_key()),
            )
            .unwrap();
        Self {
            server: rustls::ServerConnection::new(Arc::new(config)).unwrap(),
            encrypted_to_client: VecDeque::new(),
            received_plaintext: Vec::new(),
            application,
            processed_plaintext: 0,
            corrupt_next_read: false,
        }
    }

    fn corrupted_handshake(identity: &TestIdentity) -> Self {
        let mut server = Self::new(identity);
        server.corrupt_next_read = true;
        server
    }

    fn ecdsa_only(identity: &TestIdentity) -> Self {
        Self::with_cipher_suites(
            identity,
            TestApplication::FixedResponse { replied: false },
            vec![rustls_rustcrypto::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256],
        )
    }

    fn accept_client_tls(&mut self, input: &[u8]) -> Result<(), ErrorKind> {
        self.server
            .read_tls(&mut Cursor::new(input))
            .map_err(|_| ErrorKind::InvalidData)?;
        self.server
            .process_new_packets()
            .map_err(|_| ErrorKind::InvalidData)?;

        let mut plaintext = [0_u8; 256];
        loop {
            match self.server.reader().read(&mut plaintext) {
                Ok(0) => break,
                Ok(read) => self
                    .received_plaintext
                    .extend_from_slice(&plaintext[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => return Err(ErrorKind::InvalidData),
            }
        }

        match &mut self.application {
            TestApplication::FixedResponse { replied }
                if !*replied && !self.received_plaintext.is_empty() =>
            {
                self.server
                    .writer()
                    .write_all(RESPONSE)
                    .map_err(|_| ErrorKind::InvalidData)?;
                *replied = true;
            }
            TestApplication::Mqtt311 => {
                let responses = mqtt_responses(
                    &self.received_plaintext[self.processed_plaintext..],
                    &mut self.processed_plaintext,
                )?;
                self.server
                    .writer()
                    .write_all(&responses)
                    .map_err(|_| ErrorKind::InvalidData)?;
            }
            TestApplication::FixedResponse { .. } => {}
        }

        let mut encrypted = Vec::new();
        while self.server.wants_write() {
            self.server
                .write_tls(&mut encrypted)
                .map_err(|_| ErrorKind::InvalidData)?;
        }
        self.encrypted_to_client.extend(encrypted);
        Ok(())
    }
}

fn mqtt_responses(input: &[u8], processed: &mut usize) -> Result<Vec<u8>, ErrorKind> {
    let mut cursor = 0;
    let mut responses = Vec::new();
    while cursor < input.len() {
        let Some((remaining, length_bytes)) = mqtt_remaining_length(&input[cursor + 1..])? else {
            break;
        };
        let header_len = 1 + length_bytes;
        let frame_len = header_len + remaining;
        if input.len() - cursor < frame_len {
            break;
        }
        let frame = &input[cursor..cursor + frame_len];
        match frame[0] >> 4 {
            1 => responses.extend_from_slice(&[0x20, 0x02, 0x00, 0x00]),
            3 if frame[0] & 0x06 == 0x02 => {
                let topic_offset = header_len;
                if remaining < 4 {
                    return Err(ErrorKind::InvalidData);
                }
                let topic_len = usize::from(u16::from_be_bytes([
                    frame[topic_offset],
                    frame[topic_offset + 1],
                ]));
                let packet_id_offset = topic_offset + 2 + topic_len;
                let packet_id = frame
                    .get(packet_id_offset..packet_id_offset + 2)
                    .ok_or(ErrorKind::InvalidData)?;
                responses.extend_from_slice(&[0x40, 0x02, packet_id[0], packet_id[1]]);
            }
            _ => return Err(ErrorKind::InvalidData),
        }
        cursor += frame_len;
    }
    *processed += cursor;
    Ok(responses)
}

fn mqtt_remaining_length(input: &[u8]) -> Result<Option<(usize, usize)>, ErrorKind> {
    let mut multiplier = 1;
    let mut value = 0;
    for (index, byte) in input.iter().copied().take(4).enumerate() {
        value += usize::from(byte & 0x7f) * multiplier;
        if byte & 0x80 == 0 {
            return Ok(Some((value, index + 1)));
        }
        multiplier *= 128;
    }
    if input.len() < 4 {
        Ok(None)
    } else {
        Err(ErrorKind::InvalidData)
    }
}

impl ErrorType for LoopbackServer {
    type Error = ErrorKind;
}

impl Read for LoopbackServer {
    async fn read(&mut self, output: &mut [u8]) -> Result<usize, Self::Error> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.encrypted_to_client.is_empty() {
            return Err(ErrorKind::NotConnected);
        }
        let read = output.len().min(self.encrypted_to_client.len()).min(113);
        for byte in &mut output[..read] {
            *byte = self.encrypted_to_client.pop_front().unwrap();
        }
        if self.corrupt_next_read {
            output[0] ^= 0xff;
            self.corrupt_next_read = false;
        }
        Ok(read)
    }
}

struct TruncatedTransport;

impl ErrorType for TruncatedTransport {
    type Error = ErrorKind;
}

impl Read for TruncatedTransport {
    async fn read(&mut self, _output: &mut [u8]) -> Result<usize, Self::Error> {
        Ok(0)
    }
}

impl Write for TruncatedTransport {
    async fn write(&mut self, input: &[u8]) -> Result<usize, Self::Error> {
        Ok(input.len())
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl Write for LoopbackServer {
    async fn write(&mut self, input: &[u8]) -> Result<usize, Self::Error> {
        let accepted = input.len().min(127);
        self.accept_client_tls(&input[..accepted])?;
        Ok(accepted)
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn config_for(identity: &TestIdentity, time: UnixTime) -> TlsClientConfig {
    TlsClientConfig::from_der_roots([identity.certificate.as_ref()], &FixedTime(time), 1024)
        .unwrap()
}

struct FixedTime(UnixTime);

impl TrustedTime for FixedTime {
    fn now(&self) -> Result<UnixTime, TimeError> {
        Ok(self.0)
    }
}

fn buffers() -> ([u8; 4096], [u8; 2048], [u8; 2048]) {
    ([0; 4096], [0; 2048], [0; 2048])
}

#[test]
fn handshakes_with_sni_and_transfers_encrypted_bytes() {
    let identity = test_identity(HOSTNAME);
    let config = config_for(&identity, NOW);
    let server = LoopbackServer::new(&identity);
    let (mut incoming, mut outgoing, mut plaintext) = buffers();
    let mut stream = block_on(TlsStream::connect(
        server,
        &config,
        HOSTNAME,
        TlsBuffers {
            incoming_tls: &mut incoming,
            outgoing_tls: &mut outgoing,
            plaintext: &mut plaintext,
        },
    ))
    .unwrap();

    assert_eq!(stream.get_ref().server.server_name(), Some(HOSTNAME));
    assert_eq!(block_on(stream.write(REQUEST)).unwrap(), REQUEST.len());

    let mut response = [0_u8; 64];
    let read = block_on(stream.read(&mut response)).unwrap();
    assert_eq!(&response[..read], RESPONSE);
    assert_eq!(stream.get_ref().received_plaintext, REQUEST);
    block_on(stream.close()).unwrap();
}

#[test]
fn carries_mqtt311_connect_and_qos1_publish() {
    let identity = test_identity(HOSTNAME);
    let tls_config = config_for(&identity, NOW);
    let server = LoopbackServer::mqtt(&identity);
    let (mut incoming, mut outgoing, mut plaintext) = buffers();
    let tls = block_on(TlsStream::connect(
        server,
        &tls_config,
        HOSTNAME,
        TlsBuffers {
            incoming_tls: &mut incoming,
            outgoing_tls: &mut outgoing,
            plaintext: &mut plaintext,
        },
    ))
    .unwrap();

    let mqtt_config = Config::new_v311(
        BrokerHostname::new(HOSTNAME).unwrap(),
        BrokerPort::new(8883).unwrap(),
        ClientId::new("device-01").unwrap(),
        60,
        false,
        256,
    )
    .unwrap();
    let mut mqtt_rx = [0; 256];
    let mut mqtt_tx = [0; 256];
    let mut mqtt_replay = [0; 256];
    let mut mqtt = MqttClient::new(
        &mqtt_config,
        MqttBuffers {
            rx: &mut mqtt_rx,
            tx: &mut mqtt_tx,
            replay: &mut mqtt_replay,
        },
    )
    .unwrap();
    let credentials = Credentials::new("hub/device-01", b"sas-token").unwrap();
    let mut connection =
        block_on(mqtt.connect(tls, TransportSecurity::Encrypted, Some(credentials))).unwrap();
    assert_eq!(connection.connect_event(), ConnectEvent::FreshSession);

    let operation = block_on(connection.publish(
        &TopicName::new("devices/device-01/messages/events/").unwrap(),
        b"{\"temperature\":21}",
        QoS::AtLeastOnce,
    ))
    .unwrap()
    .unwrap();
    assert_eq!(
        block_on(connection.poll()).unwrap(),
        Event::Published(operation)
    );
}

#[test]
fn carries_azure_sas_telemetry_through_mqtt_and_tls() {
    let identity = test_identity(HOSTNAME);
    let tls_config = config_for(&identity, NOW);
    let server = LoopbackServer::mqtt(&identity);
    let (mut incoming, mut outgoing, mut plaintext) = buffers();
    let tls = block_on(TlsStream::connect(
        server,
        &tls_config,
        HOSTNAME,
        TlsBuffers {
            incoming_tls: &mut incoming,
            outgoing_tls: &mut outgoing,
            plaintext: &mut plaintext,
        },
    ))
    .unwrap();

    let hub_config = HubConfig::new(
        HubHostname::new(HOSTNAME).unwrap(),
        DeviceId::new("device-01").unwrap(),
        60,
        256,
    )
    .unwrap();
    let mut username_scratch = [0_u8; 512];
    let username = hub_config
        .write_mqtt_username(&mut username_scratch)
        .unwrap();
    let key = SymmetricKey::from_base64("MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=").unwrap();
    let sas = generate_device_sas(&hub_config, &key, &FixedTime(NOW), 3600, 300).unwrap();

    let mut mqtt_rx = [0; 256];
    let mut mqtt_tx = [0; 512];
    let mut mqtt_replay = [0; 512];
    let mut mqtt = MqttClient::new(
        hub_config.mqtt(),
        MqttBuffers {
            rx: &mut mqtt_rx,
            tx: &mut mqtt_tx,
            replay: &mut mqtt_replay,
        },
    )
    .unwrap();
    let connection = sas.with_password(|password| {
        let credentials = Credentials::new(username, password).unwrap();
        block_on(mqtt.connect(tls, TransportSecurity::Encrypted, Some(credentials))).unwrap()
    });

    let mut hub = HubClient::new(hub_config, HubCapabilities::TELEMETRY);
    let mut session = HubSession::new(&mut hub, connection, SessionDisposition::Fresh).unwrap();
    let mut topic_scratch = [0_u8; 256];
    let operation =
        block_on(session.publish_telemetry(b"{\"temperature\":21}", &mut topic_scratch)).unwrap();
    assert_eq!(
        block_on(session.poll()).unwrap(),
        HubSessionEvent::OutboundAcknowledged {
            operation,
            purpose: OutboundOperation::Telemetry,
        }
    );
    assert_eq!(session.snapshot().outbound_acknowledged, 1);
}

#[test]
fn rejects_wrong_hostname() {
    let identity = test_identity(HOSTNAME);
    let config = config_for(&identity, NOW);
    let server = LoopbackServer::new(&identity);
    let (mut incoming, mut outgoing, mut plaintext) = buffers();
    let result = block_on(TlsStream::connect(
        server,
        &config,
        "wrong.azure-devices.net",
        TlsBuffers {
            incoming_tls: &mut incoming,
            outgoing_tls: &mut outgoing,
            plaintext: &mut plaintext,
        },
    ));
    assert!(matches!(result, Err(Error::Tls(_))));
}

#[test]
fn rejects_unknown_root() {
    let server_identity = test_identity(HOSTNAME);
    let untrusted_identity = test_identity("untrusted.test");
    let config = config_for(&untrusted_identity, NOW);
    let server = LoopbackServer::new(&server_identity);
    let (mut incoming, mut outgoing, mut plaintext) = buffers();
    let result = block_on(TlsStream::connect(
        server,
        &config,
        HOSTNAME,
        TlsBuffers {
            incoming_tls: &mut incoming,
            outgoing_tls: &mut outgoing,
            plaintext: &mut plaintext,
        },
    ));
    assert!(matches!(result, Err(Error::Tls(_))));
}

#[test]
fn rejects_certificate_outside_validity_period() {
    let identity = test_identity(HOSTNAME);
    let after_expiry = UnixTime::from_seconds(2_100_000_000);
    let config = config_for(&identity, after_expiry);
    let server = LoopbackServer::new(&identity);
    let (mut incoming, mut outgoing, mut plaintext) = buffers();
    let result = block_on(TlsStream::connect(
        server,
        &config,
        HOSTNAME,
        TlsBuffers {
            incoming_tls: &mut incoming,
            outgoing_tls: &mut outgoing,
            plaintext: &mut plaintext,
        },
    ));
    assert!(matches!(result, Err(Error::Tls(_))));
}

#[test]
fn rejects_certificate_that_is_not_yet_valid() {
    let identity = test_identity(HOSTNAME);
    let before_validity = UnixTime::from_seconds(1_700_000_000);
    let config = config_for(&identity, before_validity);
    let server = LoopbackServer::new(&identity);
    let (mut incoming, mut outgoing, mut plaintext) = buffers();
    let result = block_on(TlsStream::connect(
        server,
        &config,
        HOSTNAME,
        TlsBuffers {
            incoming_tls: &mut incoming,
            outgoing_tls: &mut outgoing,
            plaintext: &mut plaintext,
        },
    ));
    assert!(matches!(result, Err(Error::Tls(_))));
}

#[test]
fn rejects_corrupted_handshake_record() {
    let identity = test_identity(HOSTNAME);
    let config = config_for(&identity, NOW);
    let server = LoopbackServer::corrupted_handshake(&identity);
    let (mut incoming, mut outgoing, mut plaintext) = buffers();
    let result = block_on(TlsStream::connect(
        server,
        &config,
        HOSTNAME,
        TlsBuffers {
            incoming_tls: &mut incoming,
            outgoing_tls: &mut outgoing,
            plaintext: &mut plaintext,
        },
    ));
    assert!(matches!(result, Err(Error::Tls(_))));
}

#[test]
fn distinguishes_truncated_handshake_from_tls_verification_failure() {
    let identity = test_identity(HOSTNAME);
    let config = config_for(&identity, NOW);
    let (mut incoming, mut outgoing, mut plaintext) = buffers();
    let result = block_on(TlsStream::connect(
        TruncatedTransport,
        &config,
        HOSTNAME,
        TlsBuffers {
            incoming_tls: &mut incoming,
            outgoing_tls: &mut outgoing,
            plaintext: &mut plaintext,
        },
    ));
    assert!(matches!(result, Err(Error::UnexpectedEof)));
}

#[test]
fn rejects_peer_without_a_configured_cipher_suite() {
    let identity = test_ecdsa_identity(HOSTNAME);
    let config = config_for(&identity, NOW);
    let server = LoopbackServer::ecdsa_only(&identity);
    let (mut incoming, mut outgoing, mut plaintext) = buffers();
    let result = block_on(TlsStream::connect(
        server,
        &config,
        HOSTNAME,
        TlsBuffers {
            incoming_tls: &mut incoming,
            outgoing_tls: &mut outgoing,
            plaintext: &mut plaintext,
        },
    ));
    assert!(matches!(
        result,
        Err(Error::Transport(ErrorKind::InvalidData))
    ));
}

#[test]
fn rejects_ip_address_as_server_identity() {
    let identity = test_identity(HOSTNAME);
    let config = config_for(&identity, NOW);
    let server = LoopbackServer::new(&identity);
    let (mut incoming, mut outgoing, mut plaintext) = buffers();
    let result = block_on(TlsStream::connect(
        server,
        &config,
        "127.0.0.1",
        TlsBuffers {
            incoming_tls: &mut incoming,
            outgoing_tls: &mut outgoing,
            plaintext: &mut plaintext,
        },
    ));
    assert!(matches!(result, Err(Error::InvalidServerName)));
}

// Test-only identity. It is public by definition and must never be used by firmware.
const TEST_RSA_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCaqdD2e76MJm3N
edPc0GmegC1hnrajovn7rbxXakyUC+RDgcG6FEjyAF7JXBD6Vpsa3Y7MNha4INeY
kLtYyNXP1id2WyiCdlCvquce62h/VpBOi5CyB8LhqeGnFgkwPR6Pq7riwVUg9rZd
lIIwxS21A9XjZzpHAMsS92ziWPXsVpnG97fGgZwI6KF6k6L1iRGelcKiKE8sx5Ob
dRH2c9o34tqJUyC4ylNxnhNJAB4xVJTYu5f8yzWxSi9rgbPpMPv4kphEUcDFKfTG
oy+qE5oxlrOKxICFzixKoBFkgpGtzPuNEn9H9e25i5UalPH1YHhSdXSXKcvnK0Ma
U6e1tWaHAgMBAAECggEAKq6OklcYAMliKABk7V0+qJUq8PPB52rEniYWAfG97GVT
uyWF9vo+Hzrm7Z7QuKVJ7KIUFFsg7fNyTBI1AY17I/4vqcQCa+6G2dPKMIg6sFmN
PX/akKb/qxMcyOWV55AWbQOxcX51JcGwFiczvo3LzVafokAMnyei4zsQ+24df/kA
Z4FDIcHhbtMRVhVv0YSVU2MqqfrRWJqdlUQ1CrwX2dnQYO+hmlOmdpc7Wc/PZPT3
8pD56bSZFzSGwdKuBsfswpP7YS7r8W08ykQQrNDXgcZA28EHcJ+B05wl2UkhY8+c
GvVgD9nxRc8+ZHfqiLqsBCnRJ+t3B5iKooPKwoxo0QKBgQDHeYALIJZSx0WcFe9x
DovpybOdIu3BLglwuvXBXdBEIK3BYkbqHorxm5+wyn5E7+S+KUEjYJPS8bJvahb2
nzgtxnV6DI8P9GLqRkcVeUoxtwzvXRpzYg+pkzr0joQl4juEqiFkPkv401Sq0RWa
Q9CNC5LeE4iufN0KPv4i7DO2swKBgQDGfZHLdl+l/MDijnivYigPdTD8oW7+Kg5r
H6h41q07P0xHvSSUcrXRDy6816LsznCXFM/qBu9MnQhdUJPRc+YDv+dqhdx+Ixy6
2Kweeyltpoi1vZb7esgSn97STktFXcVid6+G711WCBfirQ51qfVP4D0EYsgrC5bN
uEf/RrOa3QKBgQC/fCMui1nCnQh1jZkNLqmhA78oWR9jEo59aPwBY81JmRUzTuRE
Wo2G4Z2qWLhd9OvgoDmnfE5rcRmZWn4wwSdsydZ8ExJCfpd1zYDvXD+c+duw6+84
VCo03uD5YtX4h/Qapjbnw+WqNzRPxea27+KDg1i5Voce+T43V8EeRSBfgQKBgQCA
VxrY6r4nrkjtdF92T0pFzGmTP7JrprfR7hNZpr01zNS+of6v+Ye0GFQJCIixAz5r
gap50GgUKokJBRu+12iHTiMMjmcmK//clFKeFtaPrplAocio7BfHaxWA99zVii8h
Xu/gmI7KHMuM5oat7+nM7tmlJ1Xz9zdX5uqulYF2BQKBgDsHyJCSm/aeIy5aCimY
HdkvFFGYrEt22RG8iQOayCHPJ10Fq81aud9ZpvkBZs6cxQXGf/SGOXO3MbgGzyQ/
Fav9liH8wEhL8SpvJfBTIN0rxCEeCFxJrfyX1Db9XRIC6wHrV4Z1AaVn8g8yZZ1v
glrtJ4+vgCZmisMS7azn8xqz
-----END PRIVATE KEY-----"#;
